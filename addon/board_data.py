"""Account choice -> server query -> ranked rows; never infer a stranger's study data."""
from dataclasses import dataclass, replace
from enum import Enum
from http import HTTPStatus
from urllib.parse import urlencode

from . import today

_CHOICES = "ankiquestBoards"
_OLD_PERIOD = "ankiquestBoardPeriod"
_TOP = 5


class _Visibility(Enum):
    HIDDEN = "hide"
    SHOWN = "unhide"


@dataclass(frozen=True)
class _Query:
    period: str = "week"
    scope: str = "friends"

    def _params(self):
        return {"period": self.period, "scope": self.scope}


@dataclass
class _Board:
    query: _Query
    rows: list
    scopes: list
    can_hide: bool


def _key(api):
    return [api.base, api.name]


def _chosen(profile, api):
    choices = profile.get(_CHOICES)
    if choices is None:
        query = _Query(today.period(profile.get(_OLD_PERIOD)))
        _remember(profile, api, query)
        return query
    stored = next((item for item in reversed(choices) if item.get("account") == _key(api)), {})
    return _Query(today.period(stored.get("period")), stored.get("scope", "friends"))


def _remember(profile, api, query):
    choices = [item for item in profile.get(_CHOICES, []) if item.get("account") != _key(api)]
    profile[_CHOICES] = choices + [dict(query._params(), account=_key(api))]
    # Keep the old period for older add-on versions; new accounts use their own choice.
    profile[_OLD_PERIOD] = query.period


def _legacy(read):
    try:
        return read()
    except Exception as error:
        if getattr(error, "status", None) == HTTPStatus.NOT_FOUND:
            return {}
        raise


def _fetch(api, query):
    settings = _legacy(api.deck_settings)
    social = _legacy(lambda: api._request("/api/social/" + api.user))
    scopes = [("friends", "Friends")]
    scopes += [("group:%d" % group["id"], group["name"]) for group in social.get("groups", [])]
    if isinstance(settings.get("global_leaderboard"), bool):
        scopes.append(("global", "Global"))
    if query.scope not in dict(scopes):
        query = replace(query, scope="friends")
    features = api.server_info()[0] if "global" in dict(scopes) else frozenset()
    rows = api.leaderboard(**query._params())
    return _Board(query, rows, scopes, "global_leaderboard_hide" in features)


def _rows(board, me):
    # Older circle rows have no rank; their server order still supplies it.
    result = []
    for index, row in enumerate(board.rows, 1):
        entry = dict(row)
        if "xp" not in entry:
            entry["xp"] = (entry.get("periods") or {}).get(board.query.period, 0)
        result.append((int(entry.get("rank", index)), entry, entry.get("user") == me))
    return result


def _top(board, me):
    rows = _rows(board, me)
    top = rows[:_TOP]
    if not any(mine for _, _, mine in top):
        top += [row for row in rows[_TOP:] if row[2]][:1]
    return top


def _records(api, query):
    return api._request("/api/records?" + urlencode({"scope": query.scope}))


def _hidden(api):
    return api._request("/api/leaderboard/hidden/" + api.user)


def _hide(api, choice, user):
    return api._request("/api/leaderboard/hidden/" + api.user, {"action": choice.value, "user": user})


def _can_hide(board, row, me):
    return board.can_hide and board.query.scope == "global" and row.get("user") != me and "periods" not in row
