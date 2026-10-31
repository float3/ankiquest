"""Local study for the Today panel: daily deck states, the next deck, the leaderboard and sessions.

Nothing here imports Anki, so tests can load it alone; callers hand in the collection.
"""

import re
from collections import namedtuple
from html import escape

REVIEW_NOW, LEARNING_LATER, DONE, NOTHING = "review_now", "learning_later", "done", "nothing"
PERIODS = ("hour", "day", "week", "month", "year", "all")
DEFAULT_PERIOD = "week"
MAX_SESSION_MS = 24 * 3_600_000

NEXT_LEARNING_SQL = (
    "select case when odid != 0 then odid else did end, count(*), min(due) from cards "
    "where queue = 1 and due > ? and due < ? group by 1"
)
SESSION_SQL = (
    "select count(*), coalesce(sum(time), 0) from revlog "
    "where id > ? and id <= ? and ease > 0 and type < 4"
)


def daily_status(ready, remaining, reviewed, next_at):
    """Daily remaining includes learning beyond Anki's short learn-ahead window."""
    if ready > 0:
        return REVIEW_NOW
    if remaining > 0 and next_at is not None:
        return LEARNING_LATER
    if remaining > 0:
        return REVIEW_NOW
    if reviewed > 0:
        return DONE
    return NOTHING


class DeckDay(namedtuple("DeckDay", "id name new learning review ready remaining reviewed next_at")):
    """One deck's counts and how far today's work on it has come."""

    @property
    def due(self):
        return self.new + self.learning + self.review

    @property
    def status(self):
        return daily_status(self.ready, self.remaining, self.reviewed, self.next_at)

    @property
    def ready_count(self):
        return max(self.ready, self.remaining if self.next_at is None else 0)


def nodes(tree):
    """Every deck below the root, parents before their subdecks, as Anki lists them."""
    found = []

    def visit(node):
        for child in node.children:
            found.append(child)
            visit(child)

    visit(tree)
    return found


def _family(names, name):
    prefix = name + "::"
    return [deck for deck, other in names.items() if other == name or other.startswith(prefix)]


def deck_days(tree, snapshots, learning_rows):
    """Each deck in the tree with its daily state.

    `snapshots` are deck_completion.deck_snapshots rows; `learning_rows` are
    (home deck, count, first due in seconds) for learning due later today.
    """
    by_id = {int(row["id"]): row for row in snapshots}
    names = {deck: row["name"] for deck, row in by_id.items()}
    later = {int(deck): (count, due) for deck, count, due in learning_rows}
    days = []
    for node in nodes(tree):
        due = node.new_count + node.learn_count + node.review_count
        row = by_id.get(node.deck_id)
        if row is None:
            days.append(DeckDay(node.deck_id, node.name, node.new_count, node.learn_count, node.review_count, due, due, 0, None))
            continue
        family = _family(names, row["name"])
        waiting = sum(later.get(deck, (0, 0))[0] for deck in family)
        firsts = [later[deck][1] * 1000 for deck in family if deck in later]
        days.append(DeckDay(
            node.deck_id, row["name"], node.new_count, node.learn_count, node.review_count,
            max(due, row["remaining"] - waiting), row["remaining"], row["reviewed_today"],
            min(firsts) if firsts else None,
        ))
    return days


def load_days(col, now_ms, tree, snapshots):
    """Reads learning due later today and combines it with the tree and deck snapshots."""
    rows = col.db.all(NEXT_LEARNING_SQL, now_ms // 1000, col.sched.day_cutoff)
    return deck_days(tree, snapshots, rows)


def focus(days, selected):
    """The next deck: the selected one while it has work, else the first that does."""
    for pick in (
        lambda day: day.id == selected and day.status == REVIEW_NOW,
        lambda day: day.status == REVIEW_NOW,
        lambda day: day.id == selected,
    ):
        for day in days:
            if pick(day):
                return day
    return days[0] if days else None


def clock(ms):
    import time

    return time.strftime("%H:%M", time.localtime(ms / 1000))


def label(day, translate=lambda value: value, show_time=clock):
    """The phone app's daily line for a deck."""
    if day.status == REVIEW_NOW:
        count = day.ready_count
        return (translate("%d card ready") if count == 1 else translate("%d cards ready")) % count
    if day.status == LEARNING_LATER:
        return translate("Learning later today · next at %s") % show_time(day.next_at)
    if day.status == DONE:
        return translate("Done today")
    return translate("Nothing scheduled today")


BADGE_STYLE = {
    DONE: ("✓", "#46a302"),
    LEARNING_LATER: ("⏱", "#ff9600"),
    REVIEW_NOW: ("●", "#1cb0f6"),
}


def badge(day, translate=lambda value: value, show_time=clock):
    """A small mark for a deck row; empty when nothing is scheduled."""
    if day.status not in BADGE_STYLE:
        return ""
    icon, color = BADGE_STYLE[day.status]
    if day.status == LEARNING_LATER:
        text = translate("later · %s") % show_time(day.next_at)
    elif day.status == DONE:
        text = translate("done today")
    else:
        text = translate("ready")
    return (
        "<span class='ankiquest-day ankiquest-%s' title='%s' style='margin-left:.5em;font-size:11px;font-weight:800;"
        "color:%s;background:%s22;border-radius:999px;padding:1px 8px;white-space:nowrap'>%s %s</span>"
        % (day.status.replace("_", "-"), escape(label(day, translate, show_time), True), color, color, icon, escape(text))
    )


DECK_LINK = re.compile(r"(<a\b[^>]*?pycmd\('open:(\d+)'\)[^>]*>.*?</a>)", re.S)


def decorate(tree_html, days, translate=lambda value: value, show_time=clock):
    """Adds each deck's badge after its name in Anki's deck list markup."""
    by_id = {day.id: day for day in days}

    def mark(match):
        day = by_id.get(int(match.group(2)))
        return match.group(1) + (badge(day, translate, show_time) if day else "")

    return DECK_LINK.sub(mark, tree_html)


def period(value):
    return value if value in PERIODS else DEFAULT_PERIOD


def _xp(entry, chosen):
    periods = entry.get("periods")
    if isinstance(periods, dict) and chosen in periods:
        return int(periods.get(chosen) or 0)
    return int(entry.get("week_xp") or 0)


def ranked(board, chosen):
    """Orders a /api/leaderboard answer by the XP of `chosen`, falling back to the week for older servers."""
    entries = [entry for entry in board or [] if isinstance(entry, dict)]
    ordered = sorted(entries, key=lambda entry: (-_xp(entry, chosen), -int(entry.get("xp_total") or 0), entry.get("user") or ""))
    return [dict(entry, xp=_xp(entry, chosen)) for entry in ordered]


def standings(board, chosen, me, size=5):
    """The top `size` as (place, entry, mine), plus your own row when you are further down."""
    rows = [(place, entry, entry.get("user") == me) for place, entry in enumerate(ranked(board, chosen), 1)]
    top = rows[:size]
    if not any(mine for _, _, mine in top):
        top += [row for row in rows[size:] if row[2]][:1]
    return top


def valid_session(started_ms, now_ms):
    return bool(started_ms) and 0 < now_ms - started_ms <= MAX_SESSION_MS


def session(col, started_ms, now_ms, remaining):
    """A finished session's summary, or None when nothing was reviewed. Review counts come from the log."""
    if not valid_session(started_ms, now_ms):
        return None
    reviews, time_ms = col.db.first(SESSION_SQL, started_ms, now_ms) or (0, 0)
    if not reviews:
        return None
    return {"reviews": int(reviews), "minutes": int(time_ms or 0) // 60_000, "remaining": int(remaining), "finished_at": now_ms}


def duration(minutes, translate=lambda value: value):
    if minutes <= 0:
        return translate("less than a minute")
    return (translate("%d minute") if minutes == 1 else translate("%d minutes")) % minutes


def session_text(summary, translate=lambda value: value):
    return translate("%d reviews · %s · %d ready at session end") % (
        summary["reviews"], duration(summary["minutes"], translate), summary["remaining"],
    )


def session_body(summary, translate=lambda value: value):
    return translate("%d reviews · %s\n%d cards still available in this deck.") % (
        summary["reviews"], duration(summary["minutes"], translate), summary["remaining"],
    )
