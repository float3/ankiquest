"""Signing out drops account data without losing the upload checkpoint."""

import time
from unittest.mock import Mock

from test_addon_uploads import HeldTasks as _HeldTasks


class SignOut(_HeldTasks):
    def setUp(self):
        super().setUp()
        self.addon.client = self.current_client
        self.addon.refresh_shared_decks = Mock()
        self.addon.poll = Mock()
        self.home.sync_goal = Mock()
        self.now = int(time.time() * 1000)
        self.profile = self.mw.pm.profile
        self.checkpoint = self.addon._upload_prefs(self.api)
        self.checkpoint.update(through=self.now - 1000, shared={5: ["friend"]})

    def current_client(self):
        if not self.settings.get("token"):
            return self.addon.Client("", "", "")
        self.api.token = self.settings["token"]
        return self.api

    def sign_out(self):
        self.addon.ui.settings_dialog = Mock(return_value={"url": "", "user": "", "token": ""})
        self.addon.open_settings()

    def test_sign_out_clears_the_inbox_and_display_caches(self):
        self.addon.state["inbox"] = [{"id": 4, "body": "Private message"}]
        self.profile[self.addon.INBOX_KEY] = 4
        self.profile[self.home.PLAN_KEY] = {"target": 42}
        self.home.local["board"] = [{"user": "hill"}]
        for key in ("board_request", "board_retry", "goal_request"):
            self.home.local[key] = object()

        self.sign_out()

        self.assertEqual("", self.settings["token"])
        self.assertEqual("", self.settings["user"])
        self.assertEqual([], self.addon.state["inbox"])
        self.assertNotIn(self.addon.INBOX_KEY, self.profile)
        self.assertNotIn(self.home.PLAN_KEY, self.profile)
        self.assertIsNone(self.home.local["board"])
        for key in ("board_request", "board_retry", "goal_request"):
            self.assertIsNone(self.home.local[key])
        self.assertNotIn("shared", self.checkpoint)
        self.assertEqual(self.now - 1000, self.checkpoint["through"])

    def test_returning_to_the_same_account_uploads_only_new_reviews(self):
        old_review, new_review = self.now - 2000, self.now
        self.checkpoint["recent"] = [old_review]
        self.db.executemany(
            "insert into revlog values (?, 1, 3, 1, 5000, 1, 3)",
            [(old_review,), (new_review,)],
        )
        self.sign_out()

        self.addon._finish_sign_in({"url": self.api.base, "user": self.api.name, "token": "new-token"})
        self.finish()

        uploaded = self.api.upload.call_args.args[0]
        self.assertEqual([new_review], [review["id"] for review in uploaded])

    def test_sign_out_invalidates_an_upload_that_has_not_started(self):
        self.db.execute("insert into revlog values (?, 1, 3, 1, 5000, 1, 3)", (self.now,))
        self.addon.refresh(False)
        self.assertEqual(1, len(self.tasks))

        self.sign_out()
        self.finish()

        self.api.upload.assert_not_called()
