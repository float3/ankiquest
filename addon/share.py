"""Passing a win on: what the server is asked, what the player is told, and the square card's layout.

Plain Python, so the tests load it without Anki; share_ui.py does the drawing."""

CARD_SIZE = 1080
MARGIN = 80
ART_TOP = 90
ART_SIDE = 520
ART_GAP = 40
TITLE_GAP = 24
FOOTER_TOP = CARD_SIZE - 110
FOOTER_GAP = 30
MAX_LINES = 4
TITLE_PX = 72
BODY_PX = 44
FOOTER_PX = 36
FOOTER = "AnkiQuest"
ELLIPSIS = "…"
COLORS = {"background": "#FBF8F0", "text": "#20384A", "muted": "#5B6C76", "primary": "#245FAB"}


def win_from_entry(entry):
    """A shareable inbox row as a win."""
    return {"title": entry.get("title") or "", "body": entry.get("body") or "", "notification": int(entry["id"]), "place": None}


def win_from_place(change, place):
    """A climb into the top three, announced by the rank poll."""
    return {"title": change.get("title") or "", "body": change.get("body") or "", "notification": None, "place": place}


def request_body(win):
    """`POST /api/share/{user}` takes one of the player's own notifications or a place from 1 to 3."""
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
    """The words that travel with the card."""
    if server:
        return translate("%s: %s\nStudy with me on AnkiQuest: %s") % (win["title"], win["body"], server)
    return translate("%s: %s\nI study with AnkiQuest.") % (win["title"], win["body"])


def art_file(companion):
    """The bundled face for the chosen companion; None means no art."""
    if companion == "none":
        return None
    return "ankilope_face.png" if companion == "ankilope" else "aki_face.png"


def card_layout(art):
    """Where everything goes on the square card, as on the phone: art on top, then title and body."""
    width = CARD_SIZE - 2 * MARGIN
    top = ART_TOP
    art_rect = None
    if art:
        art_rect = ((CARD_SIZE - ART_SIDE) // 2, ART_TOP, ART_SIDE, ART_SIDE)
        top += ART_SIDE + ART_GAP
    return {
        "art": art_rect,
        "left": MARGIN,
        "width": width,
        "title_top": top,
        "footer_top": FOOTER_TOP,
    }


def lines_that_fit(top, line_height):
    """How many lines fit between `top` and the footer, never more than MAX_LINES."""
    room = FOOTER_TOP - FOOTER_GAP - top
    return max(0, min(MAX_LINES, int(room // line_height))) if line_height > 0 else 0


def _split_word(word, width, measure):
    pieces, current = [], ""
    for character in word:
        if current and measure(current + character) > width:
            pieces.append(current)
            current = character
        else:
            current += character
    return pieces + ([current] if current else [])


def _ellipsized(line, width, measure):
    while line and measure(line + ELLIPSIS) > width:
        line = line[:-1]
    return line.rstrip() + ELLIPSIS


def wrap(text, width, measure, max_lines):
    """`text` broken into lines no wider than `width` by `measure`; the last line ends in an
    ellipsis when it had to be cut."""
    lines = []
    for paragraph in (text or "").split("\n"):
        current = ""
        for word in paragraph.split():
            candidate = current + " " + word if current else word
            if measure(candidate) <= width:
                current = candidate
                continue
            if current:
                lines.append(current)
            pieces = _split_word(word, width, measure)
            lines.extend(pieces[:-1])
            current = pieces[-1] if pieces else ""
        if current:
            lines.append(current)
    if max_lines <= 0:
        return []
    if len(lines) > max_lines:
        lines = lines[:max_lines]
        lines[-1] = _ellipsized(lines[-1], width, measure)
    return lines
