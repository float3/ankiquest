"""Exercise the real Qt window, event loop and delayed account-scoped replies."""

from concurrent.futures import Future
from copy import deepcopy
from types import SimpleNamespace
import time
import tempfile
from pathlib import Path
import unittest
from unittest.mock import Mock, patch

from aqt.qt import QApplication, QPushButton, QScrollArea, QWidget
from anki.collection import Collection

from addon_loader import load_in_package

app = QApplication.instance() or QApplication([])
ui = load_in_package("moments_qt", "moments_ui")
ui.tr = ui.conquests_ui.tr = ui.share_ui.tr = lambda text: text


class Moments(unittest.TestCase):
    def setUp(self):
        self.found = {"conquest": {"at": 50, "since": 0, "lapses": 5, "answers": 20},
                      "label": None, "told": False, "friends": 2, "win": 7}
        self.requests, self.tasks = [], []
        self.defer = False
        self.api = SimpleNamespace(configured=True, base="https://quest.test", user="sam", token="token",
                                   deck_settings=lambda: {"celebrations": True}, _request=self.request)
        self.mw = QWidget()
        self.mw.state = "review"
        self.mw.pm = SimpleNamespace(profile={})
        note = SimpleNamespace(fields=["B1L02", "Hund"], note_type=lambda: {"type": 0})
        card = SimpleNamespace(question=lambda: "<b>der Hund</b>", note=lambda: note, did=5, odid=0)
        self.mw.col = SimpleNamespace(db=SimpleNamespace(scalar=lambda *args: 1), get_card=lambda cid: card,
                                      decks=SimpleNamespace(name=lambda did: "German"))
        self.mw.taskman = SimpleNamespace(run_in_background=self.run_task)
        self.addon = SimpleNamespace(mw=self.mw, client=lambda: self.api, current_companion=lambda: "aki",
                                     config=lambda: {},
                                     save_config=Mock(), redraw=Mock(), open_inbox=Mock(),
                                     home=SimpleNamespace(open_progress=Mock(), continue_studying=Mock()))
        self.presenter = ui._Presenter(self.addon)

    def tearDown(self):
        self.presenter._clear()
        self.mw.close()
        app.processEvents()

    def request(self, path, body=None, method=None):
        self.requests.append((path, body, method))
        if path.endswith("/label"):
            self.found["label"] = body["label"] if body else None
            return None
        if path.endswith("/tell"):
            self.found["told"] = method != "DELETE"
            self.found["undo_until"] = int(time.time() * 1000) + 10000
            return {"told": 2, "retracted": 2}
        return deepcopy(self.found)

    def run_task(self, work, done, uses_collection=False):
        if self.defer:
            self.tasks.append((work, done))
        else:
            self.complete((work, done))

    def complete(self, task):
        work, done = task
        future = Future()
        try:
            future.set_result(work())
        except Exception as error:
            future.set_exception(error)
        done(future)

    def flush(self):
        for _ in range(3):
            app.processEvents()

    def item(self, key="level:4", topic="level"):
        return {"key": key, "topic": topic, "at": 50, "title": "Level 4!", "body": "You reached level 4."}

    def offer(self, item=None, summary=None):
        self.presenter._offer(self.api, [item or self.item()], summary)
        self.flush()

    def transition(self, new, old):
        self.mw.state = new
        self.presenter._transition(new, old)
        self.flush()

    def conquest(self):
        self.offer(self.item("conquered:50", "conquered"))
        self.transition("overview", "review")
        return self.presenter._card._conquest

    def test_review_defers_every_win_and_one_card_merges_the_recap(self):
        self.offer()
        self.offer(self.item("goal:2", "goal"))
        self.assertIsNone(self.presenter._card)
        self.transition("overview", "review")
        card = self.presenter._card
        self.assertEqual("and 1 more", card._more.text())
        self.offer(self.item("goal:2", "goal"), {"reviews": 12, "minutes": 2, "remaining": 3})
        self.assertIs(card, self.presenter._card)
        self.assertIn("12 reviews", card._session.text())
        card._quiet.click()
        self.addon.save_config.assert_called_once_with({"session_summary": False})
        card._continue.click()
        self.addon.home.continue_studying.assert_called_once_with()
        self.assertEqual(0, self.presenter._count())

    def test_starting_review_defers_an_open_card_without_consuming_it(self):
        self.offer()
        self.transition("deckBrowser", "review")
        first = self.presenter._card
        self.transition("review", "deckBrowser")
        self.assertIsNone(self.presenter._card)
        self.assertEqual(1, self.presenter._count())
        self.transition("overview", "review")
        self.assertIsNot(first, self.presenter._card)
        self.presenter._card._dialog.close()
        self.offer(self.item("goal:2", "goal"))
        self.assertIsNone(self.presenter._card, "late wins wait after the session's one card was dismissed")
        self.presenter._open()
        self.assertEqual("goal:2", self.presenter._card._items[0]["key"])

    def test_a_late_permission_reply_cannot_cross_profiles_or_accounts(self):
        self.defer = True
        self.offer()
        old = self.mw.pm.profile
        self.mw.pm.profile = {}
        self.transition("overview", "review")
        self.complete(self.tasks.pop(0))
        self.flush()
        self.assertIsNone(self.presenter._card)
        self.assertEqual(1, len(ui.moments._pending(old, (self.api.base, self.api.user))))
        self.mw.pm.profile = old
        self.api = SimpleNamespace(**dict(vars(self.api), user="other"))
        self.assertEqual(0, self.presenter._count())

    def test_a_new_session_waits_for_the_current_celebration_preference(self):
        self.offer()
        self.transition("overview", "review")
        self.presenter._card._dialog.close()
        self.transition("review", "overview")
        self.defer = True
        self.api.deck_settings = lambda: {"celebrations": False}
        self.offer(self.item("level:5"))
        self.transition("overview", "review")
        self.assertIsNone(self.presenter._card)
        self.complete(self.tasks.pop(0))
        self.flush()
        self.assertIsNone(self.presenter._card)
        self.assertEqual(1, self.presenter._count())

    def test_disabling_daily_cues_keeps_the_server_streak_in_the_badge(self):
        self.addon.config = lambda: {"goal_celebrations": False}
        self.offer(self.item("secured:2", "secured"))
        self.transition("overview", "review")
        self.assertIsNone(self.presenter._card)
        self.assertEqual(1, self.presenter._count())
        self.presenter._open()
        self.assertIsNotNone(self.presenter._card)

    def test_disabled_server_celebrations_do_not_swallow_a_session_recap(self):
        self.api.deck_settings = lambda: {"celebrations": False}
        self.offer()
        summary = {"reviews": 2, "minutes": 1, "remaining": 3}
        self.offer(dict(self.item("session:50", "session"), local=True, summary=summary), summary)
        self.transition("overview", "review")
        card = self.presenter._card
        self.assertIsNotNone(card)
        self.assertEqual("session:50", card._choices.currentData())
        card._dialog.close()
        self.assertEqual(1, self.presenter._count(), "the disabled server win remains available manually")

    def test_conquest_load_is_read_only_and_a_new_win_keeps_the_draft_label(self):
        control = self.conquest()
        self.assertEqual("der Hund", control._label.text())
        self.assertEqual([("/api/conquests/sam/50", None, None)], self.requests)
        self.assertIn("5 lapses · 20 answers", control._stats.text())
        control._label.setText("my unsaved words")
        control._label.setModified(True)
        self.offer(self.item("level:5"))
        self.assertIs(control, self.presenter._card._conquest)
        self.assertEqual("my unsaved words", control._label.text())
        control._copy.click()
        self.assertNotIn("my unsaved words", QApplication.clipboard().text())
        control._save.click()
        self.assertIn(("/api/conquests/sam/50/label", {"label": "my unsaved words"}, None), self.requests)
        control._remove.click()
        self.assertIn(("/api/conquests/sam/50/label", None, "DELETE"), self.requests)

    def test_tell_and_undo_use_the_conquest_endpoint_and_conflict_closes_the_window(self):
        control = self.conquest()
        control._tell.click()
        self.assertTrue(control._undo.isEnabled())
        self.assertFalse(control._tell.isEnabled())
        error = RuntimeError("a friend answered")
        error.status = 409
        self.api._request = Mock(side_effect=error)
        control._undo.click()
        self.assertFalse(control._undo.isEnabled())
        self.assertIn("too late", self.presenter._card._status.text())
        self.api._request.assert_called_once_with("/api/conquests/sam/50/tell", method="DELETE")

    def test_an_expired_undo_never_sends_a_delete(self):
        self.found.update(told=True, undo_until=int(time.time() * 1000) - 1)
        control = self.conquest()
        self.assertFalse(control._undo.isEnabled())
        control._retract()
        self.assertEqual(1, len(self.requests))

    def test_a_picture_can_be_requested_again_when_the_worker_has_not_created_it(self):
        self.found["win"] = None
        control = self.conquest()
        self.assertFalse(control._image.isEnabled())
        self.found["win"] = 9
        control._retry.click()
        self.assertTrue(control._image.isEnabled())
        control._copy.click()
        self.assertIn("I learned a difficult card in German.", QApplication.clipboard().text())
        self.assertIn("/conquered/sam/50", QApplication.clipboard().text())

    def test_late_replies_cannot_write_a_closed_or_switched_card(self):
        control = self.conquest()
        self.defer = True
        control._save.click()
        self.presenter._card._dialog.close()
        self.complete(self.tasks.pop(0))
        self.assertEqual([], self.tasks, "no follow-up load from a closed card")
        self.assertIsNone(self.presenter._card)

    def test_a_later_server_row_adds_sharing_to_a_local_moment(self):
        item = dict(self.item("secured:2", "secured"), local=True)
        self.mw.state = "overview"
        self.offer(item)
        card = self.presenter._card
        self.assertNotIn("Tell friends", [button.text() for button in card._dialog.findChildren(QPushButton)])
        enriched = dict(item, share={"title": "My streak", "body": "Today counts", "win": 7})
        self.offer(enriched)
        self.assertIs(card, self.presenter._card)
        self.assertIn("Tell friends", [button.text() for button in card._dialog.findChildren(QPushButton)])

    def test_switching_accounts_drops_an_old_action_reply_without_consuming_the_win(self):
        control = self.conquest()
        old_profile, old_api = self.mw.pm.profile, self.api
        self.defer = True
        control._save.click()
        self.api = SimpleNamespace(**dict(vars(self.api), token="new-token"))
        self.complete(self.tasks.pop(0))
        self.assertEqual([], self.tasks)
        self.presenter._sync_owner()
        self.assertIsNone(self.presenter._card)
        self.assertEqual(1, len(ui.moments._pending(old_profile, (old_api.base, old_api.user))))

    def test_expanded_actions_fit_a_small_window_with_a_fixed_footer(self):
        control = self.conquest()
        card = self.presenter._card
        control._share.click()
        card._dialog.resize(480, 640)
        self.flush()
        scroll = card._dialog.findChild(QScrollArea)
        self.assertEqual(0, scroll.horizontalScrollBar().maximum())
        done = next(button for button in card._dialog.findChildren(QPushButton) if button.text() == "Done")
        self.assertTrue(done.isVisible())
        self.assertIs(card._dialog, done.parentWidget())

    def test_real_anki_templates_and_clozes_produce_the_local_label(self):
        with tempfile.TemporaryDirectory() as folder:
            col = Collection(str(Path(folder) / "collection.anki2"))
            try:
                model = col.models.new("Vocabulary")
                for name in ("ID", "German", "English"):
                    col.models.add_field(model, col.models.new_field(name))
                template = col.models.new_template("Card")
                template.update(qfmt="{{German}}", afmt="{{FrontSide}}<hr>{{English}}")
                col.models.add_template(model, template)
                col.models.add(model)
                note = col.new_note(model)
                note.fields = ["B1L02", "<b>der&nbsp;Hund</b>[sound:hund.mp3]", "the dog"]
                col.add_note(note, 1)
                cloze = col.new_note(col.models.by_name("Cloze"))
                cloze.fields[0] = "{{c1::Paris::city}} is the capital of France"
                col.add_note(cloze, 1)
                for at, entry, expected in ((50, note, "der Hund"), (51, cloze, "Paris is the capital of France")):
                    cid = entry.cards()[0].id
                    col.db.execute("insert into revlog values (?, ?, 0, 3, 25, 8, 2500, 7000, 1)", at, cid)
                    self.assertEqual(expected, ui.conquests_ui.conquests._local(col, at)[0])
            finally:
                col.close()


if __name__ == "__main__":
    unittest.main()
