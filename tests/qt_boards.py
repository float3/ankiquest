"""Native global boards use real controls and deliberately reordered asynchronous replies."""
from concurrent.futures import Future
import json
from pathlib import Path
from types import SimpleNamespace
from urllib.parse import parse_qs, urlsplit
import unittest
from unittest.mock import Mock

from aqt.qt import QApplication, QWidget
from addon_loader import load_in_package

app = QApplication.instance() or QApplication([])
ui = load_in_package("boards_qt", "board_ui")
client = load_in_package("boards_qt", "client")
ui.tr = lambda text: text
EXAMPLES = Path(__file__).resolve().parents[1] / "docs/api/examples"


class Boards(unittest.TestCase):
    def setUp(self):
        self.tasks, self.calls, self.hidden = [], [], []
        self.mw = QWidget()
        self.mw.pm = SimpleNamespace(profile={})
        self.mw.taskman = SimpleNamespace(run_in_background=lambda work, done: self.tasks.append((work, done)))
        self.api = client.Client("https://quest.test", "hill", "secret")
        self.api._request = self.request
        self.api.server_info = Mock(return_value=(frozenset({"global_leaderboard_hide"}), None))
        self.addon = SimpleNamespace(mw=self.mw, client=lambda: self.api)
        self.pictures = SimpleNamespace(_get=Mock(return_value=None), _load=Mock())
        self.choose = Mock()
        self.settings = {"global_leaderboard": True}
        ui.board_data._remember(self.mw.pm.profile, self.api, ui.board_data._Query("day", "global"))
        self.window = ui._Window(self.addon, self.pictures, self.choose)
        self.window._dialog.show()
        app.processEvents()

    def tearDown(self):
        self.window._close()
        self.mw.close()
        app.processEvents()

    def request(self, path, body=None):
        self.calls.append((path, body))
        route, query = urlsplit(path).path, parse_qs(urlsplit(path).query)
        if route.startswith("/api/decks/"):
            return self.settings
        if route.startswith("/api/social/"):
            return {"groups": [{"id": 3, "name": "Study <together>"}]}
        if route.startswith("/api/leaderboard/hidden/"):
            if body:
                self.hidden = [{"user": "ana", "display": "Ana"}] if body["action"] == "hide" else []
            return self.hidden
        if route == "/api/records":
            return [{"window": "day", "unit": "xp", "holders": [{"user": "hill", "display": "Hill", "value": 107}]}]
        if route == "/api/leaderboard":
            name = "leaderboard_global_hidden" if self.api.name == "ana" else "leaderboard_global"
            rows = json.loads((EXAMPLES / (name + ".json")).read_text(encoding="utf-8"))
            if query.get("period") == ["month"]:
                rows[0]["xp"] = 999
            return [row for row in rows if row["user"] not in {item["user"] for item in self.hidden}]
        raise AssertionError(path)

    def finish(self, index=0, error=None):
        work, done = self.tasks.pop(index)
        future = Future()
        if error is not None:
            future.set_exception(error)
        else:
            future.set_result(work())
        done(future)
        app.processEvents()

    def test_summary_rows_use_server_values_and_records_use_the_scope(self):
        self.finish()
        table = self.window._tables["board"]
        self.assertEqual("20", table.topLevelItem(1).text(2))
        self.assertEqual("Ana", table.topLevelItem(1).text(1))
        self.assertIn(("/api/leaderboard?period=day&scope=global", None), self.calls)
        self.window._tabs.setCurrentIndex(1)
        self.finish()
        self.assertIn(("/api/records?scope=global", None), self.calls)
        self.assertEqual("107 XP", self.window._tables["records"].topLevelItem(0).text(2))

    def test_hidden_own_rows_are_labeled_and_cannot_be_hidden(self):
        self.window._close()
        self.tasks.clear()
        self.api.name = self.api.user = "ana"
        ui.board_data._remember(self.mw.pm.profile, self.api, ui.board_data._Query("week", "global"))
        self.window = ui._Window(self.addon, self.pictures, self.choose)
        self.finish()
        table = self.window._tables["board"]
        table.setCurrentItem(table.topLevelItem(2))
        self.assertIn("you, not shown to others", table.topLevelItem(2).text(1))
        self.assertFalse(self.window._hide.isEnabled())

    def test_newer_period_wins_and_group_queries_are_sent_to_the_server(self):
        self.finish()
        periods = self.window._periods
        periods.setCurrentIndex(periods.findData("week"))
        periods.setCurrentIndex(periods.findData("month"))
        self.finish(1)
        self.finish(0)
        self.assertEqual("999", self.window._tables["board"].topLevelItem(0).text(2))
        self.window._scopes.setCurrentIndex(self.window._scopes.findData("group:3"))
        self.finish()
        self.assertIn(("/api/leaderboard?period=month&scope=group%3A3", None), self.calls)
        self.assertFalse(self.window._hide.isEnabled())

    def test_hide_is_single_flight_private_and_reversible(self):
        self.finish()
        table = self.window._tables["board"]
        table.setCurrentItem(table.topLevelItem(0))
        self.assertFalse(self.window._hide.isEnabled())
        table.setCurrentItem(table.topLevelItem(1))
        self.assertTrue(self.window._hide.isEnabled())
        self.window._hide.click()
        self.window._hide.click()
        self.assertEqual(1, len(self.tasks))
        self.finish()
        self.finish()
        self.assertEqual(1, table.topLevelItemCount())
        self.window._tabs.setCurrentIndex(2)
        self.finish()
        hidden = self.window._tables["hidden"]
        hidden.setCurrentItem(hidden.topLevelItem(0))
        self.window._hide.click()
        self.finish()
        self.finish()
        self.finish()
        self.assertEqual(0, hidden.topLevelItemCount())
        writes = [call for call in self.calls if call[1] is not None]
        self.assertEqual([
            ("/api/leaderboard/hidden/hill", {"action": "hide", "user": "ana"}),
            ("/api/leaderboard/hidden/hill", {"action": "unhide", "user": "ana"}),
        ], writes)

    def test_a_failed_hide_keeps_the_row_and_can_be_retried(self):
        self.finish()
        table = self.window._tables["board"]
        table.setCurrentItem(table.topLevelItem(1))
        self.window._hide.click()
        self.finish(error=OSError("offline"))
        self.assertEqual(2, table.topLevelItemCount())
        self.assertTrue(self.window._hide.isEnabled())
        self.assertIn("Could not save", self.window._status.text())

    def test_late_reads_cannot_touch_a_closed_or_switched_profile(self):
        self.mw.pm.profile = {}
        self.finish()
        self.assertEqual(0, self.window._tables["board"].topLevelItemCount())
        self.window._close()
        self.window._load()
        self.assertEqual([], self.tasks)

    def test_an_offline_board_retries_and_an_old_server_hides_global(self):
        self.finish(error=OSError("offline"))
        self.assertTrue(self.window._refresh.isEnabled())
        self.assertIn("offline", self.window._status.text())
        self.settings = {}
        self.window._refresh.click()
        self.finish()
        self.assertEqual(-1, self.window._scopes.findData("global"))
        self.assertEqual("friends", self.window._scopes.currentData())
        self.assertFalse(self.window._tabs.isTabVisible(2))


if __name__ == "__main__":
    unittest.main()
