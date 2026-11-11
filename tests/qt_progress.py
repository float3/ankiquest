"""Read-only progress, with out-of-order replies and real Qt controls."""
from concurrent.futures import Future
from copy import deepcopy
from http import HTTPStatus
from types import SimpleNamespace
import unittest
from unittest.mock import Mock

from aqt.qt import QApplication, QPushButton, QWidget
from addon_loader import load_in_package

app = QApplication.instance() or QApplication([])
ui = load_in_package("progress_qt", "progress_ui")
ui.tr = lambda text: text


class Progress(unittest.TestCase):
    def setUp(self):
        self.paths, self.tasks = [], []
        self.mw = QWidget()
        self.mw.pm = SimpleNamespace(profile={})
        self.mw.taskman = SimpleNamespace(run_in_background=lambda work, done: self.tasks.append((work, done)))
        self.api = SimpleNamespace(base="https://quest.test", user="sam", token="secret", _request=self.request)
        self.addon = SimpleNamespace(mw=self.mw, client=lambda: self.api, open_page=Mock())
        self.window = ui._Progress(self.addon)
        self.window._dialog.show()

    def tearDown(self):
        self.window._close()
        self.mw.close()
        app.processEvents()

    def request(self, path):
        self.paths.append(path)
        if path == "/api/records":
            return [{"window": "day", "unit": "xp", "holders": [{"display": "Sam", "value": 400}]}]
        year = int(path.rsplit("=", 1)[1]) if "=" in path else 2026
        return {"year": year, "years": [2026, 2025], "days": [
            {"date": "%d-01-01" % year, "state": "skipped", "reviews": 0, "time_ms": 0, "xp": 0},
            {"date": "%d-01-02" % year, "state": "met", "reviews": 12, "time_ms": 600_000, "xp": 120},
        ]}

    def finish(self, index=0, error=None):
        work, done = self.tasks.pop(index)
        future = Future()
        if error:
            future.set_exception(error)
        else:
            future.set_result(deepcopy(work()))
        done(future)
        app.processEvents()

    def test_reads_history_and_records_with_distinct_day_states(self):
        self.finish()
        self.finish()
        history = self.window._tables[ui._Section.HISTORY]
        self.assertEqual(2, history.topLevelItemCount())
        self.assertEqual("Streak minimum met", history.topLevelItem(0).text(1))
        self.assertEqual("Skipped day — streak kept", history.topLevelItem(1).text(1))
        self.assertNotEqual(history.topLevelItem(0).background(1), history.topLevelItem(1).background(1))
        records = self.window._tables[ui._Section.RECORDS]
        self.assertEqual("Sam", records.topLevelItem(0).text(1))
        self.assertEqual(["/api/study/sam", "/api/records"], self.paths)

    def test_a_late_year_reply_cannot_replace_the_selected_year(self):
        self.finish()
        self.finish()
        self.window._years.setCurrentIndex(self.window._years.findData(2025))
        self.window._years.setCurrentIndex(self.window._years.findData(2026))
        self.finish(1)
        self.finish(0)
        table = self.window._tables[ui._Section.HISTORY]
        self.assertEqual("2026-01-02", table.topLevelItem(0).text(0))

    def test_late_replies_do_not_touch_closed_windows_or_another_account(self):
        self.mw.pm.profile = {}
        self.finish()
        self.assertEqual(0, self.window._tables[ui._Section.HISTORY].topLevelItemCount())
        self.window._dialog.close()
        self.finish()

    def test_an_older_server_has_a_clear_fallback_and_retry(self):
        self.finish(error=SimpleServerError(HTTPStatus.NOT_FOUND))
        self.assertIn("does not support", self.window._statuses[ui._Section.HISTORY].text())
        self.assertTrue(self.window._retry[ui._Section.HISTORY].isEnabled())
        button = next(b for b in self.window._dialog.findChildren(QPushButton) if b.text() == "Open website")
        button.click()
        self.addon.open_page.assert_called_once_with("/history")

    def test_an_offline_read_can_be_retried_without_losing_the_other_tab(self):
        self.finish(error=OSError("offline"))
        self.finish()
        self.window._retry[ui._Section.HISTORY].click()
        self.finish()
        self.assertEqual(2, self.window._tables[ui._Section.HISTORY].topLevelItemCount())
        self.assertEqual(1, self.window._tables[ui._Section.RECORDS].topLevelItemCount())


class SimpleServerError(Exception):
    def __init__(self, status):
        self.status = status


if __name__ == "__main__":
    unittest.main()
