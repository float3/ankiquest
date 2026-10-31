import json
import urllib.error
import urllib.parse
import urllib.request
from datetime import datetime

DEFAULT_SERVER = "https://ankiquest.rationality-munich.com"

MAX_PENDING = 5000
UNDO_WINDOW_MS = 2 * 86_400_000
PENDING_SQL = (
    "select id, cid, lastIvl, ivl, time, type from revlog "
    "where id > ? and ease > 0 and type < 4 order by id limit %d" % MAX_PENDING
)


def offset_west_min():
    offset = datetime.now().astimezone().utcoffset()
    return -int(offset.total_seconds() // 60) if offset else 0


def rows_to_reviews(rows):
    return [
        {"id": r[0], "cid": r[1], "last_ivl": r[2], "ivl": r[3], "time_ms": r[4], "kind": r[5]}
        for r in rows
    ]


class ServerError(Exception):
    """The server answered with an error status. `status` is the HTTP code; `message` is the server's own words, if any."""

    def __init__(self, status, message):
        super().__init__(message or "HTTP Error %d" % status)
        self.status = status
        self.message = message


def _error_text(error):
    try:
        return json.load(error).get("error") or ""
    except (ValueError, AttributeError):
        return ""


def open_url(url, data=None, headers=None, method=None, timeout=20):
    """Opens `url`, turning an error status into a ServerError that carries the server's `{"error": ...}` text."""
    request = urllib.request.Request(url, data=data, headers=headers or {}, method=method)
    try:
        return urllib.request.urlopen(request, timeout=timeout)
    except urllib.error.HTTPError as error:
        raise ServerError(error.code, _error_text(error)) from None


def request_json(url, body=None, headers=None, timeout=20):
    """Sends `body` as JSON (a GET when it is None) and returns the decoded answer."""
    headers = dict(headers or {}, **{"Content-Type": "application/json"})
    data = None if body is None else json.dumps(body).encode()
    with open_url(url, data, headers, timeout=timeout) as response:
        return json.load(response)


def _account(base, path, body):
    return request_json(base.strip().rstrip("/") + path, body)


def sign_in(base, user, password, device):
    """Trades an account's password for a token for this device. The password is not kept."""
    return _account(base, "/api/accounts/tokens", {"user": user, "password": password, "device": device})


def sign_up(base, user, password, display, device):
    """Creates an account and returns its first device token."""
    body = {"user": user, "password": password, "device": device}
    if display:
        body["display"] = display
    return _account(base, "/api/accounts", body)


COMPANIONS = ("aki", "ankilope", "none")


def _companion(answer):
    choice = answer["companion"]
    return choice if choice in COMPANIONS else "aki"


class Client:
    def __init__(self, url, user, token, language="en"):
        self.base = url.strip().rstrip("/")
        self.name = user.strip()
        self.user = urllib.parse.quote(self.name, safe="")
        self.token = token.strip()
        self.language = language

    @property
    def configured(self):
        return bool(self.base and self.user and self.token)

    def _request(self, path, body=None):
        headers = {"Accept-Language": self.language}
        if self.token:
            headers["Authorization"] = "Bearer " + self.token
        return request_json(self.base + path, body, headers)

    def rank(self, previous):
        """The weekly order, and how this player moved since `previous`."""
        return self._request("/api/rank/" + self.user, {"previous": list(previous or [])})

    def profile(self):
        return self._request("/api/profile/" + self.user)

    def leaderboard(self):
        """Every visible player's standing, with XP for each period."""
        return self._request("/api/leaderboard")

    def companion(self):
        return _companion(self._request("/api/companion/" + self.user))

    def set_companion(self, companion):
        if companion not in COMPANIONS:
            raise ValueError("Unknown study companion")
        return _companion(self._request("/api/companion/" + self.user, {"companion": companion}))

    def notifications(self):
        return self._request("/api/notifications/" + self.user)

    def deck_settings(self):
        return self._request("/api/decks/" + self.user)

    def save_deck_settings(self, shared, unshared, recipients, nudges=None, celebrations=None):
        decks = [
            {"id": deck, "enabled": True, "recipients": list(recipients)} for deck in shared
        ]
        decks += [{"id": deck, "enabled": False, "recipients": []} for deck in unshared]
        body = {"decks": decks}
        if nudges is not None:
            body["nudges"] = bool(nudges)
        if celebrations is not None:
            body["celebrations"] = bool(celebrations)
        return self._request("/api/decks/" + self.user, body)

    def shared_decks(self):
        settings = self.deck_settings()
        return sorted(deck["id"] for deck in settings["decks"] if deck["enabled"])

    def upload(self, reviews, rollover_hour, silent, deleted=(), decks=None, clock_offset=None, catalog=False):
        body = {
            "reviews": reviews,
            "clock": {
                "offset_west_min": offset_west_min() if clock_offset is None else clock_offset,
                "rollover_hour": rollover_hour,
            },
            "silent": silent,
            "deleted": list(deleted),
        }
        if decks is not None:
            body["decks"] = decks
            body["catalog"] = catalog
        return self._request(
            "/api/reviews/" + self.user,
            body,
        )


def reconcile(window_rows, recent, known, mark):
    present = {r[0] for r in window_rows}
    deleted = sorted(recent - present) if mark else []
    restored = [r for r in window_rows if r[0] <= known and r[0] not in recent]
    return present, deleted, restored
