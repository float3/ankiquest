"""Real social controls with server fixtures and manually ordered replies."""
from concurrent.futures import Future
from copy import deepcopy
from http import HTTPStatus
import json
from pathlib import Path
import time
from types import SimpleNamespace
import unittest
from unittest.mock import Mock

from aqt.qt import QApplication, QPushButton, QWidget, Qt
from addon_loader import load_in_package

app = QApplication.instance() or QApplication([])
ui = load_in_package("social_qt", "social_ui")
client = load_in_package("social_qt", "client")
EXAMPLES = Path(__file__).resolve().parents[1] / "docs/api/examples"


class Social(unittest.TestCase):
    def setUp(self):
        self.tasks, self.calls = [], []
        self.people = json.loads((EXAMPLES / "social.json").read_text(encoding="utf-8"))
        self.goals = json.loads((EXAMPLES / "challenges.json").read_text(encoding="utf-8"))
        future = int(time.time() * 1000) + 86_400_000
        for goal in self.goals["challenges"]:
            goal["end_at"] = future
        self.goals["weekly_suggestion"]["expires_at"] = future
        self.mw = QWidget()
        self.mw.pm = SimpleNamespace(profile={})
        self.mw.taskman = SimpleNamespace(run_in_background=lambda work, done: self.tasks.append((work, done)))
        self.api = client.Client("https://quest.test", "hill", "secret")
        self.api._request = self.request
        self.changed = Mock()
        self.addon = SimpleNamespace(mw=self.mw, client=lambda: self.api)
        self.window = ui._Window(self.addon, self.changed)
        self.window._dialog.show()

    def tearDown(self):
        self.window._close()
        self.mw.close()
        app.processEvents()

    def request(self, path, body=None):
        self.calls.append((path, deepcopy(body)))
        if path.startswith("/api/invites/"):
            return {"name": path.rsplit("/", 1)[1], "members": 4}
        if path.startswith("/api/social/"):
            if body and body["action"] == "visibility":
                self.people["public"] = body["public"]
            if body and body["action"] == "befriend":
                self.people["incoming"] = []
                self.people["friends"].append({"user": body["user"], "display": "Ana"})
            return deepcopy(self.people)
        return deepcopy(self.goals)

    def finish(self, index=0, error=None):
        work, done = self.tasks.pop(index)
        future = Future()
        if error is None:
            future.set_result(work())
        else:
            future.set_exception(error)
        done(future)
        app.processEvents()

    def loaded(self):
        self.finish()
        self.finish()

    def test_incoming_request_accepts_once_and_redraws_from_the_answer(self):
        self.loaded()
        tree = self.window._people_tree
        tree.setCurrentItem(tree.topLevelItem(1).child(0))
        self.assertEqual("Accept request", self.window._person_buttons[0].text())
        self.window._person_buttons[0].click()
        self.window._person_buttons[0].click()
        self.assertEqual(1, len(self.tasks))
        self.finish()
        self.assertEqual(0, tree.topLevelItem(1).childCount())
        self.assertEqual(2, tree.topLevelItem(0).childCount())
        writes = [body for _, body in self.calls if body]
        self.assertEqual([{"action": "befriend", "user": "ana"}], writes)
        self.changed.assert_called_once_with()

    def test_visibility_failure_keeps_the_servers_choice_and_its_error(self):
        self.loaded()
        self.window._public.click()
        self.finish(error=client.ServerError(HTTPStatus.FORBIDDEN, "This profile cannot change now."))
        self.assertTrue(self.window._public.isChecked())
        self.assertIn("This profile cannot change now.", self.window._status.text())

    def test_group_owner_controls_do_not_offer_removing_yourself(self):
        self.loaded()
        self.window._groups.setCurrentIndex(self.window._groups.findData(2))
        self.assertFalse(self.window._rename.isEnabled())
        self.assertFalse(self.window._new_invite.isEnabled())
        self.window._groups.setCurrentIndex(self.window._groups.findData(1))
        self.assertTrue(self.window._rename.isEnabled())
        self.window._members.setCurrentItem(self.window._members.topLevelItem(1))
        self.assertFalse(self.window._remove.isEnabled())
        self.window._members.setCurrentItem(self.window._members.topLevelItem(0))
        self.window._remove.click()
        self.finish()
        self.assertIn(("/api/social/hill", {"action": "remove", "group": 1, "member": "ana"}), self.calls)

    def test_invite_preview_must_match_the_current_input_before_joining(self):
        self.loaded()
        self.window._invite_field.setText("first-code")
        self.window._preview_invite()
        self.window._invite_field.setText("second-code")
        self.window._preview_invite()
        self.finish(1)
        self.finish(0)
        self.assertIn("second-code", self.window._invite_status.text())
        self.window._join.click()
        self.finish()
        self.assertIn(("/api/social/hill", {"action": "join", "invite": "second-code"}), self.calls)

    def test_late_reads_after_close_or_account_switch_do_not_render(self):
        self.api = client.Client("https://quest.test", "ana", "different")
        self.finish()
        self.assertEqual(0, self.window._people_tree.topLevelItemCount())
        self.window._close()
        self.finish()
        self.assertEqual(0, self.window._goals._table.topLevelItemCount())

    def test_goal_invitation_actions_and_weekly_dismiss_use_server_routes(self):
        self.loaded()
        goals = self.window._goals
        goals._table.setCurrentItem(goals._table.topLevelItem(0))
        self.assertEqual("Accept invitation", goals._buttons[0].text())
        goals._buttons[0].click()
        self.finish()
        self.assertIn(("/api/community/challenges/hill/4", {"action": "accept"}), self.calls)
        goals._weekly_buttons[1].click()
        self.finish()
        self.assertIn(("/api/community/challenges/hill/weekly", {"action": "dismiss", "week_start": self.goals["weekly_suggestion"]["week_start"]}), self.calls)

    def test_creator_retries_the_same_payload_with_the_same_request_id(self):
        self.loaded()
        goals = self.window._goals
        goals._new()
        creator = goals._creator
        creator._recipients.item(0).setCheckState(Qt.CheckState.Checked)
        creator._save.click()
        work, done = self.tasks.pop()
        work()
        failed = Future()
        failed.set_exception(OSError("response lost"))
        done(failed)
        creator._save.click()
        self.finish()
        writes = [body for path, body in self.calls if body and path == "/api/community/challenges/hill"]
        self.assertEqual(2, len(writes))
        self.assertEqual(writes[0], writes[1])
        self.assertTrue(writes[0]["start_when_ready"])
        self.assertFalse(creator._dialog.isVisible())

    def test_reopening_a_goal_during_a_write_cannot_replace_its_request(self):
        self.loaded()
        goals = self.window._goals
        goals._table.setCurrentItem(goals._table.topLevelItem(0))
        goals._buttons[0].click()
        goals._load()
        self.assertEqual(1, len(self.tasks))
        self.assertTrue(self.window._busy())
        self.finish()
        self.assertFalse(self.window._busy())
        self.assertTrue(self.window._tabs.isEnabled())

    def test_a_late_get_does_not_overwrite_a_social_change(self):
        self.finish(1)
        old_work, old_done = self.tasks.pop()
        old_answer = Future()
        old_answer.set_result(old_work())
        self.window._username.setText("new-friend")
        self.window._add.click()
        self.finish()
        old_done(old_answer)
        friends = self.window._people["friends"]
        self.assertIn("new-friend", [row["user"] for row in friends])

    def test_closed_or_replaced_profiles_ignore_pending_changes(self):
        self.loaded()
        self.window._public.click()
        self.mw.pm.profile = {}
        self.finish()
        self.changed.assert_not_called()
        self.window._close()
        self.assertFalse(self.window._dialog.isVisible())

    def test_closing_the_window_closes_a_creator_with_a_pending_save(self):
        self.loaded()
        goals = self.window._goals
        goals._new()
        creator = goals._creator
        creator._recipients.item(0).setCheckState(Qt.CheckState.Checked)
        creator._save.click()
        self.window._close()
        self.finish()
        self.assertFalse(creator._dialog.isVisible())
        self.assertEqual({}, self.window._jobs)

    def test_challenge_inbox_button_uses_native_route_and_checks_account(self):
        inbox_ui = load_in_package("social_qt", "inbox_ui")
        route, read = Mock(), Mock()
        context = inbox_ui.Context(self.window._dialog, self.api, "aki", lambda: self.api,
                                   Mock(), route, Mock())
        context.mark_read = read
        entry = {"id": 7, "sender": "ana", "challenge_id": 4, "title": "Invitation"}
        row = inbox_ui._row(entry, context)
        button = next(button for button in row.findChildren(QPushButton) if button.text() == "View friend goal")
        button.click()
        route.assert_called_once_with("/community#challenge-4")
        read.assert_called_once()
        self.api = client.Client("https://quest.test", "other", "new")
        button.click()
        route.assert_called_once()
        row.close()


if __name__ == "__main__":
    unittest.main()
