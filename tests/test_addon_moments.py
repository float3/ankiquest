"""A moment survives queuing, account changes and a restart until it is dismissed."""

import json
import unittest

from addon_loader import load_in_package

moments = load_in_package("moments_rules", "moments")
address = load_in_package("moments_rules", "address")


def notice(number=1, **values):
    return {
        "id": number, "kind": "celebration", "moment": "level:4", "topic": "level",
        "title": "Level 4", "body": "You reached level 4", "created_at": 1000,
        "share_title": "My level", "share_body": "I reached level 4", **values,
    }


class MomentQueue(unittest.TestCase):
    def setUp(self):
        self.profile = {}
        self.account = ["https://example.test", "sam"]

    def offer(self, *entries):
        moments._offer(self.profile, self.account, [moments._notice(entry) for entry in entries])

    def pending(self):
        return moments._pending(self.profile, self.account)

    def test_every_win_survives_a_restart_and_duplicates_use_the_moment_key(self):
        self.offer(notice(1), notice(2, moment="goal:2", topic="goal"), notice(3))
        self.profile = json.loads(json.dumps(self.profile))
        self.assertEqual(["level:4", "goal:2"], [item["key"] for item in self.pending()])
        moments._acknowledge(self.profile, self.account, ["level:4"])
        self.offer(notice(4))
        self.assertEqual(["goal:2"], [item["key"] for item in self.pending()])

    def test_a_cheer_or_friend_share_never_consumes_an_own_moment(self):
        for kind in ("reaction", "share"):
            self.assertIsNone(moments._notice(notice(kind=kind, sender="friend")))
        self.offer(notice())
        self.assertEqual(1, len(self.pending()))

    def test_accounts_and_profiles_keep_separate_memories(self):
        self.offer(notice())
        moments._acknowledge(self.profile, self.account, ["level:4"])
        other = [self.account[0], "other"]
        moments._offer(self.profile, other, [moments._notice(notice())])
        self.assertEqual(1, len(moments._pending(self.profile, other)))
        self.assertEqual([], self.pending())
        fresh = {}
        moments._offer(fresh, self.account, [moments._notice(notice())])
        self.assertEqual(1, len(moments._pending(fresh, self.account)))

    def test_the_main_address_move_keeps_the_queue_and_seen_keys(self):
        self.offer(notice(), notice(2, moment="level:5"))
        moments._acknowledge(self.profile, self.account, ["level:4"])
        address.move_profile(self.profile, self.account[0], "https://main.test")
        self.account[0] = "https://main.test"
        self.offer(notice())
        self.assertEqual(["level:5"], [item["key"] for item in self.pending()])

    def test_priority_and_newest_first_match_android(self):
        self.offer(
            notice(1, moment="conquered:1", topic="conquered"),
            notice(2, moment="level:5", created_at=1001), notice(3),
            notice(4, moment="secured:3", topic="secured"),
        )
        self.assertEqual(["secured:3", "level:5", "level:4", "conquered:1"], [item["key"] for item in self.pending()])

    def test_quiet_rows_are_badged_but_the_daily_minimum_uses_the_card(self):
        self.offer(notice(1, quiet=True), notice(2, moment="secured:3", topic="secured", quiet=True))
        self.offer(notice(3, quiet=True))
        self.assertEqual(1, moments._quiet_count(self.profile, self.account))
        self.assertEqual(["secured:3"], [item["key"] for item in self.pending()])
        moments._clear_quiet(self.profile, self.account)
        self.assertEqual(0, moments._quiet_count(self.profile, self.account))

    def test_a_server_row_enriches_a_queued_local_conquest_without_replacing_it(self):
        local = {"key": "conquered:50", "topic": "conquered", "title": "My card", "body": "Local", "at": 50}
        moments._offer(self.profile, self.account, [local])
        self.offer(notice(moment="conquered:50", topic="conquered", win=12))
        self.assertEqual(1, len(self.pending()))
        self.assertEqual("My card", self.pending()[0]["title"])
        self.assertEqual(12, self.pending()[0]["share"]["win"])
        self.assertNotIn("share", local)

    def test_sharing_keeps_the_servers_first_person_words(self):
        self.offer(notice())
        self.assertEqual("My level", self.pending()[0]["share"]["title"])
        self.assertEqual("I reached level 4", self.pending()[0]["share"]["body"])

    def test_the_topic_selects_art_and_none_has_no_mascot(self):
        for topic, expected in (("secured", "streak"), ("level", "winner"), ("goal", "celebrate")):
            item = moments._notice(notice(topic=topic))
            self.assertEqual("aki_" + expected, moments._pose(item, "aki"))
            self.assertTrue(moments._pose(item, "ankilope").startswith("ankilope_"))
            self.assertIsNone(moments._pose(item, "none"))
