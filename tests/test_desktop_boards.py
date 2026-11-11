"""The server owns ranks, scopes and the privacy of global rows."""
import json
from dataclasses import replace
from http import HTTPStatus
from pathlib import Path
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

import test_desktop_today as fixture

EXAMPLES = Path(__file__).resolve().parents[1] / "docs/api/examples"


def example(name):
    return json.loads((EXAMPLES / (name + ".json")).read_text(encoding="utf-8"))


class Boards(unittest.TestCase):
    setUp = fixture.HomeTests.setUp
    run_task = fixture.HomeTests.run_task
    write_config = fixture.HomeTests.write_config
    render = fixture.HomeTests.render

    def test_the_server_rank_and_xp_survive_summary_only_rows(self):
        rows = example("leaderboard_global")
        rows[1]["rank"] = 12
        self.api.leaderboard.return_value = rows
        self.mw.pm.profile[self.home.PERIOD_KEY] = "day"
        found = self.home.board_view(self.api, {"_day_status": "met"})
        self.assertEqual([(1, 107), (12, 20)], [(place, row["xp"]) for place, row, _ in found["rows"]])
        self.api.leaderboard.assert_called_once_with(period="day", scope="friends")

    def test_a_hidden_own_row_explains_its_visibility(self):
        self.api.name = self.api.user = "ana"
        self.api.leaderboard.return_value = example("leaderboard_global_hidden")
        html = self.render().stats
        self.assertIn("you, not shown to others", html)
        self.assertIn("246 XP", html)
        self.assertIn("31 XP", html)

    def test_scope_and_period_belong_to_the_account_and_survive_sign_out(self):
        data = self.home.board_data
        first = data._chosen(self.mw.pm.profile, self.api)
        data._remember(self.mw.pm.profile, self.api, replace(first, scope="global", period="month"))
        self.home.clear_account()
        self.assertEqual("global", data._chosen(self.mw.pm.profile, self.api).scope)
        other = SimpleNamespace(base=self.api.base, name="ana")
        self.assertEqual(data._Query(), data._chosen(self.mw.pm.profile, other))
        other.base = "https://another.test"
        other.name = self.api.name
        self.assertEqual(data._Query(), data._chosen(self.mw.pm.profile, other))
        self.assertEqual("month", data._chosen(self.mw.pm.profile, self.api).period)
        self.assertNotIn(self.api.token, json.dumps(self.mw.pm.profile))

    def test_old_servers_hide_global_and_current_group_names_are_escaped(self):
        data = self.home.board_data
        self.api._request.return_value = {"groups": [{"id": 4, "name": "<Study & friends>"}]}
        data._remember(self.mw.pm.profile, self.api, data._Query("week", "global"))
        html = self.render().stats
        self.assertNotIn('value="global"', html)
        self.assertIn("&lt;Study &amp; friends&gt;", html)
        self.api.leaderboard.assert_called_once_with(period="week", scope="friends")
        self.api.deck_settings.return_value = {"global_leaderboard": False}
        self.home.refresh_board()
        self.assertIn('value="global"', self.render().stats)

    def test_the_board_choice_follows_a_verified_server_address_move(self):
        data = self.home.board_data
        chosen = data._Query("year", "group:4")
        data._remember(self.mw.pm.profile, self.api, chosen)
        self.home.address.move_profile(self.mw.pm.profile, self.api.base, "https://new.test")
        self.api.base = "https://new.test"
        self.assertEqual(chosen, data._chosen(self.mw.pm.profile, self.api))

    def test_a_late_period_or_account_reply_cannot_replace_the_new_board(self):
        tasks = []
        self.mw.taskman.run_in_background = lambda work, done: tasks.append((work, done))
        self.home.refresh_board()
        self.home.on_js_message(False, "ankiquest:period:month", self.mw.deckBrowser)
        self.run_task(*tasks.pop(1))
        newest = self.home.local["board"]
        self.run_task(*tasks.pop(0))
        self.assertIs(newest, self.home.local["board"])
        self.assertEqual("month", newest.query.period)
        self.home.refresh_board()
        self.api.token = "another-account-token"
        self.run_task(*tasks.pop(0))
        self.assertIs(newest, self.home.local["board"])

    def test_global_keeps_a_distant_own_rank_after_gaps(self):
        data = self.home.board_data
        rows = [{"user": "p%d" % i, "rank": i * 3, "xp": 50 - i} for i in range(1, 8)]
        rows.append({"user": "hill", "rank": 140, "xp": 1, "shown": False})
        found = data._Board(data._Query(scope="global"), rows, [], True)
        selected = data._top(found, "hill")
        self.assertEqual([3, 6, 9, 12, 15, 140], [place for place, _, _ in selected])
        self.assertTrue(selected[-1][2])

    def test_avatar_requests_are_bounded_cached_and_account_owned(self):
        pictures = self.home.board_pictures
        tasks, redraw = [], Mock()
        self.mw.taskman.run_in_background = lambda work, done: tasks.append((work, done))
        cache = pictures._Pictures(self.addon)
        cache._load(["ana", "ana"], redraw)
        cache._load(["ana"], redraw)
        self.assertEqual(1, len(tasks))
        with patch.object(pictures, "_avatar", return_value=b"png") as read:
            self.run_task(*tasks.pop())
            read.assert_called_once_with(self.api, "ana")
        self.assertTrue(cache._get("ana").startswith("data:image/png;base64,"))
        redraw.assert_called_once_with()
        cache._load(["ana"], redraw)
        self.assertEqual([], tasks)
        cache._load(["bo"], redraw)
        self.mw.pm.profile = {}
        with patch.object(pictures, "_avatar", return_value=b"new"):
            self.run_task(*tasks.pop())
        self.assertIsNone(cache._get("bo"))
        redraw.assert_called_once_with()

    def test_avatar_auth_never_follows_a_redirect(self):
        from urllib.error import HTTPError
        from urllib.request import Request
        handler = self.home.board_pictures._NoRedirect()
        request = Request("https://example.test/api/avatar/ana", headers={"Authorization": "Bearer secret"})
        with self.assertRaises(HTTPError):
            handler.redirect_request(request, None, HTTPStatus.FOUND, "Found", {}, "https://elsewhere.test/")

    def test_profile_close_discards_waiting_board_and_avatar_replies(self):
        tasks, redraw = [], Mock()
        self.mw.taskman.run_in_background = lambda work, done: tasks.append((work, done))
        self.addon.redraw = redraw
        self.home.refresh_board()
        pictures = self.home._pictures()
        pictures._load(["ana"], redraw)
        self.home._clear_boards()
        for task in tasks:
            self.run_task(*task)
        self.assertIsNone(self.home.local["board"])
        redraw.assert_not_called()

    def test_a_refresh_timer_does_nothing_without_an_open_profile(self):
        self.mw.pm.profile = None
        self.mw.col = None
        self.home.refresh_board()
        self.api.leaderboard.assert_not_called()

    def test_avatar_request_quotes_username_and_keeps_token_out_of_the_url(self):
        pictures = self.home.board_pictures
        with patch.object(pictures.share, "download_image", return_value=b"png") as download:
            pictures._avatar(self.api, "ana/?x=1")
        call = download.call_args
        self.assertEqual("https://example.test/api/avatar/ana%2F%3Fx%3D1", call.args[0])
        self.assertEqual("secret", call.kwargs["token"])
        self.assertLessEqual(call.kwargs["limit"], 512 * 1024)


if __name__ == "__main__":
    unittest.main()
