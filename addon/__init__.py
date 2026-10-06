import platform
import tempfile
import time
from pathlib import Path

import anki.lang

from aqt import gui_hooks, mw
from aqt.qt import QAction
from aqt.utils import openLink, tooltip

from . import board, notify, ui, updates, web
from .language import tr
from .client import (
    MAX_PENDING,
    PENDING_SQL,
    UNDO_WINDOW_MS,
    AccountError,
    Client,
    offset_west_min,
    reconcile,
    rows_to_reviews,
    sign_in,
    sign_up,
)
from .deck_completion import deck_snapshots, study_day

MARK_KEY = "ankiquestUploadedThrough"
RECENT_KEY = "ankiquestRecentUploads"
BASELINE_KEY = "ankiquestBaselineAccount"
SHARED_KEY = "ankiquestSharedDecks"
ORDER_KEY = "ankiquestLastOrder"
INBOX_KEY = "ankiquestInboxCursor"
STREAK_DAY_KEY = "ankiquestStreakDay"
UPDATE_CHECK_KEY = "ankiquestUpdateCheckedAt"
ADDON_DIR = Path(__file__).resolve().parent
RESYNC_WINDOW_MS = 7 * 86_400_000
POLL_MS = 5 * 60 * 1000

state = {
    "busy": False,
    "again": False,
    "polling": False,
    "timer": None,
    "place": None,
    "profile": None,
    "companion": "aki",
    "companion_account": None,
    "inbox": [],
    # A conquered card's big celebration, held until the player leaves the reviewer.
    "conquest": None,
}

mw.addonManager.setWebExports(__name__, r"(aki|ankilope)_face\.png")


def config():
    return mw.addonManager.getConfig(__name__) or {}


def save_config(values):
    current = config()
    current.update(values)
    mw.addonManager.writeConfig(__name__, current)


def client():
    settings = config()
    return Client(settings.get("url", ""), settings.get("user", ""), settings.get("token", ""), anki.lang.current_lang)


def refresh(show_feedback, resync=False):
    if mw.col is None:
        return
    if state["busy"]:
        state["again"] = True
        return
    api = client()
    if not api.configured:
        return
    state["busy"] = True
    rollover = int(mw.col.get_config("rollover", 4))
    mark = int(mw.pm.profile.get(MARK_KEY, 0))
    account = [api.base, api.user]
    initial_sync = mark == 0 and mw.pm.profile.get(BASELINE_KEY) != account
    start = max(0, mark - RESYNC_WINDOW_MS) if resync else mark
    window_start = int(time.time() * 1000) - UNDO_WINDOW_MS
    recent = {i for i in mw.pm.profile.get(RECENT_KEY, []) if i > window_start}
    shared = set(mw.pm.profile.get(SHARED_KEY, []))

    def work():
        window = mw.col.db.all(PENDING_SQL, window_start)
        present, deleted, restored = reconcile(window, recent, start, mark)
        known = start
        first = True
        while True:
            rows = mw.col.db.all(PENDING_SQL, known)
            full = len(rows) == MAX_PENDING
            if rows:
                known = rows[-1][0]
            sent = rows + restored if first else rows
            decks = None
            clock_offset = offset_west_min()
            if not full and shared:
                try:
                    now_ms = int(time.time() * 1000)
                    decks = deck_snapshots(mw.col, now_ms, clock_offset, rollover, only=shared)
                    # Do not send mixed-day counts if a rollover occurred while
                    # reading them. The next refresh will collect the new day.
                    if study_day(int(time.time() * 1000), clock_offset, rollover) != study_day(
                        now_ms, clock_offset, rollover
                    ):
                        decks = None
                except Exception as e:
                    print("ankiquest deck progress:", e)
            profile = api.upload(
                rows_to_reviews(sent), rollover, initial_sync or full, deleted if first else (),
                decks=decks, clock_offset=clock_offset,
            )
            first = False
            if not full:
                return max(known, mark), sorted(present), profile

    def done(future):
        state["busy"] = False
        try:
            mw.pm.profile[MARK_KEY], mw.pm.profile[RECENT_KEY], profile = future.result()
            mw.pm.profile[BASELINE_KEY] = account
        except Exception as e:
            print("ankiquest:", e)
            return
        message = notify.feedback(profile, show_feedback)
        if message:
            tooltip(message[0], period=3500 if message[1] else 1800)
        conquest = notify.conquest_path(profile)
        if conquest:
            state["conquest"] = conquest
            if mw.state != "review":
                show_conquest()
        if state["again"]:
            state["again"] = False
            refresh(True)

    mw.taskman.run_in_background(work, done, uses_collection=True)


def refresh_shared_decks():
    api = client()
    if not api.configured:
        return

    def done(future):
        try:
            mw.pm.profile[SHARED_KEY] = future.result()
        except Exception as e:
            print("ankiquest shared decks:", e)

    mw.taskman.run_in_background(api.shared_decks, done)


def poll(quiet=False):
    """Keeps the deck list board, the inbox and the streak warning up to date."""
    api = client()
    if not api.configured or state["polling"]:
        return
    state["polling"] = True

    previous = mw.pm.profile.get(ORDER_KEY) or []

    def work():
        rank, profile, notifications = api.rank(previous), api.profile(), api.notifications()
        try:
            companion = api.companion()
        except Exception:
            companion = state["companion"] if state["companion_account"] == [api.base, api.user, api.token] else "aki"
        return rank, profile, notifications, companion

    def done(future):
        state["polling"] = False
        try:
            rank, profile, inbox, companion = future.result()
        except Exception as e:
            print("ankiquest poll:", e)
            return
        order = rank.get("order") or []
        state["place"] = order.index(api.name) + 1 if api.name in order else None
        state["profile"], state["inbox"] = profile, inbox
        state["companion"], state["companion_account"] = companion, [api.base, api.user, api.token]
        announce(rank, profile, inbox, quiet)
        redraw()

    mw.taskman.run_in_background(work, done)


def announce(rank, profile, inbox, quiet):
    settings = config()
    now_ms = int(time.time() * 1000)
    mw.pm.profile[ORDER_KEY] = rank.get("order") or []
    fresh, cursor = notify.fresh_messages(inbox, mw.pm.profile.get(INBOX_KEY, 0), now_ms // 1000)
    mw.pm.profile[INBOX_KEY] = cursor
    warning = notify.streak_warning(
        profile,
        int(settings.get("streak_hours", 2) or 0),
        now_ms,
        mw.pm.profile.get(STREAK_DAY_KEY),
    )
    if warning:
        mw.pm.profile[STREAK_DAY_KEY] = warning[1]
    if quiet:
        return

    if settings.get("notify_rank", True) and rank.get("change"):
        tooltip(notify.notice(rank["change"]), period=4000)
    if warning:
        tooltip(warning[0], period=5000)
    for entry in fresh:
        text = notify.summarize(entry).replace("\n", "<br>")
        if notify.answerable(entry):
            text += "<br><i>Tools → ankiquest inbox to reply</i>"
        tooltip(text, period=6000)


def redraw():
    if mw.state == "deckBrowser":
        mw.deckBrowser.refresh()


def on_deck_browser(deck_browser, content):
    api = client()
    if not api.configured:
        content.stats += board.welcome(tr)
        return
    if state["profile"] is None and not state["polling"]:
        poll(quiet=True)
    waiting = sum(1 for entry in state["inbox"] if notify.answerable(entry))
    companion = state["companion"] if state["companion_account"] == [api.base, api.user, api.token] else "aki"
    mascot = None if companion == "none" else "/_addons/%s/%s_face.png" % (mw.addonManager.addonFromModule(__name__), companion)
    content.stats += board.html(state["profile"], state["place"], waiting, tr, mascot)


def on_js_message(handled, message, context):
    if message == "ankiquest:account":
        open_account()
        return (True, None)
    if message.startswith("ankiquest:web:"):
        page = message.split(":", 2)[2]
        if page in board.PAGES:
            open_page(board.PAGES[page])
        return (True, None)
    return handled


def open_page(path):
    api = client()
    if not api.base:
        tooltip(tr("Set the server in ankiquest settings first."))
        return
    web.open_page(mw, api, path.replace("{user}", api.user), openLink, tooltip)


def show_conquest():
    path, state["conquest"] = state["conquest"], None
    if path:
        open_page(path)


def on_state_change(new_state, _old_state):
    if new_state != "review":
        show_conquest()


def open_inbox():
    mw.pm.profile[INBOX_KEY] = max(
        [entry["id"] for entry in state["inbox"]] + [mw.pm.profile.get(INBOX_KEY, 0)]
    )
    open_page(board.PAGES["inbox"])
    redraw()


def account_error(error):
    if isinstance(error, AccountError):
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


def ask_account(url):
    return ui.account_dialog(mw, url, submit_account, state["companion"])


def open_account():
    answer = ask_account(config().get("url", ""))
    if not answer:
        return
    save_config(dict(config(), **answer))
    state["profile"] = None
    state["place"] = None
    state["companion"], state["companion_account"] = "aki", None
    refresh_shared_decks()
    refresh(False, resync=True)
    poll(quiet=True)
    tooltip(tr("Signed in as %s.") % answer["user"])


def open_settings():
    values = ui.settings_dialog(mw, config(), test_connection, upload_everything, state["companion"], ask_account)
    if values is None:
        return
    if values.get("update_channel") != config().get("update_channel", "stable"):
        mw.pm.profile[UPDATE_CHECK_KEY] = 0
    save_config(values)
    state["profile"] = None
    state["place"] = None
    state["companion"], state["companion_account"] = "aki", None
    refresh_shared_decks()
    poll(quiet=True)
    tooltip(tr("ankiquest settings saved."))


def test_connection(values):
    api = Client(values["url"], values["user"], values["token"], anki.lang.current_lang)
    if not api.configured:
        tooltip(tr("Fill in the server, player and token first."))
        return

    def done(future):
        try:
            profile = future.result()
        except Exception as e:
            tooltip("ankiquest: %s" % e)
            return
        tooltip(tr("Connected as %s, level %d.") % (values["user"], profile.get("level", 1)))

    mw.taskman.run_in_background(api.profile, done)


def upload_everything():
    mw.pm.profile[MARK_KEY] = 0
    mw.pm.profile[RECENT_KEY] = []
    refresh(False, resync=True)
    tooltip(tr("Uploading your whole review history…"))


def open_companion():
    api = client()
    if not api.configured:
        tooltip(tr("Set the server, player and token in ankiquest settings first."))
        return

    def loaded(future):
        try:
            current = future.result()
        except Exception as error:
            tooltip("ankiquest: %s" % error)
            return
        if [client().base, client().user, client().token] != [api.base, api.user, api.token]:
            return
        selected = ui.companion_dialog(mw, current)
        if selected is None or selected == current:
            return

        def saved(future):
            try:
                companion = future.result()
            except Exception as error:
                tooltip("ankiquest: %s" % error)
                return
            if [client().base, client().user, client().token] != [api.base, api.user, api.token]:
                return
            state["companion"], state["companion_account"] = companion, [api.base, api.user, api.token]
            redraw()
            tooltip(tr("Study companion saved across your devices."))

        mw.taskman.run_in_background(lambda: api.set_companion(selected), saved)

    mw.taskman.run_in_background(api.companion, loaded)


def open_deck_notifications():
    api = client()
    if mw.col is None or not api.configured:
        tooltip(tr("Set the server, player and token in ankiquest settings first."))
        return
    rollover = int(mw.col.get_config("rollover", 4))

    def work():
        clock_offset = offset_west_min()
        catalog = deck_snapshots(mw.col, int(time.time() * 1000), clock_offset, rollover)
        api.upload([], rollover, True, decks=catalog, clock_offset=clock_offset, catalog=True)
        return api.deck_settings()

    def done(future):
        try:
            settings = future.result()
        except Exception as e:
            tooltip(tr("ankiquest: could not load your decks (%s)") % e)
            return
        choice = ui.deck_dialog(mw, settings, state["companion"])
        if choice is None:
            return
        shared, unshared, recipients, nudges, celebrations = choice
        if shared and not recipients:
            tooltip(tr("Pick at least one person to notify, or share no decks."))
            return
        save_deck_choice(api, shared, unshared, recipients, nudges, celebrations)

    mw.taskman.run_in_background(work, done, uses_collection=True)


def save_deck_choice(api, shared, unshared, recipients, nudges, celebrations=None):
    def work():
        api.save_deck_settings(shared, unshared, recipients, nudges, celebrations)
        return api.shared_decks()

    def done(future):
        try:
            mw.pm.profile[SHARED_KEY] = future.result()
        except Exception as e:
            tooltip(tr("ankiquest: could not save your decks (%s)") % e)
            return
        tooltip(tr("%d deck shared." if len(shared) == 1 else "%d decks shared.") % len(shared))

    mw.taskman.run_in_background(work, done)


def open_website():
    open_page(board.PAGES["profile"])


def on_operation(changes, handler):
    if handler is mw.reviewer:
        return
    if getattr(getattr(changes, "changes", changes), "study_queues", False):
        refresh(mw.state == "review")


def on_profile_open():
    if state["timer"] is None:
        try:
            state["timer"] = mw.progress.timer(POLL_MS, poll, repeat=True, parent=mw)
        except TypeError:
            state["timer"] = mw.progress.timer(POLL_MS, poll, True)
    refresh_shared_decks()
    refresh(False, resync=True)
    poll(quiet=True)
    maybe_check_updates()


def check_updates(quiet=True):
    current = updates.installed(ADDON_DIR)
    if current is None:
        if not quiet:
            tooltip(tr("This ankiquest is a local copy; it does not update itself."))
        return
    channel = config().get("update_channel", "stable")

    def done(future):
        try:
            candidate = future.result()
        except Exception as e:
            if not quiet:
                tooltip(tr("ankiquest: could not check for updates (%s)") % e)
            return
        if updates.newer(candidate, current):
            offer_update(candidate)
        elif not quiet:
            tooltip(tr("ankiquest is up to date (%s).") % updates.label(current))

    mw.taskman.run_in_background(lambda: updates.available(channel), done)


def maybe_check_updates():
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
            tooltip(tr("ankiquest: could not download the update (%s)") % e)
            return
        install_update(path)

    mw.taskman.run_in_background(lambda: updates.download(candidate["url"], target), done)


def install_update(path):
    from aqt.addons import InstallError
    from aqt.utils import showInfo

    result = mw.addonManager.install(str(path))
    if isinstance(result, InstallError):
        tooltip(tr("ankiquest: the update could not be installed (%s)") % result.errmsg)
        return
    showInfo(tr("ankiquest was updated. Restart Anki to use the new version."))


def on_sync():
    refresh_shared_decks()
    refresh(False, resync=True)
    poll()


def add_action(title, handler):
    action = QAction(title, mw)
    action.triggered.connect(handler)
    mw.form.menuTools.addAction(action)


add_action(tr("ankiquest: sign in or create account…"), open_account)
add_action(tr("ankiquest settings…"), open_settings)
add_action(tr("ankiquest study companion…"), open_companion)
add_action(tr("ankiquest deck notifications…"), open_deck_notifications)
add_action(tr("ankiquest inbox…"), open_inbox)
add_action(tr("ankiquest on the web…"), open_website)
add_action(tr("ankiquest: check for updates…"), lambda: check_updates(quiet=False))


gui_hooks.reviewer_did_answer_card.append(lambda *_: refresh(True))
gui_hooks.operation_did_execute.append(on_operation)
gui_hooks.state_did_change.append(on_state_change)
gui_hooks.sync_did_finish.append(on_sync)
gui_hooks.profile_did_open.append(on_profile_open)
gui_hooks.deck_browser_will_render_content.append(on_deck_browser)
gui_hooks.webview_did_receive_js_message.append(on_js_message)
