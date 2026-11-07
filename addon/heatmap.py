"""The review heatmap's numbers: reviews per day, the forecast, streaks and AnkiQuest's verdicts.

A port of AnkiQuest-Android's ReviewHeatmapData.kt, so the deck list shows the same heatmap on
both. tests/fixtures/review_heatmap.json pins the rules for both.

    collection (revlog, cards) ──load──▶ summarize ──▶ Heatmap ──▶ heatmap_ui
    last profile ──server_days──▶ with_server_days ──┘

Plain Python, so the tests load it without Anki.
"""

from dataclasses import dataclass, field
from datetime import date, datetime, timedelta, timezone

FORECAST_DAYS = 90
DAY_MS = 86_400_000
MINUTE_MS = 60_000

# The server's verdict on a day (DayState in Android).
MET = "met"
SHORT = "short"
FROZEN = "frozen"
SKIPPED = "skipped"
NONE = "none"
STATES = (MET, SHORT, FROZEN, SKIPPED, NONE)
KEEPS_STREAK = (FROZEN, SKIPPED)

# Answers per study day, as Anki's day searches count them: 24-hour windows ending at the
# scheduler's cutoff. Retained reviews of deleted cards still count as study.
REVIEWS_SQL = (
    "select (? - 1 - id) / %d, count(*) from revlog "
    "where id >= 0 and id < ? and ease > 0 and type < 4 group by 1" % DAY_MS
)

# Review cards due per day ahead, as the browser's prop:due counts them: original due dates
# in filtered decks, and timestamp-based learning queues. New, suspended and buried cards
# are not scheduled answers.
FORECAST_SQL = (
    "select day_offset, count(*) from ("
    "select case when queue in (2, 3) then "
    "(case when odue != 0 then odue else due end) - ? "
    "else ((case when odue != 0 then odue else due end) - ?) / 86400 end as day_offset "
    "from cards where queue in (1, 2, 3, 4) and type != 0"
    ") where day_offset between 1 and ? group by day_offset"
)


@dataclass(frozen=True)
class Verdict:
    """What the server says of a day: its state, the minutes studied and the minutes it needed."""

    state: str
    minutes: int = 0
    streak_minutes: int = 0


@dataclass(frozen=True)
class ServerDays:
    """A profile's heatmap: each day's reviews, the days kept, the last upload and the verdicts."""

    reviews: dict
    protected: frozenset
    uploaded_until_ms: int = None
    verdicts: dict = field(default_factory=dict)


@dataclass(frozen=True)
class Heatmap:
    today: date
    reviews: dict
    scheduled: dict
    current_streak: int
    longest_streak: int
    total_reviews: int
    days_studied: int
    first_review_date: date
    daily_average: float
    protected: frozenset = frozenset()
    verdicts: dict = field(default_factory=dict)

    @property
    def forecast_end(self):
        return self.today + timedelta(days=FORECAST_DAYS)


def _int(value, default=0):
    if isinstance(value, bool):
        return default
    try:
        return int(value)
    except (TypeError, ValueError):
        return default


def _date(value):
    try:
        return date.fromisoformat(value)
    except (TypeError, ValueError):
        return None


def parse_verdict(cell):
    """The verdict in a heatmap cell, or None from servers that do not judge days yet."""
    state = cell.get("state")
    if state not in STATES:
        return None
    minutes = _int(cell.get("minutes"), _int(cell.get("time_ms")) // MINUTE_MS)
    return Verdict(state, minutes, _int(cell.get("streak_minutes")))


def server_days(profile):
    """What the profile's heatmap says of each day."""
    last = _int((profile or {}).get("last_review_id"))
    uploaded = last if last > 0 else None
    cells = (profile or {}).get("heatmap")
    if not isinstance(cells, list):
        return ServerDays({}, frozenset(), uploaded)

    reviews, protected, verdicts = {}, set(), {}
    for cell in cells:
        if not isinstance(cell, dict):
            continue
        day = _date(cell.get("date"))
        if day is None:
            continue
        reviews[day] = _int(cell.get("reviews"))
        if cell.get("frozen") is True:
            protected.add(day)
        verdict = parse_verdict(cell)
        if verdict is not None:
            verdicts[day] = verdict
    return ServerDays(reviews, frozenset(protected), uploaded, verdicts)


def with_server_days(data, server, zone=None):
    """The same history with AnkiQuest's numbering of the days it holds.

    The revlog records when a review happened but not under which clock, so a flight through
    time zones moves reviews onto a neighbouring day. The server numbers each review by the
    clock in force when it was made. Its counts are used from its first day with reviews up to
    the day before the last review it was sent, never for today: those days may hold reviews
    not uploaded yet, so they stay the collection's, as do days it does not cover.
    """
    studied = [day for day, count in server.reviews.items() if count > 0]
    first = min(studied) if studied else None
    uploaded = data.today
    if server.uploaded_until_ms is not None:
        moment = datetime.fromtimestamp(server.uploaded_until_ms / 1000, timezone.utc)
        uploaded = moment.astimezone(zone).date()
    until = min(data.today, uploaded)

    reviews = data.reviews
    if first is not None and first < until:
        reviews = {day: count for day, count in data.reviews.items() if day < first or day >= until}
        reviews.update({day: count for day, count in server.reviews.items() if first <= day < until and count > 0})
    return summarize(data.today, reviews, data.scheduled, server.protected, server.verdicts)


def _streaks(today, counted, kept):
    """Current and longest streak. A kept day bridges a streak without lengthening it."""
    current = 0
    day = today if today in counted else today - timedelta(days=1)
    while day in counted or day in kept:
        if day in counted:
            current += 1
        day -= timedelta(days=1)

    longest, run, previous = 0, 0, None
    for day in sorted(counted | kept):
        continues = previous is not None and previous + timedelta(days=1) == day
        if day in kept:
            if not continues:
                run = 0
        else:
            run = run + 1 if continues else 1
        longest = max(longest, run)
        previous = day
    return current, longest


def summarize(today, reviews, scheduled, protected=(), verdicts=None):
    history = {day: count for day, count in sorted(reviews.items()) if day <= today and count > 0}
    forecast_end = today + timedelta(days=FORECAST_DAYS)
    forecast = {day: count for day, count in sorted(scheduled.items()) if today < day <= forecast_end and count > 0}
    judged = {day: verdict for day, verdict in (verdicts or {}).items() if day <= today}

    # A frozen day keeps the streak whether or not it had reviews: short of the minimum,
    # they did not count on their own. Today is still open, so nothing has kept it yet.
    kept_days = set(protected) | {day for day, verdict in judged.items() if verdict.state in KEEPS_STREAK}
    kept = frozenset(day for day in kept_days if day < today)

    # Where the server judged a day, only its verdict says whether the day counted.
    counted = {day for day in history if day not in kept and (day not in judged or judged[day].state == MET)}
    counted |= {day for day, verdict in judged.items() if verdict.state == MET}

    current, longest = _streaks(today, counted, kept)
    total = sum(history.values())
    return Heatmap(
        today=today,
        reviews=history,
        scheduled=forecast,
        current_streak=current,
        longest_streak=longest,
        total_reviews=total,
        days_studied=len(history),
        first_review_date=next(iter(history), None),
        daily_average=total / len(history) if history else 0.0,
        protected=kept,
        verdicts=judged,
    )


def intensity(count, daily_average):
    """Colour level 0-4 of a day: none, up to half the average, the average, twice it, more."""
    average = max(daily_average, 1.0)
    if count <= 0:
        return 0
    if count <= average / 2:
        return 1
    if count <= average:
        return 2
    if count <= average * 2:
        return 3
    return 4


def search_for_offset(offset):
    """The browser search for the day `offset` days from today, or None past the forecast."""
    if offset <= 0:
        return "prop:rated=%d" % offset
    if offset <= FORECAST_DAYS:
        return "prop:due=%d -is:new" % offset
    return None


def search_for_day(data, day):
    """The search for a day with something to show; None when it has nothing."""
    offset = (day - data.today).days
    if offset <= 0 and data.reviews.get(day, 0) > 0:
        return search_for_offset(offset)
    if 1 <= offset <= FORECAST_DAYS and data.scheduled.get(day, 0) > 0:
        return search_for_offset(offset)
    return None


def study_date(next_day_at, zone=None):
    """Today's study day: the scheduler's cutoff is tomorrow's rollover."""
    return datetime.fromtimestamp(next_day_at, timezone.utc).astimezone(zone).date() - timedelta(days=1)


def load(col):
    """The collection's heatmap. Runs queries over the whole revlog: call it off the main thread
    when that matters."""
    next_day_at = col.sched.day_cutoff
    today = study_date(next_day_at)
    cutoff_ms = next_day_at * 1000

    reviews = {
        today - timedelta(days=days_ago): count
        for days_ago, count in col.db.all(REVIEWS_SQL, cutoff_ms, cutoff_ms)
    }
    scheduled = {
        today + timedelta(days=offset): count
        for offset, count in col.db.all(FORECAST_SQL, col.sched.today, next_day_at, FORECAST_DAYS)
    }
    return summarize(today, reviews, scheduled)
