"""Sharing settings and the share window; run with python -m unittest discover -s tests."""

import importlib.util
import sys
import unittest
from pathlib import Path
from types import ModuleType, SimpleNamespace
from unittest.mock import Mock

import test_deck_completion as base

ADDON = Path(__file__).resolve().parents[1] / "addon"


def load_in_package(name):
    package = ModuleType("aq_sharing_package")
    package.__path__ = [str(ADDON)]
    sys.modules[package.__name__] = package
    spec = importlib.util.spec_from_file_location(package.__name__ + "." + name, ADDON / (name + ".py"))
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


sharing = load_in_package("sharing")
language = sys.modules["aq_sharing_package.language"]


class FakeApi:
    user = "hill"

    def __init__(self, answer):
        self.answer, self.sent = answer, []

    def _request(self, path, body=None, method=None):
        self.sent.append((path, body, method))
        if isinstance(self.answer, Exception):
            raise self.answer
        return self.answer


SETTINGS = {
    "policies": {"daily_goal": "ask", "streak_secured": "private", "level": "ask", "quest": "private"},
    "daily_cap": 3,
}


class SharingSettingsTests(unittest.TestCase):
    def test_every_kind_on_the_server_is_offered_with_every_policy(self):
        self.assertEqual(
            ["daily_goal", "streak_secured", "streak", "level", "achievement", "record", "quest", "conquered", "place", "challenge"],
            [kind for kind, _ in sharing.KINDS],
        )
        self.assertEqual(["private", "ask", "auto"], [policy for policy, _ in sharing.POLICIES])

    def test_only_what_changed_is_sent(self):
        unchanged = dict(SETTINGS["policies"])
        self.assertIsNone(sharing.changes(SETTINGS, unchanged, 3))
        self.assertEqual({"policies": {"level": "auto"}}, sharing.changes(SETTINGS, dict(unchanged, level="auto"), 3))
        self.assertEqual({"daily_cap": 5}, sharing.changes(SETTINGS, unchanged, 5))
        self.assertIsNone(sharing.changes(SETTINGS, dict(unchanged, level="loud"), 3))

    def test_the_cap_stays_within_the_servers_range(self):
        self.assertEqual(3, sharing.cap({}))
        self.assertEqual(10, sharing.cap({"daily_cap": 99}))
        self.assertEqual(1, sharing.cap({"daily_cap": 0}))

    def test_settings_are_read_and_saved_on_the_server(self):
        api = FakeApi(SETTINGS)
        self.assertEqual(SETTINGS, sharing.load(api))
        sharing.save(api, {"daily_cap": 4})
        self.assertEqual([("/api/sharing/hill", None, None), ("/api/sharing/hill", {"daily_cap": 4}, "PUT")], api.sent)
        self.assertIsNone(sharing.load(FakeApi(OSError("offline"))))
        self.assertIsNone(sharing.load(FakeApi({"error": "nope"})))

    def test_every_label_is_translated(self):
        for code, table in language.TABLES.items():
            for _, label in sharing.KINDS + sharing.POLICIES:
                with self.subTest(code=code, label=label):
                    self.assertIn(label, table)


class ShareWindowTests(unittest.TestCase):
    """The share window's buttons, with Qt replaced by stand-ins."""

    setUp = base.RefreshTests.setUp
    run_task = base.RefreshTests.run_task
    read_rows = base.RefreshTests.read_rows
    write_config = base.RefreshTests.write_config

    def window(self, win):
        share_ui = sys.modules["ankiquest_test_addon.share_ui"]
        share_ui.share = SimpleNamespace(**vars(sys.modules["ankiquest_test_addon.share"]))
        share_ui.share.card = Mock(return_value={"url": "u", "image": "i", "text": "Level 5! Join me", "png": b"png"})
        share_ui.share.send = Mock(return_value=2)
        status = Mock()
        current = SimpleNamespace(base=self.api.base, user=self.api.user, token=self.api.token)
        return share_ui, share_ui.Sharing(None, self.api, win, lambda: current, self.run_task, status), status

    def test_the_servers_card_is_fetched_once_for_copying_and_saving(self):
        share_ui, sharing_window, status = self.window({"title": "Level 5!", "body": "", "win": 17})
        share_ui.ask_path = Mock(return_value=str(Path(self.id()).with_suffix(".png")))
        saved = []
        share_ui.save_card = lambda path, found: saved.append((path, found["png"])) or "saved"
        sharing_window.to_clipboard()
        sharing_window.to_file()
        share_ui.share.card.assert_called_once()
        self.assertEqual(b"png", saved[0][1])
        self.assertEqual("saved", status.setText.call_args.args[0])

    def test_a_win_for_another_account_is_not_fetched(self):
        share_ui, sharing_window, status = self.window({"title": "Level 5!", "body": "", "win": 17})
        sharing_window.current_client = lambda: SimpleNamespace(base=self.api.base, user="cerro", token="other")
        sharing_window.to_clipboard()
        share_ui.share.card.assert_not_called()

    def test_a_failed_fetch_says_why_and_tries_again_next_time(self):
        share_ui, sharing_window, status = self.window({"title": "Level 5!", "body": "", "win": 17})
        share_ui.share.card.side_effect = [OSError("timed out"), {"text": "t", "png": b"png"}]
        sharing_window.to_clipboard()
        self.assertIn("timed out", status.setText.call_args.args[0])
        sharing_window.to_clipboard()
        self.assertEqual(2, share_ui.share.card.call_count)


if __name__ == "__main__":
    unittest.main()
