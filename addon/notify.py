"""What the desktop says out loud: rank changes, streak warnings and the inbox."""

from datetime import datetime

HOUR_MS = 3_600_000
DAY_MS = 86_400_000
FRESH_SECONDS = 86_400


def rank_message(previous, board, me):
    """The same wording the phone uses when your place on the board changes."""
    order = [row["user"] for row in board]
    if me not in order or not previous or me not in previous:
        return None
    rank = order.index(me)
    before = previous.index(me)
    if rank == before:
        return None
    display = {row["user"]: row.get("display") or row["user"] for row in board}
    xp = {row["user"]: row.get("week_xp", 0) for row in board}
    if not xp.get(me):
        return None
    def names(users):
        return ", ".join(display.get(user, user) for user in users)

    gap = ""
    if rank > 0:
        ahead = order[rank - 1]
        gap = " %s XP behind %s." % ("{:,}".format(xp[ahead] - xp[me]), display[ahead])
    if rank < before:
        passed = [u for u in previous[:before] if u in order and order.index(u) > rank]
        title = "\U0001f451 You took the crown" if rank == 0 else "▲ You're now #%d" % (rank + 1)
        body = "You passed %s." % names(passed) if passed else ""
    else:
        overtakers = [u for u in order[:rank] if u in previous and previous.index(u) > before]
        who = names(overtakers) or "Someone"
        title = "\U0001f451 %s took the crown" % who if before == 0 else "▼ %s passed you" % who
        body = "You're now #%d." % (rank + 1)
    return ("%s. %s%s" % (title, body, gap)).replace(".. ", ". ").strip()


def streak_message(profile, hours, now_ms, offset_west_min, rollover_hour, notified_day):
    """Warns once a day, the configured number of hours before the day rolls over."""
    if hours <= 0 or not profile.get("at_risk"):
        return None
    since_rollover = now_ms - offset_west_min * 60_000 - rollover_hour * HOUR_MS
    remaining = DAY_MS - since_rollover % DAY_MS
    if remaining > hours * HOUR_MS:
        return None
    day = since_rollover // DAY_MS
    if notified_day == day:
        return None
    left = -(-remaining // HOUR_MS)
    tail = (
        "A freeze would cover you, but why spend it?"
        if profile.get("freezes")
        else "No freezes left."
    )
    text = "\U0001f525 Your %d day streak ends in %dh. Review a few cards to keep it. %s" % (
        profile.get("streak", 0),
        left,
        tail,
    )
    return text, day


def fresh_messages(inbox, cursor, now_s):
    """Shows recent arrivals even on the first poll, but never replays a backlog."""
    fresh = []
    for entry in sorted(inbox, key=lambda entry: entry["id"]):
        if entry["id"] <= cursor:
            continue
        cursor = entry["id"]
        if entry.get("created_at", 0) >= now_s - FRESH_SECONDS:
            fresh.append(entry)
    return fresh, cursor


def when(created_at):
    """Local date and time of an inbox entry, as the phone shows it."""
    if not created_at:
        return ""
    seconds = created_at / 1000 if created_at >= 10_000_000_000 else created_at
    moment = datetime.fromtimestamp(seconds)
    return "%s %d, %d, %s" % (moment.strftime("%b"), moment.day, moment.year, moment.strftime("%H:%M"))


def answerable(entry):
    return bool(entry.get("sender")) and not entry.get("replied")


def summarize(entry):
    return "%s\n%s" % (entry.get("title", ""), entry.get("body", ""))
