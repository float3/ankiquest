"""XP feedback for a player set up without a token: the server previews the reviews and stores nothing.
Only servers that show this player publicly answer; anything else stays quiet."""

import time
from dataclasses import dataclass

from . import notify
from .client import PENDING_SQL, rows_to_reviews

BASELINE_MAX_AGE_MS = 10 * 60 * 1000

def _empty_cache():
    return {"account": None, "baseline": 0, "at": 0, "seen": None, "owner": None, "request": None}


@dataclass(frozen=True)
class _Owner:
    collection: object
    profile: object
    account: tuple

    def _owns(self, mw, api):
        return (self.collection is mw.col and self.profile is getattr(getattr(mw, "pm", None), "profile", None)
                and self.account == (api.base, api.user, api.token))


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
    if mw.col is None or not wanted(api):
        return
    owner = state["owner"]
    if owner is None or not owner._owns(mw, api):
        clear_account()
        owner = state["owner"] = _Owner(mw.col, getattr(getattr(mw, "pm", None), "profile", None),
                                       (api.base, api.user, api.token))
    if state["request"] is not None:
        return
    collection, cache = mw.col, state
    request = cache["request"] = object()

    def work():
        if cache is not state or not owner._owns(mw, api):
            return None
        return _fetch(api, lambda since: collection.db.all(PENDING_SQL, since), int(time.time() * 1000), cache)

    def done(future):
        if cache["request"] is request:
            cache["request"] = None
        if cache is not state or not owner._owns(mw, api):
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
