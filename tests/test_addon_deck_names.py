"""Deck snapshots and choices stay local to the profile and AnkiQuest account."""

from types import SimpleNamespace
import unittest
from unittest.mock import Mock

from addon_loader import load_in_package

names = load_in_package("friendly_decks", "deck_names")


class DeckNames(unittest.TestCase):
    def setUp(self):
        self.api = SimpleNamespace(base="https://quest.test", user="hill")
        self.profile = {}
        self.card = SimpleNamespace(did=99, odid=5)
        self.col = SimpleNamespace(db=SimpleNamespace(scalar=Mock(return_value=1)),
                                   get_card=Mock(return_value=self.card),
                                   decks=SimpleNamespace(name=Mock(return_value="Spanish::Animals")))

    def test_the_first_noticed_home_deck_survives_archiving_and_moves(self):
        records = names._capture(self.col, [{"at": 50}], {})
        self.assertEqual({"50": {"id": "5", "path": "Spanish::Animals"}}, records)
        self.col.decks.name.assert_called_once_with(5)
        names._remember(self.profile, self.api, records)
        known = names._state(self.profile, self.api)["conquests"]
        self.col.decks.name.return_value = "Archive::Old"
        self.card.did, self.card.odid = 77, 0
        self.assertEqual({}, names._capture(self.col, [{"at": 50}], known))
        names._remember(self.profile, self.api, {"50": {"id": "77", "path": "Archive::Old"}})
        self.assertEqual("Animals", names._choice(self.profile, self.api, known["50"]))
        self.assertEqual("5", known["50"]["id"])

    def test_a_chosen_name_is_reused_by_deck_id_and_can_be_omitted(self):
        record = names._record(self.col, 50)
        names._choose(self.profile, self.api, record, "  My Spanish\nwords  ")
        renamed = dict(record, path="Archive::Renamed")
        self.assertEqual("My Spanish words", names._choice(self.profile, self.api, renamed))
        names._choose(self.profile, self.api, record, "")
        self.assertEqual("", names._choice(self.profile, self.api, renamed))
        other_deck = dict(record, id="6")
        self.assertEqual("Animals", names._choice(self.profile, self.api, other_deck))

    def test_names_and_snapshots_do_not_cross_profiles_servers_or_accounts(self):
        record = names._record(self.col, 50)
        names._choose(self.profile, self.api, record, "Mine")
        names._remember(self.profile, self.api, {"50": record})
        for profile, api in [({}, self.api), (self.profile, SimpleNamespace(base=self.api.base, user="other")),
                             (self.profile, SimpleNamespace(base="https://elsewhere.test", user=self.api.user))]:
            self.assertEqual("Animals", names._choice(profile, api, record))
            self.assertNotIn("50", names._state(profile, api).get("conquests", {}))

    def test_a_missing_or_deleted_card_has_no_invented_deck(self):
        self.col.db.scalar.return_value = None
        self.assertEqual({}, names._capture(self.col, [{"at": 50}], {}))
        self.assertEqual("", names._choice(self.profile, self.api, None))
        self.col.db.scalar.side_effect = RuntimeError("closed collection")
        self.assertEqual({}, names._capture(self.col, [{"at": 50}], {}))

    def test_quick_picks_remove_filing_parts_and_duplicate_names(self):
        self.assertEqual(["Spanish", "Animals"], names._parts("5 Archive::01. Spanish::Animals::Animals"))
        self.assertEqual([], names._parts("Default::Filtered"))

    def test_choices_are_plain_single_lines_and_have_a_length_limit(self):
        record = names._record(self.col, 50)
        names._choose(self.profile, self.api, record, "word " * 100)
        self.assertEqual(120, len(names._choice(self.profile, self.api, record)))


if __name__ == "__main__":
    unittest.main()
