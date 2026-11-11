"""Real inbox widgets receive replies after close, reload and account changes."""
from concurrent.futures import Future
from copy import deepcopy
from http import HTTPStatus
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

from aqt.qt import QApplication, QCoreApplication, QEvent, QLabel, QLineEdit, QPushButton, QWidget
from addon_loader import load_in_package

app = QApplication.instance() or QApplication([])
ui = load_in_package("activity_qt", "inbox_ui")
client = load_in_package("activity_qt", "client")


class Activity(unittest.TestCase):
    def setUp(self):
        self.tasks = []
        self.parent = QWidget()
        self.parent.pm = SimpleNamespace(profile={})
        self.api = client.Client("https://quest.test", "hill", "secret")
        self.page = {"items": [{"id": 4, "title": "Friend", "body": "Hello", "sender": "ana"}],
                     "unread_count": 7, "action_count": 2, "latest_id": 4, "next_before": None}
        self.api._request = Mock(side_effect=lambda *_args: deepcopy(self.page))
        self.run = lambda work, done: self.tasks.append((work, done))
        self.dialog = ui._build(self.parent, self.api, "aki", lambda: self.api, self.run, Mock(), Mock())
        self.dialog.show()

    def tearDown(self):
        try:
            self.dialog.close()
        except RuntimeError:
            pass
        self.parent.close()
        app.processEvents()

    def finish(self, index=0, error=None):
        work, done = self.tasks.pop(index)
        future = Future()
        if error is None:
            future.set_result(work())
        else:
            future.set_exception(error)
        done(future)
        app.processEvents()

    def test_a_late_load_cannot_touch_a_deleted_dialog(self):
        self.dialog.reload()
        self.dialog.close()
        QCoreApplication.sendPostedEvents(None, QEvent.Type.DeferredDelete)
        self.finish()

    def test_a_late_read_receipt_cannot_mutate_another_accounts_row(self):
        entry = {"id": 4}
        changed = Mock()
        self.dialog.context.mark_read(entry, changed)
        self.api = client.Client("https://quest.test", "ana", "new")
        self.finish()
        self.assertNotIn("read_at", entry)
        changed.assert_not_called()

    def test_the_same_account_in_a_new_profile_gets_a_new_window(self):
        ui.state["dialog"] = self.dialog
        self.parent.pm.profile = {}
        opened = ui.inbox_dialog(self.parent, self.api, "aki", lambda: self.api, self.run, Mock(), Mock())
        self.assertIsNot(self.dialog, opened)
        opened.close()

    def test_pages_append_without_duplicates_and_keep_global_counts(self):
        self.page["next_before"] = 4
        self.dialog.reload()
        self.finish()
        window = self.dialog._inbox
        self.assertIn("7 unread", window._counts.text())
        window._more.click()
        self.page["items"] = [{"id": 3, "title": "Older"}, deepcopy(self.page["items"][0])]
        self.page["next_before"] = None
        self.finish()
        self.assertEqual([4, 3], [row["id"] for row in window._page.items])
        self.assertFalse(window._more.isVisible())
        self.assertIn("before=4", self.api._request.call_args.args[0])

    def test_old_filter_replies_cannot_replace_the_new_filter(self):
        window = self.dialog._inbox
        self.dialog.reload()
        work, done = self.tasks.pop()
        old = Future()
        old.set_result(work())
        window._category.setCurrentIndex(2)
        self.page["items"] = [{"id": 8, "title": "New filter"}]
        self.finish()
        done(old)
        self.assertEqual(8, window._page.items[0]["id"])

    def test_mark_all_uses_the_displayed_latest_id_and_reloads_new_arrivals(self):
        self.dialog.reload()
        self.finish()
        window = self.dialog._inbox
        window._every.click()
        window._every.click()
        self.assertEqual(1, len(self.tasks))
        self.page["latest_id"] = 9
        self.page["items"] = [{"id": 9, "title": "New arrival"}]
        self.finish()
        self.assertEqual({"through": 4}, self.api._request.call_args.args[1])
        self.finish()
        self.assertNotIn("read_at", window._page.items[0])

    def test_a_reply_after_rows_are_replaced_cannot_touch_deleted_controls(self):
        self.dialog.reload()
        self.finish()
        context = self.dialog.context._row_context()
        status, finished = QLabel(), Mock()
        entry = {"id": 4, "sender": "ana"}
        panel = ui._reply_panel(entry, context, status, finished)
        panel.findChild(QLineEdit).setText("Thanks")
        send = next(button for button in panel.findChildren(QPushButton) if button.text() == "Send")
        send.click()
        send.click()
        self.assertEqual(1, len(self.tasks))
        self.dialog._inbox._render()
        panel.deleteLater()
        status.deleteLater()
        QCoreApplication.sendPostedEvents(None, QEvent.Type.DeferredDelete)
        self.finish()
        finished.assert_not_called()
        self.assertNotIn("replied", entry)
        self.assertEqual(1, len(self.tasks), "a successful write refreshes the current rows after an intervening render")

    def test_recap_line_breaks_and_already_cheered_state_survive_rendering(self):
        context = self.dialog.context._row_context()
        recap = ui._row({"id": 4, "kind": "reminder_weekly_recap", "body": "12 reviews.\nThree study days."}, context)
        self.assertIn("12 reviews.\nThree study days.", [label.text() for label in recap.findChildren(QLabel)])
        row = ui._row({"id": 5, "kind": "share", "sender": "ana", "reacted": True}, context)
        cheer = next(button for button in row.findChildren(QPushButton) if "Cheered" in button.text())
        self.assertFalse(cheer.isEnabled())
        recap.close()
        row.close()

    def test_late_images_cannot_repopulate_a_closed_accounts_cache(self):
        ui.state["pictures"] = {}
        with patch.object(ui.share, "download_image", return_value=b"png"):
            label = ui._thumbnail("https://quest.test/share/test.png", self.dialog.context._row_context())
            self.dialog.close()
            self.finish()
        self.assertEqual({}, ui.state["pictures"])
        label.close()

    def test_a_read_receipt_refreshes_server_counts_and_the_deck_list_badge(self):
        self.dialog.reload()
        self.finish()
        changed = self.dialog.context._after_change = Mock()
        context = self.dialog.context._row_context()
        context.mark_read(self.dialog._inbox._page.items[0], Mock())
        self.page["unread_count"] = 6
        self.finish()
        changed.assert_called_once()
        self.finish()
        self.assertIn("6 unread", self.dialog._inbox._counts.text())

    def test_a_refused_read_receipt_keeps_the_row_unread_and_can_retry(self):
        context = self.dialog.context._row_context()
        entry = {"id": 4, "title": "Hello"}
        row = ui._row(entry, context)
        button = next(button for button in row.findChildren(QPushButton) if button.text() == "Mark read")
        button.click()
        button.click()
        self.assertEqual(1, len(self.tasks))
        self.finish(error=client.ServerError(HTTPStatus.FORBIDDEN, "Reading is temporarily unavailable."))
        self.assertNotIn("read_at", entry)
        self.assertTrue(any("Reading is temporarily unavailable." in label.text() for label in row.findChildren(QLabel)))
        button.click()
        self.assertEqual(1, len(self.tasks))
        row.close()


if __name__ == "__main__":
    unittest.main()
