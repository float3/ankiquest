"""The dialogs behind the Tools menu: settings, deck sharing and the inbox."""

from aqt.qt import (
    QCheckBox,
    QDialog,
    QFrame,
    QHBoxLayout,
    QLabel,
    QLineEdit,
    QPushButton,
    QScrollArea,
    QSpinBox,
    QTreeWidget,
    QTreeWidgetItem,
    QVBoxLayout,
    QWidget,
)

from aqt.qt import Qt

from .decks import label, ordered
from .notify import answerable, when

MAX_MESSAGE = 200


def _enum(owner, group, name):
    """Qt 5 keeps these constants on the class, Qt 6 inside a nested enum."""
    return getattr(getattr(owner, group, owner), name)


def _scroller(widgets):
    inner = QWidget()
    layout = QVBoxLayout(inner)
    for widget in widgets:
        layout.addWidget(widget)
    layout.addStretch(1)
    area = QScrollArea()
    area.setWidgetResizable(True)
    area.setWidget(inner)
    return area


def _row(*widgets):
    holder = QWidget()
    layout = QHBoxLayout(holder)
    layout.setContentsMargins(0, 0, 0, 0)
    for widget in widgets:
        layout.addWidget(widget)
    return holder


def _buttons(dialog, accept_text, extra=()):
    row = QHBoxLayout()
    for widget in extra:
        row.addWidget(widget)
    row.addStretch(1)
    cancel = QPushButton("Cancel")
    cancel.clicked.connect(dialog.reject)
    accept = QPushButton(accept_text)
    accept.setDefault(True)
    accept.clicked.connect(dialog.accept)
    row.addWidget(cancel)
    row.addWidget(accept)
    return row


def settings_dialog(parent, config, on_test, on_upload_all):
    """Everything the phone keeps in its ankiquest preference screen."""
    dialog = QDialog(parent)
    dialog.setWindowTitle("ankiquest")
    layout = QVBoxLayout(dialog)

    url = QLineEdit(config.get("url", ""))
    url.setPlaceholderText("https://anki.example.com")
    user = QLineEdit(config.get("user", ""))
    token = QLineEdit(config.get("token", ""))
    token.setEchoMode(_enum(QLineEdit, "EchoMode", "Password"))
    for title, field in (("Server", url), ("Player", user), ("Token", token)):
        layout.addWidget(QLabel(title))
        layout.addWidget(field)

    rank = QCheckBox("Tell me when my place on the leaderboard changes")
    rank.setChecked(bool(config.get("notify_rank", True)))
    layout.addWidget(rank)

    hours = QSpinBox()
    hours.setRange(0, 12)
    hours.setValue(int(config.get("streak_hours", 2) or 0))
    hours.setSuffix(" h")
    layout.addWidget(_row(QLabel("Warn me before my streak ends"), hours))

    test = QPushButton("Test connection")
    test.clicked.connect(lambda: on_test(_values(url, user, token, rank, hours)))
    upload = QPushButton("Upload everything again")
    upload.clicked.connect(on_upload_all)
    layout.addLayout(_buttons(dialog, "Save", (test, upload)))

    if not dialog.exec():
        return None
    return _values(url, user, token, rank, hours)


def _values(url, user, token, rank, hours):
    return {
        "url": url.text().strip(),
        "user": user.text().strip(),
        "token": token.text().strip(),
        "notify_rank": rank.isChecked(),
        "streak_hours": hours.value(),
    }


def deck_dialog(parent, settings):
    """A tree of decks and a list of recipients; ticking a deck ticks its subdecks."""
    decks = ordered(settings.get("decks") or [])
    people = settings.get("recipients") or []
    dialog = QDialog(parent)
    dialog.setWindowTitle("Deck completion notifications")
    dialog.resize(680, 520)
    layout = QVBoxLayout(dialog)
    layout.addWidget(
        QLabel("The people you pick hear once a day when you finish a shared deck.")
    )

    tree = QTreeWidget()
    tree.setHeaderHidden(True)
    items = []
    for index, deck in enumerate(decks):
        item = QTreeWidgetItem([label(deck)])
        item.setCheckState(0, _enum(Qt, "CheckState", "Checked" if deck.get("enabled") else "Unchecked"))
        item.setToolTip(0, deck["name"])
        items.append(item)
        parts = deck["name"].split("::")
        owner = None
        for other in range(index - 1, -1, -1):
            if decks[other]["name"] == "::".join(parts[:-1]):
                owner = items[other]
                break
        if owner is None:
            tree.addTopLevelItem(item)
        else:
            owner.addChild(item)
    tree.expandAll()

    guard = {"busy": False}

    def spread(item, _column):
        """A branch shares one answer, without the change bouncing back up."""
        if guard["busy"]:
            return
        guard["busy"] = True
        state = item.checkState(0)
        stack = [item.child(i) for i in range(item.childCount())]
        while stack:
            child = stack.pop()
            child.setCheckState(0, state)
            stack.extend(child.child(i) for i in range(child.childCount()))
        guard["busy"] = False

    tree.itemChanged.connect(spread)

    chosen = set()
    for deck in decks:
        if deck.get("enabled"):
            chosen.update(deck.get("recipients") or [])
    recipients = {}
    for person in people:
        box = QCheckBox("%s (%s)" % (person.get("display") or person["user"], person["user"]))
        box.setChecked(person["user"] in chosen)
        recipients[person["user"]] = box

    columns = QHBoxLayout()
    columns.addWidget(_titled("Decks", tree), 2)
    columns.addWidget(
        _titled("Notify", _scroller(list(recipients.values()) or [QLabel("Nobody else plays yet.")])),
        1,
    )
    layout.addLayout(columns)

    nudges = QCheckBox("Nudge me when a place, my best day or the next level is within reach")
    nudges.setChecked(bool(settings.get("nudges")))
    layout.addWidget(nudges)

    every = QPushButton("All / none")

    def toggle_all():
        checked = _enum(Qt, "CheckState", "Checked")
        select = any(item.checkState(0) != checked for item in items)
        for item in items:
            item.setCheckState(0, _enum(Qt, "CheckState", "Checked" if select else "Unchecked"))

    every.clicked.connect(toggle_all)
    layout.addLayout(_buttons(dialog, "Save", (every,)))

    if not dialog.exec():
        return None
    checked = _enum(Qt, "CheckState", "Checked")
    shared = [deck["id"] for deck, item in zip(decks, items) if item.checkState(0) == checked]
    unshared = [deck["id"] for deck, item in zip(decks, items) if item.checkState(0) != checked]
    picked = [user for user, box in recipients.items() if box.isChecked()]
    return shared, unshared, picked, nudges.isChecked()


def _titled(title, widget):
    holder = QWidget()
    layout = QVBoxLayout(holder)
    layout.setContentsMargins(0, 0, 0, 0)
    label = QLabel("<b>%s</b>" % title)
    layout.addWidget(label)
    layout.addWidget(widget)
    return holder


def inbox_dialog(parent, entries, send):
    """Reads what other people have done, and answers it without leaving Anki."""
    dialog = QDialog(parent)
    dialog.setWindowTitle("ankiquest inbox")
    dialog.resize(560, 420)
    layout = QVBoxLayout(dialog)
    cards = []
    for entry in reversed(entries):
        cards.append(_message(entry, send))
    layout.addWidget(_scroller(cards or [QLabel("Nothing yet.")]))
    close = QPushButton("Close")
    close.clicked.connect(dialog.accept)
    row = QHBoxLayout()
    row.addStretch(1)
    row.addWidget(close)
    layout.addLayout(row)
    dialog.exec()


def _message(entry, send):
    card = QFrame()
    card.setFrameShape(_enum(QFrame, "Shape", "StyledPanel"))
    layout = QVBoxLayout(card)
    title = QLabel(entry.get("title", ""))
    title.setTextFormat(_enum(Qt, "TextFormat", "PlainText"))
    font = title.font()
    font.setBold(True)
    title.setFont(font)
    stamp = QLabel(when(entry.get("created_at")))
    stamp.setEnabled(False)
    stamp.setAlignment(_enum(Qt, "AlignmentFlag", "AlignRight"))
    layout.addWidget(_row(title, stamp))
    body = QLabel(entry.get("body", ""))
    body.setTextFormat(_enum(Qt, "TextFormat", "PlainText"))
    body.setWordWrap(True)
    layout.addWidget(body)
    if not answerable(entry):
        return card

    status = QLabel("")
    field = QLineEdit()
    field.setPlaceholderText("Say something nice")
    field.setMaxLength(MAX_MESSAGE)
    cheer = QPushButton("Good job!")
    reply = QPushButton("Send")

    def answer(message):
        message = message.strip()
        if not message:
            return
        for widget in (cheer, reply, field):
            widget.setEnabled(False)
        status.setText("Sending…")
        send(entry, message, status)

    cheer.clicked.connect(lambda: answer(cheer.text()))
    reply.clicked.connect(lambda: answer(field.text()))
    field.returnPressed.connect(lambda: answer(field.text()))
    layout.addWidget(_row(field, cheer, reply))
    layout.addWidget(status)
    return card
