"""The Tools menu entries for deck copies: send a deck to friends, and the decks offered to me."""

import html

from aqt.qt import (
    QAction,
    QCheckBox,
    QComboBox,
    QDialog,
    QHBoxLayout,
    QLabel,
    QPushButton,
    Qt,
    QVBoxLayout,
    QWidget,
)
from aqt.utils import tooltip

from . import copies
from .language import tr
from .ui import _brand, _enum, _scroller

SEEN_KEY = "ankiquestSeenDeckCopies"

state = {"mw": None, "client": None, "companion": None, "action": None, "polling": False, "count": 0}


def setup(mw, client, companion):
    """`client()` gives the current account; `companion()` the mascot to show."""
    state.update(mw=mw, client=client, companion=companion)
    send = QAction(tr("ankiquest: send a deck to friends…"), mw)
    send.triggered.connect(open_send)
    offered = QAction(copies.menu_title(0, tr), mw)
    offered.triggered.connect(open_offers)
    mw.form.menuTools.addAction(send)
    mw.form.menuTools.addAction(offered)
    state["action"] = offered


def _account(api):
    return [api.base, api.user, api.token]


def _failed(error, rich=True):
    """Errors can quote the server, so tooltips get them escaped; plain-text labels do not need it."""
    if isinstance(error, copies.TooLarge):
        return tr("This deck copy is over %d MB. Use Anki’s normal export to share larger decks.") % copies.megabytes(error.limit)
    detail = str(error)
    return tr("Could not complete this deck-sharing action (%s).") % (html.escape(detail) if rich else detail)


def _set_count(count):
    state["count"] = count
    action = state["action"]
    if action is not None:
        action.setText(copies.menu_title(count, tr))


def poll(quiet=False):
    """Refreshes the badge on the menu entry; new offers are announced unless `quiet`."""
    mw, api = state["mw"], state["client"]() if state["client"] else None
    if mw is None or api is None or not api.configured:
        _set_count(0)
        return
    if state["polling"]:
        return
    state["polling"] = True
    account = _account(api)

    def done(future):
        state["polling"] = False
        try:
            inbox = future.result()
        except Exception as error:
            print("ankiquest deck copies:", error)
            return
        if _account(state["client"]()) != account:
            return
        offers = copies.pending(inbox)
        _set_count(len(offers))
        announce(offers, copies.display_names(inbox), quiet)

    mw.taskman.run_in_background(lambda: copies.inbox(api), done)


def announce(offers, names, quiet):
    profile = state["mw"].pm.profile
    fresh, seen = copies.new_offers(offers, profile.get(SEEN_KEY))
    profile[SEEN_KEY] = seen
    if quiet:
        return
    for offer in fresh:
        tooltip(
            "%s<br><i>%s</i>" % (copies.offer_html(offer, names, tr), html.escape(tr("Tools → ankiquest: decks offered to me…"))),
            period=6000,
        )


def _configured():
    api = state["client"]()
    if not api.configured:
        tooltip(tr("Set the server, player and token in ankiquest settings first."))
        return None
    return api


def _load(api, then):
    def done(future):
        try:
            inbox = future.result()
        except Exception as error:
            tooltip(_failed(error))
            return
        if _account(state["client"]()) == _account(api):
            then(inbox)

    state["mw"].taskman.run_in_background(lambda: copies.inbox(api), done)


def open_send():
    mw = state["mw"]
    api = _configured()
    if api is None or mw.col is None:
        return

    def loaded(inbox):
        people = copies.friends(inbox)
        if not people:
            tooltip(tr("No friends are available on this server yet."))
            return
        decks = sorted(
            ((deck.name, deck.id) for deck in mw.col.decks.all_names_and_ids(skip_empty_default=True, include_filtered=False)),
            key=lambda deck: deck[0].lower(),
        )
        choice = send_dialog(mw, decks, people, api.name)
        if choice is not None:
            send(api, choice, copies.limit_of(inbox))

    _load(api, loaded)


def send(api, choice, limit):
    deck_id, deck_name, recipients = choice
    mw = state["mw"]
    tooltip(tr("Preparing and sending the deck copy…"))

    def done(future):
        try:
            future.result()
        except Exception as error:
            tooltip(_failed(error), period=5000)
            return
        tooltip(tr("Deck copy sent."))

    mw.taskman.run_in_background(
        lambda: copies.share(api, mw.col, deck_id, deck_name, recipients, limit), done, uses_collection=True
    )


def send_dialog(parent, decks, people, sender):
    """A deck and the friends to send it to; stays open until the choice is acceptable."""
    dialog = QDialog(parent)
    dialog.setWindowTitle(tr("Send a deck to friends"))
    dialog.resize(480, 520)
    layout = QVBoxLayout(dialog)
    _brand(dialog, layout, state["companion"]())
    about = QLabel(tr("Send an independent copy with cards and media, without your study history. Friends choose whether to import it."))
    about.setWordWrap(True)
    layout.addWidget(about)

    deck = QComboBox()
    for name, deck_id in decks:
        deck.addItem(name, deck_id)
    layout.addWidget(QLabel(tr("Choose a deck")))
    layout.addWidget(deck)

    boxes = {}
    for person in people:
        name = person.get("display") or person["user"]
        box = QCheckBox(name if name == person["user"] else "%s (%s)" % (name, person["user"]))
        boxes[person["user"]] = box
    layout.addWidget(QLabel(tr("Send to")))
    layout.addWidget(_scroller(list(boxes.values())), 1)

    status = QLabel()
    status.setTextFormat(_enum(Qt, "TextFormat", "PlainText"))
    status.setWordWrap(True)
    layout.addWidget(status)

    def chosen():
        return [user for user, box in boxes.items() if box.isChecked()]

    def check():
        reason = copies.check_recipients(chosen(), sender, tr)
        if deck.currentData() is None:
            reason = tr("Choose a deck")
        if reason:
            status.setText(reason)
            return
        dialog.accept()

    layout.addLayout(_row_buttons(dialog, tr("Send"), check))
    if not dialog.exec():
        return None
    return deck.currentData(), deck.currentText(), chosen()


def _row_buttons(dialog, accept_text, on_accept):
    row = QHBoxLayout()
    row.addStretch(1)
    cancel = QPushButton(tr("Cancel"))
    cancel.clicked.connect(dialog.reject)
    accept = QPushButton(accept_text)
    accept.setDefault(True)
    accept.clicked.connect(on_accept)
    row.addWidget(cancel)
    row.addWidget(accept)
    return row


def open_offers():
    api = _configured()
    if api is None:
        return

    def loaded(inbox):
        offers = copies.pending(inbox)
        _set_count(len(offers))
        offers_dialog(api, offers, copies.display_names(inbox), copies.limit_of(inbox))

    _load(api, loaded)


def offers_dialog(api, offers, names, limit):
    mw = state["mw"]
    dialog = QDialog(mw)
    dialog.setWindowTitle(tr("Decks offered to me"))
    dialog.resize(520, 420)
    layout = QVBoxLayout(dialog)
    _brand(dialog, layout, state["companion"]())
    status = QLabel()
    status.setTextFormat(_enum(Qt, "TextFormat", "PlainText"))
    status.setWordWrap(True)
    rows = [_offer_row(api, offer, names, limit, dialog, status) for offer in offers]
    layout.addWidget(_scroller(rows or [QLabel(tr("No deck copies are waiting for you."))]), 1)
    layout.addWidget(status)
    close = QPushButton(tr("Close"))
    close.clicked.connect(dialog.accept)
    row = QHBoxLayout()
    row.addStretch(1)
    row.addWidget(close)
    layout.addLayout(row)
    dialog.exec()


def _offer_row(api, offer, names, limit, dialog, status):
    holder = QWidget()
    line = QHBoxLayout(holder)
    line.setContentsMargins(0, 0, 0, 0)
    text = QLabel(copies.offer_html(offer, names, tr))
    text.setTextFormat(_enum(Qt, "TextFormat", "RichText"))
    text.setWordWrap(True)
    take = QPushButton(tr("Import"))
    drop = QPushButton(tr("Dismiss"))
    line.addWidget(text, 1)
    line.addWidget(take)
    line.addWidget(drop)
    buttons = (take, drop)
    take.clicked.connect(lambda: _import(api, offer, limit, dialog, status, buttons))
    drop.clicked.connect(lambda: _dismiss(api, offer, holder, status, buttons))
    return holder


def _busy(buttons, busy):
    for button in buttons:
        button.setEnabled(not busy)


def _import(api, offer, limit, dialog, status, buttons):
    mw = state["mw"]
    _busy(buttons, True)
    status.setText(tr("Downloading the deck copy…"))
    target = copies.temporary_package("ankiquest-friend-")

    def done(future):
        try:
            path = future.result()
        except Exception as error:
            _busy(buttons, False)
            status.setText(_failed(error, rich=False))
            return
        dialog.accept()
        import_package(mw, path)

    mw.taskman.run_in_background(lambda: copies.download(api, offer["id"], target, limit), done)


def import_package(mw, path):
    """Anki's own importer, so the usual import dialog and its choices appear."""
    try:
        from aqt.import_export.importing import import_file
    except ImportError:
        from aqt.importing import importFile

        importFile(mw, path)
        return
    import_file(mw, path)


def _dismiss(api, offer, holder, status, buttons):
    _busy(buttons, True)

    def done(future):
        try:
            future.result()
        except Exception as error:
            _busy(buttons, False)
            status.setText(_failed(error, rich=False))
            return
        holder.hide()
        _set_count(max(0, state["count"] - 1))
        status.setText(tr("Offer removed."))

    state["mw"].taskman.run_in_background(lambda: copies.dismiss(api, offer["id"]), done)
