"""The Tools menu entries for deck copies: send a deck to friends, and the decks offered to me."""

import html
from enum import Enum, auto

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
from .ui import _brand, _scroller

SEEN_KEY = "ankiquestSeenDeckCopies"

state = {"mw": None, "client": None, "companion": None, "action": None, "count": 0, "delivery": None,
         "generation": object(), "requests": {}, "dialogs": set()}


class _Job(Enum):
    POLL = auto()
    OFFERS = auto()
    SEND_FORM = auto()
    SEND = auto()


class _Phase(Enum):
    OPEN = auto()
    CLOSED = auto()


class _Owner:
    """Profile + collection + account generation -> request -> current UI only."""

    def __init__(self, api, dialog=None):
        self._mw, self._who = state["mw"], _account(api)
        self._profile, self._collection = self._mw.pm.profile, self._mw.col
        self._generation, self._phase = state["generation"], _Phase.OPEN
        self._pending = set()
        if dialog is not None:
            state["dialogs"].add(dialog)
            dialog.finished.connect(lambda _result: self._closed(dialog))
            dialog.destroyed.connect(lambda _object: self._closed(dialog))

    def _closed(self, dialog):
        self._phase = _Phase.CLOSED
        state["dialogs"].discard(dialog)

    def _owns(self):
        return (self._phase is _Phase.OPEN and self._generation is state["generation"]
                and self._profile is not None and self._profile is self._mw.pm.profile
                and self._collection is not None and self._collection is self._mw.col
                and self._who == _account(state["client"]()))

    def _begin(self, offer):
        if not self._owns() or offer in self._pending:
            return False
        self._pending.add(offer)
        return True

    def _end(self, offer):
        self._pending.discard(offer)
        return self._owns()


def _start(api, job):
    previous = state["requests"].get(job)
    if previous is not None and previous._owns():
        return None
    owner = _Owner(api)
    if not owner._owns():
        return None
    state["requests"][job] = owner
    return owner


def _finish(owner, job):
    if state["requests"].get(job) is not owner:
        return False
    state["requests"].pop(job)
    return owner._owns()


def _dialog_owner(api, dialog):
    if not hasattr(dialog, "_copies_owner"):
        dialog._copies_owner = _Owner(api, dialog)
    return dialog._copies_owner


def clear_account():
    state.update(generation=object(), requests={})
    for dialog in tuple(state["dialogs"]):
        dialog.close()
    _set_count(0)


def setup(mw, client, companion, menu):
    """`client()` gives the current account; `companion()` the mascot to show."""
    state.update(mw=mw, client=client, companion=companion)
    send = QAction(tr("Send a deck"), mw)
    send.triggered.connect(open_send)
    offered = QAction(copies.menu_title(0, tr), mw)
    offered.triggered.connect(open_offers)
    menu.addAction(send)
    menu.addAction(offered)
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
    if mw is None or api is None or not api.configured or mw.pm.profile is None:
        _set_count(0)
        return
    request = _start(api, _Job.POLL)
    if request is None:
        return

    def done(future):
        if not _finish(request, _Job.POLL):
            return
        try:
            inbox = future.result()
        except Exception as error:
            print("ankiquest deck copies:", error)
            return
        offers = copies.pending(inbox)
        _set_count(len(offers))
        announce(offers, copies.display_names(inbox), quiet)

    mw.taskman.run_in_background(lambda: copies.inbox(api) if request._owns() else None, done)


def announce(offers, names, quiet):
    if state["delivery"] is not None:
        from .delivery import _Mode

        state["delivery"]._receive_copies(offers, names, _Mode.QUIET if quiet else _Mode.ANNOUNCE)
        return
    profile = state["mw"].pm.profile
    fresh, seen = copies.new_offers(offers, profile.get(SEEN_KEY))
    profile[SEEN_KEY] = seen
    if quiet:
        return
    for offer in fresh:
        tooltip(
            "%s<br><i>%s</i>" % (copies.offer_html(offer, names, tr), html.escape(tr("Tools → AnkiQuest → Decks offered to me"))),
            period=6000,
        )


def _configured():
    api = state["client"]()
    if not api.configured:
        tooltip(tr("Set the server, player and token in AnkiQuest settings first."))
        return None
    return api


def _load(api, then, job=_Job.OFFERS):
    owner = _start(api, job)
    if owner is None:
        return

    def done(future):
        if not _finish(owner, job):
            return
        try:
            inbox = future.result()
        except Exception as error:
            tooltip(_failed(error))
            return
        then(inbox)

    state["mw"].taskman.run_in_background(lambda: copies.inbox(api) if owner._owns() else None, done)


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
        owner = _Owner(api)
        choice = send_dialog(mw, decks, people, api.name)
        if choice is not None and owner._owns():
            send(api, choice, copies.limit_of(inbox))

    _load(api, loaded, _Job.SEND_FORM)


def send(api, choice, limit):
    deck_id, deck_name, recipients = choice
    mw = state["mw"]
    owner = _start(api, _Job.SEND)
    if owner is None:
        return
    tooltip(tr("Preparing and sending the deck copy…"))

    def done(future):
        if not _finish(owner, _Job.SEND):
            return
        try:
            future.result()
        except Exception as error:
            tooltip(_failed(error), period=5000)
            return
        tooltip(tr("Deck copy sent."))

    def work():
        if owner._owns():
            return copies.share(api, owner._collection, deck_id, deck_name, recipients, limit)
        return None

    mw.taskman.run_in_background(work, done, uses_collection=True)


def send_dialog(parent, decks, people, sender):
    """A deck and the friends to send it to; stays open until the choice is acceptable."""
    dialog = QDialog(parent)
    _dialog_owner(state["client"](), dialog)
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
    status.setTextFormat(Qt.TextFormat.PlainText)
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
    if state["delivery"] is not None:
        state["delivery"]._ack_copies()
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
    _dialog_owner(api, dialog)
    dialog.setWindowTitle(tr("Decks offered to me"))
    dialog.resize(520, 420)
    layout = QVBoxLayout(dialog)
    _brand(dialog, layout, state["companion"]())
    status = QLabel()
    status.setTextFormat(Qt.TextFormat.PlainText)
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
    owner = _dialog_owner(api, dialog)
    holder = QWidget()
    line = QHBoxLayout(holder)
    line.setContentsMargins(0, 0, 0, 0)
    text = QLabel(copies.offer_html(offer, names, tr))
    text.setTextFormat(Qt.TextFormat.RichText)
    text.setWordWrap(True)
    take = QPushButton(tr("Import"))
    drop = QPushButton(tr("Dismiss"))
    line.addWidget(text, 1)
    line.addWidget(take)
    line.addWidget(drop)
    buttons = (take, drop)
    take.clicked.connect(lambda: _import(api, offer, limit, dialog, status, buttons))
    drop.clicked.connect(lambda: _dismiss(api, offer, holder, status, buttons, owner))
    return holder


def _busy(buttons, busy):
    for button in buttons:
        button.setEnabled(not busy)


def _import(api, offer, limit, dialog, status, buttons):
    mw = state["mw"]
    owner = _dialog_owner(api, dialog)
    if not owner._begin(offer["id"]):
        return
    _busy(buttons, True)
    status.setText(tr("Downloading the deck copy…"))
    target = copies.temporary_package("ankiquest-friend-")

    def done(future):
        try:
            path = future.result()
        except Exception as error:
            copies._remove(target)
            if not owner._end(offer["id"]):
                return
            _busy(buttons, False)
            status.setText(_failed(error, rich=False))
            return
        if not owner._end(offer["id"]) or path is None:
            copies._remove(target)
            return
        dialog.accept()
        import_package(mw, path)

    mw.taskman.run_in_background(lambda: copies.download(api, offer["id"], target, limit) if owner._owns() else None, done)


def import_package(mw, path):
    """Anki's own importer, so the usual import dialog and its choices appear."""
    from aqt.import_export.importing import import_file

    import_file(mw, path)


def _dismiss(api, offer, holder, status, buttons, owner):
    if not owner._begin(offer["id"]):
        return
    _busy(buttons, True)

    def done(future):
        if not owner._end(offer["id"]):
            return
        try:
            future.result()
        except Exception as error:
            _busy(buttons, False)
            status.setText(_failed(error, rich=False))
            return
        holder.hide()
        _set_count(max(0, state["count"] - 1))
        status.setText(tr("Offer removed."))
        # A poll started before this write must not restore the dismissed offer.
        state["requests"].pop(_Job.POLL, None)
        poll(quiet=True)

    state["mw"].taskman.run_in_background(lambda: copies.dismiss(api, offer["id"]) if owner._owns() else None, done)
