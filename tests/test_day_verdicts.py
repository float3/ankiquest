"""The add-on's verdict on today agrees with the server's golden cases."""

import json
import unittest
from pathlib import Path

from addon_loader import load_module

day_verdict = load_module("aq_day_verdict", "day_verdict.py")

FIXTURE = Path(__file__).resolve().parent / "fixtures" / "day_verdicts.json"


class DayVerdicts(unittest.TestCase):
    def test_every_server_case(self):
        cases = json.loads(FIXTURE.read_text("utf-8"))["cases"]
        self.assertTrue(cases)
        for case in cases:
            with self.subTest(case=case["name"]):
                found = day_verdict.verdict(case["server"], case["pending"], case["now_ms"])
                expected = case["expected"]
                if expected is None:
                    self.assertIsNone(found)
                    continue
                waiting = found.pop("waiting")
                self.assertEqual(expected, found)
                self.assertEqual(expected["met"] and case["server"]["streak_state"] != "studied", waiting)

    def test_an_older_server_without_day_bounds_counts_no_unsent_review(self):
        server = {
            "streak": 3, "streak_state": "pending", "day_ends_at": 2_000, "last_review_id": 100,
            "today": {"reviews": 1, "minutes": 4, "streak_minutes": 5},
        }
        found = day_verdict.verdict(server, [{"id": 500, "time_ms": 600_000}], 1_000)
        self.assertEqual((False, 240_000, 0, "pending", 3), (found["met"], found["time_ms"], found["unsent"], found["streak_state"], found["streak"]))
        reached = dict(server, today=dict(server["today"], minutes=5))
        self.assertTrue(day_verdict.verdict(reached, [], 1_000)["met"])

    def test_no_profile_says_nothing(self):
        self.assertIsNone(day_verdict.verdict(None, [], 1_000))

    def test_unsent_reads_what_the_uploader_sends(self):
        seen = []

        class Db:
            def all(self, sql, *args):
                seen.append((sql, args))
                return [(7, 1200), (8, None)]

        class Col:
            db = Db()

        self.assertEqual([{"id": 7, "time_ms": 1200}, {"id": 8, "time_ms": 0}], day_verdict.unsent(Col(), {"last_review_id": 5}))
        self.assertIn("ease > 0 and type < 4", seen[0][0])
        self.assertEqual((5,), seen[0][1])
        self.assertEqual([], day_verdict.unsent(Col(), {}))


if __name__ == "__main__":
    unittest.main()
