"""Deck-copy work cannot cross profiles or outlive its dialog."""
from concurrent.futures import Future
from pathlib import Path
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

from aqt.qt import QApplication, QCoreApplication, QDialog, QEvent, QLabel, QPushButton, QWidget
from addon_loader import load_in_package

app = QApplication.instance() or QApplication([])
ui = load_in_package("copies_qt", "copies_ui")
client = load_in_package("copies_qt", "client")


class Copies(unittest.TestCase):
    def setUp(self):
        self.tasks = []
        self.parent = QWidget()
        self.parent.pm = SimpleNamespace(profile={})
        self.parent.col = object()
        self.parent.taskman = SimpleNamespace(run_in_background=self.queue)
        self.api = client.Client("https://quest.test", "hill", "secret")
        self.api._request = Mock(return_value={"offers": []})
        ui.state.update(mw=self.parent, client=lambda: self.api, companion=lambda: "aki", action=None, delivery=None)
        ui.clear_account()
        tip = patch.object(ui, "tooltip")
        self.tip = tip.start()
        self.addCleanup(tip.stop)
        self.dialog = QDialog(self.parent)
        self.status = QLabel(self.dialog)
        self.holder = ui._offer_row(self.api, {"id": 7, "sender": "ana", "deck": "Verbs"}, {}, 1024,
                                    self.dialog, self.status)
        self.holder.setParent(self.dialog)
        self.dialog.show()

    def tearDown(self):
        ui.clear_account()
        try:
            self.dialog.close()
        except RuntimeError:
            pass
        self.parent.close()
        app.processEvents()

    def queue(self, work, done, **_kwargs):
        self.tasks.append((work, done))

    def finish(self, index=0, error=None):
        work, done = self.tasks.pop(index)
        future = Future()
        if error is not None:
            future.set_exception(error)
        else:
            future.set_result(work())
        done(future)

    def button(self, text):
        return next(button for button in self.holder.findChildren(QPushButton) if button.text() == text)

    def test_a_load_for_the_same_account_in_another_profile_is_discarded(self):
        loaded = Mock()
        ui._load(self.api, loaded)
        self.parent.pm.profile = {}
        self.finish()
        loaded.assert_not_called()

    def test_an_obsolete_load_error_is_quiet(self):
        ui._load(self.api, Mock())
        ui.clear_account()
        self.finish(error=OSError("offline"))
        self.tip.assert_not_called()

    def test_repeated_open_requests_share_one_load(self):
        ui._load(self.api, Mock())
        ui._load(self.api, Mock())
        self.assertEqual(1, len(self.tasks))

    def test_a_queued_send_cannot_export_a_new_collection(self):
        with patch.object(ui.copies, "share") as share:
            ui.send(self.api, (1, "Spanish", ["ana"]), 1024)
            self.parent.col = object()
            self.parent.pm.profile = {}
            self.tip.reset_mock()
            self.finish()
        share.assert_not_called()
        self.tip.assert_not_called()

    def test_late_send_feedback_is_quiet_after_sign_out(self):
        with patch.object(ui.copies, "share"):
            ui.send(self.api, (1, "Spanish", ["ana"]), 1024)
            work, done = self.tasks.pop()
            future = Future()
            future.set_result(work())
            ui.clear_account()
            self.tip.reset_mock()
            done(future)
        self.tip.assert_not_called()

    def test_a_closed_download_never_opens_the_importer_and_removes_its_file(self):
        def download(_api, _id, target, _limit):
            Path(target).write_bytes(b"package")
            return target

        with patch.object(ui.copies, "download", side_effect=download), patch.object(ui, "import_package") as importer:
            self.button("Import").click()
            work, done = self.tasks.pop()
            future = Future()
            target = work()
            self.addCleanup(lambda: Path(target).unlink(missing_ok=True))
            future.set_result(target)
            self.dialog.close()
            done(future)
        importer.assert_not_called()
        self.assertFalse(Path(target).exists())

    def test_a_late_dismiss_cannot_change_a_new_accounts_badge(self):
        with patch.object(ui.copies, "dismiss"):
            self.button("Dismiss").click()
            work, done = self.tasks.pop()
            future = Future()
            future.set_result(work())
            ui.clear_account()
            ui._set_count(5)
            done(future)
        self.assertEqual(5, ui.state["count"])
        self.assertEqual("", self.status.text())

    def test_a_current_failed_dismiss_can_retry(self):
        self.button("Dismiss").click()
        self.finish(error=OSError("offline"))
        self.assertTrue(self.button("Dismiss").isEnabled())
        self.assertIn("offline", self.status.text())
        self.button("Dismiss").click()
        self.assertEqual(1, len(self.tasks))

    def test_destroying_a_window_invalidates_its_pending_error(self):
        self.button("Dismiss").click()
        self.dialog.deleteLater()
        QCoreApplication.sendPostedEvents(None, QEvent.Type.DeferredDelete)
        self.assertNotIn(self.dialog, ui.state["dialogs"])
        self.finish(error=OSError("offline"))

    def test_a_new_profiles_poll_does_not_wait_for_the_old_one(self):
        ui.poll(quiet=True)
        self.parent.pm.profile = {}
        ui.poll(quiet=True)
        self.assertEqual(2, len(self.tasks))
        self.finish()
        ui.poll(quiet=True)
        self.assertEqual(1, len(self.tasks), "an old completion cannot release the new request")

    def test_the_current_download_opens_the_importer_once(self):
        def download(_api, _id, target, _limit):
            Path(target).write_bytes(b"package")
            return target

        with patch.object(ui.copies, "download", side_effect=download), patch.object(ui, "import_package") as importer:
            self.button("Import").click()
            self.button("Import").click()
            self.assertEqual(1, len(self.tasks))
            self.finish()
        importer.assert_called_once()
        Path(importer.call_args.args[1]).unlink()


if __name__ == "__main__":
    unittest.main()
