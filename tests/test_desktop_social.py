"""Social wire contracts, invite boundaries, goal lifecycle and idempotent retries."""
import json
from pathlib import Path
from types import SimpleNamespace
import unittest
from unittest.mock import Mock

from addon_loader import load_module

data = load_module("social_rules", "social_data.py")
EXAMPLES = Path(__file__).resolve().parents[1] / "docs/api/examples"


class Social(unittest.TestCase):
    def setUp(self):
        self.api = SimpleNamespace(base="https://quest.test", user="hill", _request=Mock(return_value={}))

    def test_a_creation_retry_keeps_its_key_but_an_edited_goal_gets_a_new_one(self):
        draft = data._Draft()
        fields = dict(title=" Three study days ", kind="study_days", cooperative=False, target=3,
                      duration_days=7, recipients=["cerro", "ana", "ana"], start_when_ready=True)
        first = draft._body(fields)
        same = draft._body(dict(fields, title="Three study days", recipients=["ana", "cerro"]))
        self.assertEqual(first, same)
        self.assertNotEqual(first["request_id"], draft._body(dict(fields, target=4))["request_id"])
        self.assertNotEqual(first["request_id"], data._Draft()._body(fields)["request_id"])
        self.assertRegex(first["request_id"], r"^[A-Za-z0-9_-]{8,128}$")
        self.assertNotIn("request_id", fields)

    def test_invites_accept_a_code_or_this_servers_link_and_reject_other_hosts(self):
        for value in ("abc-123", "/join/abc-123", "https://quest.test/join/abc-123"):
            self.assertEqual("abc-123", data._invite_code(value, self.api.base))
        for value in ("https://elsewhere.test/join/abc-123", "https://quest.test@evil.test/join/abc-123",
                      "file:///join/abc-123", "/join/a/b", "", "a?token=secret"):
            with self.subTest(value=value), self.assertRaises(ValueError):
                data._invite_code(value, self.api.base)

    def test_preview_is_read_only_and_group_links_are_owned(self):
        data._preview(self.api, "a/b")
        self.api._request.assert_called_once_with("/api/invites/a%2Fb")
        self.assertIsNone(data._invite_link(self.api, {"invite": None}))
        self.assertEqual("https://quest.test/join/abc", data._invite_link(self.api, {"invite": "abc"}))

    def test_social_and_goal_changes_use_the_owners_routes_and_return_the_answer(self):
        result = {"friends": [{"user": "ana"}]}
        self.api._request.return_value = result
        self.assertIs(result, data._change(self.api, data._Social.BEFRIEND, user="ana"))
        self.api._request.assert_called_with("/api/social/hill", {"action": "befriend", "user": "ana"})
        data._goal_action(self.api, 4, data._GoalAction.ACCEPT)
        self.api._request.assert_called_with("/api/community/challenges/hill/4", {"action": "accept"})
        data._weekly(self.api, 123, data._WeeklyAction.DISMISS)
        self.api._request.assert_called_with("/api/community/challenges/hill/weekly", {"action": "dismiss", "week_start": 123})

    def test_fixture_actions_match_membership_lifecycle_and_deadlines(self):
        payload = json.loads((EXAMPLES / "challenges.json").read_text(encoding="utf-8"))
        goals = {goal["id"]: goal for goal in payload["challenges"]}
        now = goals[1]["start_at"] + 1
        self.assertEqual((data._GoalAction.ACCEPT, data._GoalAction.DECLINE), data._actions(goals[4], "hill", now))
        self.assertEqual((), data._actions(goals[3], "hill", now))
        self.assertEqual((data._GoalAction.LEAVE,), data._actions(goals[2], "hill", now))
        self.assertEqual((data._GoalAction.CANCEL,), data._actions(goals[1], "hill", now))
        self.assertEqual((), data._actions(goals[1], "stranger", now))
        self.assertEqual((), data._actions(goals[4], "hill", goals[4]["end_at"]))
        self.assertEqual((), data._actions(dict(goals[1], status="future-status"), "hill", now))
        self.assertIsNotNone(data._suggestion(payload, now))
        self.assertIsNone(data._suggestion(payload, payload["weekly_suggestion"]["expires_at"]))


if __name__ == "__main__":
    unittest.main()
