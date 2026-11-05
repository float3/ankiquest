"""Moving a desktop player from an older address of their server to its main address."""

import unittest

from addon_loader import load_module

address = load_module("aq_address", "address.py")

OLD = "https://old.example.test"
MAIN = "https://anki.example.test"


class MainAddressTests(unittest.TestCase):
    def test_only_an_https_address_is_a_main_address(self):
        self.assertEqual(address.main_address({"public_url": " https://anki.example.test/ "}), MAIN)
        for url in (None, 3, "", "http://anki.example.test", "anki.example.test", "https://", "https://a.test/?x=1"):
            self.assertIsNone(address.main_address({"public_url": url}), url)
        self.assertIsNone(address.main_address(None))

    def test_a_player_already_at_the_main_address_stays(self):
        self.assertFalse(address.should_move(MAIN, MAIN))
        self.assertFalse(address.should_move(MAIN + "/", MAIN))
        self.assertFalse(address.should_move(OLD, None))
        self.assertFalse(address.should_move("", MAIN))
        self.assertTrue(address.should_move(OLD, MAIN))

    def test_records_for_the_account_move_with_it(self):
        profile = {
            "ankiquestBaselineAccount": [OLD, "ana"],
            "ankiquestServerGoal": {"account": [OLD, "ana"], "day": 3},
            "ankiquestLastStudySession": {"account": [OLD, "ana"], "minutes": 5},
            "ankiquestUploadedThrough": 1234,
            "ankiquestReviewOffsets": [[1, 60], [2, 0]],
            "ankiquestOther": [["https://elsewhere.test", "bo"]],
            "notOurs": [OLD, "ana"],
        }
        address.move_profile(profile, OLD, MAIN)
        self.assertEqual(profile["ankiquestBaselineAccount"], [MAIN, "ana"])
        self.assertEqual(profile["ankiquestServerGoal"], {"account": [MAIN, "ana"], "day": 3})
        self.assertEqual(profile["ankiquestLastStudySession"]["account"], [MAIN, "ana"])
        self.assertEqual(profile["ankiquestUploadedThrough"], 1234)
        self.assertEqual(profile["ankiquestReviewOffsets"], [[1, 60], [2, 0]])
        self.assertEqual(profile["ankiquestOther"], [["https://elsewhere.test", "bo"]])
        self.assertEqual(profile["notOurs"], [OLD, "ana"])

    def test_shown_goal_prompts_move_with_it(self):
        self.assertEqual(
            address.move_shown([OLD + " ana", "https://elsewhere.test bo"], OLD, MAIN),
            [MAIN + " ana", "https://elsewhere.test bo"],
        )
        self.assertEqual(address.move_shown(None, OLD, MAIN), [])


if __name__ == "__main__":
    unittest.main()
