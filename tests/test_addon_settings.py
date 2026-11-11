"""Exercise the settings UI with real Qt, isolated from the other tests' Anki stand-ins."""

import os
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import Mock, patch

import test_deck_completion as addon_fixture


class SettingsDialog(unittest.TestCase):
    def test_settings_reads_saves_and_account_boundaries(self):
        result = subprocess.run(
            [sys.executable, str(Path(__file__).with_name("qt_settings.py"))],
            env=dict(os.environ, QT_QPA_PLATFORM="offscreen"),
            capture_output=True, text=True, timeout=60,
        )
        self.assertEqual(0, result.returncode, result.stdout + result.stderr)


class SettingsMenu(unittest.TestCase):
    setUp = addon_fixture.RefreshTests.setUp
    read_rows = addon_fixture.RefreshTests.read_rows
    write_config = addon_fixture.RefreshTests.write_config
    run_task = addon_fixture.RefreshTests.run_task

    def test_one_tools_submenu_holds_all_eight_destinations(self):
        menu = self.addon._tools
        self.mw.form.menuTools.addAction.assert_called_once_with(menu.menuAction())
        self.assertEqual(8, menu.addAction.call_count)
        self.assertEqual(
            ["Today", "Inbox", "Friends", "Website", "Settings", "Check for updates"],
            [call.args[0] for call in self.addon.QAction.call_args_list],
        )
        self.assertEqual(
            ["Send a deck", "Decks offered to me"],
            [call.args[0] for call in self.addon.copies_ui.QAction.call_args_list],
        )

    def saved_callback(self):
        with patch.object(self.addon.preferences, "section", return_value=None) as section, \
                patch.object(self.addon.sharing, "section", return_value=None):
            self.addon.settings_sections(Mock(), {"sharing": Mock()})
            return section.call_args.kwargs["on_saved"]

    def test_friends_menu_opens_the_native_window(self):
        connections = self.addon.QAction.return_value.triggered.connect.call_args_list
        self.assertIn(self.addon.home._open_friends, [call.args[0] for call in connections])

    def test_saved_choices_refresh_after_invalidating_an_older_poll(self):
        saved = self.saved_callback()
        self.addon.state["polling"] = object()
        self.addon.poll = Mock()
        saved()
        self.assertIsNone(self.addon.state["polling"])
        self.addon.poll.assert_called_once_with(quiet=True)

    def test_late_saved_choices_do_not_refresh_another_account(self):
        saved = self.saved_callback()
        self.api.token = "another-token"
        self.addon.poll = Mock()
        saved()
        self.addon.poll.assert_not_called()


if __name__ == "__main__":
    unittest.main()
