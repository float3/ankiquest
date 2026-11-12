"""Real Qt checks run in a separate process by test_addon_settings.py."""

from concurrent.futures import Future
from copy import deepcopy
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

from aqt.qt import QApplication, QCheckBox, QComboBox, QDialog, QGroupBox, QLabel, QPushButton, QScrollArea, QTimer

from addon_loader import load_in_package


app = QApplication.instance() or QApplication([])
ui = load_in_package("settings_qt", "ui")
preferences = load_in_package("settings_qt", "preferences")
ui.tr = preferences.tr = lambda text: text


class Settings(unittest.TestCase):
    def setUp(self):
        self.config = {"url": "https://example.test", "user": "sam", "token": "token"}
        self.decks = {"nudges": True, "celebrations": False, "global_leaderboard": True, "decks": []}
        self.posts = []
        self.reminders = dict.fromkeys(preferences._REMINDERS, False)
        self.reminders.update(reminder_hour=20, quiet_start=22, quiet_end=9, daily_limit=2)
        self.api = SimpleNamespace(
            configured=True, user="sam", _request=self.request,
            deck_settings=lambda: deepcopy(self.decks), companion=lambda: "ankilope",
            set_companion=lambda value: self.posts.append(("companion", value)),
        )
        self.mw = SimpleNamespace(pm=SimpleNamespace(profile={}), taskman=SimpleNamespace(run_in_background=self.run_task))

    def request(self, path, body=None):
        if body is not None:
            self.posts.append((path, body))
            return {}
        if "/streak-freezes/" in path:
            return {"enabled": True}
        if "/community/reminders/" in path:
            return deepcopy(self.reminders)
        if "/deck-subscriptions/" in path:
            return {"enabled": True, "senders": [], "unsubscribed_senders": []}
        if "/decks/" in path:
            return deepcopy(self.decks)
        raise AssertionError(path)

    def run_task(self, work, done):
        future = Future()
        try:
            future.set_result(work())
        except Exception as error:
            future.set_exception(error)
        done(future)

    def button(self, dialog, text):
        return next(widget for widget in dialog.findChildren(QPushButton) if widget.text() == text)

    def checkbox(self, dialog, text):
        return next(widget for widget in dialog.findChildren(QCheckBox) if widget.text() == text)

    def open(self, action, on_account=None):
        errors = []

        def interact():
            dialog = app.activeModalWidget()
            try:
                self.assertIsInstance(dialog, QDialog)
                action(dialog)
            except Exception as error:
                errors.append(error)
                dialog.reject()

        QTimer.singleShot(0, interact)
        values = ui.settings_dialog(
            None, self.config, Mock(), Mock(), on_account=on_account,
            extra=lambda dialog, groups: preferences.section(dialog, groups, self.mw, self.api),
        )
        if errors:
            raise errors[0]
        return values

    def test_four_groups_scroll_with_save_outside_the_scroll_area(self):
        def inspect(dialog):
            self.assertEqual(
                ["Account", "Notify me about", "Sharing", "App and privacy"],
                [group.title() for group in dialog.findChildren(QGroupBox)],
            )
            scroll = dialog.findChild(QScrollArea, "ankiquest_settings_scroll")
            self.assertIsNotNone(scroll)
            self.assertLess(dialog.height(), dialog.screen().availableGeometry().height())
            self.assertLess(dialog.width(), dialog.screen().availableGeometry().width())
            self.assertFalse(scroll.isAncestorOf(self.button(dialog, "Save")))
            dialog.reject()

        self.assertIsNone(self.open(inspect))
        self.assertEqual([], self.posts)

    def test_reminder_hours_follow_language_and_keep_numeric_values(self):
        for code, clock in (("en_US", "8:00"), ("de_DE", "20:00")):
            def inspect(dialog):
                box = dialog.findChild(QComboBox, "ankiquest_reminder_hour")
                self.assertIn(clock, box.currentText())
                self.assertEqual(20, box.currentData())
                if code == "en_US":
                    self.assertIn("PM", box.currentText())
                dialog.reject()

            with self.subTest(language=code), patch("anki.lang.current_lang", code):
                self.open(inspect)
        self.assertEqual([], self.posts)

    def test_installed_version_is_visible_and_ankiweb_hides_github_channels(self):
        def inspect(dialog):
            labels = [widget.text() for widget in dialog.findChildren(QLabel)]
            self.assertIn("Version: addon-11", labels)
            self.assertIn("Updates are managed by AnkiWeb.", labels)
            self.assertNotIn("Update channel", labels)
            dialog.reject()

        with patch.object(ui.updates, "installed", return_value={"release": 11, "nightly": 0, "commit": "abc"}), \
                patch.object(ui.updates, "_source", return_value=ui.updates._Source.ANKIWEB):
            self.open(inspect)

    def test_reads_server_choices_and_saves_only_changed_fields(self):
        def edit(dialog):
            self.assertTrue(self.checkbox(dialog, "Show me on the global leaderboard").isChecked())
            self.assertTrue(self.checkbox(dialog, "Nudge me when a place, my best day or the next level is within reach").isChecked())
            self.assertFalse(self.checkbox(dialog, "Celebrate achievements, streak milestones and personal bests").isChecked())
            self.checkbox(dialog, "Show me on the global leaderboard").setChecked(False)
            self.button(dialog, "Save").click()

        self.assertEqual(self.config["user"], self.open(edit)["user"])
        self.assertEqual([("/api/decks/sam", {"global_leaderboard": False})], self.posts)

    def test_an_older_server_hides_the_global_switch_and_gets_no_new_field(self):
        self.decks.pop("global_leaderboard")

        def edit(dialog):
            global_choice = self.checkbox(dialog, "Show me on the global leaderboard")
            self.assertFalse(global_choice.isVisible())
            self.checkbox(dialog, "Celebrate achievements, streak milestones and personal bests").setChecked(True)
            self.button(dialog, "Save").click()

        self.open(edit)
        self.assertEqual([("/api/decks/sam", {"celebrations": True})], self.posts)

    def test_companion_is_read_from_and_saved_to_the_account(self):
        def edit(dialog):
            companion = dialog.findChild(QComboBox, "ankiquest_companion")
            self.assertIsNotNone(companion)
            self.assertEqual("ankilope", companion.currentData())
            companion.setCurrentIndex(companion.findData("aki"))
            self.button(dialog, "Save").click()

        self.open(edit)
        self.assertEqual([("companion", "aki")], self.posts)

    def test_account_switch_does_not_save_old_server_choices(self):
        account = {"url": "https://another.test", "user": "other", "token": "new-token"}

        def edit(dialog):
            self.checkbox(dialog, "Show me on the global leaderboard").setChecked(False)
            self.button(dialog, "Switch account…").click()
            self.assertFalse(self.checkbox(dialog, "Show me on the global leaderboard").isEnabled())
            self.assertFalse(self.button(dialog, "Upload everything again").isEnabled())
            self.button(dialog, "Save").click()

        values = self.open(edit, on_account=lambda _url: account)
        self.assertEqual(account, {key: values[key] for key in account})
        self.assertEqual([], self.posts)

    def test_reminders_off_disables_local_warning_and_every_server_kind(self):
        self.reminders.update(gentle_daily=True, urgent_streak=True)

        def edit(dialog):
            self.checkbox(dialog, "Reminders").setChecked(False)
            self.button(dialog, "Save").click()

        values = self.open(edit)
        self.assertEqual(0, values["streak_hours"])
        saved = self.posts[0][1]
        self.assertFalse(any(saved[key] for key in preferences._REMINDERS))
        self.assertEqual(20, saved["reminder_hour"])

    def test_reminders_on_enables_both_and_timing_preserves_remote_changes(self):
        self.config["streak_hours"] = 0

        def edit(dialog):
            self.checkbox(dialog, "Reminders").setChecked(True)
            dialog.findChild(QComboBox, "ankiquest_reminder_hour").setCurrentIndex(19)
            self.reminders["daily_limit"] = 4
            self.button(dialog, "Save").click()

        self.assertEqual(2, self.open(edit)["streak_hours"])
        saved = self.posts[0][1]
        self.assertTrue(all(saved[key] for key in preferences._REMINDERS))
        self.assertEqual((19, 4), (saved["reminder_hour"], saved["daily_limit"]))

    def test_timing_only_preserves_other_reminder_toggles(self):
        def edit(dialog):
            dialog.findChild(QComboBox, "ankiquest_quiet_end").setCurrentIndex(8)
            self.reminders["weekly_recap"] = True
            self.button(dialog, "Save").click()

        self.open(edit)
        saved = self.posts[0][1]
        self.assertTrue(saved["weekly_recap"])
        self.assertFalse(saved["gentle_daily"])
        self.assertEqual(8, saved["quiet_end"])

    def test_account_switch_drops_reminder_edits(self):
        def edit(dialog):
            self.checkbox(dialog, "Reminders").setChecked(False)
            dialog.findChild(QComboBox, "ankiquest_reminder_hour").setCurrentIndex(10)
            self.button(dialog, "Switch account…").click()
            self.assertFalse(dialog.findChild(QComboBox, "ankiquest_reminder_hour").isEnabled())
            self.button(dialog, "Save").click()

        self.open(edit, on_account=lambda _: {"url": "https://example.test", "user": "other", "token": "new"})
        self.assertEqual([], self.posts)

    def test_late_reminder_read_does_not_enable_the_staged_accounts_controls(self):
        pending = []
        self.mw.taskman.run_in_background = lambda work, done: pending.append((work, done))

        def edit(dialog):
            self.button(dialog, "Switch account…").click()
            for work, done in pending:
                self.run_task(work, done)
            self.assertFalse(dialog.findChild(QComboBox, "ankiquest_reminder_hour").isEnabled())
            self.button(dialog, "Save").click()

        self.open(edit, on_account=lambda _: {"url": "https://example.test", "user": "other", "token": "new"})
        self.assertEqual([], self.posts)

    def test_cancel_does_not_save_server_changes(self):
        def edit(dialog):
            self.checkbox(dialog, "Show me on the global leaderboard").setChecked(False)
            dialog.reject()

        self.assertIsNone(self.open(edit))
        self.assertEqual([], self.posts)

    def test_save_before_choices_arrive_keeps_the_server_values(self):
        pending = []
        self.mw.taskman.run_in_background = lambda work, done: pending.append((work, done))
        self.open(lambda dialog: self.button(dialog, "Save").click())
        self.assertEqual(2, len(pending), "Only the two reads may be scheduled")
        for work, done in pending:
            self.run_task(work, done)
        self.assertEqual([], self.posts)

    def test_unrecognized_companion_is_not_replaced_on_save(self):
        self.api.companion = lambda: "future-companion"
        self.open(lambda dialog: self.button(dialog, "Save").click())
        self.assertEqual([], self.posts)

    def test_deck_picker_only_selects_decks_and_people(self):
        errors = []

        def inspect():
            dialog = app.activeModalWidget()
            try:
                labels = [box.text() for box in dialog.findChildren(QCheckBox)]
                self.assertFalse(any("Nudge" in label or "Celebrate" in label for label in labels))
                dialog.accept()
            except Exception as error:
                errors.append(error)
                dialog.reject()

        QTimer.singleShot(0, inspect)
        values = ui.deck_dialog(None, self.decks)
        if errors:
            raise errors[0]
        self.assertEqual(([], [], [], None, None), values)


if __name__ == "__main__":
    unittest.main()
