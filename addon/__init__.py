import time

from aqt import gui_hooks, mw
from aqt.qt import QAction
from aqt.utils import openLink, tooltip

from .client import (
    MAX_PENDING,
    PENDING_SQL,
    UNDO_WINDOW_MS,
    Client,
    describe,
    offset_west_min,
    reconcile,
    rows_to_reviews,
    snapshot,
)
from .deck_completion import deck_snapshots, study_day

MARK_KEY = "ankiquestUploadedThrough"
RECENT_KEY = "ankiquestRecentUploads"
BASELINE_KEY = "ankiquestBaselineAccount"
SHARED_KEY = "ankiquestSharedDecks"
RESYNC_WINDOW_MS = 7 * 86_400_000

state = {"previous": None, "busy": False, "again": False}


def client():
    config = mw.addonManager.getConfig(__name__) or {}
    return Client(config.get("url", ""), config.get("user", ""), config.get("token", ""))


def refresh(show_feedback, resync=False):
    if mw.col is None:
        return
    if state["busy"]:
        state["again"] = True
        return
    api = client()
    if not api.configured:
        return
    state["busy"] = True
    rollover = int(mw.col.get_config("rollover", 4))
    mark = int(mw.pm.profile.get(MARK_KEY, 0))
    account = [api.base, api.user]
    initial_sync = mark == 0 and mw.pm.profile.get(BASELINE_KEY) != account
    start = max(0, mark - RESYNC_WINDOW_MS) if resync else mark
    window_start = int(time.time() * 1000) - UNDO_WINDOW_MS
    recent = {i for i in mw.pm.profile.get(RECENT_KEY, []) if i > window_start}
    shared = set(mw.pm.profile.get(SHARED_KEY, []))

    def work():
        window = mw.col.db.all(PENDING_SQL, window_start)
        present, deleted, restored = reconcile(window, recent, start, mark)
        known = start
        first = True
        while True:
            rows = mw.col.db.all(PENDING_SQL, known)
            full = len(rows) == MAX_PENDING
            if rows:
                known = rows[-1][0]
            sent = rows + restored if first else rows
            decks = None
            clock_offset = offset_west_min()
            if not full and shared:
                try:
                    now_ms = int(time.time() * 1000)
                    decks = deck_snapshots(mw.col, now_ms, clock_offset, rollover, only=shared)
                    # Do not send mixed-day counts if a rollover occurred while
                    # reading them. The next refresh will collect the new day.
                    if study_day(int(time.time() * 1000), clock_offset, rollover) != study_day(
                        now_ms, clock_offset, rollover
                    ):
                        decks = None
                except Exception as e:
                    print("ankiquest deck progress:", e)
            profile = api.upload(
                rows_to_reviews(sent), rollover, initial_sync or full, deleted if first else (),
                decks=decks, clock_offset=clock_offset,
            )
            first = False
            if not full:
                return max(known, mark), sorted(present), profile

    def done(future):
        state["busy"] = False
        try:
            mw.pm.profile[MARK_KEY], mw.pm.profile[RECENT_KEY], profile = future.result()
            mw.pm.profile[BASELINE_KEY] = account
        except Exception as e:
            print("ankiquest:", e)
            return
        after = snapshot(profile)
        before, state["previous"] = state["previous"], after
        message = describe(before, after) if before else None
        if show_feedback and message:
            text, important = message
            tooltip(text, period=3500 if important else 1800)
        if state["again"]:
            state["again"] = False
            refresh(True)

    mw.taskman.run_in_background(work, done, uses_collection=True)


def refresh_shared_decks():
    api = client()
    if not api.configured:
        return

    def done(future):
        try:
            mw.pm.profile[SHARED_KEY] = future.result()
        except Exception as e:
            print("ankiquest shared decks:", e)

    mw.taskman.run_in_background(api.shared_decks, done)


def open_deck_notifications():
    api = client()
    if mw.col is None or not api.configured:
        tooltip("Set url, user and token in the ankiquest add-on config first.")
        return
    rollover = int(mw.col.get_config("rollover", 4))

    def work():
        clock_offset = offset_west_min()
        catalog = deck_snapshots(mw.col, int(time.time() * 1000), clock_offset, rollover)
        api.upload([], rollover, False, decks=catalog, clock_offset=clock_offset, catalog=True)
        return api.shared_decks()

    def done(future):
        try:
            mw.pm.profile[SHARED_KEY] = future.result()
        except Exception as e:
            tooltip("ankiquest: could not send your deck list (%s)" % e)
            return
        openLink("%s/#%s" % (api.base, api.user))

    mw.taskman.run_in_background(work, done, uses_collection=True)


def on_operation(changes, handler):
    if handler is mw.reviewer:
        return
    if getattr(getattr(changes, "changes", changes), "study_queues", False):
        refresh(mw.state == "review")


def on_profile_open():
    state["previous"] = None
    refresh_shared_decks()
    refresh(False, resync=True)


def on_sync():
    refresh_shared_decks()
    refresh(False, resync=True)


deck_action = QAction("ankiquest deck notifications…", mw)
deck_action.triggered.connect(open_deck_notifications)
mw.form.menuTools.addAction(deck_action)


gui_hooks.reviewer_did_answer_card.append(lambda *_: refresh(True))
gui_hooks.operation_did_execute.append(on_operation)
gui_hooks.sync_did_finish.append(on_sync)
gui_hooks.profile_did_open.append(on_profile_open)
