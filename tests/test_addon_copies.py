"""Deck copies, server-side preferences and token-less previews, without Anki or Qt."""

import importlib.util
import json
import os
import sys
import tempfile
import threading
import types
import unittest
import urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock

ADDON = Path(__file__).resolve().parents[1] / "addon"
PACKAGE = "aq_copies_pkg"


def load(name):
    """Loads one add-on module inside a stand-in package, so relative imports work without running __init__."""
    if PACKAGE not in sys.modules:
        package = types.ModuleType(PACKAGE)
        package.__path__ = [str(ADDON)]
        sys.modules[PACKAGE] = package
    full = PACKAGE + "." + name
    if full in sys.modules:
        return sys.modules[full]
    spec = importlib.util.spec_from_file_location(full, ADDON / (name + ".py"))
    module = importlib.util.module_from_spec(spec)
    sys.modules[full] = module
    spec.loader.exec_module(module)
    return module


copies = load("copies")
preferences = load("preferences")
preview = load("preview")
client = load("client")
english = lambda text: text


class Server(BaseHTTPRequestHandler):
    received = []
    package = b"PK\x03\x04" + b"x" * 100

    def do_GET(self):
        Server.received.append(("GET", self.path, dict(self.headers), b""))
        if self.path == "/api/deck-copies/ana":
            self.json(200, {"offers": [{"id": 7, "sender": "ben", "deck": "Verbs", "bytes": 104, "created_at": 1}],
                            "friends": [{"user": "ben", "display": "Ben"}], "max_bytes": 1000})
        elif self.path == "/api/deck-copies/ana/7":
            self.reply(200, Server.package, "application/octet-stream")
        else:
            self.json(404, {"error": "missing"})

    def do_POST(self):
        body = self.rfile.read(int(self.headers["Content-Length"]))
        Server.received.append(("POST", self.path, dict(self.headers), body))
        self.json(200, {"id": 9})

    def do_DELETE(self):
        Server.received.append(("DELETE", self.path, dict(self.headers), b""))
        self.reply(204, b"", None)

    def json(self, status, body):
        self.reply(status, json.dumps(body).encode(), "application/json")

    def reply(self, status, payload, content_type):
        self.send_response(status)
        if content_type:
            self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def log_message(self, *_):
        pass


class DeckCopyRules(unittest.TestCase):
    def test_the_size_limit_is_the_servers_and_inclusive(self):
        self.assertEqual(copies.MAX_BYTES, 25 * 1024 * 1024)
        self.assertFalse(copies.too_large(copies.MAX_BYTES))
        self.assertTrue(copies.too_large(copies.MAX_BYTES + 1))
        self.assertEqual(copies.limit_of({"max_bytes": 1000}), 1000)
        self.assertEqual(copies.limit_of({"max_bytes": "big"}), copies.MAX_BYTES)
        self.assertEqual(copies.limit_of(None), copies.MAX_BYTES)
        self.assertEqual(copies.megabytes(copies.MAX_BYTES), 25)

    def test_deck_names_fit_what_the_server_accepts(self):
        self.assertEqual(copies.deck_label("Spanish\n::Verbs\x00"), "Spanish ::Verbs")
        long = copies.deck_label("ñ" * 200)
        self.assertLessEqual(len(long.encode("utf-8")), 160)
        self.assertEqual(copies.deck_label("\t"), "Deck")

    def test_the_upload_carries_the_deck_and_recipients_in_the_query(self):
        api = client.Client("https://example.test/", "ana", "secret")
        parts = urllib.parse.urlsplit(copies.upload_url(api, "Spanish::Verbs", ["ben", "cy"]))
        self.assertEqual(parts.path, "/api/deck-copies/ana")
        query = urllib.parse.parse_qs(parts.query)
        self.assertEqual(query["deck"], ["Spanish::Verbs"])
        self.assertEqual(json.loads(query["recipients"][0]), ["ben", "cy"])

    def test_recipients_must_be_chosen_and_at_most_twenty(self):
        self.assertEqual(copies.check_recipients([], "ana", english), "Choose at least one friend.")
        self.assertEqual(copies.check_recipients(["ana"], "ana", english), "Choose at least one friend.")
        self.assertEqual(copies.check_recipients(["u%d" % i for i in range(21)], "ana", english), "Choose at most 20 friends.")
        self.assertIsNone(copies.check_recipients(["ben"], "ana", english))

    def test_offers_from_other_people_are_escaped(self):
        offer = {"id": 1, "sender": "ben", "deck": "<script>x</script>", "bytes": 2 * 1024 * 1024}
        text = copies.offer_html(offer, {"ben": "Ben & <b>Co</b>"}, english)
        self.assertNotIn("<script>", text)
        self.assertIn("&lt;script&gt;", text)
        self.assertIn("Ben &amp; &lt;b&gt;Co&lt;/b&gt;", text)
        self.assertIn("2.0 MB", text)

    def test_the_menu_shows_how_many_offers_wait(self):
        self.assertEqual(copies.menu_title(0, english), "ankiquest: decks offered to me…")
        self.assertEqual(copies.menu_title(3, english), "ankiquest: decks offered to me… (3)")

    def test_each_offer_is_announced_once(self):
        offers = [{"id": 5}, {"id": 3}]
        fresh, seen = copies.new_offers(offers, [3])
        self.assertEqual(fresh, [{"id": 5}])
        self.assertEqual(seen, [3, 5])
        self.assertEqual(copies.new_offers(offers, seen)[0], [])

    def test_a_package_over_the_limit_is_refused_before_uploading(self):
        handle, name = tempfile.mkstemp(suffix=".apkg")
        os.write(handle, b"x" * 11)
        os.close(handle)
        self.addCleanup(os.remove, name)
        api = SimpleNamespace(base="http://127.0.0.1:9", user="ana", token="t", language="en")
        with self.assertRaises(copies.TooLarge) as refused:
            copies.send(api, "Deck", ["ben"], name, limit=10)
        self.assertEqual(refused.exception.limit, 10)

    def test_sharing_exports_without_scheduling_and_removes_the_package(self):
        made = []

        def export(col, deck_id, target):
            made.append((deck_id, target))
            Path(target).write_bytes(b"x" * 20)

        original = copies.export_deck
        copies.export_deck = export
        self.addCleanup(setattr, copies, "export_deck", original)
        api = SimpleNamespace(base="http://127.0.0.1:9", user="ana", token="t", language="en")
        with self.assertRaises(copies.TooLarge):
            copies.share(api, None, 42, "Deck", ["ben"], limit=10)
        self.assertEqual(made[0][0], 42)
        self.assertFalse(os.path.exists(made[0][1]))


class DeckCopyRequests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.server = HTTPServer(("127.0.0.1", 0), Server)
        threading.Thread(target=cls.server.serve_forever, daemon=True).start()
        cls.api = client.Client("http://127.0.0.1:%d" % cls.server.server_port, "ana", "secret", "es")

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()

    def setUp(self):
        Server.received.clear()

    def test_the_inbox_lists_offers_friends_and_the_limit(self):
        inbox = copies.inbox(self.api)
        self.assertEqual(copies.display_names(inbox), {"ben": "Ben"})
        self.assertEqual(copies.limit_of(inbox), 1000)
        self.assertEqual(Server.received[0][2]["Authorization"], "Bearer secret")

    def test_uploading_posts_the_package_bytes(self):
        handle, name = tempfile.mkstemp(suffix=".apkg")
        os.write(handle, Server.package)
        os.close(handle)
        self.addCleanup(os.remove, name)
        self.assertEqual(copies.send(self.api, "Verbs", ["ben"], name), {"id": 9})
        method, path, headers, body = Server.received[0]
        self.assertEqual(method, "POST")
        self.assertTrue(path.startswith("/api/deck-copies/ana?"))
        self.assertEqual(headers["Content-Type"], "application/octet-stream")
        self.assertEqual(headers["Accept-Language"], "es")
        self.assertEqual(body, Server.package)

    def test_downloading_saves_the_package_and_stops_past_the_limit(self):
        target = copies.temporary_package("ankiquest-test-")
        self.addCleanup(lambda: os.path.exists(target) and os.remove(target))
        copies.download(self.api, 7, target)
        self.assertEqual(Path(target).read_bytes(), Server.package)
        with self.assertRaises(copies.TooLarge):
            copies.download(self.api, 7, target, limit=50)
        self.assertFalse(os.path.exists(target))

    def test_dismissing_deletes_my_copy_of_the_offer(self):
        copies.dismiss(self.api, 7)
        self.assertEqual(Server.received[0][:2], ("DELETE", "/api/deck-copies/ana/7"))


ALERTS = {
    "enabled": True,
    "senders": [{"user": "ben", "display": "Ben"}, {"user": "cy", "display": "Cy"}, {"user": "dee", "display": "dee"}],
    "sharing_senders": ["ben"],
    "unsubscribed_senders": ["cy"],
    "muted_senders": ["old"],
}


class Preferences(unittest.TestCase):
    def test_senders_match_the_web_client_including_mutes_and_people_who_left(self):
        senders, unsubscribed = preferences.alert_senders(ALERTS)
        self.assertEqual(senders, [("ben", "Ben"), ("cy", "Cy"), ("old", "old")])
        self.assertEqual(unsubscribed, {"cy", "old"})

    def test_saving_keeps_mutes_as_unsubscribes(self):
        before = ["cy", "old"]
        body = preferences.subscription_body(ALERTS, True, before, ["cy", "old", "ben"])
        self.assertEqual(body, {"enabled": True, "unsubscribed_senders": ["ben", "cy", "old"]})

    def test_saving_applies_only_my_changes_to_a_fresh_read(self):
        fresh = dict(ALERTS, unsubscribed_senders=["cy", "dee"], muted_senders=["old"])
        body = preferences.subscription_body(fresh, False, ["cy", "old"], ["old"])
        self.assertEqual(body, {"enabled": False, "unsubscribed_senders": ["dee", "old"]})

    def test_nothing_is_written_when_nothing_changed(self):
        api = SimpleNamespace(user="ana", _request=Mock())
        loaded = {"freezes": {"enabled": True}, "alerts": ALERTS}
        preferences.save(api, loaded, {"freezes": True, "alerts": True, "before": ["cy", "old"], "after": ["old", "cy"]})
        api._request.assert_not_called()

    def test_changes_reread_then_post_the_full_replacement(self):
        api = SimpleNamespace(user="ana", _request=Mock(return_value=ALERTS))
        loaded = {"freezes": {"enabled": True}, "alerts": ALERTS}
        preferences.save(api, loaded, {"freezes": False, "alerts": True, "before": ["cy", "old"], "after": ["old"]})
        calls = [call.args for call in api._request.call_args_list]
        self.assertEqual(calls, [
            ("/api/streak-freezes/ana", {"enabled": False}),
            ("/api/deck-subscriptions/ana",),
            ("/api/deck-subscriptions/ana", {"enabled": True, "unsubscribed_senders": ["old"]}),
        ])

    def test_settings_the_server_lacks_are_left_alone(self):
        api = SimpleNamespace(user="ana", _request=Mock(side_effect=client.ServerError(404, "")))
        loaded = preferences.load(api)
        self.assertEqual(loaded, {"freezes": None, "alerts": None})
        preferences.save(api, loaded, {"freezes": False, "alerts": False, "before": [], "after": []})
        self.assertEqual(api._request.call_count, 2)

    def test_the_last_upload_line(self):
        self.assertEqual(preferences.last_upload_text(None, english), "Last upload: never")
        self.assertRegex(preferences.last_upload_text(1_700_000_000_000, english), r"^Last upload: \d{4}-\d\d-\d\d \d\d:\d\d$")


class Preview(unittest.TestCase):
    def setUp(self):
        preview.state.update(account=None, baseline=0, at=0, seen=None, busy=False)

    def test_only_a_server_and_player_without_a_token_preview(self):
        self.assertTrue(preview.wanted(SimpleNamespace(base="https://x", user="ana", token="")))
        self.assertFalse(preview.wanted(SimpleNamespace(base="https://x", user="ana", token="t")))
        self.assertFalse(preview.wanted(SimpleNamespace(base="", user="ana", token="")))

    def test_reviews_after_the_servers_last_one_are_previewed_and_remembered(self):
        api = SimpleNamespace(base="https://x", user="ana", token="", profile=Mock(return_value={"last_review_id": 100}),
                              _request=Mock(return_value={"feedback": {"status": "+5 XP"}}))
        rows = Mock(return_value=[(101, 1, 1, 3, 1000, 1)])
        self.assertEqual(preview.fetch(api, rows, 1_000), {"feedback": {"status": "+5 XP"}})
        rows.assert_called_once_with(100)
        path, body = api._request.call_args.args
        self.assertEqual(path, "/api/preview/ana")
        self.assertNotIn("seen_through", body)
        self.assertEqual(body["reviews"][0]["id"], 101)
        preview.fetch(api, rows, 2_000)
        self.assertEqual(api._request.call_args.args[1]["seen_through"], 101)
        api.profile.assert_called_once()

    def test_private_servers_stay_quiet(self):
        def run(work, done, uses_collection=False):
            from concurrent.futures import Future
            future = Future()
            future.set_exception(client.ServerError(401, ""))
            done(future)

        say = Mock()
        mw = SimpleNamespace(col=object(), taskman=SimpleNamespace(run_in_background=run))
        preview.refresh(mw, SimpleNamespace(base="https://x", user="ana", token=""), True, say)
        say.assert_not_called()
        self.assertFalse(preview.state["busy"])


if __name__ == "__main__":
    unittest.main()
