"""One upload at a time: a refresh asked for during an upload runs once it ends, with what was asked."""

import time
import unittest
from http import HTTPStatus
from unittest.mock import Mock

import test_desktop_today as today

DAY_MS = 86_400_000


class HeldTasks(unittest.TestCase):
    """The add-on with Anki replaced by stand-ins, whose background tasks wait until finished."""

    setUp_home = today.HomeTests.setUp
    write_config = today.HomeTests.write_config

    def setUp(self):
        self.tasks = []
        self.setUp_home()
        # The columns an upload reads that the Today tests' review log leaves out.
        self.db.executescript("alter table revlog add column lastIvl integer; alter table revlog add column ivl integer;")
        self.addon.state["profile"] = {"user": "hill"}
        self.api.upload = Mock(return_value={"feedback": {"status": "+5 XP"}})
        self.addon.tooltip = Mock()
        self.mw.taskman.run_on_main = lambda work: work()

    def run_task(self, work, done, uses_collection=False):
        self.tasks.append((work, done))

    def finish(self, fail=False):
        """Ends the oldest upload, as Anki would on its main thread."""
        work, done = self.tasks.pop(0)
        future = today.Future()
        if fail:
            future.set_exception(OSError("offline"))
        else:
            try:
                future.set_result(work())
            except Exception as error:
                future.set_exception(error)
        done(future)


class UploadQueue(HeldTasks):
    def test_a_refresh_asked_for_during_a_failed_upload_still_runs(self):
        self.addon.refresh(False)
        self.addon.refresh(False)
        self.finish(fail=True)
        self.assertEqual(1, len(self.tasks), "the queued refresh starts")
        self.finish()
        self.assertEqual([], self.tasks, "and nothing is left over to start later")

    def test_the_queued_refresh_keeps_its_resync_and_quiet_feedback(self):
        now = int(time.time() * 1000)
        self.mw.pm.profile[self.addon.MARK_KEY] = now
        self.mw.pm.profile[self.addon.BASELINE_KEY] = ["https://example.test", "hill"]
        self.db.execute("insert into revlog values (?, 1, 3, 1, 5000, 1, 3)", (now - DAY_MS,))

        self.addon.refresh(False)
        self.addon.refresh(False, resync=True)
        self.finish()
        self.finish()

        resent = self.api.upload.call_args_list[-1].args[0]
        self.assertEqual(1, len(resent), "the resync sends the day-old review again")
        self.addon.tooltip.assert_not_called()

    def test_feedback_shows_when_any_queued_refresh_asked_for_it(self):
        self.addon.refresh(False)
        self.addon.refresh(True)
        self.addon.refresh(False)
        self.finish()
        self.addon.tooltip.assert_not_called()
        self.finish()
        self.addon.tooltip.assert_called_once()


class UploadBatches(HeldTasks):
    def setUp(self):
        super().setUp()
        self.now = int(time.time() * 1000)
        self.profile = self.mw.pm.profile
        self.profile[self.addon.MARK_KEY] = self.now
        self.profile[self.addon.BASELINE_KEY] = [self.api.base, self.api.user]

    def add_reviews(self, ids, ease=3):
        self.db.executemany("insert into revlog values (?, 1, ?, 1, 5000, 1, 3)", ((i, ease) for i in ids))

    def test_undo_sends_only_reviews_absent_from_the_revlog(self):
        present, undone = self.now - 2, self.now - 1
        self.profile[self.addon.RECENT_KEY] = [present, undone]
        self.add_reviews([present], ease=0)

        self.addon.refresh(False)
        self.finish()

        self.assertEqual([undone], list(self.api.upload.call_args.args[3]))

    def test_a_large_drop_does_not_delete_any_reviews(self):
        self.profile[self.addon.RECENT_KEY] = list(range(self.now - 101, self.now))

        self.addon.refresh(False)
        self.finish()

        self.assertFalse(self.api.upload.call_args.args[3])

    def test_pending_and_restored_reviews_share_the_batch_limit(self):
        restored = self.now - 1
        pending = range(self.now + 1, self.now + self.addon.MAX_PENDING + 2)
        self.add_reviews([restored, *pending])

        self.addon.refresh(False)
        self.finish()

        batches = [call.args[0] for call in self.api.upload.call_args_list]
        self.assertEqual([self.addon.MAX_PENDING, 2], [len(batch) for batch in batches])
        self.assertEqual([restored, *pending], [row["id"] for batch in batches for row in batch])
        self.assertEqual(pending[-1], self.profile[self.addon.MARK_KEY])

    def test_a_failed_batch_leaves_the_previous_mark(self):
        self.add_reviews(range(self.now + 1, self.now + self.addon.MAX_PENDING + 2))
        self.api.upload.side_effect = [{}, OSError("offline")]

        self.addon.refresh(False)
        self.finish()

        self.assertEqual(2, self.api.upload.call_count)
        self.assertEqual(self.now, self.profile[self.addon.MARK_KEY])

    def test_upload_all_is_silent_and_never_resets_the_mark(self):
        old = self.now - 10 * DAY_MS
        self.add_reviews([old, self.now])

        self.addon.upload_everything()
        self.addon.upload_everything()
        self.assertEqual(self.now, self.profile[self.addon.MARK_KEY])
        self.finish()

        self.assertEqual([], self.tasks)
        reviews, _, silent, _ = self.api.upload.call_args.args
        self.assertTrue(silent)
        self.assertEqual([old, self.now], [row["id"] for row in reviews])
        self.assertEqual(self.now, self.profile[self.addon.MARK_KEY])
        self.assertIn("2", self.addon.tooltip.call_args.args[0])

    def test_a_finished_upload_cannot_move_another_profiles_mark(self):
        self.add_reviews([self.now + 1])
        self.addon.refresh(False)
        self.mw.pm.profile = {self.addon.MARK_KEY: 1}

        self.finish()

        self.assertEqual(1, self.mw.pm.profile[self.addon.MARK_KEY])
        self.api.upload.assert_not_called()

    def test_reopening_only_resends_the_week_once_but_sync_still_resends(self):
        old = self.now - DAY_MS
        self.add_reviews([old])
        self.profile[self.addon.RECENT_KEY] = [old]
        self.addon.refresh_shared_decks = Mock()
        self.addon.poll = Mock()
        self.addon.maybe_check_updates = Mock()

        for trigger in (self.addon.on_profile_open, self.addon.on_profile_open, self.addon.on_sync):
            trigger()
            self.finish()

        self.assertEqual([[old], [], [old]], [
            [row["id"] for row in call.args[0]] for call in self.api.upload.call_args_list
        ])

    def test_offline_upload_waits_for_its_backoff_and_recovers(self):
        self.api.upload.side_effect = [OSError("offline"), {}]
        self.addon.refresh(False)
        self.finish()

        self.assertIn("will retry", self.addon._upload_status(self.api))
        delay, retry = self.mw.progress.timer.call_args.args[:2]
        self.assertGreater(delay, 0)
        self.addon.refresh(False)
        self.assertEqual([], self.tasks)
        retry()
        self.finish()

        self.assertEqual(2, self.api.upload.call_count)
        self.assertNotIn("will retry", self.addon._upload_status(self.api))

    def test_unauthorized_upload_asks_for_sign_in_until_the_token_changes(self):
        self.api.upload.side_effect = [self.addon.ServerError(HTTPStatus.UNAUTHORIZED, "unauthorized"), {}]
        self.addon.refresh(False)
        self.finish()
        self.addon.refresh(False)

        self.assertIn("Sign in", self.addon._upload_status(self.api))
        self.assertEqual([], self.tasks)
        self.mw.progress.timer.assert_not_called()

        self.api.token = "new-token"
        self.addon.refresh(False)
        self.finish()
        self.assertNotIn("Sign in", self.addon._upload_status(self.api))


if __name__ == "__main__":
    unittest.main()
