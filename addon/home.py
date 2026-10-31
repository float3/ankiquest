"""Connects the Today panel, deck list badges, goals and session recaps to Anki."""

import time
from pathlib import Path

from aqt import gui_hooks, mw

from . import goals, moods, today, today_ui
from .board import PAGES
from .client import offset_west_min
from .deck_completion import deck_snapshots
from .language import tr

GOAL_KEY = "ankiquestDayGoal"
STREAK_MINUTES_KEY = "ankiquestStreakMinutes"
CELEBRATED_KEY = "ankiquestGoalCelebrated"
PERIOD_KEY = "ankiquestBoardPeriod"
SESSION_KEY = "ankiquestLastStudySession"
ART = Path(__file__).resolve().parent / "art"
BOARD_MS = 5 * 60 * 1000
CACHE_MS = 2000

local = {
    "addon": None,
    "board": None,
    "board_account": None,
    "board_loading": False,
    "session": None,
    "timer": None,
    "days": None,
}


def now_ms():
    return int(time.time() * 1000)


def addon():
    return local["addon"]


def account(api):
    return [api.base, api.user, api.token]


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


def goal_status(tree=None):
    """Today's goal status; the first reading each day fixes the goal."""
    api = addon().client()
    minutes, kept = goals.remembered_streak_minutes(
        addon().state["profile"], mw.pm.profile.get(STREAK_MINUTES_KEY), [api.base, api.user],
    )
    mw.pm.profile[STREAK_MINUTES_KEY] = kept
    load = goals.load(mw.col, tree)
    status, record = goals.status(load, goals.settings(addon().config()), mw.pm.profile.get(GOAL_KEY), minutes)
    mw.pm.profile[GOAL_KEY] = record
    return status


def goal_view(status):
    view = {"minimum": goals.minimum_text(status, tr), "minimum_meter": None, "daily": None, "backlog": None}
    if status.streak_minutes > 0 and not status.minimum_done:
        view["minimum_meter"] = (status.minutes, status.streak_minutes)
    if status.goal is None:
        return view
    if status.goal_done:
        view["daily"] = tr("Daily goal reached! 🎉")
    else:
        view["daily"] = tr("Daily goal: %s") % goals.progress_text(status, tr)
    view["daily_meter"] = (status.progress, status.goal.target)
    if status.mode == goals.BACKLOG and status.goal.backlog > 0:
        view["backlog"] = tr(
            "Today’s due cards plus %d of your %d overdue cards (%d%% a day), so the backlog shrinks without taking over your day."
        ) % (status.goal.backlog_share, status.goal.backlog, status.backlog_percent)
    return view


def focus_view(day):
    if day is None:
        return None
    return {
        "id": day.id, "name": day.name, "review": day.review, "learning": day.learning, "new": day.new,
        "status": day.status, "label": today.label(day, tr), "ready": day.ready_count,
    }


def board_view(api):
    if local["board_account"] != account(api):
        local["board"] = None
    if local["board"] is None:
        refresh_board()
    if local["board"] is None:
        return None
    chosen = today.period(mw.pm.profile.get(PERIOD_KEY))
    return {"period": chosen, "rows": today.standings(local["board"], chosen, api.name)}


def last_session(api):
    stored = mw.pm.profile.get(SESSION_KEY)
    if not isinstance(stored, dict) or stored.get("account") != [api.base, api.user]:
        return None
    return today.session_text(stored, tr)


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
    mood = moods.mood([day.due for day in days], focus.due if focus else 0, addon().state["profile"], now_ms())
    return {
        "art": art_url(chosen_companion, mood),
        "mood": tr(moods.message(chosen_companion, mood)) if chosen_companion != "none" else None,
        "winner": art_url(chosen_companion, moods.WINNER),
        "focus": focus_view(focus),
        "goal": goal_view(status),
        "board": board_view(api),
        "session": last_session(api),
    }


def on_deck_browser(deck_browser, content):
    """Marks decks done today, learning later today, or ready now."""
    if not addon().config().get("deck_status", True) or mw.col is None:
        return
    try:
        _, days = days_for(deck_browser)
        content.tree = today.decorate(content.tree, days, tr)
    except Exception as e:
        print("ankiquest deck status:", e)


def refresh_board():
    api = addon().client()
    if not api.configured or local["board_loading"]:
        return
    local["board_loading"] = True
    who = account(api)

    def done(future):
        local["board_loading"] = False
        try:
            board = future.result()
        except Exception as e:
            print("ankiquest leaderboard:", e)
            return
        if account(addon().client()) != who:
            return
        local["board"], local["board_account"] = board, who
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
    if message.startswith("ankiquest:study:"):
        deck = message.rsplit(":", 1)[1]
        if deck.isdigit():
            open_deck(int(deck))
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
    mw.pm.profile[SESSION_KEY] = dict(summary, account=[api.base, api.user])
    local["board"] = None
    refresh_board()
    if not celebrate(summary):
        recap(summary)


def celebrate(summary):
    """Shows the once-a-day streak or goal celebration, with the session in it. True when shown."""
    if not addon().config().get("goal_celebrations", True):
        return False
    try:
        status = goal_status()
    except Exception as e:
        print("ankiquest goal:", e)
        return False
    earned, record = goals.claim(status, mw.pm.profile.get(CELEBRATED_KEY))
    if earned is None:
        return False
    mw.pm.profile[CELEBRATED_KEY] = record
    today_ui.session_dialog(
        mw,
        goals.celebration_title(earned, tr),
        goals.celebration_text(earned, status, tr) + "\n\n" + today.session_body(summary, tr),
        art_path(companion(), moods.STREAK if earned == goals.MINIMUM else moods.CELEBRATE),
        continue_studying if summary["remaining"] > 0 else None,
        open_progress,
        done_text=tr("Nice!"),
    )
    return True


def recap(summary):
    if not addon().config().get("session_summary", True):
        return
    today_ui.session_dialog(
        mw,
        tr("Your study session"),
        today.session_body(summary, tr),
        art_path(companion(), moods.CELEBRATE if summary["remaining"] == 0 else moods.STREAK),
        continue_studying if summary["remaining"] > 0 else None,
        open_progress,
        never_again=lambda: addon().save_config({"session_summary": False}),
    )


def continue_studying():
    if mw.col is None:
        return
    try:
        mw.col.startTimebox()
    except Exception:
        pass
    mw.moveToState("review")


def open_progress():
    addon().open_page(PAGES["history"])


def on_profile_open():
    local["board"], local["days"], local["session"] = None, None, None
    if local["timer"] is None:
        try:
            local["timer"] = mw.progress.timer(BOARD_MS, refresh_board, repeat=True, parent=mw)
        except TypeError:
            local["timer"] = mw.progress.timer(BOARD_MS, refresh_board, True)
    refresh_board()


def install(module):
    """Hooks the Today features into Anki; `module` is the add-on package, for its config and client."""
    local["addon"] = module
    gui_hooks.deck_browser_will_render_content.append(on_deck_browser)
    gui_hooks.webview_did_receive_js_message.append(on_js_message)
    gui_hooks.state_did_change.append(on_state_change)
    gui_hooks.profile_did_open.append(on_profile_open)
    gui_hooks.sync_did_finish.append(refresh_board)
