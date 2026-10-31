"""XP feedback for a player set up without a token: the server previews the reviews and stores nothing.
Only servers that show this player publicly answer; anything else stays quiet."""

import time

from . import notify
from .client import PENDING_SQL, rows_to_reviews

BASELINE_MAX_AGE_MS = 10 * 60 * 1000

state = {"account": None, "baseline": 0, "at": 0, "seen": None, "busy": False}


def wanted(api):
    return bool(api.base and api.user and not api.token)


def baseline_due(account, now_ms):
    return state["account"] != account or now_ms - state["at"] > BASELINE_MAX_AGE_MS


def body(reviews, account):
    """The preview request; `seen_through` lets the server word only what is new since last time."""
    found = {"reviews": reviews}
    if state["seen"] and state["seen"][0] == account:
        found["seen_through"] = state["seen"][1]
    return found


def remember(account, baseline, reviews):
    sent = reviews[-1]["id"] if reviews else 0
    state["seen"] = (account, max(baseline, sent))


def fetch(api, rows_after, now_ms):
    account = (api.base, api.user)
    if baseline_due(account, now_ms):
        state["baseline"] = int(api.profile().get("last_review_id") or 0)
        state["account"], state["at"] = account, now_ms
    reviews = rows_to_reviews(rows_after(state["baseline"]))
    answer = api._request("/api/preview/" + api.user, body(reviews, account))
    remember(account, state["baseline"], reviews)
    return answer


def refresh(mw, api, show_feedback, say):
    """Shows what an upload would have said; 401 and 404 mean the server keeps this player private."""
    if mw.col is None or not wanted(api) or state["busy"]:
        return
    state["busy"] = True

    def work():
        return fetch(api, lambda since: mw.col.db.all(PENDING_SQL, since), int(time.time() * 1000))

    def done(future):
        state["busy"] = False
        try:
            answer = future.result()
        except Exception as error:
            if getattr(error, "status", None) not in (401, 404):
                print("ankiquest preview:", error)
            return
        message = notify.feedback(answer, show_feedback)
        if message:
            say(message[0], period=3500 if message[1] else 1800)

    mw.taskman.run_in_background(work, done, uses_collection=True)
