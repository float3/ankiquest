"""The dialogs behind the Tools menu: settings and deck sharing."""

from pathlib import Path

from aqt.qt import (
    QCheckBox,
    QComboBox,
    QDialog,
    QHBoxLayout,
    QIcon,
    QLabel,
    QLineEdit,
    QPushButton,
    QScrollArea,
    QSpinBox,
    QTreeWidget,
    QTreeWidgetItem,
    QVBoxLayout,
    QPixmap,
    QWidget,
)

from aqt.qt import Qt

from .decks import label, ordered
from .language import tr


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
    cancel = QPushButton(tr("Cancel"))
    cancel.clicked.connect(dialog.reject)
    accept = QPushButton(accept_text)
    accept.setDefault(True)
    accept.clicked.connect(dialog.accept)
    row.addWidget(cancel)
    row.addWidget(accept)
    return row


def _brand(dialog, layout, companion="aki"):
    """Local art works even before a server is configured or while offline."""
    if companion == "none":
        return
    asset = str(Path(__file__).with_name("ankilope_face.png" if companion == "ankilope" else "aki_face.png"))
    dialog.setWindowIcon(QIcon(asset))
    image = QLabel()
    image.setPixmap(QPixmap(asset).scaled(
        48, 48,
        _enum(Qt, "AspectRatioMode", "KeepAspectRatio"),
        _enum(Qt, "TransformationMode", "SmoothTransformation"),
    ))
    layout.addWidget(_row(image, QLabel(tr("Your study companion."))))


def settings_dialog(parent, config, on_test, on_upload_all, companion="aki"):
    """Everything the phone keeps in its ankiquest preference screen."""
    dialog = QDialog(parent)
    dialog.setWindowTitle("ankiquest")
    layout = QVBoxLayout(dialog)
    _brand(dialog, layout, companion)

    url = QLineEdit(config.get("url", ""))
    url.setPlaceholderText("https://anki.example.com")
    user = QLineEdit(config.get("user", ""))
    token = QLineEdit(config.get("token", ""))
    token.setEchoMode(_enum(QLineEdit, "EchoMode", "Password"))
    for title, field in ((tr("Server"), url), (tr("Player"), user), (tr("Token"), token)):
        layout.addWidget(QLabel(title))
        layout.addWidget(field)

    rank = QCheckBox(tr("Tell me when my place on the leaderboard changes"))
    rank.setChecked(bool(config.get("notify_rank", True)))
    layout.addWidget(rank)

    hours = QSpinBox()
    hours.setRange(0, 12)
    hours.setValue(int(config.get("streak_hours", 2) or 0))
    hours.setSuffix(" h")
    layout.addWidget(_row(QLabel(tr("Warn me before my streak ends")), hours))

    channel = QComboBox()
    for value, title in (("stable", tr("Stable releases")), ("nightly", tr("Nightly builds (untested)"))):
        channel.addItem(title, value)
    channel.setCurrentIndex(1 if config.get("update_channel") == "nightly" else 0)
    layout.addWidget(_row(QLabel(tr("Update channel")), channel))

    test = QPushButton(tr("Test connection"))
    test.clicked.connect(lambda: on_test(_values(url, user, token, rank, hours, channel)))
    upload = QPushButton(tr("Upload everything again"))
    upload.clicked.connect(on_upload_all)
    layout.addLayout(_buttons(dialog, tr("Save"), (test, upload)))

    if not dialog.exec():
        return None
    return _values(url, user, token, rank, hours, channel)


def _values(url, user, token, rank, hours, channel):
    return {
        "update_channel": channel.currentData() or "stable",
        "url": url.text().strip(),
        "user": user.text().strip(),
        "token": token.text().strip(),
        "notify_rank": rank.isChecked(),
        "streak_hours": hours.value(),
    }


def deck_dialog(parent, settings, companion="aki"):
    """A tree of decks and a list of recipients; ticking a deck ticks its subdecks."""
    decks = ordered(settings.get("decks") or [])
    people = settings.get("recipients") or []
    dialog = QDialog(parent)
    dialog.setWindowTitle(tr("Deck completion notifications"))
    dialog.resize(680, 520)
    layout = QVBoxLayout(dialog)
    _brand(dialog, layout, companion)
    layout.addWidget(
        QLabel(tr("The people you pick hear once a day when you finish a shared deck."))
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
    columns.addWidget(_titled(tr("Decks"), tree), 2)
    columns.addWidget(
        _titled(tr("Notify"), _scroller(list(recipients.values()) or [QLabel(tr("Nobody else plays yet."))])),
        1,
    )
    layout.addLayout(columns)

    nudges = QCheckBox(tr("Nudge me when a place, my best day or the next level is within reach"))
    nudges.setChecked(bool(settings.get("nudges")))
    layout.addWidget(nudges)
    celebrations = QCheckBox(tr("Celebrate achievements, streak milestones and personal bests"))
    celebrations.setChecked(settings.get("celebrations") is not False)
    layout.addWidget(celebrations)

    every = QPushButton(tr("All / none"))

    def toggle_all():
        checked = _enum(Qt, "CheckState", "Checked")
        select = any(item.checkState(0) != checked for item in items)
        for item in items:
            item.setCheckState(0, _enum(Qt, "CheckState", "Checked" if select else "Unchecked"))

    every.clicked.connect(toggle_all)
    layout.addLayout(_buttons(dialog, tr("Save"), (every,)))

    if not dialog.exec():
        return None
    checked = _enum(Qt, "CheckState", "Checked")
    shared = [deck["id"] for deck, item in zip(decks, items) if item.checkState(0) == checked]
    unshared = [deck["id"] for deck, item in zip(decks, items) if item.checkState(0) != checked]
    picked = [user for user, box in recipients.items() if box.isChecked()]
    return shared, unshared, picked, nudges.isChecked(), celebrations.isChecked()


def _titled(title, widget):
    holder = QWidget()
    layout = QVBoxLayout(holder)
    layout.setContentsMargins(0, 0, 0, 0)
    label = QLabel("<b>%s</b>" % title)
    layout.addWidget(label)
    layout.addWidget(widget)
    return holder


def companion_dialog(parent, selected):
    """A native picker; the caller saves the answer to the current account."""
    dialog = QDialog(parent)
    dialog.setWindowTitle(tr("Study companion"))
    layout = QVBoxLayout(dialog)
    layout.addWidget(QLabel(tr("Choose who cheers you on. This choice follows your account across devices.")))
    choice = QComboBox()
    options = (("Aki", "aki"), ("Ankilope", "ankilope"), (tr("No companion"), "none"))
    for name, value in options:
        choice.addItem(name, value)
    choice.setCurrentIndex(next((i for i, (_, value) in enumerate(options) if value == selected), 0))
    layout.addWidget(choice)
    preview = QLabel()
    preview.setAlignment(_enum(Qt, "AlignmentFlag", "AlignCenter"))
    layout.addWidget(preview)

    def update_preview():
        value = choice.currentData()
        if value == "none":
            preview.clear()
        else:
            asset = str(Path(__file__).with_name("ankilope_face.png" if value == "ankilope" else "aki_face.png"))
            preview.setPixmap(QPixmap(asset).scaled(
                96, 96, _enum(Qt, "AspectRatioMode", "KeepAspectRatio"),
                _enum(Qt, "TransformationMode", "SmoothTransformation"),
            ))

    choice.currentIndexChanged.connect(update_preview)
    update_preview()
    layout.addLayout(_buttons(dialog, tr("Save")))
    return choice.currentData() if dialog.exec() else None
