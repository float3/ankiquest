"""Desktop deck progress checks; run with python -m unittest discover -s tests."""

import importlib.util
import sqlite3
import sys
import unittest
from concurrent.futures import Future
from pathlib import Path
from types import ModuleType, SimpleNamespace
from unittest.mock import Mock, patch


ADDON = Path(__file__).resolve().parents[1] / "addon"


def load_module(name, path, package=False):
    spec = importlib.util.spec_from_file_location(
        name, path, submodule_search_locations=[str(ADDON)] if package else None
    )
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


progress = load_module("ankiquest_progress", ADDON / "deck_completion.py")
client = load_module("ankiquest_client", ADDON / "client.py")


def node(deck_id, new=0, review=0, learn=0, intraday=0, children=()):
    return SimpleNamespace(
        deck_id=deck_id, new_count=new, review_count=review, learn_count=learn,
        intraday_learning=intraday, children=children,
    )


class DeckSnapshotTests(unittest.TestCase):
    def setUp(self):
        self.db = sqlite3.connect(":memory:")
        self.addCleanup(self.db.close)
        self.db.executescript(
            "create table cards (id integer, did integer, odid integer, queue integer, due integer);"
            "create table revlog (id integer, cid integer, ease integer, type integer);"
        )
        self.day = 20_000
        self.start = self.day * progress.DAY_MS + 2 * 3_600_000
        self.end = self.start + progress.DAY_MS
        self.now = self.start + 3_600_000
        self.decks = Mock()
        self.decks.all_names_and_ids.return_value = [
            SimpleNamespace(id=1, name="Spanish"),
            SimpleNamespace(id=2, name="Spanish::Verbs"),
            SimpleNamespace(id=3, name="Geography"),
        ]
        self.scheduler = SimpleNamespace(
            deck_due_tree=Mock(return_value=node(0, children=[
                node(1, children=[node(2)]), node(3), node(9),
            ])),
            today=100,
        )
        self.col = SimpleNamespace(
            decks=self.decks, sched=self.scheduler,
            db=SimpleNamespace(all=lambda sql, *args: self.db.execute(sql, args).fetchall()),
        )

    def card(self, cid, did, queue=2, due=101, odid=0):
        self.db.execute("insert into cards values (?, ?, ?, ?, ?)", (cid, did, odid, queue, due))

    def review(self, cid, timestamp=None, ease=3, kind=1):
        self.db.execute("insert into revlog values (?, ?, ?, ?)", (
            self.now if timestamp is None else timestamp, cid, ease, kind,
        ))

    def snapshots(self):
        return {d["id"]: d for d in progress.deck_snapshots(self.col, self.now, -120, 4)}

    def test_only_limits_reported_decks_but_keeps_subdeck_totals(self):
        decks = progress.deck_snapshots(self.col, self.now, -120, 4, only={"1"})
        self.assertEqual([d["id"] for d in decks], ["1"])

    def test_complete_decks_are_independent_and_parents_include_descendants(self):
        self.card(10, 2)
        self.review(10)
        self.scheduler.deck_due_tree.return_value.children[1] = node(3, review=4)
        snapshots = self.snapshots()
        self.assertEqual(snapshots["1"]["reviewed_today"], 1)
        self.assertEqual(snapshots["2"]["reviewed_today"], 1)
        self.assertEqual(snapshots["1"]["remaining"], 0)
        self.assertEqual(snapshots["3"]["remaining"], 4)
        self.assertEqual(snapshots["3"]["reviewed_today"], 0)
        self.assertEqual(snapshots["2"]["name"], "Spanish::Verbs")
        self.assertEqual(snapshots["1"]["day"], self.day)
        self.decks.all_names_and_ids.assert_called_once_with(include_filtered=False)
        self.assertNotIn("9", snapshots)

    def test_daily_limit_counts_are_not_replaced_with_raw_due_card_counts(self):
        self.card(10, 1, due=99)
        self.card(11, 1, queue=0, due=0)
        self.review(10)
        self.assertEqual(self.snapshots()["1"]["remaining"], 0)

    def test_learning_due_later_today_prevents_premature_completion(self):
        self.card(10, 2, queue=1, due=self.now // 1000 + 3600)
        self.review(10)
        snapshots = self.snapshots()
        self.assertEqual(snapshots["1"]["remaining"], 1)
        self.assertEqual(snapshots["2"]["remaining"], 1)

    def test_learning_steps_already_in_tree_are_not_counted_twice(self):
        self.card(10, 2, queue=1, due=self.now // 1000)
        self.card(11, 2, queue=1, due=self.now // 1000 + 3600)
        self.scheduler.deck_due_tree.return_value.children[0] = node(
            1, learn=2, children=[node(2, learn=2, intraday=1)]
        )
        snapshots = self.snapshots()
        # One intraday card already counted, one later today, one interday.
        self.assertEqual(snapshots["1"]["remaining"], 3)
        self.assertEqual(snapshots["2"]["remaining"], 3)

    def test_future_days_and_suspended_learning_do_not_block_completion(self):
        self.card(10, 1, queue=1, due=self.end // 1000)
        self.card(11, 1, queue=-1, due=self.now // 1000)
        self.card(12, 1, queue=-2, due=self.now // 1000)
        self.assertEqual(self.snapshots()["1"]["remaining"], 0)

    def test_learning_lookahead_past_rollover_does_not_block_today(self):
        self.card(10, 1, queue=1, due=self.end // 1000)
        self.scheduler.deck_due_tree.return_value.children[0] = node(1, learn=1, intraday=1)
        self.assertEqual(self.snapshots()["1"]["remaining"], 0)

    def test_filtered_reviews_and_outstanding_cards_use_original_deck(self):
        self.card(10, 9, odid=2, due=99)
        self.card(11, 9, odid=2, queue=1, due=self.now // 1000 + 3600)
        self.card(12, 9, odid=3, due=101)
        self.review(10, kind=3)
        snapshots = self.snapshots()
        self.assertEqual(snapshots["1"]["reviewed_today"], 1)
        self.assertEqual(snapshots["2"]["reviewed_today"], 1)
        self.assertEqual(snapshots["1"]["remaining"], 2)
        self.assertEqual(snapshots["2"]["remaining"], 2)
        self.assertEqual(snapshots["3"]["remaining"], 0)

    def test_only_real_reviews_in_current_study_day_are_counted(self):
        self.card(10, 1)
        for timestamp in (self.start - 1, self.start, self.end - 1, self.end):
            self.review(10, timestamp)
        self.review(10, ease=0)
        self.review(10, kind=4)
        self.review(99)  # deleted card
        self.assertEqual(self.snapshots()["1"]["reviewed_today"], 2)

    def test_missing_counts_never_imply_completion(self):
        self.scheduler.deck_due_tree.return_value.children = []
        self.card(10, 1)
        self.review(10)
        self.assertGreater(self.snapshots()["1"]["remaining"], 0)
        self.scheduler.deck_due_tree.return_value = None
        with self.assertRaises(ValueError):
            self.snapshots()

    def test_study_day_changes_at_local_rollover(self):
        self.assertEqual(progress.study_day(self.start - 1, -120, 4), self.day - 1)
        self.assertEqual(progress.study_day(self.start, -120, 4), self.day)
        self.assertEqual(progress.study_day(self.end, -120, 4), self.day + 1)


class UploadTests(unittest.TestCase):
    def test_optional_decks_keep_existing_upload_shape(self):
        api = client.Client("https://example.test", "cerro", "token")
        api._request = Mock()
        api.upload([], 4, False)
        self.assertNotIn("decks", api._request.call_args.args[1])
        decks = [{"id": "1", "name": "Spanish", "remaining": 0, "reviewed_today": 4, "day": 20000}]
        api.upload([], 4, False, decks=decks, clock_offset=-120)
        body = api._request.call_args.args[1]
        self.assertEqual(body["decks"], decks)
        self.assertEqual(body["clock"], {"offset_west_min": -120, "rollover_hour": 4})


class RefreshTests(unittest.TestCase):
    def setUp(self):
        self.api = SimpleNamespace(
            configured=True, base="https://example.test", user="cerro", upload=Mock(return_value={})
        )
        self.pending = [[(101, 1, 1, 1000, 1)]]
        self.mw = SimpleNamespace(
            col=SimpleNamespace(get_config=lambda *args: 4, db=SimpleNamespace(all=self.read_rows)),
            pm=SimpleNamespace(profile={"ankiquestUploadedThrough": 100, "ankiquestSharedDecks": ["1"]}),
            taskman=SimpleNamespace(run_in_background=self.run_task),
            form=SimpleNamespace(menuTools=SimpleNamespace(addAction=Mock())),
        )
        aqt = ModuleType("aqt")
        aqt.mw = self.mw
        aqt.gui_hooks = SimpleNamespace(**{name: [] for name in (
            "reviewer_did_answer_card", "operation_did_execute", "sync_did_finish", "profile_did_open",
        )})
        utils = ModuleType("aqt.utils")
        utils.tooltip = Mock()
        utils.openLink = Mock()
        qt = ModuleType("aqt.qt")
        qt.QAction = Mock()
        with patch.dict(sys.modules, {"aqt": aqt, "aqt.utils": utils, "aqt.qt": qt}):
            self.addon = load_module("ankiquest_test_addon", ADDON / "__init__.py", package=True)
        self.addon.client = lambda: self.api
        self.addon.snapshot = lambda _: {}
        self.addon.offset_west_min = lambda: -120
        self.decks = [{"id": "1", "name": "Spanish", "remaining": 0, "reviewed_today": 1, "day": 20000}]
        self.addon.deck_snapshots = Mock(return_value=self.decks)

    def read_rows(self, sql, since):
        # First read reconciles the recent undo window; others paginate uploads.
        if since > 1000:
            return []
        return self.pending.pop(0) if self.pending else []

    def run_task(self, work, done, uses_collection):
        self.assertTrue(uses_collection)
        future = Future()
        try:
            future.set_result(work())
        except Exception as error:
            future.set_exception(error)
        done(future)

    def test_no_shared_decks_skips_deck_progress(self):
        self.mw.pm.profile["ankiquestSharedDecks"] = []
        self.addon.refresh(False)
        self.addon.deck_snapshots.assert_not_called()
        self.assertIsNone(self.api.upload.call_args.kwargs["decks"])

    def test_only_shared_decks_are_reported(self):
        self.addon.refresh(False)
        self.assertEqual(self.addon.deck_snapshots.call_args.kwargs, {"only": {"1"}})

    def test_deck_notifications_menu_sends_full_catalog_and_opens_dashboard(self):
        self.api.shared_decks = Mock(return_value=["1", "3"])
        self.addon.open_deck_notifications()
        self.assertEqual(self.addon.deck_snapshots.call_args.kwargs, {})
        self.assertTrue(self.api.upload.call_args.kwargs["catalog"])
        self.assertEqual(self.mw.pm.profile["ankiquestSharedDecks"], ["1", "3"])
        self.addon.openLink.assert_called_once_with("https://example.test/#cerro")

    def test_snapshot_uses_same_clock_as_upload(self):
        self.addon.refresh(False)
        self.assertEqual(self.api.upload.call_args.kwargs, {"decks": self.decks, "clock_offset": -120})
        self.assertEqual(self.addon.deck_snapshots.call_args.args[2:], (-120, 4))

    def test_full_silent_chunks_do_not_consume_completion(self):
        self.addon.MAX_PENDING = 1
        self.addon.refresh(False)
        calls = self.api.upload.call_args_list
        self.assertEqual(len(calls), 2)
        self.assertTrue(calls[0].args[2])
        self.assertIsNone(calls[0].kwargs["decks"])
        self.assertFalse(calls[1].args[2])
        self.assertEqual(calls[1].kwargs["decks"], self.decks)

    def test_initial_history_import_is_silent(self):
        self.mw.pm.profile["ankiquestUploadedThrough"] = 0
        self.addon.refresh(False, resync=True)
        self.assertTrue(self.api.upload.call_args.args[2])

    def test_first_review_after_successful_empty_baseline_can_notify(self):
        self.mw.pm.profile["ankiquestUploadedThrough"] = 0
        self.pending = []
        self.addon.refresh(False, resync=True)
        self.assertTrue(self.api.upload.call_args.args[2])
        self.assertEqual(self.mw.pm.profile["ankiquestUploadedThrough"], 0)
        self.pending = [[(101, 1, 1, 1000, 1)]]
        self.addon.refresh(False)
        self.assertFalse(self.api.upload.call_args.args[2])

    def test_successful_empty_baseline_is_scoped_to_account(self):
        self.mw.pm.profile["ankiquestUploadedThrough"] = 0
        self.pending = []
        self.addon.refresh(False, resync=True)
        self.api.user = "other-player"
        self.pending = [[(101, 1, 1, 1000, 1)]]
        self.addon.refresh(False)
        self.assertTrue(self.api.upload.call_args.args[2])

    def test_unavailable_counts_do_not_stop_review_uploads_or_report_completion(self):
        self.addon.deck_snapshots.side_effect = RuntimeError("unavailable")
        with patch("builtins.print"):
            self.addon.refresh(False)
        self.assertIsNone(self.api.upload.call_args.kwargs["decks"])
        self.assertEqual(self.mw.pm.profile["ankiquestUploadedThrough"], 101)

    def test_rollover_during_snapshot_omits_inconsistent_progress(self):
        start = 20000 * progress.DAY_MS + 2 * 3_600_000
        with patch.object(self.addon.time, "time", side_effect=[start / 1000 - 1, start / 1000 - 1, start / 1000]):
            self.addon.refresh(False)
        self.assertIsNone(self.api.upload.call_args.kwargs["decks"])


if __name__ == "__main__":
    unittest.main()
