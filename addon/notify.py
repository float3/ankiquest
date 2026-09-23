"""What the desktop says out loud. The server writes the words; this decides when."""

FRESH_SECONDS = 86_400
HOUR_MS = 3_600_000


def feedback(response, show_feedback):
    """The upload banner: headlines always, the plain XP line only while reviewing."""
    told = response.get("feedback") or {}
    headlines = list(told.get("headlines") or [])
    status = told.get("status")
    if headlines:
        return "<br>".join(headlines + ([status] if status else [])), True
    if status and show_feedback:
        return status, False
    return None


def notice(entry):
    title, body = entry.get("title", ""), entry.get("body", "")
    return "%s<br>%s" % (title, body) if body else title


def streak_warning(profile, hours, now_ms, warned_for):
    """Once per Anki day, within the configured hours before it ends."""
    warning = profile.get("streak_warning")
    ends_at = profile.get("day_ends_at")
    if hours <= 0 or not warning or not ends_at or warned_for == ends_at:
        return None
    if ends_at - now_ms > hours * HOUR_MS:
        return None
    return notice(warning), ends_at


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


def answerable(entry):
    return bool(entry.get("sender")) and not entry.get("replied")


def summarize(entry):
    return "%s\n%s" % (entry.get("title", ""), entry.get("body", ""))
