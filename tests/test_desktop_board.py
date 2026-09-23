"""Desktop deck list line, notifications and web window; run with python -m unittest discover -s tests."""

import importlib.util
import json
import re
import sys
import unittest
from pathlib import Path

ADDON = Path(__file__).resolve().parents[1] / "addon"


def load_module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


board = load_module("ankiquest_board", ADDON / "board.py")
notify = load_module("ankiquest_notify", ADDON / "notify.py")
tree = load_module("ankiquest_decks", ADDON / "decks.py")
web = load_module("ankiquest_web", ADDON / "web.py")
client = load_module("ankiquest_client_shared", ADDON / "client.py")
progress = load_module("ankiquest_progress_shared", ADDON / "deck_completion.py")


FIXTURES = json.loads((Path(__file__).resolve().parent / "fixtures" / "clients.json").read_text("utf-8"))


class SummaryTests(unittest.TestCase):
    def test_the_deck_list_line_shows_place_level_streak_and_links(self):
        html = board.html({"level": 4, "xp_into_level": 1200, "xp_for_next": 2000, "streak": 3}, 2, 0)
        self.assertIn("#2 this week", html)
        self.assertIn("Lv 4  1,200/2,000 XP", html)
        self.assertIn("\U0001f525 3 day streak", html)
        self.assertIn("pycmd('ankiquest:web:board')", html)
        self.assertNotIn("ankiquest:web:inbox", html)

    def test_a_waiting_inbox_and_a_risky_streak_are_visible(self):
        html = board.html({"level": 1, "streak": 5, "at_risk": True}, None, 2)
        self.assertIn("streak at risk today", html)
        self.assertIn("pycmd('ankiquest:web:inbox')", html)
        self.assertIn("2 new", html)
        self.assertNotIn("this week", html)

    def test_every_link_opens_a_known_page(self):
        html = board.html({}, 1, 1)
        for page in re.findall(r"ankiquest:web:(\w+)", html):
            self.assertIn(page, board.PAGES)


class FeedbackTests(unittest.TestCase):
    def test_headlines_always_show_and_carry_the_status(self):
        response = {"feedback": {"headlines": ["Level 5!"], "status": "+10 XP"}}
        self.assertEqual(("Level 5!<br>+10 XP", True), notify.feedback(response, False))

    def test_a_plain_status_only_shows_while_reviewing(self):
        response = {"feedback": {"headlines": [], "status": "+10 XP"}}
        self.assertEqual(("+10 XP", False), notify.feedback(response, True))
        self.assertIsNone(notify.feedback(response, False))

    def test_older_servers_and_quiet_uploads_say_nothing(self):
        self.assertIsNone(notify.feedback({}, True))
        self.assertIsNone(notify.feedback({"feedback": {"headlines": [], "status": None}}, True))


class NotifyTests(unittest.TestCase):
    def test_the_streak_warning_waits_for_the_last_hours_and_speaks_once(self):
        ends = 100 * notify.HOUR_MS
        profile = {
            "streak_warning": {"title": "\U0001f525 Your 7 day streak ends in 1h", "body": "No rush."},
            "day_ends_at": ends,
        }
        self.assertIsNone(notify.streak_warning(profile, 2, ends - 3 * notify.HOUR_MS, None))
        text, marked = notify.streak_warning(profile, 2, ends - notify.HOUR_MS, None)
        self.assertEqual("\U0001f525 Your 7 day streak ends in 1h<br>No rush.", text)
        self.assertEqual(ends, marked)
        self.assertIsNone(notify.streak_warning(profile, 2, ends - notify.HOUR_MS, marked))
        self.assertIsNone(notify.streak_warning(profile, 0, ends - notify.HOUR_MS, None))
        self.assertIsNone(notify.streak_warning({"day_ends_at": ends}, 2, ends - 1, None))

    def test_only_recent_arrivals_are_announced_but_all_are_consumed(self):
        now = 1_700_000_000
        inbox = [
            {"id": 1, "created_at": now - 90_000, "title": "Deck complete", "body": "old"},
            {"id": 3, "created_at": now - 30, "title": "Deck complete", "body": "new"},
            {"id": 2, "created_at": now - 60, "title": "Deck complete", "body": "also new"},
        ]
        fresh, cursor = notify.fresh_messages(inbox, 0, now)
        self.assertEqual(["also new", "new"], [entry["body"] for entry in fresh])
        self.assertEqual(3, cursor)
        self.assertEqual(([], 3), notify.fresh_messages(inbox, cursor, now))

    def test_a_message_can_be_answered_until_it_has_been(self):
        self.assertTrue(notify.answerable({"sender": "cerro"}))
        self.assertFalse(notify.answerable({"sender": "cerro", "replied": True}))
        self.assertFalse(notify.answerable({"sender": ""}))


class WebTests(unittest.TestCase):
    base = "https://anki.example.com"

    def test_only_the_servers_own_pages_are_signed_in(self):
        for url in ("/", "/week", "/community#activity", "/#hill", "/community?period=week"):
            self.assertTrue(web.allowed(self.base, self.base + url), url)
        for url in (
            "https://evil.example.com/week",
            "http://anki.example.com/week",
            "https://anki.example.com:8443/week",
            "https://user:pw@anki.example.com/week",
            "https://anki.example.com/api/profile/hill",
            "https://anki.example.com/login",
        ):
            self.assertFalse(web.allowed(self.base, url), url)

    def test_a_server_below_a_path_keeps_its_prefix(self):
        base = "https://example.com/anki"
        self.assertTrue(web.allowed(base, "https://example.com/anki/community#activity"))
        self.assertFalse(web.allowed(base, "https://example.com/community"))
        self.assertEqual("https://example.com/anki/week", web.page_url(base + "/", "/week"))

    def test_the_session_script_checks_the_origin_and_escapes_the_token(self):
        script = web.session_script(self.base, "hill", 'to"ken')
        self.assertIn('location.origin !== "https://anki.example.com"', script)
        self.assertIn('"token": "to\\"ken"', script)
        self.assertIn("ankiquest-auth", script)
        self.assertIn("window.ankiquestSession = null", web.session_script(self.base, "hill", ""))


class SharedFixtureTests(unittest.TestCase):
    """tests/fixtures/clients.json is checked by the server and the Android app too."""

    def test_study_day(self):
        for case in FIXTURES["study_day"]:
            self.assertEqual(
                case["day"],
                progress.study_day(case["now_ms"], case["offset_west_min"], case["rollover_hour"]),
                case,
            )

    def test_reconcile(self):
        for case in FIXTURES["reconcile"]:
            rows = [(i, 0, 0, 0, 0) for i in case["window"]]
            present, deleted, restored = client.reconcile(rows, set(case["recent"]), case["known"], case["mark"])
            self.assertEqual(set(case["window"]), present)
            self.assertEqual(case["deleted"], deleted, case)
            self.assertEqual(case["restored"], [row[0] for row in restored], case)

    def test_review_row(self):
        for case in FIXTURES["review_row"]:
            self.assertEqual([case["review"]], client.rows_to_reviews([tuple(case["row"])]))


class DeckTreeTests(unittest.TestCase):
    def decks(self, *names):
        return tree.ordered([{"id": str(i + 1), "name": name} for i, name in enumerate(names)])

    def test_a_subdeck_follows_its_parent_indented_by_its_depth(self):
        decks = self.decks("Spanish::Verbs::Irregular", "German", "Spanish", "Spanish::Nouns")
        self.assertEqual(
            ["German", "Spanish", "Spanish::Nouns", "Spanish::Verbs::Irregular"],
            [deck["name"] for deck in decks],
        )
        self.assertEqual([0, 0, 1, 2], [tree.depth(deck) for deck in decks])
        self.assertEqual(["German", "Spanish", "Nouns", "Irregular"], [tree.label(deck) for deck in decks])

    def test_a_deck_that_merely_starts_with_the_same_letters_is_not_a_subdeck(self):
        decks = self.decks("Spanish", "Spanish::Verbs", "Spanishly", "Spanish Extra")
        spanish = [deck["name"] for deck in decks].index("Spanish")
        self.assertEqual(["Spanish::Verbs"], [decks[i]["name"] for i in tree.descendants(decks, spanish)])
        self.assertEqual([spanish] + tree.descendants(decks, spanish), tree.branch(decks, spanish))
        last = len(decks) - 1
        self.assertEqual([], tree.descendants(decks, last))

    def test_sorting_ignores_case_but_keeps_families_together(self):
        decks = self.decks("spanish::verbs", "Spanish", "SPANISH::nouns")
        self.assertEqual(["Spanish", "SPANISH::nouns", "spanish::verbs"], [deck["name"] for deck in decks])


if __name__ == "__main__":
    unittest.main()
