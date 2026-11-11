"""One reminders switch combines the local streak warning with server choices."""
from enum import Enum, auto

from aqt.qt import QCheckBox, QComboBox, QHBoxLayout, QLabel, QSpinBox, QWidget

from .language import _hour, tr
from .preferences import _REMINDERS


class _Account(Enum):
    SAVED = auto()
    STAGED = auto()


def _row(layout, title, *controls):
    holder = QWidget()
    row = QHBoxLayout(holder)
    row.setContentsMargins(0, 0, 0, 0)
    row.addWidget(QLabel(title))
    for control in controls:
        row.addWidget(control)
    layout.addWidget(holder)


def _hours(name):
    box = QComboBox()
    box.setObjectName(name)
    for hour in range(24):
        box.addItem(_hour(hour), hour)
    return box


class _Controls:
    def __init__(self, layout, config):
        self._loaded_settings = None
        self._account = _Account.STAGED
        self._touched = set()
        self._enabled = QCheckBox(tr("Reminders"))
        self._enabled.setObjectName("ankiquest_reminders")
        self._enabled.setChecked(bool(config.get("streak_hours", 2)))
        layout.addWidget(self._enabled)
        self._hours = QSpinBox()
        self._hours.setObjectName("ankiquest_streak_hours")
        self._hours.setRange(0, 12)
        self._hours.setValue(int(config.get("streak_hours", 2) or 0))
        self._hours.setSuffix(" h")
        _row(layout, tr("Warn me before my streak ends"), self._hours)
        self._time = _hours("ankiquest_reminder_hour")
        self._start = _hours("ankiquest_quiet_start")
        self._end = _hours("ankiquest_quiet_end")
        _row(layout, tr("Daily reminder time"), self._time)
        _row(layout, tr("Quiet hours"), self._start, QLabel(tr("to")), self._end)
        self._note = QLabel(tr("Equal start and end times turn quiet hours off. Quiet hours also hold desktop messages."))
        self._note.setWordWrap(True)
        layout.addWidget(self._note)
        self._timing = {"reminder_hour": self._time, "quiet_start": self._start, "quiet_end": self._end}
        for key, box in self._timing.items():
            box.setEnabled(False)
            box.currentIndexChanged.connect(lambda _index, field=key: self._touched.add(field))
        self._enabled.toggled.connect(self._toggle)
        self._hours.valueChanged.connect(self._warning_changed)
        self._system = QCheckBox(tr("Show system notifications when Anki is in the background"))
        self._system.setChecked(bool(config.get("system_notifications", False)))
        layout.addWidget(self._system)

    def _toggle(self, enabled):
        self._touched.add("enabled")
        self._hours.blockSignals(True)
        self._hours.setValue((self._hours.value() or 2) if enabled else 0)
        self._hours.blockSignals(False)

    def _warning_changed(self, hours):
        if hours > 0 and not self._enabled.isChecked():
            self._enabled.setChecked(True)

    def _loaded(self, settings):
        self._loaded_settings = settings
        if settings is None:
            self._note.setText(tr("Server reminders are unavailable. You can still use the local streak warning."))
            return
        if "enabled" not in self._touched:
            self._enabled.blockSignals(True)
            self._enabled.setChecked(self._hours.value() > 0 or any(settings.get(key) for key in _REMINDERS))
            self._enabled.blockSignals(False)
        for key, box in self._timing.items():
            box.blockSignals(True)
            box.setCurrentIndex(box.findData(settings[key]))
            box.blockSignals(False)
            box.setEnabled(self._account is _Account.SAVED)

    def _set_account(self, account):
        self._account = account
        for box in self._timing.values():
            box.setEnabled(account is _Account.SAVED and self._loaded_settings is not None)

    def _changes(self):
        settings = self._loaded_settings
        if settings is None:
            return {}
        changed = {key: box.currentData() for key, box in self._timing.items()
                   if key in self._touched and box.currentData() != settings[key]}
        if "enabled" in self._touched:
            changed.update({key: self._enabled.isChecked() for key in _REMINDERS})
        return changed

    def _values(self):
        return {"streak_hours": self._hours.value() if self._enabled.isChecked() else 0,
                "system_notifications": self._system.isChecked()}
