"""The desktop heatmap follows the rules the Android app shares in fixtures/review_heatmap.json."""

import json
import unittest
from datetime import date, datetime, time, timedelta, timezone
from pathlib import Path
from zoneinfo import ZoneInfo

from addon_loader import load_module

heatmap = load_module("aq_heatmap", "heatmap.py")

FIXTURE = Path(__file__).resolve().parent / "fixtures" / "review_heatmap.json"


def days(values):
    return {date.fromisoformat(day): count for day, count in values.items()}


def isos(values):
    return {day.isoformat(): count for day, count in values.items()}


class SharedCases(unittest.TestCase):
    def test_every_case(self):
        fixture = json.loads(FIXTURE.read_text("utf-8"))
        zone = ZoneInfo(fixture["zone"])
        self.assertTrue(fixture["cases"])
        for case in fixture["cases"]:
            with self.subTest(case=case["name"]):
                today = date.fromisoformat(case["today"])
                data = heatmap.summarize(today, days(case["reviews"]), days(case["scheduled"]))
                if "profile" in case:
                    data = heatmap.with_server_days(data, heatmap.server_days(case["profile"]), zone)

                expected = case["expected"]
                first = data.first_review_date.isoformat() if data.first_review_date else None
                verdicts = {
                    day.isoformat(): {"state": v.state, "minutes": v.minutes, "streak_minutes": v.streak_minutes}
                    for day, v in data.verdicts.items()
                }
                self.assertEqual(expected["reviews"], isos(data.reviews))
                self.assertEqual(expected["scheduled"], isos(data.scheduled))
                self.assertEqual(expected["current_streak"], data.current_streak)
                self.assertEqual(expected["longest_streak"], data.longest_streak)
                self.assertEqual(expected["total_reviews"], data.total_reviews)
                self.assertEqual(expected["days_studied"], data.days_studied)
                self.assertEqual(expected["first_review_date"], first)
                self.assertAlmostEqual(expected["daily_average"], data.daily_average)
                self.assertEqual(expected["protected_days"], sorted(day.isoformat() for day in data.protected))
                self.assertEqual(expected["verdicts"], verdicts)
                for day, search in expected.get("searches", {}).items():
                    self.assertEqual(search, heatmap.search_for_day(data, date.fromisoformat(day)), day)


class Levels(unittest.TestCase):
    def test_levels_follow_the_daily_average(self):
        # Half the average, the average, twice it, more.
        found = [heatmap.intensity(count, 10.0) for count in (0, 5, 6, 10, 20, 21)]
        self.assertEqual([0, 1, 2, 2, 3, 4], found)

    def test_a_tiny_average_counts_as_one(self):
        self.assertEqual([0, 2, 3, 4], [heatmap.intensity(count, 0.2) for count in (0, 1, 2, 3)])


class StudyDate(unittest.TestCase):
    def test_follows_the_rollover_through_dst_and_midnight(self):
        berlin = ZoneInfo("Europe/Berlin")
        for day in (date(2024, 3, 31), date(2024, 10, 27)):
            for rollover in (0, 4, 23):
                cutoff = datetime.combine(day + timedelta(days=1), time(rollover), berlin).timestamp()
                self.assertEqual(day, heatmap.study_date(int(cutoff), berlin))

    def test_before_the_rollover_is_still_yesterday(self):
        los_angeles = ZoneInfo("America/Los_Angeles")
        cutoff = datetime(2024, 1, 1, 4, tzinfo=los_angeles).timestamp()
        self.assertEqual(date(2023, 12, 31), heatmap.study_date(int(cutoff), los_angeles))


class Load(unittest.TestCase):
    def test_numbers_days_back_from_the_cutoff(self):
        cutoff = int(datetime(2024, 3, 2, 4, tzinfo=timezone.utc).timestamp())

        class Sched:
            day_cutoff = cutoff
            today = 700

        class Db:
            def all(self, sql, *args):
                if "revlog" in sql:
                    self.reviews = args
                    return [(0, 4), (2, 6)]
                self.forecast = args
                return [(1, 3)]

        class Col:
            sched = Sched()
            db = Db()

        data = heatmap.load(Col())
        self.assertEqual((cutoff * 1000, cutoff * 1000), Col.db.reviews)
        self.assertEqual((700, cutoff, heatmap.FORECAST_DAYS), Col.db.forecast)
        self.assertEqual(data.today, heatmap.study_date(cutoff))
        self.assertEqual({0: 4, -2: 6}, {(day - data.today).days: n for day, n in data.reviews.items()})
        self.assertEqual({1: 3}, {(day - data.today).days: n for day, n in data.scheduled.items()})


if __name__ == "__main__":
    unittest.main()
