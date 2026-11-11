"""The remaining Today details use the same cached account and day as the main panel."""

import time
import unittest
from types import SimpleNamespace
from unittest.mock import Mock

import test_desktop_today as fixture


class TodayDetails(unittest.TestCase):
    setUp = fixture.HomeTests.setUp
    run_task = fixture.HomeTests.run_task
    write_config = fixture.HomeTests.write_config
    render = fixture.HomeTests.render
    served_today = fixture.HomeTests.served_today

    def test_freeze_stock_survives_midnight_but_the_earned_cue_does_not(self):
        self.served_today(streak_minutes=5)
        profile = self.addon.state["profile"]
        profile.update(stored_freezes=2, freezes_enabled=True, freeze_earned_today=True)
        html = self.render().stats
        self.assertIn("2 freezes saved", html)
        self.assertIn("Freeze earned today", html)
        profile["day_ends_at"] = int(time.time() * 1000) - 1
        html = self.render().stats
        self.assertIn("2 freezes saved", html)
        self.assertNotIn("Freeze earned today", html)
        profile["freezes_enabled"] = False
        self.assertIn("Streak freezes off", self.render().stats)

    def test_the_weekly_recap_is_own_fresh_escaped_and_one_sentence_per_line(self):
        self.served_today()
        now = int(time.time())
        self.addon.state["inbox"] = [
            {"id": 1, "kind": "reminder_weekly_recap", "created_at": now - 2, "body": "12 reviews.\n<Nice week!>"},
            {"id": 2, "kind": "reminder_weekly_recap", "created_at": now - 1, "sender": "friend", "body": "Other player"},
        ]
        html = self.render().stats
        self.assertIn("Weekly recap", html)
        self.assertIn("<p>12 reviews.</p><p>&lt;Nice week!&gt;</p>", html)
        self.assertNotIn("Other player", html)
        self.addon.state["inbox"][0]["created_at"] = now - 8 * 86_400
        self.assertNotIn("Weekly recap", self.render().stats)

    def test_a_failed_first_poll_has_a_hint_and_does_not_loop_on_render(self):
        self.api.profile.side_effect = OSError("offline")
        self.addon.poll(quiet=True)
        for _ in range(3):
            html = self.render().stats
        self.assertIn("Could not refresh AnkiQuest", html)
        self.assertIn("Retry connection", html)
        self.api.profile.assert_called_once()
        self.api.profile.side_effect = None
        self.addon.on_js_message((False, None), "ankiquest:retry-connection", self.mw.deckBrowser)
        self.assertEqual(2, self.api.profile.call_count)
        self.assertNotIn("Could not refresh AnkiQuest", self.render().stats)

    def test_the_toolbar_uses_the_day_verdict_outside_the_deck_list(self):
        self.served_today()
        self.addon.state["profile"].update(streak=7, streak_state="studied")
        toolbar = SimpleNamespace(create_link=Mock(return_value="<a>AnkiQuest</a>"), web=SimpleNamespace(eval=Mock()))
        links = []
        self.home._toolbar_links(links, toolbar)
        self.assertEqual(["<a>AnkiQuest</a>"], links)
        self.assertIn("7", toolbar.create_link.call_args.args[1])
        self.assertIn("Secured", toolbar.create_link.call_args.args[1])
        self.mw.toolbar = toolbar
        self.mw.state = "overview"
        self.addon.state["profile"]["day_ends_at"] = int(time.time() * 1000) - 1
        self.addon.redraw()
        self.assertIn("Streak unknown", toolbar.web.eval.call_args.args[0])
        self.assertNotIn("7", toolbar.web.eval.call_args.args[0])

    def test_an_own_only_board_explains_how_to_add_friends(self):
        self.assertIn("Add friends to study together", self.render().stats)


if __name__ == "__main__":
    unittest.main()
