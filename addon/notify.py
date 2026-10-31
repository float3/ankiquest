"""What the desktop says out loud. The server writes the words; this decides when."""

import html
import re
import urllib.parse

FRESH_SECONDS = 86_400
HOUR_MS = 3_600_000


def rich(text):
    """Server words for a rich-text tooltip: never markup, line breaks kept."""
    return html.escape(str(text)).replace("\n", "<br>")


def feedback(response, show_feedback):
    """The upload banner: headlines always, the plain XP line only while reviewing."""
    told = response.get("feedback") or {}
    headlines = list(told.get("headlines") or [])
    status = told.get("status")
    if headlines:
        return "<br>".join(rich(line) for line in headlines + ([status] if status else [])), True
    if status and show_feedback:
        return rich(status), False
    return None


def conquest_path(response):
    """The page for the hard card this upload announced as learned at last, if any."""
    headlines = (response.get("feedback") or {}).get("headlines") or []
    conquests = response.get("conquests") or []
    if not conquests or not any(line.startswith("🏆") for line in headlines):
        return None
    return "/conquered/{user}/%d" % conquests[0]["at"]


MAX_LABEL = 120


def suggested_label(field):
    """A card's first field as one line of plain text, short enough to be a label."""
    text = re.sub(r"\[sound:[^\]]*\]", " ", field or "")
    text = html.unescape(re.sub(r"<[^>]*>", " ", text))
    return " ".join(text.split())[:MAX_LABEL]


def with_suggestion(path, label):
    """Offers the label in the fragment: the page shows it to the player, but the
    browser never sends a fragment to the server, so nothing leaves until they save it."""
    return path + "#suggest=" + urllib.parse.quote(label) if label else path


def notice(entry):
    """A server notification as tooltip text, with its title and body escaped."""
    title, body = rich(entry.get("title", "")), rich(entry.get("body", ""))
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
