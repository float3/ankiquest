"""Today's two targets, ported from the phone app's daily goal.

- the streak minimum: the review minutes the server needs for today to count toward the
  streak (`today.streak_minutes`; 0 means any review counts), and
- the daily goal: by default the cards due today plus a share of the overdue backlog, so a
  backlog shrinks a little every day. Players can pick a fixed number of cards or minutes,
  or turn the goal off.
"""

import math
from collections import namedtuple

OFF, BACKLOG, CARDS, MINUTES = "off", "backlog", "cards", "minutes"
MODES = (BACKLOG, CARDS, MINUTES, OFF)
BACKLOG_PERCENTS = (5, 10, 20, 34, 100)

DEFAULT_MODE = BACKLOG
DEFAULT_BACKLOG_PERCENT = 10
DEFAULT_CARDS = 100
DEFAULT_MINUTES = 20

MINIMUM, GOAL, BOTH = "minimum", "goal", "both"

Load = namedtuple("Load", "day studied_cards minutes due_today backlog")
Settings = namedtuple("Settings", "mode backlog_percent cards minutes")
Goal = namedtuple("Goal", "target backlog backlog_share")


class Status(namedtuple("Status", "day minutes streak_minutes studied_cards mode backlog_percent goal")):
    """Today's targets and progress. `goal` is None when the daily goal is off."""

    @property
    def progress(self):
        return self.minutes if self.mode == MINUTES else self.studied_cards

    @property
    def minimum_done(self):
        """Today counts toward the streak, by the same rule as the server."""
        return self.studied_cards > 0 and self.minutes >= self.streak_minutes

    @property
    def goal_done(self):
        return self.goal is not None and self.progress > 0 and self.progress >= self.goal.target


def _int(value, default):
    try:
        return int(value)
    except (TypeError, ValueError):
        return default


def settings(config):
    """The goal settings from the add-on config, with the phone app's defaults."""
    mode = config.get("goal_mode", DEFAULT_MODE)
    return Settings(
        mode if mode in MODES else DEFAULT_MODE,
        _int(config.get("goal_backlog_percent"), DEFAULT_BACKLOG_PERCENT),
        _int(config.get("goal_cards"), DEFAULT_CARDS),
        _int(config.get("goal_minutes"), DEFAULT_MINUTES),
    )


def signature(chosen):
    return "%s:%d:%d:%d" % tuple(chosen)


def backlog_share(backlog, percent):
    """Cards from a backlog to work off today: at least one while any is left."""
    if backlog <= 0 or percent <= 0:
        return 0
    return min(backlog, max(1, math.ceil(backlog * min(percent, 100) / 100)))


def goal(load, chosen):
    """The goal for `load` under `chosen` settings; cards already studied today stay part of it."""
    if chosen.mode == OFF:
        return None
    if chosen.mode == CARDS:
        return Goal(max(1, chosen.cards), 0, 0)
    if chosen.mode == MINUTES:
        return Goal(max(1, chosen.minutes), 0, 0)
    share = backlog_share(load.backlog, chosen.backlog_percent)
    return Goal(load.studied_cards + load.due_today + share, load.backlog, share)


def fixed_goal(load, chosen, stored):
    """Today's goal, fixed when first worked out so studying never moves it.

    Returns the goal and what to store for the rest of the day.
    """
    stored = stored if isinstance(stored, dict) else {}
    if stored.get("day") == load.day and stored.get("settings") == signature(chosen):
        target = stored.get("target")
        if target is None:
            return None, stored
        return Goal(target, stored.get("backlog", 0), stored.get("share", 0)), stored
    fresh = goal(load, chosen)
    record = {"day": load.day, "settings": signature(chosen)}
    if fresh is not None:
        record.update(target=fresh.target, backlog=fresh.backlog, share=fresh.backlog_share)
    return fresh, record


def status(load, chosen, stored, streak_minutes):
    """Today's status and the record that keeps its goal fixed."""
    fixed, record = fixed_goal(load, chosen, stored)
    return Status(
        load.day, load.minutes, streak_minutes, load.studied_cards, chosen.mode, chosen.backlog_percent, fixed,
    ), record


def remembered_streak_minutes(profile, stored, account):
    """The server's streak minimum for `account`, kept so it is known offline.

    Returns the minutes and what to store; another account's minimum is never assumed.
    """
    stored = stored if isinstance(stored, dict) else {}
    today = (profile or {}).get("today")
    if isinstance(today, dict) and "streak_minutes" in today:
        minutes = max(0, _int(today.get("streak_minutes"), 0))
        return minutes, {"account": list(account), "minutes": minutes}
    if stored.get("account") == list(account):
        return max(0, _int(stored.get("minutes"), 0)), stored
    return 0, stored


def celebration(current, shown):
    """Which celebration `current` earned that was not `shown` yet today."""
    minimum = current.minimum_done and MINIMUM not in shown
    reached = current.goal_done and GOAL not in shown
    if minimum and reached:
        return BOTH
    if reached:
        return GOAL
    if minimum:
        return MINIMUM
    return None


def claim(current, stored):
    """The celebration earned since the last one shown, and the record marking it shown."""
    stored = stored if isinstance(stored, dict) else {}
    shown = set()
    if stored.get("day") == current.day:
        shown = {name for name in (MINIMUM, GOAL) if stored.get(name)}
    earned = celebration(current, shown)
    if earned is None:
        return None, stored
    return earned, {
        "day": current.day,
        MINIMUM: MINIMUM in shown or earned != GOAL,
        GOAL: GOAL in shown or earned != MINIMUM,
    }


STUDIED_SQL = (
    "select count(distinct cid), coalesce(sum(time), 0) from revlog "
    "where id >= ? and ease > 0 and type < 4"
)
REVIEWS_SQL = (
    "select coalesce(sum(day = ?), 0), coalesce(sum(day < ?), 0) from "
    "(select case when odid != 0 then odue else due end as day from cards where queue = 2)"
)
LEARNING_SQL = "select count() from cards where (queue in (1, 4) and due < ?) or (queue = 3 and due <= ?)"


def load(col, tree=None):
    """What the collection says about today. Call where the collection may be read."""
    today = col.sched.today
    cutoff = col.sched.day_cutoff
    studied, time_ms = col.db.first(STUDIED_SQL, (cutoff - 86_400) * 1000)
    due_reviews, backlog = col.db.first(REVIEWS_SQL, today, today)
    learning = col.db.scalar(LEARNING_SQL, cutoff, today)
    tree = col.sched.deck_due_tree() if tree is None else tree
    new_cards = sum(child.new_count for child in tree.children)
    return Load(today, studied or 0, int(time_ms or 0) // 60_000, (due_reviews or 0) + (learning or 0) + new_cards, backlog or 0)


def progress_text(current, translate=lambda value: value):
    """Progress toward the daily goal, like "34 of 50 cards" or "12 of 20 minutes"."""
    if current.mode == MINUTES:
        template = translate("%d of %d minutes")
    else:
        template = translate("%d of %d cards")
    return template % (min(current.progress, current.goal.target), current.goal.target)


def minimum_text(current, translate=lambda value: value):
    if current.minimum_done and current.streak_minutes > 0:
        return translate("Streak secured: you reached today’s %d-minute minimum.") % current.streak_minutes
    if current.minimum_done:
        return translate("Streak secured: today counts for your streak.")
    if current.streak_minutes > 0:
        return translate("Streak minimum: %d minutes of review today") % current.streak_minutes
    return translate("Any review today keeps your streak going.")


def celebration_title(earned, translate=lambda value: value):
    return translate("Streak secured! 🔥") if earned == MINIMUM else translate("Daily goal reached! 🎉")


def celebration_text(earned, current, translate=lambda value: value):
    minimum = minimum_text(current, translate)
    if earned == MINIMUM:
        if current.goal is None:
            return minimum
        return translate("%s Next up, your daily goal: %s so far.") % (minimum, progress_text(current, translate))
    if earned == GOAL:
        return translate("You reached today’s goal: %s.") % progress_text(current, translate)
    return translate("%s And you reached today’s goal: %s.") % (minimum, progress_text(current, translate))
