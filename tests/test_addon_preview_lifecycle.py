"""Token-less previews stay with the collection and profile that started them."""
from concurrent.futures import Future
from types import SimpleNamespace
import unittest
from unittest.mock import Mock

from addon_loader import load_in_package

preview = load_in_package("preview_lifecycle", "preview")


class PreviewLifecycle(unittest.TestCase):
    def setUp(self):
        preview.clear_account()
        self.tasks = []
        self.mw = SimpleNamespace(col=SimpleNamespace(db=SimpleNamespace(all=Mock(return_value=[]))),
                                  pm=SimpleNamespace(profile={}),
                                  taskman=SimpleNamespace(run_in_background=lambda work, done, **_kw: self.tasks.append((work, done))))
        self.api = SimpleNamespace(base="https://quest.test", user="hill", token="", profile=Mock(return_value={}),
                                   _request=Mock(return_value={"feedback": {"status": "+5 XP"}}))
        self.say = Mock()

    def test_a_new_profile_does_not_wait_for_an_old_preview(self):
        preview.refresh(self.mw, self.api, True, self.say)
        self.mw.pm.profile = {}
        preview.refresh(self.mw, self.api, True, self.say)
        self.assertEqual(2, len(self.tasks))

    def test_a_late_preview_is_not_shown_in_another_profile(self):
        preview.refresh(self.mw, self.api, True, self.say)
        work, done = self.tasks.pop()
        future = Future()
        future.set_result(work())
        self.mw.pm.profile = {}
        done(future)
        self.say.assert_not_called()

    def test_an_old_completion_cannot_release_a_new_preview(self):
        preview.refresh(self.mw, self.api, True, self.say)
        old_work, old_done = self.tasks.pop()
        preview.clear_account()
        preview.refresh(self.mw, self.api, True, self.say)
        future = Future()
        future.set_result(old_work())
        old_done(future)
        preview.refresh(self.mw, self.api, True, self.say)
        self.assertEqual(1, len(self.tasks))


if __name__ == "__main__":
    unittest.main()
