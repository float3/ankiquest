"""Qt pieces of the Today features: the session recap and the daily goal settings."""

from aqt.qt import (
    QCheckBox,
    QComboBox,
    QDialog,
    QHBoxLayout,
    QLabel,
    QPixmap,
    QPushButton,
    QSpinBox,
    QVBoxLayout,
    QWidget,
    Qt,
)

from . import goals
from .language import tr

GOAL_MODES = (
    (goals.BACKLOG, "Due today plus part of the backlog"),
    (goals.CARDS, "A number of cards"),
    (goals.MINUTES, "A number of minutes"),
    (goals.OFF, "No daily goal"),
)
BACKLOG_CHOICES = (
    (5, "5 % (clears in about 20 days)"),
    (10, "10 % (about 10 days)"),
    (20, "20 % (about 5 days)"),
    (34, "34 % (about 3 days)"),
    (100, "All of it"),
)


def _enum(owner, group, name):
    """Qt 5 keeps these constants on the class, Qt 6 inside a nested enum."""
    return getattr(getattr(owner, group, owner), name)


def _mascot(path):
    image = QLabel()
    image.setPixmap(QPixmap(path).scaled(
        112, 112,
        _enum(Qt, "AspectRatioMode", "KeepAspectRatio"),
        _enum(Qt, "TransformationMode", "SmoothTransformation"),
    ))
    image.setAlignment(_enum(Qt, "AlignmentFlag", "AlignHCenter"))
    return image


def _button(text, dialog, action=None):
    button = QPushButton(text)

    def clicked():
        dialog.accept()
        if action is not None:
            action()

    button.clicked.connect(clicked)
    return button


def session_dialog(parent, title, text, art, on_continue, on_progress, never_again=None, done_text=None):
    """A small, non-blocking recap with the companion, Continue and progress buttons."""
    dialog = QDialog(parent)
    dialog.setWindowTitle(title)
    dialog.setAttribute(_enum(Qt, "WidgetAttribute", "WA_DeleteOnClose"))
    layout = QVBoxLayout(dialog)
    if art:
        layout.addWidget(_mascot(art))
    heading = QLabel(title)
    heading.setStyleSheet("font-weight:bold;font-size:15px")
    heading.setAlignment(_enum(Qt, "AlignmentFlag", "AlignHCenter"))
    layout.addWidget(heading)
    message = QLabel(text)
    message.setWordWrap(True)
    message.setTextFormat(_enum(Qt, "TextFormat", "PlainText"))
    layout.addWidget(message)
    quiet = None
    if never_again is not None:
        quiet = QCheckBox(tr("Don’t show this again"))
        layout.addWidget(quiet)
    row = QHBoxLayout()
    row.addWidget(_button(tr("View progress"), dialog, on_progress))
    row.addStretch(1)
    finished = _button(done_text or tr("Done"), dialog)
    row.addWidget(finished)
    if on_continue is not None:
        go = _button(tr("Continue studying"), dialog, on_continue)
        go.setDefault(True)
        row.addWidget(go)
    else:
        finished.setDefault(True)
    layout.addLayout(row)
    if quiet is not None:
        dialog.finished.connect(lambda _result: never_again() if quiet.isChecked() else None)
    dialog.setMinimumWidth(360)
    dialog.show()
    return dialog


def _combo(choices, current):
    box = QComboBox()
    for value, text in choices:
        box.addItem(tr(text), value)
    values = [value for value, _ in choices]
    box.setCurrentIndex(values.index(current) if current in values else 0)
    return box


def _spin(low, high, value, suffix):
    box = QSpinBox()
    box.setRange(low, high)
    box.setValue(max(low, min(high, value)))
    box.setSuffix(suffix)
    return box


def _line(title, widget):
    holder = QWidget()
    row = QHBoxLayout(holder)
    row.setContentsMargins(0, 0, 0, 0)
    row.addWidget(QLabel(title))
    row.addWidget(widget)
    return holder


def settings_section(layout, config):
    """Appends the Today settings to a settings dialog; returns a function that reads them back."""
    chosen = goals.settings(config)
    heading = QLabel(tr("Today"))
    heading.setStyleSheet("font-weight:bold;margin-top:8px")
    layout.addWidget(heading)
    mode = _combo(GOAL_MODES, chosen.mode)
    backlog = _combo(BACKLOG_CHOICES, chosen.backlog_percent)
    cards = _spin(1, 9999, chosen.cards, " " + tr("cards"))
    minutes = _spin(1, 600, chosen.minutes, " min")
    rows = {
        goals.BACKLOG: _line(tr("Backlog to work off each day"), backlog),
        goals.CARDS: _line(tr("Cards per day"), cards),
        goals.MINUTES: _line(tr("Minutes per day"), minutes),
    }
    layout.addWidget(_line(tr("Daily goal"), mode))
    for row in rows.values():
        layout.addWidget(row)

    def shown():
        for key, row in rows.items():
            row.setVisible(mode.currentData() == key)

    mode.currentIndexChanged.connect(lambda _index: shown())
    shown()
    toggles = {}
    for key, text in (
        ("deck_status", "Mark decks finished today, and learning due later today"),
        ("session_summary", "Show a recap when you leave a study session"),
        ("goal_celebrations", "Celebrate when today counts for your streak and when you reach your daily goal"),
    ):
        toggles[key] = QCheckBox(tr(text))
        toggles[key].setChecked(bool(config.get(key, True)))
        layout.addWidget(toggles[key])

    def values():
        found = {
            "goal_mode": mode.currentData() or goals.DEFAULT_MODE,
            "goal_backlog_percent": backlog.currentData() or goals.DEFAULT_BACKLOG_PERCENT,
            "goal_cards": cards.value(),
            "goal_minutes": minutes.value(),
        }
        found.update((key, box.isChecked()) for key, box in toggles.items())
        return found

    return values
