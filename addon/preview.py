"""XP feedback for a player set up without a token: the server previews the reviews and stores nothing.
Only servers that show this player publicly answer; anything else stays quiet."""

import time

from . import notify
from .client import PENDING_SQL, rows_to_reviews

BASELINE_MAX_AGE_MS = 10 * 60 * 1000

def _empty_cache():
    return {"account": None, "baseline": 0, "at": 0, "seen": None, "busy": False}


state = _empty_cache()


def clear_account():
    global state
    state = _empty_cache()


def wanted(api):
    return bool(api.base and api.user and not api.token)


def _body(reviews, account, cache):
    """The preview request; `seen_through` lets the server word only what is new since last time."""
    found = {"reviews": reviews}
    if cache["seen"] and cache["seen"][0] == account:
        found["seen_through"] = cache["seen"][1]
    return found


def fetch(api, rows_after, now_ms):
    return _fetch(api, rows_after, now_ms, state)


def _fetch(api, rows_after, now_ms, cache):
    account = (api.base, api.user)
    if cache["account"] != account or now_ms - cache["at"] > BASELINE_MAX_AGE_MS:
        cache["baseline"] = int(api.profile().get("last_review_id") or 0)
        cache["account"], cache["at"] = account, now_ms
    reviews = rows_to_reviews(rows_after(cache["baseline"]))
    answer = api._request("/api/preview/" + api.user, _body(reviews, account, cache))
    sent = reviews[-1]["id"] if reviews else 0
    cache["seen"] = (account, max(cache["baseline"], sent))
    return answer


def refresh(mw, api, show_feedback, say):
    """Shows what an upload would have said; 401 and 404 mean the server keeps this player private."""
    if mw.col is None or not wanted(api) or state["busy"]:
        return
    collection, cache = mw.col, state
    cache["busy"] = True

    def work():
        if cache is not state or mw.col is not collection:
            return None
        return _fetch(api, lambda since: collection.db.all(PENDING_SQL, since), int(time.time() * 1000), cache)

    def done(future):
        cache["busy"] = False
        if cache is not state or mw.col is not collection:
            return
        try:
            answer = future.result()
        except Exception as error:
            if getattr(error, "status", None) not in (401, 404):
                print("ankiquest preview:", error)
            return
        if answer is None:
            return
        message = notify.feedback(answer, show_feedback)
        if message:
            say(message[0], period=3500 if message[1] else 1800)

    mw.taskman.run_in_background(work, done, uses_collection=True)
