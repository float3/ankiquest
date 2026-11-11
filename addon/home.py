"""Connects the Today panel, deck list badges, the daily goal and session recaps to Anki."""

import time
import json
from dataclasses import dataclass
from datetime import date, timedelta
from enum import Enum, auto
from pathlib import Path
from html import escape

from aqt import gui_hooks, mw

from . import address, day_verdict, goal_ui, goals, heatmap_ui, moods, today, today_ui
from .board import PAGES, _day_label
from .client import Client, offset_west_min
from .deck_completion import deck_snapshots, study_day
from .language import tr
from . import progress_ui

GOAL_KEY = "ankiquestDayGoal"
PLAN_KEY = "ankiquestServerGoal"
REACHED_KEY = "ankiquestGoalReported"
SETUP_SHOWN = "goal_setup_shown"
STREAK_MINUTES_KEY = "ankiquestStreakMinutes"
CELEBRATED_KEY = "ankiquestGoalCelebrated"
PERIOD_KEY = "ankiquestBoardPeriod"
SESSION_KEY = "ankiquestLastStudySession"
ART = Path(__file__).resolve().parent / "art"
BOARD_MS = 5 * 60 * 1000
BOARD_RETRY_MS = 30_000
BOARD_RETRY_CAP_MS = BOARD_MS
CACHE_MS = 2000
_RECAP_MS = 7 * 86_400_000

local = {
    "addon": None,
    "board": None,
    "board_account": None,
    "board_request": None,
    "board_retry": None,
    "session": None,
    "timer": None,
    "days": None,
    "restarted": None,
    "reporting": False,
    "goal_request": None,
    "capabilities": None,
    "capabilities_loading": False,
    "moving": False,
    "progress": None,
    "browser_buttons": set(),
}


def now_ms():
    return int(time.time() * 1000)


def addon():
    return local["addon"]


def account(api):
    return [api.base, api.user, api.token]


@dataclass
class _Request:
    who: list
    profile: dict

    def owns(self, api):
        return self.who == account(api) and self.profile is mw.pm.profile


class _GoalMode(Enum):
    SILENT = auto()
    OFFER_SETUP = auto()


@dataclass
class _GoalRequest(_Request):
    mode: _GoalMode


@dataclass
class _BoardRetry:
    request: _Request
    at: int
    delay: int


class _BoardFetch(Enum):
    AUTOMATIC = auto()
    MANUAL = auto()


def companion():
    state, api = addon().state, addon().client()
    return state["companion"] if state["companion_account"] == account(api) else "aki"


def art_url(chosen_companion, mood):
    name = moods.art(chosen_companion, mood)
    if name is None:
        return None
    return "/_addons/%s/art/%s.png" % (mw.addonManager.addonFromModule(__name__), name)


def art_path(chosen_companion, mood):
    name = moods.art(chosen_companion, mood)
    return None if name is None else str(ART / (name + ".png"))


def days_for(deck_browser=None):
    """The deck tree and each deck's daily state, read once per deck list render."""
    tree = getattr(deck_browser, "_dueTree", None) if deck_browser is not None else None
    cached = local["days"]
    if cached and tree is not None and cached[0] is tree and now_ms() - cached[1] < CACHE_MS:
        return tree, cached[2]
    col = mw.col
    tree = tree or col.sched.deck_due_tree()
    moment = now_ms()
    rollover = int(col.get_config("rollover", 4))
    snapshots = deck_snapshots(col, moment, offset_west_min(), rollover, tree=tree)
    days = today.load_days(col, moment, tree, snapshots)
    local["days"] = (tree, moment, days)
    return tree, days


def server_day():
    """Today as the server counts Anki days: days since the epoch, starting at the rollover hour."""
    rollover = int(mw.col.get_config("rollover", 4)) if mw.col is not None else 4
    return study_day(now_ms(), offset_west_min(), rollover)


def owner(api):
    return [api.base, api.user]


def current_plan(day):
    """The server's goal: from the latest profile, else as last remembered for this account."""
    api = addon().client()
    view = (moods.current(addon().state["profile"], now_ms()) or {}).get("goal")
    found, record = goals.remembered_plan(view, mw.pm.profile.get(PLAN_KEY), owner(api), day)
    if record is not None:
        mw.pm.profile[PLAN_KEY] = record
    return found


def remember_plan(answer):
    """Keeps a `/api/goal` answer, and puts it in the profile so the panel shows it before the next poll."""
    api = addon().client()
    found, record = goals.remembered_plan(answer, mw.pm.profile.get(PLAN_KEY), owner(api), server_day())
    if record is None:
        return found
    mw.pm.profile[PLAN_KEY] = record
    profile = addon().state["profile"]
    if isinstance(profile, dict):
        profile["goal"] = {"mode": found.mode, "value": found.value, "since": found.since, "progress": None, "reached": False}
    return found


def goal_status(tree=None):
    """Today's goal status; the first reading each day fixes the goal."""
    api = addon().client()
    minutes, kept = goals.remembered_streak_minutes(
        addon().state["profile"], mw.pm.profile.get(STREAK_MINUTES_KEY), owner(api),
    )
    mw.pm.profile[STREAK_MINUTES_KEY] = kept
    day = server_day()
    found = current_plan(day)
    load = goals.load(mw.col, tree)
    moment = now_ms()
    profile = moods.current(addon().state["profile"], moment)
    verdict = day_verdict.verdict(profile, day_verdict.unsent(mw.col, profile), moment) if profile else None
    status, record, restart = goals.status(
        load, found, day, mw.pm.profile.get(GOAL_KEY), minutes, profile, verdict,
    )
    mw.pm.profile[GOAL_KEY] = record
    if restart:
        restart_plan(api, found, day)
    report_reached(api, status, day)
    return status


def restart_plan(api, found, day):
    """The backlog plan ran out: it starts again from today, once."""
    if not api.configured or local["restarted"] == [owner(api), day]:
        return
    local["restarted"] = [owner(api), day]

    def done(future):
        try:
            answer = future.result()
        except Exception as e:
            print("ankiquest goal restart:", e)
            return
        if owner(addon().client()) == owner(api):
            remember_plan(answer)

    mw.taskman.run_in_background(lambda: goals.put(api, goals.BACKLOG, found.value, restart=True), done)


def report_reached(api, status, day):
    """Tells the server, once a day, that a backlog goal is reached; it judges the other goals itself."""
    if not api.configured or local["reporting"]:
        return
    if not goals.to_report(status, owner(api), day, mw.pm.profile.get(REACHED_KEY)):
        return
    local["reporting"] = True

    def done(future):
        local["reporting"] = False
        try:
            future.result()
        except Exception as e:
            if getattr(e, "status", None) != 409:
                print("ankiquest goal reached:", e)
                return
        mw.pm.profile[REACHED_KEY] = {"account": owner(api), "day": day}

    mw.taskman.run_in_background(lambda: goals.report_reached(api, day), done)


def sync_goal(prompt=False):
    """Reads the server's goal, moves an older add-on's local goal there if none was chosen yet,
    and with `prompt` asks for a goal once per account while none is chosen."""
    api = addon().client()
    if not api.configured:
        return
    # An explicit setup joins a silent read for the same profile.
    mode = _GoalMode.OFFER_SETUP if prompt else _GoalMode.SILENT
    pending = local["goal_request"]
    if pending is not None and pending.owns(api):
        if mode is _GoalMode.OFFER_SETUP:
            pending.mode = mode
        return
    request = _GoalRequest(account(api), mw.pm.profile, mode)
    local["goal_request"] = request
    legacy = goals.legacy_goal(addon().config())

    def work():
        answer = goals.fetch(api)
        if legacy and not answer.get("chosen"):
            answer = goals.put(api, legacy["mode"], legacy.get("value"))
        return answer

    def done(future):
        if local["goal_request"] is not request:
            return
        local["goal_request"] = None
        try:
            answer = future.result()
        except Exception as e:
            print("ankiquest goal:", e)
            return
        if not request.owns(addon().client()):
            return
        addon().drop_config(goals.LEGACY_KEYS)
        remember_plan(answer)
        addon().redraw()
        if request.mode is _GoalMode.OFFER_SETUP:
            setup_prompt(api, answer)

    mw.taskman.run_in_background(work, done)


def setup_prompt(api, answer):
    """Opens the goal dialog once per account while the player has not chosen a goal.
    Closing it without saving counts as shown."""
    if not isinstance(answer, dict) or answer.get("chosen"):
        return
    key = "%s %s" % (api.base, api.name)
    shown = list(addon().config().get(SETUP_SHOWN) or [])
    if key in shown:
        return
    addon().save_config({SETUP_SHOWN: shown + [key]})
    open_goal_dialog()


def collection_forecast():
    if mw.col is None:
        return None
    try:
        return goals.forecast(mw.col)
    except Exception as e:
        print("ankiquest goal forecast:", e)
        return None


def open_goal_dialog():
    """The daily goal dialog, saved to the server."""
    api = addon().client()
    if not api.configured:
        addon().tooltip(tr("Set the server, player and token in AnkiQuest settings first."))
        return
    answer = goal_ui.goal_dialog(mw, current_plan(server_day()), collection_forecast())
    if answer is None:
        return
    mode, value = answer

    def done(future):
        try:
            saved = future.result()
        except Exception as e:
            addon().failed(tr("Could not save your daily goal (%s)."), e)
            return
        if owner(addon().client()) != owner(api):
            return
        remember_plan(saved)
        addon().redraw()
        addon().tooltip(tr("Daily goal saved."))

    mw.taskman.run_in_background(lambda: goals.put(api, mode, value), done)


def goal_view(status):
    view = {
        "minimum": goals.minimum_text(status, tr), "minimum_meter": None, "daily": None, "backlog": None,
        "action": tr("Set a daily goal") if status.goal is None else tr("Change goal"),
    }
    if status.streak_minutes > 0 and not status.minimum_done:
        view["minimum_meter"] = (status.minimum_minutes, status.streak_minutes)
    if status.goal is None:
        return view
    if status.goal_done:
        view["daily"] = tr("Daily goal reached! 🎉")
    else:
        view["daily"] = tr("Daily goal: %s") % goals.progress_text(status, tr)
    view["daily_meter"] = (status.progress, status.goal.target)
    view["backlog"] = goals.backlog_text(status, tr)
    return view


def focus_view(day):
    if day is None:
        return None
    return {
        "id": day.id, "name": day.name, "review": day.review, "learning": day.learning, "new": day.new,
        "status": day.status, "label": today.label(day, tr), "ready": day.ready_count,
    }


def board_view(api, profile=None):
    if local["board_account"] != account(api):
        local["board"] = None
    if local["board"] is None:
        refresh_board()
    if local["board"] is None:
        retry = local["board_retry"]
        return {"retry": True} if retry is not None and retry.request.owns(api) else None
    chosen = today.period(mw.pm.profile.get(PERIOD_KEY))
    rows = []
    for place, entry, mine in today.standings(local["board"], chosen, api.name):
        if mine:
            entry = dict(entry, day=profile or {"_day_status": "unknown"})
            if chosen == "day" and entry["day"].get("_day_status") == "unknown":
                entry["xp"] = None
        rows.append((place, entry, mine))
    return {"period": chosen, "rows": rows}


def _profile_for_display(profile, now=None):
    moment = now_ms() if now is None else now
    try:
        found = day_verdict.verdict(profile, day_verdict.unsent(mw.col, profile), moment)
    except Exception as error:
        print("ankiquest day verdict:", error)
        found = None
    return day_verdict._for_display(profile, found)


def last_session(api):
    stored = mw.pm.profile.get(SESSION_KEY)
    if not isinstance(stored, dict) or stored.get("account") != [api.base, api.user]:
        return None
    return today.session_text(stored, tr)


def _session_day(summary):
    """The session's day, judged by the server; never infer a past verdict from its minutes."""
    profile = addon().state["profile"] or {}
    starts = (profile.get("today") or {}).get("starts_at")
    ends, finished = profile.get("day_ends_at"), summary.get("finished_at", 0)
    if starts is not None and ends and starts <= finished < ends:
        view = _profile_for_display(profile)
        state = view["_day_status"]
        label = _day_label(view, tr)
        if state == "pending":
            state = "short" if view["today"].get("reviews", 0) else "none"
            label = heatmap_ui._state_label(state, tr)
        return {"state": state, "label": label}
    cell = next((cell for cell in profile.get("heatmap", []) if cell.get("date") == summary.get("day")), {})
    state = cell.get("state", "unknown")
    return {"state": state, "label": heatmap_ui._state_label(state, tr)}


def _streak_title(summary):
    return tr("Streak secured! 🔥") if _session_day(summary)["state"] == "met" else tr("Streak minimum reached — syncing")


def today_view(deck_browser, chosen_companion):
    """What board.html needs for the Today panel, or None when the collection cannot be read."""
    try:
        tree, days = days_for(deck_browser)
        focus = today.focus(days, mw.col.decks.get_current_id())
        status = goal_status(tree)
    except Exception as e:
        print("ankiquest today:", e)
        return None
    api = addon().client()
    profile = day_verdict._for_display(addon().state["profile"], status.verdict)
    mood = moods.mood([day.due for day in days], focus.due if focus else 0, profile, now_ms())
    session = last_session(api)
    return {
        "profile": profile,
        "art": art_url(chosen_companion, mood),
        "mood": tr(moods.message(chosen_companion, mood)) if chosen_companion != "none" else None,
        "winner": art_url(chosen_companion, moods.WINNER),
        "focus": focus_view(focus),
        "goal": goal_view(status),
        "board": board_view(api, profile),
        "session": session,
        "session_day": _session_day(mw.pm.profile[SESSION_KEY]) if session else None,
        "moments": addon()._moments._count(),
        "weekly": _weekly_recap(),
    }


def _weekly_recap():
    # A recap stays on Today until the next weekly recap is due.
    now = now_ms()
    entries = [entry for entry in addon().state["inbox"]
               if entry.get("kind") == "reminder_weekly_recap" and not entry.get("sender")
               and 0 <= now - int(entry.get("created_at", 0)) * 1000 < _RECAP_MS]
    return max(entries, key=lambda entry: entry["created_at"], default=None)


def _toolbar_values():
    if mw.col is None or not addon().client().configured:
        return "AnkiQuest", tr("Sign in to AnkiQuest")
    profile = _profile_for_display(addon().state["profile"]) or {}
    status = profile.get("_day_status", "unknown")
    label = tr({"met": "Secured", "syncing": "Syncing", "protected": "Protected", "pending": "Not yet"}.get(status, "Streak unknown"))
    streak = profile.get("streak")
    return "AnkiQuest · " + ("%d · " % streak if streak is not None else "") + label, tr("Open Today")


def _open_today():
    if not addon().client().configured:
        addon().open_account()
    elif mw.col is not None:
        mw.moveToState("deckBrowser")
        for name in ("show", "raise_", "activateWindow"):
            action = getattr(mw, name, None)
            if action is not None:
                action()


def _toolbar_links(links, toolbar):
    label, tip = _toolbar_values()
    links.append(toolbar.create_link("ankiquest-today", escape(label, quote=True), _open_today,
                                     tip=escape(tip, quote=True), id="ankiquest-today"))


def _refresh_toolbar(toolbar=None):
    label, tip = _toolbar_values()
    for button in local["browser_buttons"]:
        button.setText(label)
        button.setToolTip(tip)
    toolbar = toolbar or getattr(mw, "toolbar", None)
    if toolbar is None:
        return
    toolbar.web.eval("var aq=document.getElementById('ankiquest-today');if(aq){aq.textContent=%s;aq.title=%s;aq.setAttribute('aria-label',%s);}" % (
        json.dumps(label), json.dumps(tip), json.dumps(label),
    ))


def _browser_entry(browser):
    from aqt.qt import QToolButton, Qt

    grid = getattr(browser.form, "gridLayout", None)
    if grid is None:
        return
    button = QToolButton(browser)
    button.setToolButtonStyle(Qt.ToolButtonStyle.ToolButtonTextOnly)
    button.clicked.connect(_open_today)
    local["browser_buttons"].add(button)
    button.destroyed.connect(lambda: local["browser_buttons"].discard(button))
    grid.addWidget(button, 0, 2)
    _refresh_toolbar()


def on_deck_browser(deck_browser, content):
    """Marks decks done today, learning later today, or ready now."""
    if not addon().config().get("deck_status", True) or mw.col is None:
        return
    try:
        _, days = days_for(deck_browser)
        content.tree = today.decorate(content.tree, days, tr)
    except Exception as e:
        print("ankiquest deck status:", e)


def refresh_board(mode=_BoardFetch.AUTOMATIC):
    api = addon().client()
    if not api.configured:
        return
    # Only the same account and profile share a flight or its retry delay.
    pending = local["board_request"]
    if pending is not None and pending.owns(api):
        return
    retry = local["board_retry"]
    delay = BOARD_RETRY_MS
    if retry is not None and retry.request.owns(api):
        if mode is _BoardFetch.AUTOMATIC and time.monotonic() * 1000 < retry.at:
            return
        delay = min(retry.delay * 2, BOARD_RETRY_CAP_MS)
    request = _Request(account(api), mw.pm.profile)
    local["board_request"] = request

    def done(future):
        if local["board_request"] is not request:
            return
        local["board_request"] = None
        if not request.owns(addon().client()):
            return
        try:
            board = future.result()
        except Exception as e:
            local["board_retry"] = _BoardRetry(request, int(time.monotonic() * 1000) + delay, delay)
            print("ankiquest leaderboard:", e)
            if local["board"] is None or local["board_account"] != request.who:
                addon().redraw()
            return
        local["board_retry"] = None
        local["board"], local["board_account"] = board, request.who
        addon().redraw()

    mw.taskman.run_in_background(api.leaderboard, done)


def open_deck(deck_id):
    if mw.col is None or mw.col.decks.get(deck_id, default=False) is None:
        return
    mw.col.decks.select(deck_id)
    mw.onOverview()


def on_js_message(handled, message, context):
    if context is not mw.deckBrowser:
        return handled
    if message == "ankiquest:moments":
        addon()._moments._open()
        return (True, None)
    if message.startswith("ankiquest:study:"):
        deck = message.rsplit(":", 1)[1]
        if deck.isdigit():
            open_deck(int(deck))
        return (True, None)
    if message == "ankiquest:goal":
        open_goal_dialog()
        return (True, None)
    if message == "ankiquest:retry-board":
        refresh_board(_BoardFetch.MANUAL)
        return (True, None)
    if message.startswith("ankiquest:period:"):
        mw.pm.profile[PERIOD_KEY] = today.period(message.rsplit(":", 1)[1])
        addon().redraw()
        return (True, None)
    return handled


def remaining_cards():
    try:
        return sum(mw.col.sched.counts())
    except Exception:
        return 0


def on_state_change(new_state, old_state):
    _refresh_toolbar()
    if new_state == "review" and old_state != "review":
        api = addon().client()
        local["session"] = {"account": account(api), "started": now_ms()} if api.configured else None
        return
    if old_state == "review" and new_state in ("overview", "deckBrowser"):
        finish_session()


def finish_session():
    started, local["session"] = local["session"], None
    api = addon().client()
    if not started or mw.col is None or started["account"] != account(api):
        return
    try:
        summary = today.session(mw.col, started["started"], now_ms(), remaining_cards())
    except Exception as e:
        print("ankiquest session:", e)
        return
    if summary is None:
        return
    summary["day"] = (date(1970, 1, 1) + timedelta(days=server_day())).isoformat()
    mw.pm.profile[SESSION_KEY] = dict(summary, account=[api.base, api.user])
    local["board"] = None
    refresh_board()
    if not celebrate(summary):
        recap(summary)


def load_capabilities(api):
    """Learns, in the background, which features the player's server has."""
    if not api.configured or local["capabilities_loading"]:
        return
    local["capabilities_loading"] = True

    def done(future):
        local["capabilities_loading"] = False
        try:
            features, public_url = future.result()
        except Exception as e:
            print("ankiquest capabilities:", e)
            return
        local["capabilities"] = (api.base, features)
        move_to_main_address(api, address.main_address({"public_url": public_url}))

    mw.taskman.run_in_background(lambda: api.server_info(), done)


def move_to_main_address(api, main):
    """Moves a player saved at an older address of their server to its main address `main`,
    once one signed-in request with their token works there (the device list, which needs it). Keeps the saved address when it
    does not, and says nothing either way."""
    if not address.should_move(api.base, main) or local["moving"]:
        return
    local["moving"] = True
    there = Client(main, api.name, api.token, api.language)

    def done(future):
        local["moving"] = False
        try:
            future.result()
        except Exception as e:
            print("ankiquest main address:", e)
            return
        if account(addon().client()) != account(api):
            return
        address.move_profile(mw.pm.profile, api.base, there.base)
        values = {"url": there.base}
        shown = addon().config().get(SETUP_SHOWN)
        if shown:
            values[SETUP_SHOWN] = address.move_shown(shown, api.base, there.base)
        addon().save_config(values)
        local["capabilities"] = (there.base, local["capabilities"][1]) if local["capabilities"] else None
        local["board_account"] = None
        if addon().state.get("companion_account") == account(api):
            addon().state["companion_account"] = account(there)

    mw.taskman.run_in_background(there.devices, done)


def server_celebrates_goal(api):
    """Whether the server turns a reached daily goal into a win with its own celebration.

    Only a server known to lack wins leaves the goal to the add-on. Until the answer is in,
    the server is trusted: it celebrates once the reviews arrive.
    """
    known = local["capabilities"]
    if known is None or known[0] != api.base:
        load_capabilities(api)
        return True
    return "wins" in known[1]


def celebrate(summary):
    """Shows the once-a-day streak or goal celebration, with the session in it. True when shown."""
    if not addon().config().get("goal_celebrations", True):
        return False
    try:
        status = goal_status()
    except Exception as e:
        print("ankiquest goal:", e)
        return False
    earned, record = goals.claim(
        status, mw.pm.profile.get(CELEBRATED_KEY), server_celebrates_goal(addon().client()),
    )
    if earned is None:
        return False
    mw.pm.profile[CELEBRATED_KEY] = record
    topics = ["secured", "goal"] if earned == goals.BOTH else ["secured" if earned == goals.MINIMUM else "goal"]
    items = [{
        "key": "%s:%d" % (topic, status.day), "topic": topic,
        "title": _streak_title(summary) if topic == "secured" else goals.celebration_title(goals.GOAL, tr),
        "body": goals.celebration_text(goals.MINIMUM if topic == "secured" else goals.GOAL, status, tr),
        "at": summary["finished_at"], "local": True, "summary": summary,
    } for topic in topics]
    addon()._moments._offer(addon().client(), items, summary)
    return True


def recap(summary):
    if not addon().config().get("session_summary", True):
        return
    addon()._moments._offer(addon().client(), [{
        "key": "session:%d" % summary["finished_at"], "topic": "session",
        "title": tr("Your study session"), "body": today.session_body(summary, tr),
        "at": summary["finished_at"], "local": True, "summary": summary,
    }], summary)


def continue_studying():
    if mw.col is None:
        return
    try:
        mw.col.startTimebox()
    except Exception:
        pass
    mw.moveToState("review")


def open_progress():
    if not addon().client().configured:
        addon().open_account()
        return
    opened = local["progress"]
    if opened is not None and opened._owns():
        opened._dialog.raise_()
        opened._dialog.activateWindow()
        return
    _close_progress()
    opened = local["progress"] = progress_ui._Progress(addon())
    opened._dialog.finished.connect(lambda _result: local.update(progress=None) if local["progress"] is opened else None)
    opened._dialog.show()


def _close_progress():
    opened, local["progress"] = local["progress"], None
    if opened is not None:
        opened._close()


def clear_account():
    """Drop the previous account's Home data while keeping the periodic refresh."""
    _close_progress()
    local.update(
        board=None, board_account=None, board_request=None, board_retry=None, goal_request=None,
        days=None, session=None, capabilities=None, restarted=None,
    )
    profile = mw.pm.profile
    if profile is not None:
        for key in (GOAL_KEY, PLAN_KEY, REACHED_KEY, STREAK_MINUTES_KEY, CELEBRATED_KEY, SESSION_KEY):
            profile.pop(key, None)


def on_profile_open():
    _close_progress()
    local["board"], local["days"], local["session"] = None, None, None
    if local["timer"] is None:
        try:
            local["timer"] = mw.progress.timer(BOARD_MS, refresh_board, repeat=True, parent=mw)
        except TypeError:
            local["timer"] = mw.progress.timer(BOARD_MS, refresh_board, True)
    refresh_board()
    sync_goal(prompt=True)
    load_capabilities(addon().client())


def install(module):
    """Hooks the Today features into Anki; `module` is the add-on package, for its config and client."""
    local["addon"] = module
    gui_hooks.deck_browser_will_render_content.append(on_deck_browser)
    gui_hooks.webview_did_receive_js_message.append(on_js_message)
    gui_hooks.state_did_change.append(on_state_change)
    gui_hooks.profile_did_open.append(on_profile_open)
    gui_hooks.sync_did_finish.append(refresh_board)
    for name, callback in (("top_toolbar_did_init_links", _toolbar_links), ("top_toolbar_did_redraw", _refresh_toolbar),
                           ("profile_will_close", _close_progress), ("browser_will_show", _browser_entry)):
        hook = getattr(gui_hooks, name, None)
        if hook is not None:
            hook.append(callback)
