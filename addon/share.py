"""Passing a win on: what the server is asked, what the player is told, and fetching the
server's card picture.

Plain Python, so the tests load it without Anki; share_ui.py shows it."""

import urllib.parse
import urllib.request
from pathlib import Path

MAX_IMAGE = 4 * 1024 * 1024
IMAGE_TIMEOUT = 15
PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"


def win_id(entry):
    value = (entry or {}).get("win")
    return value if isinstance(value, int) and not isinstance(value, bool) and value > 0 else None


def _share_words(notice):
    """Use the player's own words, with a fallback for older notices."""
    return {
        "title": notice.get("share_title") or notice.get("title") or "",
        "body": notice.get("share_body") or notice.get("body") or "",
    }


def win_from_entry(entry):
    """A shareable inbox row as a win; `win` is the server's win id when the row has one."""
    return {
        **_share_words(entry),
        "notification": int(entry["id"]), "place": None, "win": win_id(entry),
    }


def win_from_place(change, place):
    """A climb into the top three, announced by the rank poll."""
    return {**_share_words(change), "notification": None, "place": place, "win": None}


def has_card(win):
    """Wins the server knows by id have a card picture and a link; older rows travel as text."""
    return win_id(win) is not None


def request_body(win):
    """`POST /api/share/{user}` shares a win by id, or else one of the player's own notifications
    or a place from 1 to 3."""
    if has_card(win):
        return {"win": win["win"], "to": "friends"}
    if win.get("notification"):
        return {"notification": int(win["notification"])}
    place = win.get("place")
    if isinstance(place, int) and 1 <= place <= 3:
        return {"place": place}
    raise ValueError("nothing to share")


def send(api, win):
    """Shares with the player's friends and answers how many got it now."""
    answer = api._request("/api/share/" + api.user, request_body(win))
    return int((answer or {}).get("recipients") or 0)


def absolute(base, url):
    """The server may answer with paths; they are relative to the server the add-on talks to."""
    return urllib.parse.urljoin(base.rstrip("/") + "/", url or "")


def link(api, win):
    """Publishes the win: its public link, card picture and the server's invitation text.
    Only for when the player chooses to share a link; the picture alone needs none."""
    answer = api._request("/api/share/" + api.user, {"win": win["win"], "to": "link"}) or {}
    return {
        "url": absolute(api.base, answer.get("url")),
        "image": absolute(api.base, answer.get("image")),
        "text": answer.get("text") or "",
    }


def _web_url(url):
    return urllib.parse.urlsplit(url or "").scheme in ("http", "https")


def download_image(url, limit=MAX_IMAGE, timeout=IMAGE_TIMEOUT, opener=urllib.request.urlopen, token=None):
    """A card picture's PNG bytes. Public card pictures need no token; the owner's own card
    is fetched with `token`, which callers only pass for their own server's address."""
    if not _web_url(url):
        raise ValueError("not a web address")
    headers = {"Accept": "image/png"}
    if token:
        headers["Authorization"] = "Bearer " + token
    with opener(urllib.request.Request(url, headers=headers), timeout=timeout) as response:
        data = response.read(limit + 1)
    if len(data) > limit:
        raise ValueError("the picture is too large")
    if not data.startswith(PNG_SIGNATURE):
        raise ValueError("not a PNG picture")
    return data


def card_url(api, win):
    """The owner-only picture of a win, which exists before any link and publishes nothing."""
    return "%s/api/wins/%s/%d/card.png" % (api.base, api.user, win_id(win))


def card(api, win, opener=urllib.request.urlopen):
    """The win's card picture for copying or saving, without making a public link; call off
    the main thread."""
    return {"png": download_image(card_url(api, win), opener=opener, token=api.token)}


def climbed_to(previous, order, me):
    """The 1-based place `me` climbed to between two leaderboard orders, or None when they did not climb."""
    order, previous = list(order or []), list(previous or [])
    if me not in order or me not in previous:
        return None
    now, before = order.index(me), previous.index(me)
    return now + 1 if before > now else None


def podium(previous, order, me):
    """A climb worth sharing: into the top three."""
    place = climbed_to(previous, order, me)
    return place if place and place <= 3 else None


def sent_text(recipients, translate=lambda value: value):
    if recipients <= 0:
        return translate("Nobody new got it: your friends already have it, or you have no friends in AnkiQuest yet.")
    return (translate("Shared with %d friend") if recipients == 1 else translate("Shared with %d friends")) % recipients


def invitation(win, server, translate=lambda value: value):
    """The words that travel with a win the server has no card for."""
    if server:
        return translate("%s: %s\nStudy with me on AnkiQuest: %s") % (win["title"], win["body"], server)
    return translate("%s: %s\nI study with AnkiQuest.") % (win["title"], win["body"])


def face_path(companion):
    """The bundled face for the chosen companion; None means no art."""
    if companion == "none":
        return None
    return str(Path(__file__).with_name("ankilope_face.png" if companion == "ankilope" else "aki_face.png"))
