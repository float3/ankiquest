"""The desktop inbox, replies and sharing a win; run with python -m unittest discover -s tests."""

import ast
import importlib.util
import sys
import time
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock

import test_deck_completion as base

ADDON = Path(__file__).resolve().parents[1] / "addon"


def load(name):
    spec = importlib.util.spec_from_file_location("aq_inbox_" + name, ADDON / (name + ".py"))
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


inbox = load("inbox")
share = load("share")
language = load("language")


class Refused(Exception):
    def __init__(self, status, message=""):
        super().__init__(message or "HTTP Error %d" % status)
        self.status, self.message = status, message


class FakeApi:
    """Answers like client.Client._request: a GET without a body, a POST with one."""

    user = "hill"
    base = "https://quest.example"
    token = "token"

    def __init__(self, answers):
        self.answers, self.sent = answers, []

    def _request(self, path, body=None):
        self.sent.append((path, body))
        answer = self.answers[path]
        if isinstance(answer, Exception):
            raise answer
        return answer


class ReplyTextTests(unittest.TestCase):
    def test_control_characters_become_spaces_and_the_ends_are_trimmed(self):
        self.assertEqual("one two\u00a0three", inbox.clean_reply("\n one\ntwo\u00a0three\t\x85"))
        self.assertEqual("a b", inbox.clean_reply("a\x7fb"))
        self.assertEqual("", inbox.clean_reply(None))

    def test_a_reply_is_cut_to_200_utf8_bytes_without_splitting_a_character(self):
        self.assertEqual("x" * 200, inbox.clean_reply("x" * 250))
        cut = inbox.clean_reply("ñ" * 150)
        self.assertEqual(100, len(cut))
        self.assertLessEqual(len(cut.encode()), 200)
        flames = inbox.clean_reply("x" + "\U0001f525" * 60)
        self.assertEqual(197, len(flames.encode()))
        self.assertEqual("x" * 199, inbox.clean_reply("x" * 199 + " y"))

    def test_quick_picks_follow_the_kind_like_the_phone(self):
        self.assertEqual(["Good job!", "Nice one \U0001f525", "Keep it up!"], inbox.reply_choices("completion"))
        self.assertEqual(inbox.reply_choices("completion"), inbox.reply_choices("share"))
        self.assertEqual(inbox.reply_choices("completion"), inbox.reply_choices(None))
        self.assertEqual(["Thanks!", "You made my day!", "I appreciate it!"], inbox.reply_choices("reply"))
        self.assertEqual(["On it!", "Starting now", "I will study later"], inbox.reply_choices("nudge"))
        self.assertEqual([], inbox.reply_choices("message"))

    def test_quick_picks_are_translated(self):
        spanish = lambda value: language.tr(value, "es")
        self.assertEqual(["¡Voy!", "Empiezo ahora", "Estudiaré más tarde"], inbox.reply_choices("nudge", spanish))


class ActionTests(unittest.TestCase):
    def test_a_friends_message_can_be_answered_until_it_has_been(self):
        entry = {"id": 4, "sender": "cerro", "kind": "completion"}
        self.assertEqual(["reply", "cheer", "open", "read"], inbox.actions(entry))
        self.assertEqual(["cheer", "open", "read"], inbox.actions(dict(entry, replied=True)))
        self.assertEqual(["reply", "cheer", "open"], inbox.actions(dict(entry, read_at=1790000000)))

    def test_challenge_rows_are_answered_on_the_website(self):
        invite = {"id": 5, "sender": "alice", "kind": "challenge_invite", "challenge_id": 8, "route": "/community#challenge-8"}
        self.assertEqual(["open", "read"], inbox.actions(invite))
        self.assertEqual("/community#challenge-8", inbox.web_path(invite))
        self.assertEqual("/community#challenge-3", inbox.web_path({"route": "/community#challenge-3"}))
        self.assertEqual(inbox.ACTIVITY_PATH, inbox.web_path({"route": "/community#challenge-0"}))
        self.assertEqual(inbox.ACTIVITY_PATH, inbox.web_path({"challenge_id": True, "route": "https://evil.example"}))

    def test_only_the_players_own_wins_are_shareable(self):
        for kind in ("celebration", "challenge_complete", "reminder_weekly_recap"):
            self.assertEqual(["share", "open"], inbox.actions({"id": 9, "kind": kind, "sender": "", "read_at": 1}), kind)
        self.assertFalse(inbox.shareable({"id": 9, "kind": "celebration", "sender": "cerro"}))
        self.assertFalse(inbox.shareable({"id": 9, "kind": "completion"}))
        self.assertFalse(inbox.shareable({"kind": "celebration"}))

    def test_legacy_rows_have_no_read_state(self):
        self.assertEqual(["open"], inbox.actions({"id": 1, "kind": "message"}, legacy=True))
        self.assertFalse(inbox.unread({"id": 1}, legacy=True))
        self.assertTrue(inbox.unread({"id": 1, "read_at": None}))

    def test_times_are_local_and_missing_ones_are_blank(self):
        self.assertRegex(inbox.when(1790000000), r"^\d{4}-\d\d-\d\d \d\d:\d\d$")
        self.assertEqual("", inbox.when(None))
        self.assertEqual("", inbox.when("soon"))


class ServerTests(unittest.TestCase):
    def test_the_inbox_comes_from_activity_and_falls_back_to_the_legacy_list(self):
        api = FakeApi({"/api/activity/hill": {"items": [{"id": 3}, {"id": 2}], "unread_count": 1}})
        self.assertEqual(([{"id": 3}, {"id": 2}], False), inbox.load(api))
        old = FakeApi({"/api/activity/hill": Refused(404), "/api/notifications/hill": [{"id": 1}, {"id": 2}]})
        self.assertEqual(([{"id": 2}, {"id": 1}], True), inbox.load(old))
        broken = FakeApi({"/api/activity/hill": Refused(500)})
        with self.assertRaises(Refused):
            inbox.load(broken)

    def test_marking_read_is_batched_and_an_empty_204_is_success(self):
        self.assertEqual([{"ids": [1, 2, 3]}], inbox.read_bodies([3, 1, 2, 2, 0, -4]))
        self.assertEqual([200, 200, 1], [len(body["ids"]) for body in inbox.read_bodies(range(1, 402))])
        api = FakeApi({"/api/activity/hill/read": ValueError("Expecting value")})
        inbox.mark_read(api, [42])
        inbox.mark_all_read(api, 50)
        self.assertEqual([("/api/activity/hill/read", {"ids": [42]}), ("/api/activity/hill/read", {"through": 50})], api.sent)
        refused = FakeApi({"/api/activity/hill/read": Refused(401)})
        with self.assertRaises(Refused):
            inbox.mark_read(refused, [42])

    def test_a_reply_sends_the_cleaned_text_and_answers_who_got_it(self):
        api = FakeApi({"/api/reply/hill": {"sent_to": "Cerro"}})
        self.assertEqual("Cerro", inbox.reply(api, 42, " Nice\none "))
        self.assertEqual([("/api/reply/hill", {"notification": 42, "message": "Nice one"})], api.sent)
        with self.assertRaises(ValueError):
            inbox.reply(api, 42, "\n\t")

    def test_errors_quote_the_server_when_it_said_something(self):
        self.assertEqual("That username is taken.", inbox.error_text(Refused(409, "That username is taken.")))
        self.assertEqual("HTTP 404", inbox.error_text(Refused(404)))
        self.assertEqual("timed out", inbox.error_text(OSError("timed out")))


class CheerTests(unittest.TestCase):
    def test_friends_shared_wins_and_finished_decks_can_be_cheered(self):
        self.assertTrue(inbox.cheerable({"id": 8, "kind": "share", "sender": "cerro"}))
        self.assertTrue(inbox.cheerable({"id": 8, "kind": "completion", "sender": "cerro"}))
        self.assertFalse(inbox.cheerable({"id": 8, "kind": "share", "sender": ""}))
        self.assertFalse(inbox.cheerable({"id": 8, "kind": "reply", "sender": "cerro"}))
        self.assertFalse(inbox.cheerable({"id": 0, "kind": "share", "sender": "cerro"}))
        self.assertIn("cheer", inbox.actions({"id": 8, "kind": "share", "sender": "cerro", "reacted": True}))

    def test_a_cheer_is_sent_once_and_says_whether_it_was_already(self):
        api = FakeApi({"/api/react/hill": {"already": False}})
        self.assertFalse(inbox.react(api, 42))
        self.assertEqual([("/api/react/hill", {"notification": 42})], api.sent)
        self.assertTrue(inbox.react(FakeApi({"/api/react/hill": {"already": True}}), 42))

    def test_a_cheer_back_reads_as_one_line_and_can_be_answered(self):
        row = {"id": 9, "kind": "reaction", "sender": "cerro", "title": "🎉 Cerro cheered", "body": "“Level 5!”"}
        self.assertEqual(("🎉 Cerro cheered: “Level 5!”", ""), inbox.headline(row))
        self.assertEqual(("Deck done", "Spanish"), inbox.headline({"title": "Deck done", "body": "Spanish"}))
        self.assertEqual(["reply", "open", "read"], inbox.actions(row))
        self.assertEqual(inbox.reply_choices("reply"), inbox.reply_choices("reaction"))


class Response:
    def __init__(self, data):
        self.data = data

    def read(self, size):
        return self.data[:size]

    def __enter__(self):
        return self

    def __exit__(self, *_):
        return False


PNG = b"\x89PNG\r\n\x1a\n" + b"pixels"


class ShareTests(unittest.TestCase):
    def test_wins_are_shared_by_id_and_older_rows_by_notification_or_place(self):
        win = share.win_from_entry({"id": 42, "title": "Level 5!", "body": "Well done", "kind": "celebration", "win": 17})
        self.assertEqual({"win": 17, "to": "friends"}, share.request_body(win))
        self.assertTrue(share.has_card(win))
        old = share.win_from_entry({"id": 42, "title": "3 new unlocks", "body": "", "kind": "celebration"})
        self.assertEqual({"notification": 42}, share.request_body(old))
        self.assertFalse(share.has_card(old))
        self.assertFalse(share.has_card({"win": True}))
        self.assertEqual({"place": 2}, share.request_body(share.win_from_place({"title": "#2", "body": "Up one"}, 2)))
        for place in (0, 4, None, "1"):
            with self.assertRaises(ValueError):
                share.request_body({"notification": None, "place": place})

    def test_sharing_answers_how_many_friends_got_it(self):
        api = FakeApi({"/api/share/hill": {"recipients": 3, "link": {"url": "/w/t", "image": "/w/t.png"}}})
        self.assertEqual(3, share.send(api, {"notification": 7, "place": None, "win": 17}))
        self.assertEqual([("/api/share/hill", {"win": 17, "to": "friends"})], api.sent)
        self.assertEqual(0, share.send(FakeApi({"/api/share/hill": {}}), {"notification": None, "place": 1}))

    def test_a_link_comes_with_the_servers_picture_and_words(self):
        api = FakeApi({"/api/share/hill": {"url": "/w/tok", "image": "https://cdn.example/w/tok.png", "text": "Level 5! Join me"}})
        self.assertEqual(
            {"url": "https://quest.example/w/tok", "image": "https://cdn.example/w/tok.png", "text": "Level 5! Join me"},
            share.link(api, {"win": 17}),
        )
        self.assertEqual([("/api/share/hill", {"win": 17, "to": "link"})], api.sent)

    def test_the_picture_is_fetched_without_the_token_and_within_limits(self):
        asked = []

        def opener(request, timeout):
            asked.append((request.full_url, request.headers, timeout))
            return Response(PNG)

        self.assertEqual(PNG, share.download_image("https://quest.example/w/t.png", opener=opener))
        url, headers, timeout = asked[0]
        self.assertNotIn("Authorization", headers)
        self.assertEqual(share.IMAGE_TIMEOUT, timeout)
        with self.assertRaises(ValueError):
            share.download_image("https://quest.example/w/t.png", limit=4, opener=opener)
        with self.assertRaises(ValueError):
            share.download_image("https://quest.example/w/t.png", opener=lambda request, timeout: Response(b"<html>"))
        for url in ("file:///etc/passwd", "", None):
            with self.assertRaises(ValueError):
                share.download_image(url, opener=opener)
        self.assertEqual(2, len(asked))

    def test_what_the_player_is_told(self):
        self.assertTrue(share.sent_text(0).startswith("Nobody new got it"))
        self.assertEqual("Shared with 1 friend", share.sent_text(1))
        self.assertEqual("Shared with 4 friends", share.sent_text(4))
        spanish = lambda value: language.tr(value, "es")
        self.assertEqual("Compartido con 4 amigos", share.sent_text(4, spanish))

    def test_only_a_climb_into_the_top_three_is_offered(self):
        self.assertEqual(2, share.podium(["a", "b", "me"], ["a", "me", "b"], "me"))
        self.assertIsNone(share.podium(["a", "b", "c", "d", "me"], ["a", "b", "c", "me", "d"], "me"))
        self.assertIsNone(share.podium(["me", "a"], ["a", "me"], "me"))
        self.assertIsNone(share.podium([], ["me"], "me"))
        self.assertIsNone(share.podium(["a", "me"], ["a"], "me"))
        self.assertEqual(4, share.climbed_to(["a", "b", "c", "d", "me"], ["a", "b", "c", "me", "d"], "me"))

    def test_a_win_without_a_card_travels_as_text_naming_the_server(self):
        win = {"title": "Level 5!", "body": "Well done"}
        self.assertEqual("Level 5!: Well done\nStudy with me on AnkiQuest: https://quest.example",
                         share.invitation(win, "https://quest.example"))
        self.assertEqual("Level 5!: Well done\nI study with AnkiQuest.", share.invitation(win, ""))

    def test_the_window_shows_the_chosen_companion(self):
        self.assertEqual("aki_face.png", share.art_file("aki"))
        self.assertEqual("ankilope_face.png", share.art_file("ankilope"))
        self.assertIsNone(share.art_file("none"))
        for companion in ("aki", "ankilope"):
            self.assertTrue((ADDON / share.art_file(companion)).is_file())


def translated_literals(path):
    tree = ast.parse(path.read_text("utf-8"))
    for node in ast.walk(tree):
        if (
            isinstance(node, ast.Call)
            and getattr(node.func, "id", None) in ("tr", "translate")
            and node.args
            and isinstance(node.args[0], ast.Constant)
            and isinstance(node.args[0].value, str)
        ):
            yield node.args[0].value


class TranslationTests(unittest.TestCase):
    def test_every_new_label_has_a_spanish_translation(self):
        wanted = set()
        for name in ("inbox_ui", "share_ui", "share", "inbox", "sharing"):
            wanted.update(translated_literals(ADDON / (name + ".py")))
        for group in inbox.QUICK.values():
            wanted.update(group)
        wanted.add("Tools → ankiquest inbox… to reply")
        self.assertEqual([], sorted(label for label in wanted if label not in language.SPANISH))

    def test_formatted_translations_keep_their_placeholders(self):
        for source, spanish in language.SPANISH.items():
            self.assertEqual(source.count("%s"), spanish.count("%s"), source)
            self.assertEqual(source.count("%d"), spanish.count("%d"), source)


class CelebrationHookTests(unittest.TestCase):
    """The poll offers a just-announced win once, against the stubbed Anki of test_deck_completion."""

    setUp = base.RefreshTests.setUp
    with_board = base.RefreshTests.with_board
    run_task = base.RefreshTests.run_task
    read_rows = base.RefreshTests.read_rows
    write_config = base.RefreshTests.write_config

    def offered(self, celebrations=True):
        self.with_board()
        self.api.deck_settings = Mock(return_value={"celebrations": celebrations})
        self.addon.share_ui = SimpleNamespace(offer=Mock(), same_account=lambda a, b: a is b)
        return self.addon.share_ui.offer

    def test_a_climb_onto_the_podium_is_offered_for_sharing(self):
        offer = self.offered()
        self.mw.pm.profile["ankiquestLastOrder"] = ["hill", "cerro"]
        self.api.rank.return_value = {"order": ["cerro", "hill"], "change": {"title": "Crown", "body": "You passed Hill."}}
        self.addon.poll()
        win = offer.call_args.args[2]
        self.assertEqual({"title": "Crown", "body": "You passed Hill.", "notification": None, "place": 1, "win": None}, win)

    def test_only_the_newest_own_win_is_offered(self):
        offer = self.offered()
        now = int(time.time())
        self.inbox[:] = [
            {"id": 6, "title": "Level 5!", "body": "a", "created_at": now, "kind": "celebration", "sender": ""},
            {"id": 7, "title": "Recap", "body": "b", "created_at": now, "kind": "reminder_weekly_recap", "sender": "", "win": 21},
            {"id": 8, "title": "Hill shared a win", "body": "c", "created_at": now, "kind": "share", "sender": "hill"},
        ]
        self.addon.poll()
        offer.assert_called_once()
        self.assertEqual(7, offer.call_args.args[2]["notification"])
        self.assertEqual(21, offer.call_args.args[2]["win"])

    def test_nothing_pops_up_with_celebrations_off_or_on_a_quiet_poll(self):
        offer = self.offered(celebrations=False)
        self.inbox[:] = [{"id": 6, "title": "Level 5!", "body": "a", "created_at": int(time.time()), "kind": "celebration"}]
        self.addon.poll()
        offer.assert_not_called()
        offer = self.offered()
        self.inbox.append({"id": 9, "title": "Level 6!", "body": "a", "created_at": int(time.time()), "kind": "celebration"})
        self.addon.poll(quiet=True)
        offer.assert_not_called()

    def test_the_reply_hint_points_at_the_native_inbox(self):
        self.offered()
        self.addon.poll()
        self.assertIn("Tools → ankiquest inbox… to reply", self.addon.tooltip.call_args.args[0])


if __name__ == "__main__":
    unittest.main()
