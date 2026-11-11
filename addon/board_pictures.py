"""Visible rows -> bounded authenticated PNG read -> account-owned memory cache."""
import base64
import time
from enum import Enum, auto
from urllib.error import HTTPError
from urllib.parse import quote
from urllib.request import HTTPRedirectHandler, build_opener

from . import share, share_ui

_LIFETIME = 300
_RETRY = 60
_MAX_AVATAR = 512 * 1024


class _Phase(Enum):
    OPEN = auto()
    CLOSED = auto()


class _NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, msg, headers, newurl):
        # A picture endpoint must not forward the player's token to another address.
        raise HTTPError(request.full_url, code, "Avatar redirect refused", headers, fp)


def _avatar(api, user):
    return share.download_image(api.base + "/api/avatar/" + quote(user, safe=""),
                                limit=_MAX_AVATAR, token=api.token, opener=build_opener(_NoRedirect()).open)


class _Pictures:
    def __init__(self, addon):
        self._addon, self._api, self._profile = addon, addon.client(), addon.mw.pm.profile
        self._cache, self._pending = {}, {}
        self._phase = _Phase.OPEN

    def _owns(self):
        return self._phase is _Phase.OPEN and self._profile is self._addon.mw.pm.profile and share_ui.same_account(self._api, self._addon.client())

    def _close(self):
        self._phase = _Phase.CLOSED
        self._cache.clear()
        self._pending.clear()

    def _get(self, user):
        found = self._cache.get(user)
        return found[0] if found and time.monotonic() < found[1] else None

    def _load(self, users, redraw):
        if not self._owns():
            return
        missing = []
        now = time.monotonic()
        for user in dict.fromkeys(users):
            found = self._cache.get(user)
            if found and now < found[1]:
                continue
            if user not in self._pending:
                self._pending[user] = set()
                missing.append(user)
            self._pending[user].add(redraw)
        if not missing:
            return

        def work():
            answers = {}
            for user in missing:
                try:
                    answers[user] = _avatar(self._api, user)
                except Exception:
                    answers[user] = None
            return answers

        def done(future):
            if not self._owns():
                return
            answers = future.result()
            callbacks = set()
            for user, data in answers.items():
                uri = "data:image/png;base64," + base64.b64encode(data).decode("ascii") if data else None
                self._cache[user] = (uri, time.monotonic() + (_LIFETIME if data else _RETRY))
                callbacks.update(self._pending.pop(user, set()))
            for callback in callbacks:
                callback()

        self._addon.mw.taskman.run_in_background(work, done)
