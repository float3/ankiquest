"""Home background replies belong to their starting account and profile."""
from concurrent.futures import Future
from types import SimpleNamespace
from unittest.mock import Mock, patch

from test_addon_uploads import HeldTasks


class HomeRequests(HeldTasks):
    def setUp(self):
        super().setUp()
        self.home.clear_account()
        self.api.language = "en"
        self.addon.failed = Mock()

    def hold_result(self):
        work, done = self.tasks.pop(0)
        future = Future()
        future.set_result(work())
        return lambda: done(future)

    def test_capabilities_cannot_arrive_in_another_profile(self):
        self.home.load_capabilities(self.api)
        done = self.hold_result()
        self.mw.pm.profile = {}
        done()
        self.assertIsNone(self.home.local["capabilities"])

    def test_a_new_profile_can_load_while_the_old_one_waits(self):
        self.home.load_capabilities(self.api)
        self.mw.pm.profile = {}
        self.home.load_capabilities(self.api)
        self.assertEqual(2, len(self.tasks))
        self.finish()
        self.home.load_capabilities(self.api)
        self.assertEqual(1, len(self.tasks))

    def test_a_late_goal_report_never_marks_another_profile(self):
        status = SimpleNamespace(mode="backlog", goal_done=True, reached=False)
        self.home.report_reached(self.api, status, 20000)
        done = self.hold_result()
        self.mw.pm.profile = {}
        done()
        self.assertNotIn(self.home.REACHED_KEY, self.mw.pm.profile)

    def test_a_late_address_check_cannot_change_another_profile(self):
        there = SimpleNamespace(base="https://main.test", devices=Mock())
        with patch.object(self.home, "Client", return_value=there):
            self.home.move_to_main_address(self.api, there.base)
        done = self.hold_result()
        self.mw.pm.profile = {}
        before = dict(self.settings)
        done()
        self.assertEqual(before, self.settings)

    def test_a_late_restart_never_updates_another_profiles_goal(self):
        with patch.object(self.home.goals, "put", return_value={}), patch.object(self.home, "remember_plan") as remember:
            self.home.restart_plan(self.api, SimpleNamespace(value=7), 20000)
            done = self.hold_result()
            self.mw.pm.profile = {}
            done()
        remember.assert_not_called()

    def test_goal_dialog_cannot_save_after_its_profile_changes(self):
        def answer(*_args):
            self.mw.pm.profile = {}
            return "cards", 50

        with patch.object(self.home.goal_ui, "goal_dialog", side_effect=answer):
            self.home.open_goal_dialog()
        self.assertEqual([], self.tasks)

    def test_an_old_goal_save_failure_stays_quiet(self):
        with patch.object(self.home.goal_ui, "goal_dialog", return_value=("cards", 50)):
            self.home.open_goal_dialog()
        self.home.clear_account()
        self.finish(fail=True)
        self.addon.failed.assert_not_called()
