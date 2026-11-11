"""Native social choices -> authenticated API actions; creation retries keep their request ID."""
import json
import re
import time
from enum import Enum
from urllib.parse import quote, urlsplit
from uuid import uuid4


class _Social(Enum):
    BEFRIEND = "befriend"
    UNFRIEND = "unfriend"
    MUTE = "mute"
    UNMUTE = "unmute"
    CREATE_GROUP = "create_group"
    RENAME_GROUP = "rename_group"
    NEW_INVITE = "new_invite"
    JOIN = "join"
    LEAVE = "leave"
    REMOVE = "remove"
    VISIBILITY = "visibility"


class _GoalAction(Enum):
    ACCEPT = "accept"
    DECLINE = "decline"
    LEAVE = "leave"
    CANCEL = "cancel"


class _WeeklyAction(Enum):
    INVITE = "invite"
    DISMISS = "dismiss"


class _GoalState(Enum):
    ACTIVE = "active"
    UPCOMING = "upcoming"
    WAITING = "waiting"
    COMPLETE = "complete"
    ENDED = "ended"
    CANCELLED = "cancelled"
    DECLINED = "declined"
    UNKNOWN = "unknown"


class _Member(Enum):
    INVITED = "invited"
    ACCEPTED = "accepted"
    DECLINED = "declined"
    LEFT = "left"


class _Kind(Enum):
    STUDY_DAYS = "study_days"
    REVIEWS = "reviews"


def _people(api):
    return api._request("/api/social/" + api.user)


def _change(api, action, **fields):
    return api._request("/api/social/" + api.user, dict(fields, action=action.value))


def _invite_code(text, base):
    value = text.strip()
    parts = urlsplit(value)
    if parts.scheme or parts.netloc:
        origin = urlsplit(base)
        if parts.scheme != origin.scheme or parts.netloc.lower() != origin.netloc.lower():
            raise ValueError("Use an invitation from this server.")
        value = parts.path
    if value.startswith("/join/"):
        value = value[len("/join/"):].rstrip("/")
    if not re.fullmatch(r"[A-Za-z0-9_-]{1,128}", value):
        raise ValueError("Enter a group invitation link or code.")
    return value


def _preview(api, code):
    return api._request("/api/invites/" + quote(code, safe=""))


def _invite_link(api, group):
    code = group.get("invite")
    return api.base + "/join/" + quote(code, safe="") if code else None


def _goals(api):
    return api._request("/api/community/challenges/" + api.user)


def _goal_action(api, goal, action):
    return api._request("/api/community/challenges/%s/%d" % (api.user, goal), {"action": action.value})


def _weekly(api, week, action):
    return api._request("/api/community/challenges/%s/weekly" % api.user, {"week_start": week, "action": action.value})


def _state(goal):
    try:
        return _GoalState(goal.get("status"))
    except ValueError:
        return _GoalState.UNKNOWN


def _actions(goal, me, now=None):
    moment = int(time.time() * 1000) if now is None else now
    if _state(goal) in (_GoalState.ENDED, _GoalState.CANCELLED, _GoalState.DECLINED, _GoalState.UNKNOWN) or goal.get("end_at", 0) <= moment:
        return ()
    member = next((row for row in goal.get("members", []) if row["user"] == me), {})
    if member.get("status") == _Member.INVITED.value:
        return (_GoalAction.ACCEPT, _GoalAction.DECLINE)
    if member.get("status") != _Member.ACCEPTED.value:
        return ()
    return (_GoalAction.CANCEL,) if goal.get("creator") == me else (_GoalAction.LEAVE,)


def _suggestion(data, now=None):
    moment = int(time.time() * 1000) if now is None else now
    suggestion = data.get("weekly_suggestion")
    if not suggestion or suggestion.get("dismissed") or suggestion.get("challenge_id") is not None or suggestion.get("expires_at", 0) <= moment:
        return None
    return suggestion


class _Draft:
    def __init__(self):
        self._payload, self._id = None, None

    def _body(self, fields):
        wanted = dict(fields, title=fields["title"].strip(), recipients=sorted(set(fields["recipients"])))
        normalized = json.dumps(wanted, sort_keys=True)
        if normalized != self._payload:
            self._payload, self._id = normalized, str(uuid4())
        return dict(wanted, request_id=self._id)


def _create(api, body):
    return api._request("/api/community/challenges/" + api.user, body)
