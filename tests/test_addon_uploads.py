"""One upload at a time: a refresh asked for during an upload runs once it ends, with what was asked."""

import time
import unittest
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

    def run_task(self, work, done, uses_collection=False):
        self.tasks.append((work, done))

    def finish(self, fail=False):
        """Ends the oldest upload, as Anki would on its main thread."""
        work, done = self.tasks.pop(0)
        future = today.Future()
        if fail:
            future.set_exception(OSError("offline"))
        else:
            future.set_result(work())
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


if __name__ == "__main__":
    unittest.main()
