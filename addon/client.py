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


class AccountError(Exception):
    """The server turned down a sign-in or sign-up. `status` says why; `message` is the server's own words."""

    def __init__(self, status, message):
        super().__init__(message)
        self.status = status
        self.message = message


def _account(base, path, body):
    request = urllib.request.Request(
        base.strip().rstrip("/") + path,
        data=json.dumps(body).encode(),
        headers={"Content-Type": "application/json"},
    )
    try:
        with urllib.request.urlopen(request, timeout=20) as response:
            return json.load(response)
    except urllib.error.HTTPError as error:
        try:
            message = json.load(error).get("error") or ""
        except (ValueError, AttributeError):
            message = ""
        raise AccountError(error.code, message) from None


def sign_in(base, user, password, device):
    """Trades an account's password for a token for this device. The password is not kept."""
    return _account(base, "/api/accounts/tokens", {"user": user, "password": password, "device": device})


def sign_up(base, user, password, display, device):
    """Creates an account and returns its first device token."""
    body = {"user": user, "password": password, "device": device}
    if display:
        body["display"] = display
    return _account(base, "/api/accounts", body)


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
        headers = {"Content-Type": "application/json", "Accept-Language": self.language}
        if self.token:
            headers["Authorization"] = "Bearer " + self.token
        request = urllib.request.Request(
            self.base + path,
            data=None if body is None else json.dumps(body).encode(),
            headers=headers,
        )
        with urllib.request.urlopen(request, timeout=20) as response:
            return json.load(response)

    def rank(self, previous):
        """The weekly order, and how this player moved since `previous`."""
        return self._request("/api/rank/" + self.user, {"previous": list(previous or [])})

    def profile(self):
        return self._request("/api/profile/" + self.user)

    def companion(self):
        choice = self._request("/api/companion/" + self.user)["companion"]
        return choice if choice in ("aki", "ankilope", "none") else "aki"

    def set_companion(self, companion):
        if companion not in ("aki", "ankilope", "none"):
            raise ValueError("Unknown study companion")
        choice = self._request("/api/companion/" + self.user, {"companion": companion})["companion"]
        return choice if choice in ("aki", "ankilope", "none") else "aki"

    def notifications(self):
        return self._request("/api/notifications/" + self.user)

    def reply(self, notification, message):
        answer = self._request(
            "/api/reply/" + self.user,
            {"notification": notification, "message": message},
        )
        return answer["sent_to"]

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
        settings = self._request("/api/decks/" + self.user)
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
