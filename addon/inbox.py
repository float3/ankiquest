"""The native inbox's rules: which rows get which buttons, what a reply may say, and the server calls.

Plain Python, so the tests load it without Anki. `api` is a client.Client; `translate` is language.tr."""

import re
from datetime import datetime

MAX_MESSAGE = 200
READ_BATCH = 200
SHAREABLE = ("celebration", "challenge_complete", "reminder_weekly_recap")
CHEERED = ("completion", "share", "conquest")
THANKED = ("reaction",)
REACTABLE = ("share", "completion")
CHALLENGE_ROUTE = re.compile(r"^/community#challenge-([1-9]\d*)$")
ACTIVITY_PATH = "/community#activity"

QUICK = {
    "cheer": ("Good job!", "Nice one \U0001f525", "Keep it up!"),
    "reply": ("Thanks!", "You made my day!", "I appreciate it!"),
    "nudge": ("On it!", "Starting now", "I will study later"),
}


def _utf8_size(character):
    point = ord(character)
    return 1 if point < 0x80 else 2 if point < 0x800 else 3 if point < 0x10000 else 4


def clean_reply(text):
    """`text` as the server accepts a message: control characters become spaces, and it is cut
    to MAX_MESSAGE UTF-8 bytes without splitting a character."""
    spaced = "".join(" " if ord(c) < 0x20 or 0x7F <= ord(c) <= 0x9F else c for c in text or "").strip()
    size, end = 0, 0
    for character in spaced:
        size += _utf8_size(character)
        if size > MAX_MESSAGE:
            break
        end += 1
    return spaced[:end].rstrip()


def reply_choices(kind, translate=lambda value: value):
    """The one-tap answers for a notification kind, the quick one first, as the phone offers them."""
    kind = kind or "completion"
    group = "cheer" if kind in CHEERED else "reply" if kind in THANKED else kind
    return [translate(choice) for choice in QUICK.get(group, ())]


def challenge_target(entry):
    """The challenge a row is about, from its id or its route; None for other rows."""
    challenge = entry.get("challenge_id")
    if isinstance(challenge, int) and not isinstance(challenge, bool) and challenge > 0:
        return challenge
    match = CHALLENGE_ROUTE.match(entry.get("route") or "")
    return int(match.group(1)) if match else None


def answerable(entry):
    """A friend's message that has not been answered; challenge rows are answered on the website."""
    return bool(entry.get("sender")) and not entry.get("replied") and challenge_target(entry) is None


def shareable(entry):
    """The player's own win: a celebration, a finished challenge or a weekly recap nobody sent."""
    return entry.get("kind") in SHAREABLE and not entry.get("sender") and int(entry.get("id") or 0) > 0


def cheerable(entry):
    """A friend's shared win or finished deck, which can get a 🎉 back."""
    return entry.get("kind") in REACTABLE and bool(entry.get("sender")) and int(entry.get("id") or 0) > 0


def headline(entry):
    """A row's title, and the body still to show under it. A cheer reads as one line:
    "🎉 X cheered: “Level 5!”"."""
    title, body = entry.get("title") or "", entry.get("body") or ""
    if entry.get("kind") == "reaction" and body:
        return "%s: %s" % (title, body), ""
    return title, body


def unread(entry, legacy=False):
    return not legacy and not entry.get("read_at")


def actions(entry, legacy=False):
    """The buttons a row gets, in order."""
    found = []
    if answerable(entry):
        found.append("reply")
    if shareable(entry):
        found.append("share")
    if cheerable(entry):
        found.append("cheer")
    found.append("open")
    if unread(entry, legacy):
        found.append("read")
    return found


def web_path(entry):
    target = challenge_target(entry)
    return "/community#challenge-%d" % target if target else ACTIVITY_PATH


def when(created_at):
    """A row's local time; empty when the server sent none."""
    try:
        return datetime.fromtimestamp(int(created_at)).strftime("%Y-%m-%d %H:%M") if created_at else ""
    except (OverflowError, OSError, TypeError, ValueError):
        return ""


def read_bodies(ids):
    """`POST /api/activity/{user}/read` takes at most READ_BATCH positive ids at a time."""
    wanted = sorted({int(i) for i in ids if int(i) > 0})
    return [{"ids": wanted[i:i + READ_BATCH]} for i in range(0, len(wanted), READ_BATCH)]


def reply_body(notification, message):
    return {"notification": int(notification), "message": clean_reply(message)}


def load(api):
    """Recent rows newest first, and whether they came from an older server's legacy list,
    whose rows have no read state."""
    try:
        page = api._request("/api/activity/" + api.user)
    except Exception as error:
        if getattr(error, "status", None) != 404:
            raise
        rows = api._request("/api/notifications/" + api.user)
        return list(reversed(rows if isinstance(rows, list) else [])), True
    items = page.get("items") if isinstance(page, dict) else None
    return list(items if isinstance(items, list) else []), False


def _post_empty(api, path, body):
    """The answer is `204 No Content`; an empty body is not an error."""
    try:
        api._request(path, body)
    except ValueError:
        pass


def mark_read(api, ids):
    for body in read_bodies(ids):
        _post_empty(api, "/api/activity/%s/read" % api.user, body)


def mark_all_read(api, through):
    _post_empty(api, "/api/activity/%s/read" % api.user, {"through": int(through)})


def reply(api, notification, message):
    """Sends a reply and answers who it went to."""
    body = reply_body(notification, message)
    if not body["message"]:
        raise ValueError("empty reply")
    answer = api._request("/api/reply/" + api.user, body)
    return (answer or {}).get("sent_to") or ""


def react(api, notification):
    """Cheers a row with 🎉; answers whether it had been cheered already."""
    answer = api._request("/api/react/" + api.user, {"notification": int(notification)})
    return bool((answer or {}).get("already"))


def error_text(error):
    """The server's own words when it gave any, else the status or the error itself."""
    message = getattr(error, "message", None)
    status = getattr(error, "status", None)
    if message:
        return str(message)
    if status:
        return "HTTP %d" % status
    return str(error)
