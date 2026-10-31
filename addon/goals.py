"""Today's two targets: the streak minimum and the daily goal the server keeps.

- the streak minimum: the review minutes the server needs for today to count toward the
  streak (`today.streak_minutes`; 0 means any review counts), and
- the daily goal (`/api/goal`): a number of cards or minutes, which the server judges itself,
  or the backlog plan, which only the collection can work out: today's due cards plus enough of
  the overdue backlog that it is gone `value` days after the plan started.

Plain Python, so the tests load it without Anki; callers hand in the collection and the client.
"""

import math
from collections import namedtuple

OFF, BACKLOG, CARDS, MINUTES = "off", "backlog", "cards", "minutes"
MODES = (BACKLOG, CARDS, MINUTES, OFF)
DEFAULTS = {BACKLOG: 30, CARDS: 100, MINUTES: 20}
RANGES = {BACKLOG: (1, 365), CARDS: (1, 5000), MINUTES: (1, 600)}
PRESET_DAYS = (7, 14, 30, 60)
DEFAULT_SECONDS_PER_CARD = 8
FORECAST_DAYS = 7
ANSWER_WINDOW_DAYS = 30

LEGACY_KEYS = ("goal_mode", "goal_backlog_percent", "goal_cards", "goal_minutes")
LEGACY_DEFAULT = (BACKLOG, 10)
PERCENT_DAYS = {5: 60, 10: 30, 20: 14, 34: 7, 100: 1}

MINIMUM, GOAL, BOTH = "minimum", "goal", "both"

Load = namedtuple("Load", "day studied_cards minutes due_today backlog")
Plan = namedtuple("Plan", "mode value since chosen")
Goal = namedtuple("Goal", "target backlog extra days_left")
Forecast = namedtuple("Forecast", "due backlog daily_due seconds_per_card")
Projection = namedtuple("Projection", "cards seconds cleared_in")


class Status(namedtuple("Status", "day minutes streak_minutes studied_cards mode goal served reached")):
    """Today's targets and progress. `goal` is None when the daily goal is off; `served` is the
    server's own count for a cards or minutes goal, `reached` whether it says the goal is met."""

    @property
    def progress(self):
        local = self.minutes if self.mode == MINUTES else self.studied_cards
        return local if self.served is None else max(local, self.served)

    @property
    def minimum_done(self):
        """Today counts toward the streak, by the same rule as the server."""
        return self.studied_cards > 0 and self.minutes >= self.streak_minutes

    @property
    def goal_done(self):
        if self.goal is None:
            return False
        return bool(self.reached) or (self.progress > 0 and self.progress >= self.goal.target)


def _int(value, default):
    try:
        return int(value)
    except (TypeError, ValueError):
        return default


def clamp(mode, value):
    """`value` within `mode`'s range, or the mode's default when it is missing."""
    if mode not in RANGES:
        return None
    low, high = RANGES[mode]
    return max(low, min(high, _int(value, DEFAULTS[mode])))


def default_plan(today):
    return Plan(BACKLOG, DEFAULTS[BACKLOG], today, False)


def plan(answer, today, chosen=None):
    """A `/api/goal` answer (or a profile's `goal`) as a Plan; the default plan when it is unusable."""
    if not isinstance(answer, dict) or answer.get("mode") not in MODES:
        return default_plan(today)
    mode = answer["mode"]
    value = clamp(mode, answer.get("value")) if mode != OFF else _int(answer.get("value"), 0) or None
    since = _int(answer.get("since"), today)
    picked = answer.get("chosen", chosen)
    return Plan(mode, value, since, bool(picked))


def plan_record(found, account):
    return {"account": list(account), "mode": found.mode, "value": found.value, "since": found.since, "chosen": found.chosen}


def remembered_plan(answer, stored, account, today):
    """The server's goal for `account`, kept so it is known offline and between polls.

    `answer` is a `/api/goal` answer or a profile's `goal`, which leaves out `chosen`. Returns the
    plan and what to store; another account's goal is never assumed."""
    stored = stored if isinstance(stored, dict) and stored.get("account") == list(account) else None
    if isinstance(answer, dict) and answer.get("mode") in MODES:
        found = plan(answer, today, stored.get("chosen", False) if stored else False)
        return found, plan_record(found, account)
    if stored:
        return plan(stored, today), stored
    return default_plan(today), None


def days_left(found, today):
    return found.value - (today - found.since)


def backlog_extra(backlog, left):
    """Cards from the backlog to clear today so that it is gone when the days run out."""
    if backlog <= 0:
        return 0
    return max(1, math.ceil(backlog / max(1, left)))


def goal(load, found, today):
    """Today's goal under `found`, and whether the backlog plan ran out and starts again.
    Cards already studied today stay part of a backlog goal."""
    if found.mode == OFF:
        return None, False
    if found.mode in (CARDS, MINUTES):
        return Goal(max(1, found.value), 0, 0, None), False
    left = days_left(found, today)
    restart = left < 1
    if restart:
        left = found.value
    extra = backlog_extra(load.backlog, left)
    return Goal(load.studied_cards + load.due_today + extra, load.backlog, extra, left), restart


def signature(found):
    return "%s:%s" % (found.mode, found.value)


def fixed_goal(load, found, today, stored):
    """Today's goal, fixed when first worked out so studying never moves it.

    Returns the goal, what to store for the rest of the day, and whether the plan must restart."""
    stored = stored if isinstance(stored, dict) else {}
    if stored.get("day") == load.day and stored.get("plan") == signature(found):
        target = stored.get("target")
        if target is None:
            return None, stored, False
        return Goal(target, stored.get("backlog", 0), stored.get("extra", 0), stored.get("days_left")), stored, False
    fresh, restart = goal(load, found, today)
    record = {"day": load.day, "plan": signature(found)}
    if fresh is not None:
        record.update(target=fresh.target, backlog=fresh.backlog, extra=fresh.extra, days_left=fresh.days_left)
    return fresh, record, restart


def served(profile, found, today):
    """The server's progress and verdict for today's goal; (None, False) when the profile
    describes another goal or another day."""
    view = (profile or {}).get("goal")
    if not isinstance(view, dict) or view.get("mode") != found.mode:
        return None, False
    if view.get("day") is not None and _int(view.get("day"), None) != today:
        return None, False
    progress = view.get("progress")
    return (_int(progress, None) if progress is not None else None), bool(view.get("reached"))


def status(load, found, today, stored, streak_minutes, profile=None):
    """Today's status, the record that keeps its goal fixed and whether the plan must restart."""
    fixed, record, restart = fixed_goal(load, found, today, stored)
    progress, reached = served(profile, found, today)
    return Status(
        load.day, load.minutes, streak_minutes, load.studied_cards, found.mode, fixed, progress, reached,
    ), record, restart


def legacy_goal(config):
    """A goal picked in an older add-on's settings, as a `PUT /api/goal` body; None when there is
    none, or it was left at the old default."""
    mode = config.get("goal_mode")
    if mode not in MODES:
        return None
    percent = _int(config.get("goal_backlog_percent"), LEGACY_DEFAULT[1])
    if mode == BACKLOG and percent == LEGACY_DEFAULT[1]:
        return None
    if mode == BACKLOG:
        return {"mode": BACKLOG, "value": PERCENT_DAYS.get(percent, DEFAULTS[BACKLOG])}
    if mode == OFF:
        return {"mode": OFF}
    return {"mode": mode, "value": clamp(mode, config.get("goal_" + mode))}


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


def to_report(current, account, server_day, stored):
    """A reached backlog goal the server has not been told about yet; it judges the others itself."""
    if current.mode != BACKLOG or not current.goal_done or current.reached:
        return False
    stored = stored if isinstance(stored, dict) else {}
    return stored.get("account") != list(account) or stored.get("day") != server_day


def clear_days(backlog, cards, daily_due):
    """Days until the backlog is gone at `cards` a day: 0 with nothing overdue, None when it never shrinks."""
    if backlog <= 0:
        return 0
    if cards <= daily_due:
        return None
    return math.ceil(backlog / (cards - daily_due))


def projection(mode, value, forecast):
    """What a choice means today and for the backlog, as the goal dialog shows it; None for no goal."""
    pace = forecast.seconds_per_card
    if mode == BACKLOG:
        cards = forecast.due + backlog_extra(forecast.backlog, value)
        return Projection(cards, cards * pace, value if forecast.backlog > 0 else 0)
    if mode == CARDS:
        return Projection(value, value * pace, clear_days(forecast.backlog, value, forecast.daily_due))
    if mode == MINUTES:
        cards = int(value * 60 // pace)
        return Projection(cards, value * 60, clear_days(forecast.backlog, cards, forecast.daily_due))
    return None


def shrinking_pace(forecast):
    """The smallest daily goal that makes the backlog shrink; with none, what keeps it at zero."""
    return forecast.daily_due + 1 if forecast.backlog > 0 else forecast.daily_due


STUDIED_SQL = (
    "select count(distinct cid), coalesce(sum(time), 0) from revlog "
    "where id >= ? and ease > 0 and type < 4"
)
REVIEWS_SQL = (
    "select coalesce(sum(day = ?), 0), coalesce(sum(day < ?), 0) from "
    "(select case when odid != 0 then odue else due end as day from cards where queue = 2)"
)
LEARNING_SQL = "select count() from cards where (queue in (1, 4) and due < ?) or (queue = 3 and due <= ?)"
FORECAST_SQL = (
    "select count() from (select case when odid != 0 then odue else due end as day from cards "
    "where type = 2 and queue = 2) where day between ? and ?"
)
ANSWER_SQL = "select avg(time) from revlog where id >= ? and ease > 0 and type < 4 and time > 0"


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


def daily_due(col):
    """The average number of review cards due per day over the next week, from the cards table.

    Review cards (type and queue 2) keep their due date as an Anki day number, the same count
    as `col.sched.today`; cards in filtered decks keep it in `odue`. Days 1 to 7 after today
    are counted, so the backlog, which is due today or earlier, is left out."""
    today = col.sched.today
    total = col.db.scalar(FORECAST_SQL, today + 1, today + FORECAST_DAYS) or 0
    return int(round(total / FORECAST_DAYS))


def seconds_per_card(col):
    """The average answer time over the last 30 days (revlog keeps it in milliseconds), or 8 s."""
    since = (col.sched.day_cutoff - (ANSWER_WINDOW_DAYS + 1) * 86_400) * 1000
    average = col.db.scalar(ANSWER_SQL, since)
    return round(average / 1000, 1) if average and average >= 1000 else DEFAULT_SECONDS_PER_CARD


def forecast(col, tree=None):
    """Everything the goal dialog's estimates need. Call where the collection may be read."""
    today = load(col, tree)
    return Forecast(today.due_today, today.backlog, daily_due(col), seconds_per_card(col))


def fetch(api):
    return api._request("/api/goal/" + api.user)


def put(api, mode, value=None, restart=False):
    """Saves a goal; the server answers with it as saved."""
    body = {"mode": mode}
    if mode != OFF and value is not None:
        body["value"] = int(value)
    if restart:
        body["restart"] = True
    return api._request("/api/goal/" + api.user, body, method="PUT")


def report_reached(api, day):
    """Tells the server today's backlog goal is reached. Its answer is `204 No Content`."""
    try:
        api._request("/api/goal/%s/reached" % api.user, {"day": int(day)})
    except ValueError:
        pass


def progress_text(current, translate=lambda value: value):
    """Progress toward the daily goal, like "34 of 50 cards" or "12 of 20 minutes"."""
    if current.mode == MINUTES:
        template = translate("%d of %d minutes")
    else:
        template = translate("%d of %d cards")
    return template % (min(current.progress, current.goal.target), current.goal.target)


def backlog_text(current, translate=lambda value: value):
    """What today's backlog goal is made of; None when nothing is overdue."""
    fixed = current.goal
    if current.mode != BACKLOG or fixed is None or not fixed.backlog:
        return None
    left = fixed.days_left or 1
    template = translate(
        "Today’s due cards plus %d of your %d overdue cards, so the backlog is gone in %d day."
        if left == 1 else
        "Today’s due cards plus %d of your %d overdue cards, so the backlog is gone in %d days."
    )
    return template % (fixed.extra, fixed.backlog, left)


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


def time_text(seconds, translate=lambda value: value):
    minutes = int(round(seconds / 60))
    if minutes < 1:
        return translate("Time today: less than a minute")
    return (translate("Time today: about %d minute") if minutes == 1 else translate("Time today: about %d minutes")) % minutes


def cleared_text(days, translate=lambda value: value):
    if days == 0:
        return translate("Nothing overdue")
    if days is None:
        return translate("At this pace your backlog is never cleared.")
    return (translate("Backlog cleared in %d day") if days == 1 else translate("Backlog cleared in %d days")) % days


def pace_text(forecast, translate=lambda value: value):
    if forecast.backlog > 0:
        return translate("To shrink your backlog, study at least %d cards a day.") % shrinking_pace(forecast)
    return translate("Nothing overdue. About %d cards a day keeps it that way.") % shrinking_pace(forecast)


def summary(mode, value, forecast, translate=lambda value: value):
    """The goal dialog's estimate lines for a choice."""
    if forecast is None:
        return []
    found = projection(mode, value, forecast)
    if found is None:
        return [translate("Without a daily goal, the Today panel only follows your streak."), pace_text(forecast, translate)]
    return [
        translate("Cards today: about %d") % found.cards,
        time_text(found.seconds, translate),
        cleared_text(found.cleared_in, translate),
        pace_text(forecast, translate),
    ]
