"""Activity contracts preserve global counts, fallback rules and notification ownership."""
from http import HTTPStatus
import json
from pathlib import Path
from types import SimpleNamespace
import unittest
from unittest.mock import Mock

from addon_loader import load_module
import test_deck_completion as fixture

activity = load_module("activity_rules", "activity.py")
EXAMPLES = Path(__file__).resolve().parents[1] / "docs/api/examples"


class Refused(Exception):
    def __init__(self, status):
        self.status = status


class Pages(unittest.TestCase):
    def test_recorded_pages_keep_global_counts_and_replace_duplicate_rows(self):
        first = activity._parse(json.loads((EXAMPLES / "activity.json").read_text(encoding="utf-8")))
        older = activity._parse(json.loads((EXAMPLES / "activity_older.json").read_text(encoding="utf-8")))
        self.assertGreater(first.unread, len(first.items))
        self.assertGreater(first.actions, 0)
        self.assertTrue(first.attention)
        self.assertEqual(first.unread, older.unread)
        combined = activity._append(first, older)
        self.assertEqual(4, len(combined.items))
        self.assertEqual(combined, activity._append(combined, older))
        self.assertEqual(older.before, combined.before)

    def test_only_first_page_404_uses_legacy_and_never_infers_unread(self):
        api = SimpleNamespace(user="hill", _request=Mock(side_effect=Refused(HTTPStatus.NOT_FOUND)),
                              notifications=Mock(return_value=[{"id": 7}, {"id": 8}]))
        page = activity._load(api)
        self.assertEqual([8, 7], [row["id"] for row in page.items])
        self.assertEqual((0, 0, None), (page.unread, page.actions, page.before))
        self.assertIs(activity._Source.LEGACY, page.source)
        api.notifications.assert_called_once()
        for code in (HTTPStatus.UNAUTHORIZED, HTTPStatus.FORBIDDEN, HTTPStatus.INTERNAL_SERVER_ERROR):
            api._request.side_effect = Refused(code)
            with self.assertRaises(Refused):
                activity._load(api)
        api._request.side_effect = Refused(HTTPStatus.NOT_FOUND)
        with self.assertRaises(Refused):
            activity._load(api, before=7)
        api.notifications.assert_called_once()

    def test_an_unsupported_filter_retries_unfiltered_not_legacy(self):
        api = SimpleNamespace(user="hill", notifications=Mock(), _request=Mock(side_effect=[
            Refused(HTTPStatus.BAD_REQUEST), {"items": [], "unread_count": 9, "action_count": 2},
        ]))
        page = activity._load(api, activity._Category.MESSAGES, activity._Read.UNREAD, 20)
        self.assertIs(activity._Source.UNFILTERED, page.source)
        self.assertEqual("/api/activity/hill?limit=50&category=messages&unread_only=true&before=20", api._request.call_args_list[0].args[0])
        self.assertEqual("/api/activity/hill?limit=50&before=20", api._request.call_args_list[1].args[0])
        api.notifications.assert_not_called()


class Poll(unittest.TestCase):
    setUp = fixture.RefreshTests.setUp
    with_board = fixture.RefreshTests.with_board
    run_task = fixture.RefreshTests.run_task
    read_rows = fixture.RefreshTests.read_rows
    write_config = fixture.RefreshTests.write_config

    def test_badges_use_server_counts_even_when_no_rows_are_loaded(self):
        self.with_board()
        self.api._request = Mock(return_value={"items": [], "unread_count": 23, "action_count": 5})
        self.addon.poll(quiet=True)
        content = SimpleNamespace(stats="")
        self.addon.on_deck_browser(None, content)
        self.assertIn("23 new", content.stats)
        self.assertIn("5 awaiting action", content.stats)
        self.api.notifications.assert_not_called()

    def test_a_read_invitation_still_has_its_action_badge(self):
        self.with_board()
        self.api._request = Mock(return_value={"items": [], "unread_count": 0, "action_count": 1})
        self.addon.poll(quiet=True)
        content = SimpleNamespace(stats="")
        self.addon.on_deck_browser(None, content)
        self.assertIn("Inbox · 1 awaiting action", content.stats)

    def test_an_already_seen_own_moment_is_not_replayed_but_a_friends_cheer_is(self):
        self.with_board()
        self.api.deck_settings = Mock(return_value={"celebrations": True})
        self.addon.poll()
        self.addon.tooltip.reset_mock()
        profile, account = self.mw.pm.profile, (self.api.base, self.api.user)
        self.addon.moments._acknowledge(profile, account, ["level:3"])
        self.inbox.extend([
            {"id": 5, "kind": "celebration", "moment": "level:3", "topic": "level", "title": "Level 3!", "created_at": int(__import__("time").time())},
            {"id": 6, "kind": "reaction", "sender": "friend", "moment": "level:3", "title": "Cheered!", "created_at": int(__import__("time").time())},
        ])
        self.addon.poll()
        self.assertEqual([], self.addon.moments._pending(profile, account))
        self.assertTrue(any("Cheered!" in call.args[0] for call in self.addon.tooltip.call_args_list))
