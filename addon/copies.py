"""Deck copies: an independent .apkg sent to chosen friends, without the sender's review history."""

import html
import json
import os
import tempfile
import urllib.parse

from .client import open_url

MAX_BYTES = 25 * 1024 * 1024
MAX_RECIPIENTS = 20
MAX_DECK_NAME_BYTES = 160
CHUNK = 64 * 1024


class TooLarge(Exception):
    """The package is over the server's limit; `limit` is that limit in bytes."""

    def __init__(self, limit):
        super().__init__("Deck copy exceeds %d bytes" % limit)
        self.limit = limit


def limit_of(inbox):
    """The server's own limit when it states one, otherwise the documented 25 MB."""
    value = (inbox or {}).get("max_bytes")
    return value if isinstance(value, int) and value > 0 else MAX_BYTES


def too_large(size, limit=MAX_BYTES):
    return size > limit


def megabytes(size):
    return max(1, round(size / (1024 * 1024)))


def size_text(size):
    if size >= 1024 * 1024:
        return "%.1f MB" % (size / (1024 * 1024))
    return "%d KB" % max(1, round(size / 1024))


def deck_label(name):
    """The deck name the server accepts: no control characters, at most 160 UTF-8 bytes."""
    clean = "".join(" " if ch < " " or ch == "\x7f" else ch for ch in name).strip()
    while len(clean.encode("utf-8")) > MAX_DECK_NAME_BYTES:
        clean = clean[:-1]
    return clean.strip() or "Deck"


def path(api, offer=None):
    base = "/api/deck-copies/" + api.user
    return base if offer is None else "%s/%d" % (base, int(offer))


def upload_url(api, deck, recipients):
    query = urllib.parse.urlencode({"deck": deck_label(deck), "recipients": json.dumps(list(recipients))})
    return api.base + path(api) + "?" + query


def headers(api, content_type=None):
    found = {"Accept-Language": getattr(api, "language", "en"), "Authorization": "Bearer " + api.token}
    if content_type:
        found["Content-Type"] = content_type
    return found


def pending(inbox):
    return list((inbox or {}).get("offers") or [])


def friends(inbox):
    return [person for person in (inbox or {}).get("friends") or [] if person.get("user")]


def display_names(inbox):
    return {person["user"]: person.get("display") or person["user"] for person in friends(inbox)}


def offer_html(offer, names, translate):
    """One offer as rich text; the sender and the deck name come from other people and are escaped."""
    sender = names.get(offer.get("sender"), offer.get("sender") or "")
    text = translate("%s shared %s") % (
        "<b>%s</b>" % html.escape(str(sender)),
        "<b>%s</b>" % html.escape(str(offer.get("deck") or "")),
    )
    size = offer.get("bytes")
    return text + (" · %s" % size_text(size) if isinstance(size, int) and size > 0 else "")


def menu_title(count, translate):
    title = translate("ankiquest: decks offered to me…")
    return title if not count else "%s (%d)" % (title, count)


def new_offers(offers, seen):
    """Offers not announced before, oldest first, and every id to remember."""
    known = set(seen or [])
    fresh = sorted((offer for offer in offers if offer.get("id") not in known), key=lambda offer: offer.get("id", 0))
    return fresh, sorted(offer["id"] for offer in offers if "id" in offer)


def check_recipients(recipients, sender, translate):
    """None when the choice is acceptable, otherwise the reason it is not, already translated."""
    chosen = [user for user in recipients if user != sender]
    if not chosen:
        return translate("Choose at least one friend.")
    if len(chosen) > MAX_RECIPIENTS:
        return translate("Choose at most %d friends.") % MAX_RECIPIENTS
    return None


def inbox(api):
    return api._request(path(api))


def send(api, deck, recipients, package, limit=MAX_BYTES):
    """Uploads the package file at `package` as an offer to `recipients`."""
    size = os.path.getsize(package)
    if too_large(size, limit):
        raise TooLarge(limit)
    with open(package, "rb") as handle:
        data = handle.read()
    with open_url(upload_url(api, deck, recipients), data, headers(api, "application/octet-stream"), "POST", timeout=90) as response:
        body = response.read()
    return json.loads(body) if body else {}


def download(api, offer, target, limit=MAX_BYTES):
    """Saves the offer's package to `target`, stopping as soon as it passes `limit`."""
    size = 0
    try:
        with open_url(api.base + path(api, offer), None, headers(api), timeout=90) as response, open(target, "wb") as out:
            while True:
                chunk = response.read(CHUNK)
                if not chunk:
                    break
                size += len(chunk)
                if too_large(size, limit):
                    raise TooLarge(limit)
                out.write(chunk)
        if size == 0:
            raise ValueError("Empty deck copy")
    except Exception:
        _remove(target)
        raise
    return target


def dismiss(api, offer):
    with open_url(api.base + path(api, offer), None, headers(api), "DELETE") as response:
        response.read()


def temporary_package(prefix):
    handle, name = tempfile.mkstemp(prefix=prefix, suffix=".apkg")
    os.close(handle)
    return name


def _remove(name):
    try:
        os.remove(name)
    except OSError:
        pass


def export_deck(col, deck_id, target):
    """Cards, notes, media and deck options, never scheduling, with the modern exporter where Anki has it."""
    try:
        from anki.collection import DeckIdLimit
        from anki.import_export_pb2 import ExportAnkiPackageOptions

        options = ExportAnkiPackageOptions(
            with_scheduling=False, with_deck_configs=True, with_media=True, legacy=False
        )
        col.export_anki_package(out_path=target, options=options, limit=DeckIdLimit(deck_id))
    except (ImportError, TypeError, ValueError, AttributeError):
        _legacy_export(col, deck_id, target)


def _legacy_export(col, deck_id, target):
    from anki.exporting import AnkiPackageExporter

    exporter = AnkiPackageExporter(col)
    exporter.did = deck_id
    exporter.includeSched = False
    exporter.includeMedia = True
    exporter.exportInto(target)


def share(api, col, deck_id, deck_name, recipients, limit=MAX_BYTES):
    """Exports the deck to a temporary package, uploads it and always removes the file."""
    package = temporary_package("ankiquest-share-")
    try:
        export_deck(col, deck_id, package)
        return send(api, deck_name, recipients, package, limit)
    finally:
        _remove(package)
