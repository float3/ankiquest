"""The Today panel and its warning must use the same verdict as the goal meter."""

import time
import unittest

import test_desktop_today as fixture


class DayState(unittest.TestCase):
    setUp = fixture.HomeTests.setUp
    run_task = fixture.HomeTests.run_task
    write_config = fixture.HomeTests.write_config
    render = fixture.HomeTests.render
    review = fixture.HomeTests.review
    served_today = fixture.HomeTests.served_today
    remember = fixture.HomeTests.remember

    def pending_day(self):
        self.remember("cards", 100)
        self.served_today(streak_minutes=5, reviews=1, time_ms=240_000, minutes=4)
        self.addon.state["profile"].update(
            streak=7, at_risk=True, streak_warning={"title": "Your streak is at risk", "body": "Study today"},
        )
        self.tree = fixture.node(0, children=[fixture.node(5, "Spanish")])

    def test_unsent_reviews_update_the_pill_mood_and_warning_together(self):
        self.pending_day()
        self.review(int(time.time() * 1000) - 1, 60)
        html = self.render().stats
        self.assertIn("8 day streak", html)
        self.assertIn("Syncing", html)
        self.assertNotIn("streak at risk today", html)
        self.assertIn("aki_celebrate.png", html)
        self.addon.tooltip.reset_mock()
        self.addon.announce({}, self.addon.state["profile"], [], False)
        self.addon.tooltip.assert_not_called()

    def test_an_earlier_unsent_day_hides_the_streak_length(self):
        self.pending_day()
        starts = self.addon.state["profile"]["today"]["starts_at"]
        self.review(starts - 1, 60)
        html = self.render().stats
        self.assertNotIn("7 day streak", html)
        self.assertIn("Streak unknown", html)
        self.assertNotIn("streak at risk today", html)

    def test_a_freeze_is_named_and_expired_days_are_unknown(self):
        self.pending_day()
        self.addon.state["profile"].update(streak_state="protected", at_risk=False)
        self.assertIn("Protected by a freeze", self.render().stats)
        self.addon.state["profile"]["day_ends_at"] = int(time.time() * 1000) - 1
        html = self.render().stats
        self.assertNotIn("7 day streak", html)
        self.assertNotIn("Protected by a freeze", html)
        self.assertIn("Streak unknown", html)


if __name__ == "__main__":
    unittest.main()
