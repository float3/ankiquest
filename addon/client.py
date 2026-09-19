import json
import urllib.parse
import urllib.request
from datetime import datetime

MAX_PENDING = 5000
UNDO_WINDOW_MS = 2 * 86_400_000
PENDING_SQL = (
    "select id, cid, lastIvl, time, type from revlog "
    "where id > ? and ease > 0 and type < 4 order by id limit %d" % MAX_PENDING
)


def offset_west_min():
    offset = datetime.now().astimezone().utcoffset()
    return -int(offset.total_seconds() // 60) if offset else 0


def rows_to_reviews(rows):
    return [
        {"id": r[0], "cid": r[1], "last_ivl": r[2], "time_ms": r[3], "kind": r[4]}
        for r in rows
    ]


class Client:
    def __init__(self, url, user, token):
        self.base = url.strip().rstrip("/")
        self.user = urllib.parse.quote(user.strip(), safe="")
        self.token = token.strip()

    @property
    def configured(self):
        return bool(self.base and self.user and self.token)

    def _request(self, path, body=None):
        headers = {"Content-Type": "application/json"}
        if self.token:
            headers["Authorization"] = "Bearer " + self.token
        request = urllib.request.Request(
            self.base + path,
            data=None if body is None else json.dumps(body).encode(),
            headers=headers,
        )
        with urllib.request.urlopen(request, timeout=20) as response:
            return json.load(response)

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


def snapshot(profile):
    return {
        "xp": profile["xp_total"],
        "level": profile["level"],
        "into": profile["xp_into_level"],
        "need": profile["xp_for_next"],
        "streak": profile["streak"],
        "combo": profile["today"]["current_combo"],
        "quests": {q["title"] for q in profile["quests"] if q["done"]},
        "achievements": {a["title"] for a in profile["achievements"] if a["unlocked"]},
    }


def describe(before, after):
    gained = after["xp"] - before["xp"]
    if gained == 0:
        return None
    if gained < 0:
        status = "−%d XP  ·  combo %d" % (-gained, after["combo"])
        status += "  ·  Lv %d  %d/%d" % (after["level"], after["into"], after["need"])
        return status, False
    lines = []
    if after["level"] > before["level"]:
        lines.append("Level %d!" % after["level"])
    lines += ["Achievement: " + t for t in sorted(after["achievements"] - before["achievements"])]
    lines += ["Quest complete: " + t for t in sorted(after["quests"] - before["quests"])]
    if after["streak"] > before["streak"]:
        lines.append("%d day streak" % after["streak"])
    status = "+%d XP" % gained
    if after["combo"] >= 5:
        status += "  ·  combo %d" % after["combo"]
    status += "  ·  Lv %d  %d/%d" % (after["level"], after["into"], after["need"])
    return "<br>".join(lines + [status]), bool(lines)
