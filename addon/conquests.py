"""Native conquest data and local-only label suggestions."""

import html
import re

from . import share

_HIDDEN = re.compile(r"<(style|script)\b[^>]*>.*?</\1\s*>|<!--.*?-->", re.I | re.S)
_SOUND = re.compile(r"\[(?:sound|anki:play):[^\]]*\]")
_CLOZE = re.compile(r"\{\{c\d+::(.*?)(?:::[^}]*)?\}\}", re.S)
_ID = re.compile(r"[A-Za-z]{0,4}[-_.#]?\d+(?:[-_.:/]?[A-Za-z]{0,2}\d+)*[A-Za-z]?")
_HEX = re.compile(r"[0-9A-Fa-f]{8,}(?:-[0-9A-Fa-f]{4,})*")


def _line(value):
    return " ".join((value or "").split())[:120]


def _text(value):
    text = _CLOZE.sub(r"\1", _SOUND.sub(" ", _HIDDEN.sub(" ", value or "")))
    return _line(html.unescape(re.sub(r"<[^>]*>", " ", text)))


def _identifier(value):
    return bool(_ID.fullmatch(value) or _HEX.fullmatch(value) and any(letter.isdigit() for letter in value))


def _suggest(question, fields, cloze):
    front = next((field for field in fields if _CLOZE.search(field)), "") if cloze else question
    for value in [front, *fields]:
        text = _text(value)
        if text and not _identifier(text):
            return text
    return _text(fields[0]) if fields else ""


def _local(col, at):
    """Read on the collection thread. No card text or deck name is sent to the server."""
    try:
        cid = col.db.scalar("select cid from revlog where id = ?", at)
        if not cid:
            return "", ""
        card = col.get_card(cid)
        note = card.note()
        try:
            question = card.question()
        except Exception:
            question = ""
        label = _suggest(question, note.fields, note.note_type().get("type") == 1)
        return label, col.decks.name(card.odid or card.did)
    except Exception as error:
        print("ankiquest local conquest:", error)
        return "", ""


def _path(api, at):
    return "/api/conquests/%s/%d" % (api.user, at)


def _load(api, at):
    return api._request(_path(api, at))


def _save(api, at, label):
    value = _line(label)
    if value:
        return api._request(_path(api, at) + "/label", {"label": value})
    return api._request(_path(api, at) + "/label", method="DELETE")


def _tell(api, at):
    return api._request(_path(api, at) + "/tell", {})


def _undo(api, at):
    return api._request(_path(api, at) + "/tell", method="DELETE")


def _can_undo(found, now):
    return bool(found.get("told") and (found.get("undo_until") or 0) > now)


def _win(api, at, found, deck, translate):
    label = found.get("label") or translate("a difficult card")
    words = translate("I learned %s in %s.") % (label, deck) if deck else translate("I learned %s.") % label
    page = "%s/conquered/%s/%d" % (api.base, api.user, at)
    return {"title": translate("I conquered a card!"), "body": words + "\n" + page, "win": share.win_id(found)}


def _stats(found, translate):
    card = found["conquest"]
    days = max(0, (card["at"] - card["since"]) // 86_400_000)
    return translate("%d lapses · %d answers · %d days since the first answer") % (card["lapses"], card["answers"], days)
