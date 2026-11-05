"""Moving to the server's main address, the `public_url` it names at /api/capabilities.

A player who saved an older address of the same server (one that proxies to it) moves to the
main one once their token is known to work there. Everything the add-on keeps per address moves
with them, so nothing is uploaded again and no celebration repeats."""

import urllib.parse


def main_address(answer):
    """The server's main address from a capabilities answer: an https address without a
    trailing slash, or None."""
    url = answer.get("public_url") if isinstance(answer, dict) else None
    if not isinstance(url, str):
        return None
    url = url.strip().rstrip("/")
    parts = urllib.parse.urlsplit(url)
    if parts.scheme != "https" or not parts.netloc or parts.query or parts.fragment:
        return None
    return url


def should_move(base, main):
    """Whether a player saved at `base` should try the main address `main`."""
    return bool(base and main) and base.strip().rstrip("/") != main


def _moved(value, old, new):
    if isinstance(value, list) and value and value[0] == old:
        return [new] + [_moved(item, old, new) for item in value[1:]]
    if isinstance(value, list):
        return [_moved(item, old, new) for item in value]
    if isinstance(value, dict):
        return {key: _moved(item, old, new) for key, item in value.items()}
    return value


def move_profile(profile, old, new):
    """Rewrites the add-on's per-account records in the Anki profile from address `old` to `new`.
    Accounts are lists that start with the address, also inside records."""
    for key in [key for key in profile if isinstance(key, str) and key.startswith("ankiquest")]:
        profile[key] = _moved(profile[key], old, new)


def move_shown(shown, old, new):
    """The goal setup prompts shown, kept as "address player", for address `new`."""
    prefix = old + " "
    return [new + " " + key[len(prefix):] if isinstance(key, str) and key.startswith(prefix) else key for key in shown or []]
