"""Local deck names for sharing, independent of the player's filing structure."""

import re
import unicodedata

_PREFIX = re.compile(r"^(?:#?\d+[\s.)_-]+|\[[A-Za-z0-9]+\]\s*)")
_FILING = {
    "archive", "archives", "inbox", "default", "custom study session", "filtered", "old", "done",
    "archiv", "posteingang", "standard", "benutzerdefiniertes lernen", "gefiltert", "alt", "erledigt",
    "archivo", "archivados", "bandeja de entrada", "predeterminado", "sesión de estudio personalizada",
    "filtrado", "antiguo", "hecho", "boîte de réception", "par défaut", "séance d'étude personnalisée",
    "filtré", "ancien", "terminé", "arquivo", "arquivados", "caixa de entrada", "padrão",
    "sessão de estudo personalizado", "antigo", "concluído",
}
_GENERIC = {
    "book", "lesson", "level", "unit", "vocab", "vocabulary", "cards",
    "buch", "lektion", "stufe", "einheit", "vokabeln", "karten",
    "libro", "lección", "nivel", "unidad", "vocabulario", "tarjetas",
    "livre", "leçon", "niveau", "unité", "vocabulaire", "cartes",
    "livro", "lição", "nível", "unidade", "vocabulário", "cartões",
}
_KEY = "ankiquestConquestDecks"
_LIMIT = 120


def _fold(text):
    return unicodedata.normalize("NFC", text).casefold()


def _parts(path):
    parts = []
    for raw in (path or "").split("::"):
        part = " ".join(_PREFIX.sub("", raw.strip()).split())
        if part and _fold(part) not in _FILING and (not parts or _fold(part) != _fold(parts[-1])):
            parts.append(part)
    return parts


def _name(path):
    parts = _parts(path)
    if not parts:
        return ""
    leaf = parts[-1]
    words = set(re.findall(r"[^\W\d_]+", _fold(leaf)))
    parent = set(re.findall(r"[^\W\d_]+", _fold(parts[-2]))) if len(parts) > 1 else set()
    generic = len(leaf) <= 3 or leaf.isnumeric() or words <= _GENERIC or bool(words and words < parent)
    return " › ".join(parts[-2:] if generic else parts[-1:])[:_LIMIT]


def _record(col, at):
    """Capture the home deck on the collection thread; filtered decks use odid."""
    cid = col.db.scalar("select cid from revlog where id = ?", at)
    if not cid:
        return None
    card = col.get_card(cid)
    did = card.odid or card.did
    return {"id": str(did), "path": col.decks.name(did)}


def _state(profile, api):
    return profile.setdefault(_KEY, {}).setdefault(api.base, {}).setdefault(api.user, {})


def _capture(col, items, known):
    """Return new snapshots without touching profile preferences from the worker."""
    found = {}
    for item in items:
        at = item.get("at")
        if not at or str(at) in known:
            continue
        try:
            record = _record(col, at)
            if record:
                found[str(at)] = record
        except Exception as error:
            print("ankiquest conquest deck:", error)
    return found


def _remember(profile, api, records):
    known = _state(profile, api).setdefault("conquests", {})
    for at, record in records.items():
        known.setdefault(str(at), record)


def _choice(profile, api, record):
    if not record:
        return ""
    return _state(profile, api).get("names", {}).get(record["id"], _name(record["path"]))


def _choose(profile, api, record, text):
    if record:
        _state(profile, api).setdefault("names", {})[record["id"]] = " ".join(text.split())[:_LIMIT]
