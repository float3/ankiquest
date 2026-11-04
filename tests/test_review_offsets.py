"""The journal of UTC offsets for reviews answered on this computer; run with python -m unittest discover -s tests."""

import unittest
from datetime import timedelta
from unittest.mock import patch

from addon_loader import load_module

offsets = load_module("ankiquest_review_offsets", "review_offsets.py")

NOW = 1_791_331_200_000
DAY = 86_400_000


class ReviewOffsetTests(unittest.TestCase):
    def test_the_offset_is_minutes_west_at_that_moment(self):
        for east, west in ((timedelta(hours=1), -60), (timedelta(hours=-3), 180), (timedelta(0), 0)):
            with patch.object(offsets, "datetime") as clock:
                clock.fromtimestamp.return_value.astimezone.return_value.utcoffset.return_value = east
                self.assertEqual(offsets.offset_at(NOW), west)
                clock.fromtimestamp.assert_called_once_with(NOW / 1000)
        self.assertIsInstance(offsets.offset_at(NOW), int)

    def test_only_a_fresh_answer_is_recorded(self):
        profile = {}
        offsets.record(profile, NOW - 1_000, NOW, offset=180)
        offsets.record(profile, NOW - DAY, NOW, offset=-60)
        offsets.record(profile, None, NOW, offset=-60)
        self.assertEqual(offsets.journal(profile), {NOW - 1_000: 180})
        offsets.record(None, NOW, NOW, offset=0)

    def test_stamps_go_only_on_journaled_rows(self):
        rows = [{"id": 1, "cid": 9}, {"id": 2, "cid": 9}]
        offsets.stamp(rows, {1: -60, 3: 0})
        self.assertEqual(rows, [{"id": 1, "cid": 9, "offset_west_min": -60}, {"id": 2, "cid": 9}])

    def test_uploaded_and_stale_entries_are_pruned(self):
        profile = {offsets.JOURNAL_KEY: {NOW - 40 * DAY: 0, NOW - 2_000: -60, NOW - 1_000: -60, NOW + 1: -60}}
        offsets.prune(profile, NOW - 1_000, NOW)
        self.assertEqual(profile[offsets.JOURNAL_KEY], {NOW + 1: -60})
        empty = {}
        offsets.prune(empty, NOW, NOW)
        self.assertEqual(empty, {}, "a profile without a journal gets none")

    def test_a_malformed_journal_reads_as_empty_or_partial(self):
        self.assertEqual(offsets.journal({offsets.JOURNAL_KEY: ["nope"]}), {})
        self.assertEqual(offsets.journal({offsets.JOURNAL_KEY: {"5": 60, "x": 1, 6: "y"}}), {5: 60})
        self.assertEqual(offsets.journal(None), {})


if __name__ == "__main__":
    unittest.main()
