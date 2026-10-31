"""The daily goal dialog: pick a goal and see what it means for your own collection."""

from aqt.qt import (
    QComboBox,
    QDialog,
    QHBoxLayout,
    QLabel,
    QPushButton,
    QSpinBox,
    QVBoxLayout,
    QWidget,
    Qt,
)

from . import goals
from .language import tr

CUSTOM = 0


def _enum(owner, group, name):
    """Qt 5 keeps these constants on the class, Qt 6 inside a nested enum."""
    return getattr(getattr(owner, group, owner), name)


def _modes():
    return (
        (goals.BACKLOG, tr("Clear my backlog in a number of days")),
        (goals.CARDS, tr("Cards per day")),
        (goals.MINUTES, tr("Minutes per day")),
        (goals.OFF, tr("No daily goal")),
    )


def _spin(mode, value, suffix):
    low, high = goals.RANGES[mode]
    box = QSpinBox()
    box.setRange(low, high)
    box.setValue(goals.clamp(mode, value))
    box.setSuffix(suffix)
    return box


def _line(title, *widgets):
    holder = QWidget()
    row = QHBoxLayout(holder)
    row.setContentsMargins(0, 0, 0, 0)
    row.addWidget(QLabel(title))
    for widget in widgets:
        row.addWidget(widget)
    row.addStretch(1)
    return holder


def _combo(choices, current):
    box = QComboBox()
    for value, text in choices:
        box.addItem(text, value)
    index = box.findData(current)
    box.setCurrentIndex(max(0, index))
    return box


def _start(chosen):
    """The value each field starts from: the current goal's for its own mode, else the default."""
    return {
        mode: chosen.value if chosen.mode == mode and chosen.value else goals.DEFAULTS[mode]
        for mode in goals.RANGES
    }


class Fields:
    """The dialog's inputs and what they amount to."""

    def __init__(self, chosen):
        start = _start(chosen)
        days = start[goals.BACKLOG]
        self.mode = _combo(_modes(), chosen.mode)
        presets = [(n, tr("%d days") % n) for n in goals.PRESET_DAYS] + [(CUSTOM, tr("Custom"))]
        self.preset = _combo(presets, days if days in goals.PRESET_DAYS else CUSTOM)
        self.days = _spin(goals.BACKLOG, days, " " + tr("days"))
        self.cards = _spin(goals.CARDS, start[goals.CARDS], " " + tr("cards"))
        self.minutes = _spin(goals.MINUTES, start[goals.MINUTES], " min")
        self.rows = {
            goals.BACKLOG: _line(tr("Clear my backlog in"), self.preset, self.days),
            goals.CARDS: _line(tr("Cards per day"), self.cards),
            goals.MINUTES: _line(tr("Minutes per day"), self.minutes),
        }

    def chosen(self):
        mode = self.mode.currentData() or goals.BACKLOG
        if mode == goals.BACKLOG:
            return mode, self.preset.currentData() or self.days.value()
        if mode == goals.CARDS:
            return mode, self.cards.value()
        if mode == goals.MINUTES:
            return mode, self.minutes.value()
        return goals.OFF, None

    def changed(self, react):
        self.mode.currentIndexChanged.connect(lambda _index: react())
        self.preset.currentIndexChanged.connect(lambda _index: react())
        for box in (self.days, self.cards, self.minutes):
            box.valueChanged.connect(lambda _value: react())

    def show(self):
        mode = self.mode.currentData()
        for key, row in self.rows.items():
            row.setVisible(mode == key)
        self.days.setVisible(self.preset.currentData() == CUSTOM)


def _summary_label():
    label = QLabel("")
    label.setWordWrap(True)
    label.setTextFormat(_enum(Qt, "TextFormat", "PlainText"))
    label.setStyleSheet("padding:8px;border-radius:6px;background:rgba(28,176,246,0.12)")
    return label


def goal_dialog(parent, chosen, forecast):
    """Asks for a daily goal, starting from `chosen` (a goals.Plan). `forecast` is a goals.Forecast,
    or None when the collection could not be read. Answers (mode, value), or None when cancelled."""
    dialog = QDialog(parent)
    dialog.setWindowTitle(tr("Daily goal"))
    layout = QVBoxLayout(dialog)
    intro = QLabel(tr("Pick a goal for each day. The estimates come from your own cards and answer times."))
    intro.setWordWrap(True)
    layout.addWidget(intro)
    fields = Fields(chosen)
    layout.addWidget(fields.mode)
    for row in fields.rows.values():
        layout.addWidget(row)
    summary = _summary_label()
    layout.addWidget(summary)

    def react():
        fields.show()
        lines = goals.summary(*fields.chosen(), forecast, tr)
        summary.setText("\n".join(lines))
        summary.setVisible(bool(lines))

    fields.changed(react)
    react()
    layout.addLayout(_buttons(dialog))
    dialog.setMinimumWidth(420)
    if not dialog.exec():
        return None
    return fields.chosen()


def _buttons(dialog):
    row = QHBoxLayout()
    row.addStretch(1)
    later = QPushButton(tr("Not now"))
    later.clicked.connect(dialog.reject)
    save = QPushButton(tr("Save"))
    save.setDefault(True)
    save.clicked.connect(dialog.accept)
    row.addWidget(later)
    row.addWidget(save)
    return row
