import html
import platform
import sys
import tempfile
import time
from dataclasses import dataclass
from enum import IntEnum
from http import HTTPStatus
from pathlib import Path
from urllib.parse import urlsplit

import anki.lang

from aqt import gui_hooks, mw
from aqt.qt import QAction, QKeySequence, QMenu, QTimer
from aqt.utils import openLink, tooltip

from . import board, delivery, heatmap_ui, home, notify, ui, updates, web
from . import inbox_ui, moments, moments_ui, share, sharing
from .inbox import CHALLENGE_ROUTE
from .language import _plural, tr
from .client import (
    COUNT_SQL,
    DEFAULT_SERVER,
    MAX_PENDING,
    PENDING_SQL,
    UNDO_WINDOW_MS,
    WINDOW_SQL,
    InsecureServer,
    ServerError,
    Client,
    check_deletes,
    offset_west_min,
    reconcile,
    rows_to_reviews,
    sign_in,
    sign_up,
)
from .deck_completion import deck_snapshots, study_day
from . import copies_ui, deck_names, preferences, preview, review_offsets

MARK_KEY = "ankiquestUploadedThrough"
RECENT_KEY = "ankiquestRecentUploads"
BASELINE_KEY = "ankiquestBaselineAccount"
RESYNC_DAY_KEY = "ankiquestResyncedDays"
SHARED_KEY = "ankiquestSharedDecks"
UPLOADS_KEY = "ankiquestUploadsByAccount"
ORDER_KEY = "ankiquestLastOrder"
ORDER_WEEK_KEY = "ankiquestLastOrderWeek"
INBOX_KEY = "ankiquestInboxCursor"
STREAK_DAY_KEY = "ankiquestStreakDay"
UPDATE_CHECK_KEY = "ankiquestUpdateCheckedAt"
SIGN_IN_OFFER_KEY = "ankiquestSignInOffered"
ADDON_DIR = Path(__file__).resolve().parent
RESYNC_WINDOW_MS = 7 * 86_400_000
POLL_MS = 5 * 60 * 1000
RETRY_START_MS = 5_000
RETRY_MAX_STEPS = 6
WEBSITE_SHORTCUT = "Ctrl+Shift+K"


class UploadMode(IntEnum):
    PENDING = 0
    PROFILE_OPEN = 1
    RESYNC = 2
    HISTORY = 3


@dataclass(frozen=True)
class Refresh:
    """Keep the widest requested upload and any XP feedback while a batch runs."""

    show_feedback: bool
    mode: UploadMode

    def merged(self, other):
        if other is None:
            return self
        return Refresh(self.show_feedback or other.show_feedback, max(self.mode, other.mode))


@dataclass(frozen=True)
class UploadTarget:
    collection: object
    prefs: dict
    account: tuple

    def current(self, api):
        return (
            mw.col is self.collection and mw.pm.profile is self.prefs
            and self.account == (api.base, api.user, api.token)
        )


@dataclass
class Uploading:
    """An upload runs; `queued` is every refresh asked for meanwhile, merged into one.

        idle (None) ──refresh──▶ Uploading ──done──▶ idle, then the queued refresh
                                  │    ▲
                                  └────┘ refresh: queue it
    """

    target: UploadTarget
    request: Refresh
    attempts: int
    queued: Refresh = None


@dataclass
class _Waiting:
    target: UploadTarget
    request: Refresh
    attempts: int
    message: str


@dataclass(frozen=True)
class _NeedsSignIn:
    target: UploadTarget
    message: str


@dataclass(frozen=True)
class _Failed:
    target: UploadTarget
    message: str


@dataclass(frozen=True)
class _PollFailure:
    retry_at: float


state = {
    # Idle, uploading, waiting for a retry, or holding the last failure.
    "upload": None,
    # A poll belongs to one open profile and credential.
    "polling": None,
    "poll_failure": None,
    "reauth": None,
    "timer": None,
    "place": None,
    "profile": None,
    "companion": "aki",
    "companion_account": None,
    "inbox": [],
    "activity": None,
}

mw.addonManager.setWebExports(__name__, r"((aki|ankilope)_face|art/(aki|ankilope)_[a-z]+)\.png")


def config():
    return mw.addonManager.getConfig(__name__) or {}


def save_config(values):
    current = config()
    current.update(values)
    mw.addonManager.writeConfig(__name__, current)


def drop_config(keys):
    """Removes settings an older version kept, once they live elsewhere."""
    current = config()
    if any(key in current for key in keys):
        mw.addonManager.writeConfig(__name__, {key: value for key, value in current.items() if key not in keys})


def client():
    settings = config()
    return Client(settings.get("url", ""), settings.get("user", ""), settings.get("token", ""), anki.lang.current_lang)


def refresh(show_feedback, resync=False):
    _request_upload(Refresh(show_feedback, UploadMode.RESYNC if resync else UploadMode.PENDING))


def _upload_prefs(api):
    prefs = mw.pm.profile
    accounts = prefs.setdefault(UPLOADS_KEY, {})

    # Move the old single cursor to its recorded account. Builds before account
    # tracking used the profile's configured account. Deck sharing is refetched.
    keys = {MARK_KEY: "through", RECENT_KEY: "recent", preferences.LAST_UPLOAD_KEY: "last_upload"}
    if any(key in prefs for key in keys):
        baseline = prefs.pop(BASELINE_KEY, None)
        base, user = baseline or (api.base, api.user)
        record = accounts.setdefault(base, {}).setdefault(user, {})
        for key, field in keys.items():
            if key in prefs:
                value = prefs.pop(key)
                if key != MARK_KEY or value or baseline:
                    record.setdefault(field, value)
    for base, user, day in prefs.pop(RESYNC_DAY_KEY, []):
        accounts.setdefault(base, {}).setdefault(user, {}).setdefault("resynced_day", day)
    prefs.pop(SHARED_KEY, None)
    return accounts.setdefault(api.base, {}).setdefault(api.user, {})


def _current_upload(api):
    upload = state["upload"]
    return upload if upload is not None and upload.target.current(api) else None


def _upload_status(api):
    at = _upload_prefs(api).get("last_upload") if api.configured else None
    last = preferences.last_upload_text(at)
    upload = _current_upload(api)
    return f"{last} · {upload.message}" if isinstance(upload, (_Waiting, _NeedsSignIn, _Failed)) else last


def _retry_upload(waiting):
    if waiting is not _current_upload(client()):
        return
    state["upload"] = None
    _request_upload(waiting.request, attempts=waiting.attempts)


def _upload_failed(upload, error):
    # Recover from transient failures without re-sending on every answer.
    print("ankiquest:", error)
    if isinstance(error, ServerError) and error.status == HTTPStatus.UNAUTHORIZED:
        message = tr("Sign in to AnkiQuest again to resume uploads.")
        state["upload"] = _NeedsSignIn(upload.target, message)
        redraw()
        tooltip(message)
        return
    elif isinstance(error, ServerError) and (error.status == HTTPStatus.TOO_MANY_REQUESTS or error.status >= HTTPStatus.INTERNAL_SERVER_ERROR):
        message = tr("Last upload failed (%s), will retry") % error
    elif isinstance(error, OSError):
        message = tr("Offline, will retry")
    else:
        state["upload"] = _Failed(upload.target, tr("Last upload failed (%s)") % error)
        redraw()
        if upload.request.mode == UploadMode.HISTORY:
            failed(tr("Could not upload review history (%s)."), error)
        if upload.queued is not None:
            _request_upload(upload.queued)
        return

    request = upload.request.merged(upload.queued)
    # Give a refresh already queued during the first batch one immediate attempt.
    # This keeps its live review feedback timely, then backoff takes over.
    if upload.queued is not None and upload.attempts == 0 and upload.request.mode != UploadMode.HISTORY:
        _request_upload(request, attempts=1)
        return

    waiting = _Waiting(upload.target, request, upload.attempts + 1, message)
    state["upload"] = waiting
    delay = RETRY_START_MS * 2 ** min(upload.attempts, RETRY_MAX_STEPS)
    mw.progress.timer(delay, lambda: _retry_upload(waiting), False)
    redraw()
    if request.mode == UploadMode.HISTORY:
        tooltip(html.escape(message))


def _request_upload(request, *, attempts=0):
    if mw.col is None:
        return
    api = client()
    history = request.mode == UploadMode.HISTORY
    if not api.configured:
        if history:
            tooltip(tr("Set the server, player and token in AnkiQuest settings first."))
        else:
            preview.refresh(mw, api, request.show_feedback, tooltip)
        return

    active = _current_upload(api)
    if isinstance(active, Uploading):
        if history and active.request.mode == UploadMode.HISTORY:
            tooltip(tr("Uploading your whole review history…"))
            return
        active.queued = request.merged(active.queued)
        if history:
            tooltip(tr("Full history upload is queued."))
        return
    if isinstance(active, _NeedsSignIn):
        if history:
            tooltip(active.message)
        return
    if isinstance(active, _Waiting):
        if not history or active.request.mode == UploadMode.HISTORY:
            active.request = request.merged(active.request)
            if history:
                tooltip(html.escape(active.message))
            return
        request = request.merged(active.request)
        attempts = active.attempts

    # Capture the collection and profile before scheduling any work.
    col, prefs = mw.col, mw.pm.profile
    target = UploadTarget(col, prefs, (api.base, api.user, api.token))
    upload = state["upload"] = Uploading(target, request, attempts)
    saved = _upload_prefs(api)
    mark = int(saved.get("through", 0))
    initial_sync = "through" not in saved
    rollover = int(col.get_config("rollover", 4))
    now_ms = int(time.time() * 1000)
    day = study_day(now_ms, offset_west_min(), rollover)
    resync = request.mode >= UploadMode.RESYNC or request.mode == UploadMode.PROFILE_OPEN and saved.get("resynced_day") != day
    start = 0 if history else max(0, mark - RESYNC_WINDOW_MS) if resync else mark
    window_start = now_ms - UNDO_WINDOW_MS
    recent = {i for i in saved.get("recent", []) if i > window_start}
    shared = set(saved.get("shared", [])) if not history else set()
    stamps = review_offsets.journal(prefs)
    known_decks = dict(deck_names._state(prefs, api).get("conquests", {}))

    def progress(sent, total):
        if state["upload"] is upload and target.current(client()):
            tooltip(tr("%s of %s reviews uploaded") % (sent, total))

    def check_target():
        if state["upload"] is not upload or mw.col is not col or mw.pm.profile is not prefs:
            raise RuntimeError("AnkiQuest connection changed during upload")

    def remember_decks(records):
        if target.current(client()):
            deck_names._remember(prefs, api, records)

    def work():
        check_target()
        window = col.db.all(WINDOW_SQL, window_start)
        present, candidates, restored = reconcile(window, recent, start, mark)
        deleted = check_deletes(col.db, present, candidates)
        total = col.db.scalar(COUNT_SQL, start) if history else 0
        known = start
        first = True
        next_restore = sent_count = 0
        conquest_decks = {}

        # Restored rows and new rows share the same upload limit.
        while True:
            extra = restored[next_restore:next_restore + MAX_PENDING]
            room = MAX_PENDING - len(extra)
            rows = col.db.all(PENDING_SQL, known)[:room] if room else []
            if rows:
                known = rows[-1][0]
            sent = extra + rows
            full = len(sent) == MAX_PENDING
            decks = None
            clock_offset = offset_west_min()
            if not full and shared:
                try:
                    now_ms = int(time.time() * 1000)
                    decks = deck_snapshots(col, now_ms, clock_offset, rollover, only=shared)
                    # Do not send mixed-day counts if a rollover occurred while
                    # reading them. The next refresh will collect the new day.
                    if study_day(int(time.time() * 1000), clock_offset, rollover) != study_day(
                        now_ms, clock_offset, rollover
                    ):
                        decks = None
                except Exception as e:
                    print("ankiquest deck progress:", e)
            check_target()
            profile = api.upload(
                review_offsets.stamp(rows_to_reviews(sent), stamps), rollover, history or initial_sync or full, deleted if first else (),
                decks=decks, clock_offset=clock_offset,
            )
            found = deck_names._capture(col, profile.get("conquests", []), known_decks | conquest_decks)
            conquest_decks.update(found)
            if found:
                # Keep what this successful batch noticed even if a later batch fails.
                mw.taskman.run_on_main(lambda records=dict(found): remember_decks(records))
            first = False
            next_restore += len(extra)
            sent_count += len(sent)
            if history and sent:
                mw.taskman.run_on_main(lambda count=sent_count: progress(count, max(count, total)))
            if not full:
                return max(known, mark), sorted(present), profile, sent_count, conquest_decks

    def done(future):
        if state["upload"] is not upload:
            return
        state["upload"] = None
        if not target.current(client()):
            return
        try:
            result = future.result()
        except Exception as error:
            _upload_failed(upload, error)
            return
        try:
            uploaded(result)
        finally:
            if upload.queued is not None:
                _request_upload(upload.queued)

    def uploaded(result):
        # Advance the mark only after the whole upload was accepted.
        through, present, profile, sent, conquest_decks = result
        uploaded_at = int(time.time() * 1000)
        saved["through"], saved["recent"] = through, present
        saved["last_upload"] = uploaded_at
        deck_names._remember(prefs, api, conquest_decks)
        if resync:
            saved["resynced_day"] = day
        review_offsets.prune(prefs, through, uploaded_at)
        keep_goal(profile)
        redraw()
        if history:
            tooltip(tr("Uploaded %s reviews.") % sent)
            return
        # Wins arrive through the moment queue. Deck delivery confirmations and XP stay here.
        feedback = dict(profile.get("feedback") or {})
        feedback["headlines"] = [line for line in feedback.get("headlines", []) if line.startswith("📣")]
        message = notify.feedback({"feedback": feedback}, request.show_feedback)
        if message:
            tooltip(message[0], period=3500 if message[1] else 1800)
        if not initial_sync:
            _moments._offer(api, [moments._conquest(item, tr) for item in profile.get("conquests", [])
                                  if mark < item["at"] <= through])

    if history:
        tooltip(tr("Uploading your whole review history…"))
    mw.taskman.run_in_background(work, done, uses_collection=True)


# What an upload's answer says about today, kept over the last poll's: the goal's progress and
# the day's verdict, so reviews it just took in are not counted again as unsent.
FRESH_KEYS = ("goal", "today", "streak", "streak_state", "day_ends_at", "last_review_id", "at_risk", "streak_warning")


def keep_goal(answer):
    """An upload answers with the profile; its goal progress and today are fresher than the last poll's."""
    if not isinstance(answer, dict):
        return
    fresh = answer.get("profile") if isinstance(answer.get("profile"), dict) else answer
    if not isinstance(state["profile"], dict) or state["profile"].get("user") != fresh.get("user"):
        return
    for key in FRESH_KEYS:
        if key == "goal" and not isinstance(fresh.get(key), dict):
            continue
        if key in fresh:
            state["profile"][key] = fresh[key]


def refresh_shared_decks():
    api = client()
    if not api.configured:
        return
    target = UploadTarget(mw.col, mw.pm.profile, (api.base, api.user, api.token))

    def done(future):
        if not target.current(client()):
            return
        try:
            decks = future.result()
            _upload_prefs(api)["shared"] = decks
        except Exception as e:
            print("ankiquest shared decks:", e)

    mw.taskman.run_in_background(api.shared_decks, done)


def poll(quiet=False):
    """Keeps the deck list board, the inbox and the streak warning up to date."""
    # Queue offers first; the activity poll refreshes quiet hours before delivery.
    copies_ui.poll(quiet=True)
    api = client()
    account = [api.base, api.user, api.token]
    prefs = mw.pm.profile
    if not api.configured or prefs is None:
        return
    active = state["polling"]
    if active is not None and active[0] is prefs and active[1] == account:
        return
    request = (prefs, account)
    state["polling"] = request

    previous = prefs.get(ORDER_KEY) or []
    week = prefs.get(ORDER_WEEK_KEY)
    announcements = delivery._record(prefs, api)
    cursor = announcements["cursor"]

    def work():
        rank, profile = api.rank(previous, week), api.profile()
        notifications, arrivals = delivery._fetch(api, cursor)
        reminders = preferences._load_reminders(api)
        try:
            companion = api.companion()
        except Exception:
            companion = state["companion"] if state["companion_account"] == [api.base, api.user, api.token] else "aki"
        return rank, profile, notifications, companion, arrivals, reminders

    def done(future):
        if state["polling"] is not request:
            return
        state["polling"] = None
        current = client()
        if mw.pm.profile is not prefs or [current.base, current.user, current.token] != account:
            return
        try:
            rank, profile, inbox, companion, arrivals, reminders = future.result()
        except Exception as e:
            # Keep a direct sign-in link on the deck list for a revoked credential.
            if isinstance(e, ServerError) and e.status == HTTPStatus.UNAUTHORIZED:
                state.update(reauth=account, profile=None, place=None, inbox=[], activity=None)
                redraw()
                return
            print("ankiquest poll:", e)
            state["poll_failure"] = _PollFailure(time.monotonic() + RETRY_START_MS / 1000)
            redraw()
            return
        state["reauth"] = None
        state["poll_failure"] = None
        order = rank.get("order") or []
        state["place"] = order.index(api.name) + 1 if api.name in order else None
        state["profile"], state["activity"], state["inbox"] = profile, inbox, inbox.items
        state["companion"], state["companion_account"] = companion, account
        if reminders is not None:
            announcements["reminders"] = reminders
        announce(rank, profile, arrivals, quiet)
        redraw()

    mw.taskman.run_in_background(work, done)


def announce(rank, profile, inbox, quiet):
    settings = config()
    now_ms = int(time.time() * 1000)
    previous = mw.pm.profile.get(ORDER_KEY) or []
    mw.pm.profile[ORDER_KEY] = rank.get("order") or []
    mw.pm.profile[ORDER_WEEK_KEY] = rank.get("week")
    previous_cursor = mw.pm.profile.get(INBOX_KEY, 0)
    _, cursor = notify.fresh_messages(inbox, previous_cursor, now_ms // 1000)
    wins = [item for entry in inbox if entry["id"] > previous_cursor
            and entry.get("created_at", 0) >= now_ms // 1000 - notify.FRESH_SECONDS
            if (item := moments._notice(entry)) is not None]
    _delivery._receive(inbox, delivery._Mode.QUIET if quiet else delivery._Mode.ANNOUNCE)
    mw.pm.profile[INBOX_KEY] = cursor
    if quiet:
        return
    if settings.get("notify_rank", True) and rank.get("change"):
        place = share.climbed_to(previous, rank.get("order"), client().name)
        if place:
            wins.append(moments._rank(rank["change"], place, rank.get("week"), now_ms))
    _moments._offer(client(), wins)


def failed(template, error):
    """Errors can quote the server, so they are escaped before the rich-text tooltip."""
    tooltip(template % html.escape(str(error)))


def redraw():
    home._refresh_toolbar()
    if mw.state == "deckBrowser":
        mw.deckBrowser.refresh()


def on_deck_browser(deck_browser, content):
    api = client()
    if not api.configured:
        content.stats += board.welcome(tr)
        return
    if state["reauth"] == [api.base, api.user, api.token]:
        content.stats += board.welcome(tr, text=tr("Sign in to AnkiQuest again"))
        return
    failure = state["poll_failure"]
    if state["profile"] is None and not state["polling"] and (failure is None or time.monotonic() >= failure.retry_at):
        poll(quiet=True)
    page = state["activity"]
    waiting, actions = (page.unread, page.actions) if page is not None else (0, 0)
    companion = state["companion"] if state["companion_account"] == [api.base, api.user, api.token] else "aki"
    mascot = None if companion == "none" else "/_addons/%s/%s_face.png" % (mw.addonManager.addonFromModule(__name__), companion)
    today = home.today_view(deck_browser, companion)
    profile = today["profile"] if today is not None else home._profile_for_display(state["profile"])
    content.stats += board.html(profile, state["place"], waiting, tr, mascot, today, actions=actions)
    content.stats += board._connection_note(state["profile"], state["poll_failure"], tr)
    content.stats += '<p style="text-align:center;font-size:12px">%s</p>' % html.escape(_upload_status(api))


def on_js_message(handled, message, context):
    if message == "ankiquest:retry-connection" and context is mw.deckBrowser:
        poll(quiet=True)
        return (True, None)
    if message == "ankiquest:account":
        open_account()
        return (True, None)
    if message.startswith("ankiquest:web:"):
        page = message.split(":", 2)[2]
        if page == "history":
            home.open_progress()
            return (True, None)
        if page == "board":
            home._open_board()
            return (True, None)
        if page == "challenges":
            home._open_friends()
            return (True, None)
        if page in board.PAGES:
            open_page(board.PAGES[page])
        return (True, None)
    return handled


def open_page(path):
    api = client()
    if not api.base:
        tooltip(tr("Set the server in AnkiQuest settings first."))
        return
    web.open_page(mw, api, path.replace("{user}", api.user), openLink, tooltip)


def on_state_change(new_state, old_state):
    _moments._transition(new_state, old_state)
    _delivery._flush()


def open_inbox():
    mw.pm.profile[INBOX_KEY] = max(
        [entry["id"] for entry in state["inbox"]] + [mw.pm.profile.get(INBOX_KEY, 0)]
    )
    _delivery._acknowledge({entry["id"] for entry in state["inbox"]})
    open_native_inbox()
    redraw()


def account_error(error):
    if isinstance(error, InsecureServer):
        return tr("Use HTTPS to send a password outside your local network.")
    if isinstance(error, ServerError):
        if error.status == 401:
            return tr("Username or password not recognized.")
        if error.status == 403:
            return tr("This server does not take new accounts. Ask whoever runs it for a token.")
        if error.status == 409:
            return tr("That username is taken. Pick another one.")
        if error.status == 429:
            return tr("Too many attempts. Try again later.")
        if error.status == 400 and error.message:
            return error.message
        return tr("The server could not sign you in (%s).") % (error.message or error.status)
    return tr("Could not reach the server (%s).") % error


def submit_account(values, finish):
    device = "Anki desktop (%s)" % (platform.system() or "computer")

    def work():
        if values["new"]:
            return sign_up(values["url"], values["user"], values["password"], values["display"], device)
        return sign_in(values["url"], values["user"], values["password"], device)

    def done(future):
        try:
            answer = future.result()
        except Exception as e:
            finish(None, account_error(e))
            return
        finish({"url": values["url"], "user": answer["user"], "token": answer["token"]}, None)

    mw.taskman.run_in_background(work, done)


def ask_account(url, dismiss=None):
    return ui.account_dialog(mw, url, submit_account, state["companion"], dismiss=dismiss)


def open_account():
    answer = ask_account(config().get("url", ""))
    if answer:
        _finish_sign_in(answer)


def _finish_sign_in(answer):
    _save_account_config(answer)
    state["profile"] = None
    state["place"] = None
    state["companion"], state["companion_account"] = "aki", None
    refresh_shared_decks()
    refresh(False)
    poll(quiet=True)
    tooltip(tr("Signed in as %s.") % html.escape(answer["user"]))
    home.sync_goal(prompt=True)


def _save_account_config(values):
    """Keep upload progress with its account; discard data shown for the old connection."""
    old = client()
    settings = dict(config(), **values)
    new = Client(settings.get("url", ""), settings.get("user", ""), settings.get("token", ""))
    changed = (old.base, old.user, old.token) != (new.base, new.user, new.token)
    profile = mw.pm.profile
    checkpoint = _upload_prefs(old) if changed and old.configured and profile is not None else None
    if changed and old.configured and profile is not None:
        delivery._record(profile, old)
    save_config(values)
    if not changed:
        return

    # Retain the review watermark and undo history for a return to this account.
    if checkpoint is not None:
        checkpoint.pop("shared", None)
        checkpoint.pop("last_upload", None)
    if profile is not None:
        for key in (ORDER_KEY, ORDER_WEEK_KEY, INBOX_KEY, STREAK_DAY_KEY, copies_ui.SEEN_KEY):
            profile.pop(key, None)
    state.update(
        upload=None, polling=None, poll_failure=None, reauth=None, profile=None, place=None, inbox=[], activity=None,
        companion="aki", companion_account=None,
    )
    _moments._clear()
    _delivery._clear()
    home.clear_account()
    preview.clear_account()
    copies_ui.clear_account()
    inbox_ui.clear_account()
    web.clear_account()


def _offer_sign_in():
    profile = mw.pm.profile
    if profile is None:
        return

    def show():
        # Profile open -> ready deck browser -> one sign-in offer for this profile.
        settings = config()
        if (mw.pm.profile is not profile or profile.get(SIGN_IN_OFFER_KEY)
                or settings.get("user") or mw.state == "review"):
            return
        profile[SIGN_IN_OFFER_KEY] = True
        answer = ask_account(settings.get("url", ""), dismiss=tr("Not now"))
        if answer and mw.pm.profile is profile:
            _finish_sign_in(answer)

    QTimer.singleShot(0, show)


def open_settings():
    values = ui.settings_dialog(
        mw, config(), test_connection, upload_everything, state["companion"], ask_account,
        extra=settings_sections, on_manage=_manage_account,
        on_decks=open_deck_notifications, on_privacy=_open_privacy,
    )
    if values is None:
        return
    if values.get("update_channel") != config().get("update_channel", "stable"):
        mw.pm.profile[UPDATE_CHECK_KEY] = 0
    _save_account_config(values)
    state["profile"] = None
    state["place"] = None
    state["companion"], state["companion_account"] = "aki", None
    refresh_shared_decks()
    poll(quiet=True)
    home.sync_goal()
    redraw()
    tooltip(tr("AnkiQuest settings saved."))


def _manage_account(base=None):
    if base is None:
        api = client()
        if not api.configured:
            return
        base = api.base
    _open_site_page(base, "/settings")


def _open_privacy():
    _open_site_page(client().base or DEFAULT_SERVER, "/privacy")


def _open_site_page(base, path):
    try:
        server = urlsplit(base)
    except ValueError:
        return
    if server.scheme in ("https", "http") and server.hostname and server.username is None and not (server.query or server.fragment):
        openLink(web.page_url(base, path))


def settings_sections(dialog, groups):
    """Load account choices into their groups and return their Save callbacks."""
    api = client()
    target = UploadTarget(mw.col, mw.pm.profile, (api.base, api.user, api.token))

    def changed():
        # A poll started before Save must not overwrite the newly saved companion.
        if not target.current(client()):
            return
        state["polling"] = None
        poll(quiet=True)

    finishes = [found for found in (
        preferences.section(dialog, groups, mw, api, upload_status=_upload_status(api), on_saved=changed),
        sharing.section(groups["sharing"], mw, api),
    ) if found]
    if not finishes:
        return None
    return lambda: [finish() for finish in finishes]


def test_connection(values):
    api = Client(values["url"], values["user"], values["token"], anki.lang.current_lang)
    if not api.configured:
        tooltip(tr("Fill in the server, player and token first."))
        return

    def done(future):
        try:
            profile = future.result()
        except Exception as e:
            failed("AnkiQuest: %s", e)
            return
        tooltip(tr("Connected as %s, level %d.") % (values["user"], profile.get("level", 1)))

    mw.taskman.run_in_background(api.profile, done)


def upload_everything():
    _request_upload(Refresh(False, UploadMode.HISTORY))


def open_deck_notifications():
    api = client()
    if mw.col is None or not api.configured:
        tooltip(tr("Set the server, player and token in AnkiQuest settings first."))
        return
    col = mw.col
    target = UploadTarget(col, mw.pm.profile, (api.base, api.user, api.token))
    rollover = int(col.get_config("rollover", 4))

    def work():
        clock_offset = offset_west_min()
        catalog = deck_snapshots(col, int(time.time() * 1000), clock_offset, rollover)
        api.upload([], rollover, True, decks=catalog, clock_offset=clock_offset, catalog=True)
        return api.deck_settings()

    def done(future):
        if not target.current(client()):
            return
        try:
            settings = future.result()
        except Exception as e:
            failed(tr("AnkiQuest: could not load your decks (%s)"), e)
            return
        choice = ui.deck_dialog(mw, settings, state["companion"])
        if choice is None or not target.current(client()):
            return
        shared, unshared, recipients, nudges, celebrations = choice
        if shared and not recipients:
            tooltip(tr("Pick at least one person to notify, or share no decks."))
            return
        save_deck_choice(api, shared, unshared, recipients, nudges, celebrations)

    mw.taskman.run_in_background(work, done, uses_collection=True)


def save_deck_choice(api, shared, unshared, recipients, nudges, celebrations=None):
    target = UploadTarget(mw.col, mw.pm.profile, (api.base, api.user, api.token))
    if not target.current(client()):
        return

    def work():
        api.save_deck_settings(shared, unshared, recipients, nudges, celebrations)
        return api.shared_decks()

    def done(future):
        if not target.current(client()):
            return
        try:
            decks = future.result()
            _upload_prefs(api)["shared"] = decks
        except Exception as e:
            failed(tr("AnkiQuest: could not save your decks (%s)"), e)
            return
        tooltip(_plural("%d deck shared.", "%d decks shared.", len(shared)) % len(shared))

    mw.taskman.run_in_background(work, done)


def open_website():
    open_page(board.PAGES["profile"])


def on_operation(changes, handler):
    if handler is mw.reviewer:
        return
    if getattr(getattr(changes, "changes", changes), "study_queues", False):
        refresh(mw.state == "review")


def on_profile_open():
    state["poll_failure"] = None
    if state["timer"] is None:
        state["timer"] = mw.progress.timer(POLL_MS, poll, repeat=True, parent=mw)
    refresh_shared_decks()
    _request_upload(Refresh(False, UploadMode.PROFILE_OPEN))
    poll()
    maybe_check_updates()
    _offer_sign_in()
    _moments._transition(mw.state, None)


def check_updates(quiet=True):
    if updates._source(ADDON_DIR) is updates._Source.ANKIWEB:
        if not quiet:
            tooltip(tr("This installation updates through AnkiWeb. Use Tools → Add-ons → Check for Updates."))
        return
    current = updates.installed(ADDON_DIR)
    if current is None:
        if not quiet:
            tooltip(tr("This AnkiQuest is a local copy; it does not update itself."))
        return
    channel = config().get("update_channel", "stable")

    def done(future):
        try:
            candidate = future.result()
        except Exception as e:
            if not quiet:
                failed(tr("AnkiQuest: could not check for updates (%s)"), e)
            return
        if updates.newer(candidate, current):
            offer_update(candidate)
        elif not quiet:
            tooltip(tr("AnkiQuest is up to date (%s).") % updates.label(current))

    mw.taskman.run_in_background(lambda: updates.available(channel), done)


def maybe_check_updates():
    if updates._source(ADDON_DIR) is updates._Source.ANKIWEB:
        return
    now_ms = int(time.time() * 1000)
    if not updates.due(mw.pm.profile.get(UPDATE_CHECK_KEY, 0), now_ms):
        return
    mw.pm.profile[UPDATE_CHECK_KEY] = now_ms
    check_updates(quiet=True)


def offer_update(candidate):
    from aqt.utils import askUser

    if not askUser(tr("%s is available. Install it now? Anki needs a restart afterwards.") % updates.label(candidate)):
        return
    target = Path(tempfile.gettempdir()) / "ankiquest-update.ankiaddon"

    def done(future):
        try:
            path = future.result()
        except Exception as e:
            failed(tr("AnkiQuest: could not download the update (%s)"), e)
            return
        install_update(path)

    mw.taskman.run_in_background(lambda: updates.download(candidate["url"], target), done)


def install_update(path):
    from aqt.addons import InstallError
    from aqt.utils import showInfo

    result = mw.addonManager.install(str(path))
    if isinstance(result, InstallError):
        tooltip(tr("AnkiQuest: the update could not be installed (%s)") % result.errmsg)
        return
    showInfo(tr("AnkiQuest was updated. Restart Anki to use the new version."))


def on_answer(_reviewer, card, _ease):
    """Remembers the time zone the answer was made in, then uploads it."""
    try:
        answered = mw.col.db.scalar("select max(id) from revlog where cid = ?", card.id)
        review_offsets.record(mw.pm.profile, answered, int(time.time() * 1000))
    except Exception as e:
        print("ankiquest review offset:", e)
    refresh(True)


def on_sync():
    refresh_shared_decks()
    refresh(False, resync=True)
    poll()


def add_action(title, handler, shortcut=None):
    action = QAction(title, mw)
    action.triggered.connect(handler)
    if shortcut:
        action.setShortcut(QKeySequence(shortcut))
    _tools.addAction(action)
    return action


# Keep one Tools entry; deck-copy counts update their action inside the same submenu.
_tools = QMenu("AnkiQuest", mw)
mw.form.menuTools.addAction(_tools.menuAction())
add_action(tr("Today"), lambda: open_page(board.PAGES["today"]))
add_action(tr("Inbox"), open_inbox, inbox_ui.SHORTCUT)
add_action(tr("Friends"), home._open_friends)
copies_ui.setup(mw, lambda: client(), lambda: state["companion"], _tools)
add_action(tr("Website"), open_website, WEBSITE_SHORTCUT)
add_action(tr("Settings"), open_settings)
add_action(tr("Check for updates"), lambda: check_updates(quiet=False))


gui_hooks.reviewer_did_answer_card.append(on_answer)
gui_hooks.operation_did_execute.append(on_operation)
gui_hooks.state_did_change.append(on_state_change)
gui_hooks.sync_did_finish.append(on_sync)
gui_hooks.profile_did_open.append(on_profile_open)
gui_hooks.deck_browser_will_render_content.append(on_deck_browser)
gui_hooks.webview_did_receive_js_message.append(on_js_message)
if hasattr(gui_hooks, "profile_will_close"):
    gui_hooks.profile_will_close.append(inbox_ui.clear_account)
    gui_hooks.profile_will_close.append(copies_ui.clear_account)
    gui_hooks.profile_will_close.append(preview.clear_account)
    gui_hooks.profile_will_close.append(lambda: _delivery._clear())


def current_companion():
    api = client()
    return state["companion"] if state["companion_account"] == [api.base, api.user, api.token] else "aki"


def open_native_inbox():
    api = client()
    if not api.configured:
        tooltip(tr("Set the server, player and token in AnkiQuest settings first."))
        return
    dialog = inbox_ui.inbox_dialog(mw, api, current_companion(), client, mw.taskman.run_in_background, _open_inbox_path, on_replied)
    dialog.context._after_change = lambda: poll(quiet=True)


def _open_inbox_path(path):
    target = CHALLENGE_ROUTE.fullmatch(path)
    if target:
        home._open_friends(int(target.group(1)))
        return
    open_page(path)


def on_replied(notification):
    for entry in state["inbox"]:
        if entry.get("id") == notification:
            entry["replied"] = True
    redraw()


_moments = moments_ui._Presenter(sys.modules[__name__])
_delivery = delivery._Delivery(sys.modules[__name__])
copies_ui.state["delivery"] = _delivery
heatmap_ui.install(sys.modules[__name__])
home.install(sys.modules[__name__])
