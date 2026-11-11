"""Messages wait for a resting screen and survive quiet refreshes/restarts."""
import time
import unittest
from copy import deepcopy
from unittest.mock import Mock, patch

import test_deck_completion as fixture


class Announcements(unittest.TestCase):
    read_rows = fixture.RefreshTests.read_rows
    write_config = fixture.RefreshTests.write_config
    run_task = fixture.RefreshTests.run_task

    def setUp(self):
        fixture.RefreshTests.setUp(self)
        fixture.RefreshTests.with_board(self)
        self.addon.copies_ui.poll = Mock()
        self.mw.isActiveWindow = lambda: True
        self.mw.isMinimized = lambda: False

    def test_review_defers_messages_but_updates_the_badge(self):
        self.mw.state = "review"
        self.addon.poll()
        self.addon.tooltip.assert_not_called()
        self.assertEqual(1, self.addon.state["activity"].unread)
        self.mw.state = "deckBrowser"
        self.addon.on_state_change("deckBrowser", "review")
        self.assertIn("Hill finished", self.addon.tooltip.call_args.args[0])
        self.addon.poll()
        self.assertEqual(1, self.addon.tooltip.call_count)

    def test_quiet_refresh_retains_messages_for_the_next_poll(self):
        self.addon.poll(quiet=True)
        self.addon.tooltip.assert_not_called()
        self.addon.poll()
        self.assertIn("Hill finished", self.addon.tooltip.call_args.args[0])
        self.addon.poll()
        self.assertEqual(1, self.addon.tooltip.call_count)

    def test_old_messages_get_one_summary_and_no_replay(self):
        for number in range(5, 9):
            self.inbox.append(dict(self.inbox[0], id=number, created_at=int(time.time()) - 3 * 86400))
        self.addon.poll()
        self.addon.tooltip.assert_called_once()
        self.assertIn("5 new messages", self.addon.tooltip.call_args.args[0])
        self.addon.poll()
        self.assertEqual(1, self.addon.tooltip.call_count)

    def test_background_without_tray_waits_until_foreground(self):
        self.mw.isActiveWindow = lambda: False
        self.addon.poll()
        self.addon.tooltip.assert_not_called()
        self.mw.isActiveWindow = lambda: True
        self.addon.poll()
        self.assertIn("Hill finished", self.addon.tooltip.call_args.args[0])

    def test_a_new_profile_cannot_deliver_the_old_profiles_queue(self):
        self.mw.state = "review"
        self.addon.poll()
        self.mw.pm.profile = {}
        self.mw.state = "deckBrowser"
        self.addon.on_state_change("deckBrowser", "review")
        self.addon.tooltip.assert_not_called()

    def test_an_expired_day_never_emits_a_streak_warning(self):
        now = int(time.time() * 1000)
        profile = {"day_ends_at": now - 1, "streak_warning": {"title": "Old warning"}}
        self.assertIsNone(self.addon.notify.streak_warning(profile, 2, now, None))

    def record(self):
        return self.addon.delivery._record(self.mw.pm.profile, self.api)

    def test_quiet_hours_hold_messages_until_the_boundary(self):
        self.record()["reminders"] = {"quiet_start": 22, "quiet_end": 9}
        self.addon.offset_west_min = lambda: -120
        self.inbox[0]["created_at"] = 0
        with patch.object(self.addon.time, "time", return_value=21 * 3600):
            self.addon.poll()
        self.addon.tooltip.assert_not_called()
        with patch.object(self.addon.time, "time", return_value=(24 + 7) * 3600):
            self.addon.poll()
        self.assertIn("Hill finished", self.addon.tooltip.call_args.args[0])

    def test_quiet_hours_cover_daytime_and_equal_endpoints_disable_them(self):
        quiet = self.addon.delivery._quiet
        self.assertTrue(quiet({"quiet_start": 8, "quiet_end": 16}, 12 * 3600, 0))
        self.assertFalse(quiet({"quiet_start": 8, "quiet_end": 16}, 16 * 3600, 0))
        self.assertFalse(quiet({"quiet_start": 8, "quiet_end": 8}, 8 * 3600, 0))

    def test_queue_survives_a_restart_and_does_not_replay_after_delivery(self):
        self.addon.poll(quiet=True)
        self.mw.pm.profile = deepcopy(self.mw.pm.profile)
        self.addon._delivery = self.addon.delivery._Delivery(self.addon)
        self.addon.poll()
        self.addon.tooltip.assert_called_once()
        self.mw.pm.profile = deepcopy(self.mw.pm.profile)
        self.addon._delivery = self.addon.delivery._Delivery(self.addon)
        self.addon.poll()
        self.assertEqual(1, self.addon.tooltip.call_count)

    def test_an_account_switch_never_delivers_old_messages_to_the_new_account(self):
        self.addon.poll(quiet=True)
        self.api.user = "other"
        self.addon._delivery._flush()
        self.addon.tooltip.assert_not_called()
        self.api.user = "cerro"
        self.addon._delivery._flush()
        self.addon.tooltip.assert_called_once()

    def test_background_delivery_uses_plain_tray_text_and_an_owned_click(self):
        self.settings["system_notifications"] = True
        self.mw.isActiveWindow = lambda: False
        self.inbox[0]["title"] = "<b>Hill</b>"
        self.addon.open_inbox = Mock()
        tray_type = Mock()
        with patch.dict("sys.modules", {"aqt.qt": Mock(QSystemTrayIcon=tray_type, QIcon=Mock())}):
            self.addon.poll()
        self.addon.tooltip.assert_not_called()
        tray = tray_type.return_value
        tray.showMessage.assert_called_once()
        self.assertEqual("<b>Hill</b>", tray.showMessage.call_args.args[0])
        clicked = tray.messageClicked.connect.call_args.args[0]
        self.api.token = "changed"
        clicked()
        self.addon.open_inbox.assert_not_called()
        self.addon._delivery._clear()
        tray.hide.assert_called_once()

    def test_an_unavailable_tray_keeps_the_message_for_foreground(self):
        self.settings["system_notifications"] = True
        self.mw.isActiveWindow = lambda: False
        tray_type = Mock()
        tray_type.isSystemTrayAvailable.return_value = False
        with patch.dict("sys.modules", {"aqt.qt": Mock(QSystemTrayIcon=tray_type, QIcon=Mock())}):
            self.addon.poll()
        tray_type.return_value.showMessage.assert_not_called()
        self.assertEqual(1, len(self.record()["pending"]))
        self.mw.isActiveWindow = lambda: True
        self.addon.poll()
        self.addon.tooltip.assert_called_once()

    def test_read_receipt_cancels_an_unannounced_message(self):
        self.addon.poll(quiet=True)
        self.inbox[0]["read_at"] = 1
        self.addon.poll()
        self.addon.tooltip.assert_not_called()

    def test_paging_includes_missed_messages_beyond_the_first_page(self):
        newer = [{"id": number, "title": str(number)} for number in range(3, 53)]
        older = [{"id": 2, "title": "missed"}, {"id": 1, "title": "seen"}]
        self.record()["cursor"] = 1
        def request(path):
            if path.endswith("before=3"):
                return {"items": older, "next_before": None}
            if "/activity/" in path:
                return {"items": newer, "next_before": 3, "unread_count": 51}
            return {}
        self.api._request = request
        self.addon.poll()
        self.assertIn("51 new messages", self.addon.tooltip.call_args.args[0])
        self.assertEqual(50, len(self.addon.state["activity"].items))
        self.assertEqual(52, self.record()["cursor"])

    def test_failed_pagination_does_not_advance_the_cursor(self):
        self.record()["cursor"] = 1
        self.api._request = Mock(side_effect=[{"items": [{"id": 5}], "next_before": 5}, OSError("offline")])
        self.addon.poll()
        self.assertEqual(1, self.record()["cursor"])
        self.addon.tooltip.assert_not_called()

    def test_stale_system_reminders_do_not_join_the_human_catchup_summary(self):
        self.record()["reminders"] = {"gentle_daily": True}
        self.inbox.append({"id": 5, "kind": "reminder_daily", "created_at": int(time.time()) - 90000, "title": "Stale"})
        self.addon.poll()
        self.addon.tooltip.assert_called_once()
        self.assertIn("Hill finished", self.addon.tooltip.call_args.args[0])
        self.assertNotIn("Stale", self.addon.tooltip.call_args.args[0])

    def test_deck_offers_also_wait_until_review_ends(self):
        self.mw.state = "review"
        self.addon.copies_ui.announce([{"id": 1, "sender": "hill", "deck": "Spanish"}], {"hill": "Hill"}, False)
        self.addon.tooltip.assert_not_called()
        self.mw.state = "deckBrowser"
        self.addon.on_state_change("deckBrowser", "review")
        self.assertIn("Hill shared Spanish", self.addon.tooltip.call_args.args[0])
        self.addon.copies_ui.announce([{"id": 1, "sender": "hill", "deck": "Spanish"}], {}, False)
        self.assertEqual(1, self.addon.tooltip.call_count)

    def test_a_current_daily_reminder_does_not_require_an_existing_streak(self):
        now = int(time.time())
        reminder = {"id": 7, "kind": "reminder_daily", "created_at": now}
        profile = {"day_ends_at": (now + 3600) * 1000, "_day_status": "pending", "streak": 0}
        eligible = self.addon.delivery._eligible
        self.assertTrue(eligible(reminder, now, profile, {"gentle_daily": True}))
        profile["_day_status"] = "syncing"
        self.assertFalse(eligible(reminder, now, profile, {"gentle_daily": True}))

    def test_local_warning_is_not_consumed_during_quiet_hours(self):
        self.inbox.clear()
        now = 21 * 3600
        self.record()["reminders"] = {"quiet_start": 22, "quiet_end": 0}
        profile = {"streak_warning": {"title": "Study soon"}, "day_ends_at": (now + 7200) * 1000}
        with patch.object(self.addon.home, "_profile_for_display", return_value=profile), \
                patch.object(self.addon.time, "time", return_value=now):
            self.addon.poll()
            self.addon.tooltip.assert_not_called()
            self.record()["reminders"]["quiet_end"] = 22
            self.addon.poll()
            self.addon.poll()
        self.addon.tooltip.assert_called_once()


if __name__ == "__main__":
    unittest.main()
