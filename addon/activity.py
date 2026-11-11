"""Activity pages keep server counts and cursors; only a 404 selects legacy notifications."""
from dataclasses import dataclass, replace
from enum import Enum, auto
from http import HTTPStatus
from typing import Optional
from urllib.parse import urlencode


class _Category(Enum):
    ALL = ""
    NEEDS_ACTION = "needs_action"
    MESSAGES = "messages"
    CHALLENGES = "challenges"
    DECK_COMPLETIONS = "deck_completions"
    STUDY_UPDATES = "study_updates"


class _Read(Enum):
    ALL = auto()
    UNREAD = auto()


class _Source(Enum):
    ACTIVITY = auto()
    LEGACY = auto()
    UNFILTERED = auto()


@dataclass
class _Page:
    items: list
    attention: list
    unread: int
    actions: int
    latest: Optional[int]
    before: Optional[int]
    source: _Source = _Source.ACTIVITY


def _parse(answer):
    if not isinstance(answer, dict) or not isinstance(answer.get("items"), list):
        raise ValueError("Invalid activity response")
    return _Page(answer["items"], answer.get("attention") or [], int(answer.get("unread_count", 0)),
                 int(answer.get("action_count", 0)), answer.get("latest_id"), answer.get("next_before"))


def _request(api, category, read, before):
    query = {"limit": 50}
    if category is not _Category.ALL:
        query["category"] = category.value
    if read is _Read.UNREAD:
        query["unread_only"] = "true"
    if before is not None:
        query["before"] = before
    return _parse(api._request("/api/activity/%s?%s" % (api.user, urlencode(query))))


def _load(api, category=_Category.ALL, read=_Read.ALL, before=None):
    try:
        return _request(api, category, read, before)
    except Exception as error:
        status = getattr(error, "status", None)
        if status == HTTPStatus.BAD_REQUEST and (category is not _Category.ALL or read is not _Read.ALL):
            return replace(_request(api, _Category.ALL, _Read.ALL, before), source=_Source.UNFILTERED)
        if status != HTTPStatus.NOT_FOUND or before is not None:
            raise
        items = api.notifications()
        return _Page(sorted(items, key=lambda row: row["id"], reverse=True), [], 0, 0,
                     max((row["id"] for row in items), default=None), None, _Source.LEGACY)


def _append(previous, page):
    # A refreshed row replaces its old copy; pagination never duplicates a notification.
    rows = {row["id"]: row for row in previous.items}
    rows.update({row["id"]: row for row in page.items})
    return replace(page, items=sorted(rows.values(), key=lambda row: row["id"], reverse=True))
