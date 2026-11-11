"""A card stand-in for hook tests; qt_moments.py also exercises the actual window."""

from types import SimpleNamespace
from unittest.mock import Mock, patch


class _Signal:
    def __init__(self):
        self.slots = []

    def connect(self, slot):
        self.slots.append(slot)

    def emit(self, value):
        for slot in self.slots:
            slot(value)


class _Card:
    def __init__(self, addon, api, profile, items, summary):
        self._profile = profile
        self._append(items, summary)
        finished = _Signal()
        self._dialog = SimpleNamespace(finished=finished, show=Mock(), close=lambda: finished.emit(0))

    def _append(self, items, summary):
        self._items, self._summary = list(items), summary


def cards(case):
    module = case.addon.moments_ui
    factory = Mock(side_effect=_Card)
    for name, value in (("_Card", factory), ("QTimer", SimpleNamespace(singleShot=lambda delay, callback: callback()))):
        changed = patch.object(module, name, value)
        changed.start()
        case.addCleanup(changed.stop)
    return factory
