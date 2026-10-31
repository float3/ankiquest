"""Desktop Today panel, daily goal, deck states and session recaps; run with python -m unittest discover -s tests."""

import ast
import importlib.util
import re
import sqlite3
import sys
import unittest
from pathlib import Path
from concurrent.futures import Future
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


goals = load_module("ankiquest_goals", ADDON / "goals.py")
moods = load_module("ankiquest_moods", ADDON / "moods.py")
today = load_module("ankiquest_today", ADDON / "today.py")
board = load_module("ankiquest_today_board", ADDON / "board.py")
language = load_module("ankiquest_today_language", ADDON / "language.py")
progress = load_module("ankiquest_today_progress", ADDON / "deck_completion.py")


def node(deck_id, name="", new=0, review=0, learn=0, intraday=0, children=()):
    return SimpleNamespace(
        deck_id=deck_id, name=name, new_count=new, review_count=review, learn_count=learn,
        intraday_learning=intraday, children=list(children),
    )


def database():
    db = sqlite3.connect(":memory:")
    db.executescript(
        "create table cards (id integer, did integer, odid integer, queue integer, due integer, odue integer);"
        "create table revlog (id integer, cid integer, ease integer, type integer, time integer);"
    )
    return db


def collection(db, tree, today_number=100, cutoff=1_000_000):
    return SimpleNamespace(
        sched=SimpleNamespace(today=today_number, day_cutoff=cutoff, deck_due_tree=lambda: tree),
        db=SimpleNamespace(
            all=lambda sql, *args: db.execute(sql, args).fetchall(),
            first=lambda sql, *args: db.execute(sql, args).fetchone(),
            scalar=lambda sql, *args: db.execute(sql, args).fetchone()[0],
        ),
    )


class GoalTests(unittest.TestCase):
    def test_defaults_match_the_phone(self):
        self.assertEqual(goals.Settings("backlog", 10, 100, 20), goals.settings({}))
        self.assertEqual("backlog", goals.settings({"goal_mode": "nonsense"}).mode)
        self.assertEqual(goals.Settings("minutes", 5, 30, 15), goals.settings(
            {"goal_mode": "minutes", "goal_backlog_percent": "5", "goal_cards": 30, "goal_minutes": 15}
        ))

    def test_backlog_share_is_at_least_one_card_while_any_is_left(self):
        self.assertEqual(0, goals.backlog_share(0, 10))
        self.assertEqual(0, goals.backlog_share(50, 0))
        self.assertEqual(1, goals.backlog_share(3, 10))
        self.assertEqual(11, goals.backlog_share(101, 10))
        self.assertEqual(40, goals.backlog_share(40, 250))

    def test_each_mode_sets_its_own_target(self):
        load = goals.Load(5, 12, 9, 30, 200)
        self.assertEqual(goals.Goal(62, 200, 20), goals.goal(load, goals.Settings("backlog", 10, 100, 20)))
        self.assertEqual(goals.Goal(100, 0, 0), goals.goal(load, goals.Settings("cards", 10, 100, 20)))
        self.assertEqual(goals.Goal(1, 0, 0), goals.goal(load, goals.Settings("minutes", 10, 100, 0)))
        self.assertIsNone(goals.goal(load, goals.Settings("off", 10, 100, 20)))

    def test_the_goal_is_fixed_for_the_day_until_the_settings_change(self):
        chosen = goals.Settings("backlog", 10, 100, 20)
        first, stored = goals.fixed_goal(goals.Load(5, 0, 0, 40, 100), chosen, None)
        self.assertEqual(50, first.target)
        later, kept = goals.fixed_goal(goals.Load(5, 30, 12, 10, 100), chosen, stored)
        self.assertEqual(first, later)
        self.assertEqual(stored, kept)
        tomorrow, _ = goals.fixed_goal(goals.Load(6, 0, 0, 10, 0), chosen, stored)
        self.assertEqual(10, tomorrow.target)
        changed, _ = goals.fixed_goal(goals.Load(5, 30, 12, 10, 100), chosen._replace(mode="cards"), stored)
        self.assertEqual(100, changed.target)
        off, record = goals.fixed_goal(goals.Load(5, 0, 0, 40, 100), chosen._replace(mode="off"), stored)
        self.assertIsNone(off)
        self.assertEqual((None, record), goals.fixed_goal(goals.Load(5, 3, 1, 40, 100), chosen._replace(mode="off"), record))

    def test_minimum_and_goal_follow_the_servers_rule(self):
        status = goals.Status(5, 4, 5, 10, "cards", 10, goals.Goal(10, 0, 0))
        self.assertFalse(status.minimum_done)
        self.assertTrue(status.goal_done)
        self.assertTrue(status._replace(minutes=5).minimum_done)
        self.assertFalse(status._replace(studied_cards=0, minutes=9, streak_minutes=0).minimum_done)
        self.assertEqual(4, status._replace(mode="minutes").progress)
        self.assertFalse(status._replace(goal=None).goal_done)

    def test_each_celebration_shows_once_a_day(self):
        chosen = goals.Settings("backlog", 10, 100, 20)
        stored, shown = None, None

        def claim(load):
            nonlocal stored, shown
            status, stored = goals.status(load, chosen, stored, 5)
            earned, shown = goals.claim(status, shown)
            return earned

        claim(goals.Load(5, 0, 0, 20, 0))
        self.assertIsNone(claim(goals.Load(5, 3, 2, 17, 0)))
        self.assertEqual(goals.MINIMUM, claim(goals.Load(5, 8, 6, 12, 0)))
        self.assertIsNone(claim(goals.Load(5, 9, 7, 11, 0)))
        self.assertEqual(goals.GOAL, claim(goals.Load(5, 20, 15, 0, 0)))
        self.assertIsNone(claim(goals.Load(5, 21, 16, 0, 0)))
        claim(goals.Load(6, 0, 0, 5, 0))
        self.assertEqual(goals.BOTH, claim(goals.Load(6, 5, 6, 0, 0)))

    def test_streak_minimum_is_remembered_per_account(self):
        me, other = ["https://quest.example", "hill"], ["https://quest.example", "cerro"]
        minutes, stored = goals.remembered_streak_minutes({"today": {"streak_minutes": 7}}, None, me)
        self.assertEqual(7, minutes)
        self.assertEqual((7, stored), goals.remembered_streak_minutes(None, stored, me))
        self.assertEqual(0, goals.remembered_streak_minutes(None, stored, other)[0])
        self.assertEqual(0, goals.remembered_streak_minutes({"today": {"streak_minutes": -3}}, stored, me)[0])

    def test_texts_say_what_is_left(self):
        status = goals.Status(5, 3, 10, 4, "cards", 10, goals.Goal(50, 0, 0))
        self.assertEqual("Streak minimum: 10 minutes of review today", goals.minimum_text(status))
        self.assertEqual("4 of 50 cards", goals.progress_text(status))
        self.assertEqual("Any review today keeps your streak going.", goals.minimum_text(status._replace(streak_minutes=0, studied_cards=0)))
        done = status._replace(minutes=10)
        self.assertEqual("Streak secured! 🔥", goals.celebration_title(goals.MINIMUM))
        self.assertIn("Next up, your daily goal: 4 of 50 cards so far.", goals.celebration_text(goals.MINIMUM, done))
        self.assertNotIn("Next up", goals.celebration_text(goals.MINIMUM, done._replace(goal=None)))
        self.assertEqual("12 of 12 minutes", goals.progress_text(status._replace(mode="minutes", minutes=30, goal=goals.Goal(12, 0, 0))))
        spanish = lambda value: language.tr(value, "es")
        self.assertEqual("4 de 50 tarjetas", goals.progress_text(status, spanish))

    def test_load_reads_todays_work_and_backlog(self):
        db = database()
        cutoff = 1_000_000
        start_ms = (cutoff - 86_400) * 1000
        rows = [
            (1, 1, 0, 2, 100, 0), (2, 1, 0, 2, 100, 0), (3, 1, 0, 2, 90, 0), (4, 1, 0, 2, 105, 0),
            (5, 9, 1, 2, 0, 80), (6, 1, 0, 1, cutoff - 10, 0), (7, 1, 0, 1, cutoff + 10, 0), (8, 1, 0, 3, 100, 0),
        ]
        db.executemany("insert into cards values (?, ?, ?, ?, ?, ?)", rows)
        db.executemany("insert into revlog values (?, ?, ?, ?, ?)", [
            (start_ms + 1, 1, 3, 1, 60_000), (start_ms + 2, 1, 3, 1, 30_000), (start_ms + 3, 2, 3, 1, 50_000),
            (start_ms - 1, 3, 3, 1, 999_999), (start_ms + 4, 3, 0, 4, 999_999),
        ])
        tree = node(0, children=[node(1, new=4), node(2, new=3)])
        load = goals.load(collection(db, tree, 100, cutoff))
        self.assertEqual(goals.Load(100, 2, 2, 2 + 2 + 7, 2), load)


class MoodTests(unittest.TestCase):
    now = 1_000_000

    def profile(self, state, reviews, ends=2_000_000):
        return {"streak_state": state, "today": {"reviews": reviews}, "day_ends_at": ends}

    def test_moods_follow_the_phone(self):
        self.assertEqual(moods.FREEZE, moods.mood([5], 5, self.profile("protected", 0), self.now))
        self.assertEqual(moods.CELEBRATE, moods.mood([0, 0], 0, self.profile("studied", 4), self.now))
        self.assertEqual(moods.REVIEW, moods.mood([3, 0], 3, self.profile("studied", 4), self.now))
        self.assertEqual(moods.STREAK, moods.mood([], 0, self.profile("studied", 4), self.now))
        self.assertEqual(moods.WELCOME, moods.mood([0], 0, None, self.now))

    def test_a_profile_from_an_ended_day_is_ignored(self):
        stale = self.profile("protected", 0, ends=self.now)
        self.assertEqual(moods.WELCOME, moods.mood([0], 0, stale, self.now))

    def test_art_respects_the_companion(self):
        self.assertEqual("aki_freeze", moods.art("aki", moods.FREEZE))
        self.assertEqual("ankilope_study", moods.art("ankilope", moods.FREEZE))
        self.assertEqual("ankilope_celebrate", moods.art("ankilope", moods.WINNER))
        self.assertIsNone(moods.art("none", moods.REVIEW))
        self.assertIn("Aki", moods.message("aki", moods.WELCOME))
        self.assertNotIn("Aki", moods.message("ankilope", moods.WELCOME))

    def test_every_mood_has_bundled_and_exported_art(self):
        source = (ADDON / "__init__.py").read_text("utf-8")
        pattern = re.search(r'setWebExports\(__name__, r"(.+?)"\)', source).group(1)
        for companion in ("aki", "ankilope"):
            for mood in moods.MESSAGES:
                name = "art/%s.png" % moods.art(companion, mood)
                self.assertTrue((ADDON / name).is_file(), name)
                self.assertTrue(re.fullmatch(pattern, name), name)
        self.assertTrue(re.fullmatch(pattern, "aki_face.png"))
        self.assertFalse(re.fullmatch(pattern, "art/../config.json"))


class DeckDayTests(unittest.TestCase):
    def test_status_order_matches_the_phone(self):
        self.assertEqual(today.REVIEW_NOW, today.daily_status(1, 3, 0, 5))
        self.assertEqual(today.LEARNING_LATER, today.daily_status(0, 3, 4, 5))
        self.assertEqual(today.REVIEW_NOW, today.daily_status(0, 3, 4, None))
        self.assertEqual(today.DONE, today.daily_status(0, 0, 4, None))
        self.assertEqual(today.NOTHING, today.daily_status(0, 0, 0, None))

    def tree(self):
        return node(0, children=[
            node(1, "Spanish", review=2, children=[node(2, "Verbs", review=2)]),
            node(3, "Geography"),
            node(4, "Filtered", new=1),
        ])

    def snapshots(self):
        return [
            {"id": "1", "name": "Spanish", "remaining": 4, "reviewed_today": 6},
            {"id": "2", "name": "Spanish::Verbs", "remaining": 4, "reviewed_today": 6},
            {"id": "3", "name": "Geography", "remaining": 2, "reviewed_today": 1},
        ]

    def test_learning_later_rolls_up_to_parents(self):
        days = {day.id: day for day in today.deck_days(self.tree(), self.snapshots(), [(2, 2, 7200), (3, 2, 3600)])}
        self.assertEqual("Spanish::Verbs", days[2].name)
        self.assertEqual(today.REVIEW_NOW, days[1].status)
        self.assertEqual(2, days[1].ready_count)
        self.assertEqual(7_200_000, days[1].next_at)
        self.assertEqual(today.LEARNING_LATER, days[3].status)
        self.assertEqual(3_600_000, days[3].next_at)
        self.assertEqual((1, today.REVIEW_NOW), (days[4].ready_count, days[4].status))
        self.assertEqual([1, 2, 3, 4], [day.id for day in today.deck_days(self.tree(), self.snapshots(), [])])

    def test_load_days_queries_learning_due_before_rollover(self):
        db = database()
        db.executemany("insert into cards values (?, ?, ?, ?, ?, ?)", [
            (1, 3, 0, 1, 2000, 0), (2, 3, 0, 1, 1500, 0), (3, 3, 0, 1, 900, 0), (4, 3, 0, 1, 9000, 0), (5, 9, 3, 1, 1200, 0),
        ])
        col = collection(db, self.tree(), cutoff=5000)
        days = {day.id: day for day in today.load_days(col, 1_000_000, self.tree(), self.snapshots())}
        self.assertEqual(1_200_000, days[3].next_at)
        self.assertEqual(0, days[3].ready)

    def test_focus_prefers_the_selected_deck_with_work(self):
        days = today.deck_days(self.tree(), self.snapshots(), [(3, 2, 3600)])
        self.assertEqual(4, today.focus(days, 4).id)
        self.assertEqual(1, today.focus(days, 3).id)
        done = [day._replace(review=0, new=0, ready=0, remaining=0) for day in days]
        self.assertEqual(3, today.focus(done, 3).id)
        self.assertEqual(1, today.focus(done, 99).id)
        self.assertIsNone(today.focus([], 1))

    def test_labels_and_badges(self):
        days = {day.id: day for day in today.deck_days(self.tree(), self.snapshots(), [(3, 2, 3600)])}
        at = lambda ms: "at %d" % (ms // 1000)
        self.assertEqual("4 cards ready", today.label(days[1], show_time=at))
        self.assertEqual("1 card ready", today.label(days[4], show_time=at))
        self.assertEqual("Learning later today · next at at 3600", today.label(days[3], show_time=at))
        self.assertEqual("Done today", today.label(days[1]._replace(review=0, ready=0, remaining=0)))
        self.assertEqual("", today.badge(days[1]._replace(review=0, ready=0, remaining=0, reviewed=0)))
        self.assertIn("ankiquest-learning-later", today.badge(days[3], show_time=at))

    def test_badges_land_after_each_deck_name_in_ankis_markup(self):
        markup = (
            "<tr class='deck current' id='1'><td class=decktd colspan=5>"
            "<a class=collapse href=# onclick='return pycmd(\"collapse:1\")'>-</a>"
            "<a class=\"deck \" href=# onclick=\"return pycmd('open:1')\">Spanish </a></td></tr>"
            "<tr class='deck' id='3'><td><a class=deck href=# onclick=\"return pycmd('open:3')\">Geo&lt;graphy</a></td></tr>"
            "<tr class='deck' id='77'><td><a class=deck href=# onclick=\"return pycmd('open:77')\">Unknown</a></td></tr>"
        )
        days = today.deck_days(self.tree(), self.snapshots(), [(3, 2, 3600)])
        decorated = today.decorate(markup, days, show_time=lambda ms: "10:00")
        self.assertIn("Spanish </a><span class='ankiquest-day ankiquest-review-now'", decorated)
        self.assertIn("Geo&lt;graphy</a><span class='ankiquest-day ankiquest-learning-later'", decorated)
        self.assertIn("Unknown</a></td>", decorated)
        self.assertIn("pycmd(\"collapse:1\")'>-</a><a", decorated)


class LeaderboardTests(unittest.TestCase):
    board = [
        {"user": "a", "display": "Ana", "xp_total": 900, "week_xp": 50, "periods": {"day": 5, "week": 50, "all": 900}},
        {"user": "b", "display": "Bo", "xp_total": 100, "week_xp": 70, "periods": {"day": 30, "week": 70, "all": 100}},
        {"user": "c", "display": "Cy", "xp_total": 50, "week_xp": 10},
        {"user": "d", "display": "Di", "xp_total": 40, "week_xp": 9, "periods": {"week": 9}},
        {"user": "e", "display": "Ed", "xp_total": 30, "week_xp": 8, "periods": {"week": 8}},
        {"user": "f", "display": "Fa", "xp_total": 20, "week_xp": 7, "periods": {"week": 7}},
        {"user": "hill", "display": "Hill", "xp_total": 10, "week_xp": 1, "periods": {"week": 1, "day": 1}},
    ]

    def test_periods_reorder_the_board(self):
        self.assertEqual(["b", "a", "c"], [entry["user"] for entry in today.ranked(self.board, "week")][:3])
        self.assertEqual(["a", "b"], [entry["user"] for entry in today.ranked(self.board, "all")][:2])
        self.assertEqual([30, 10, 9], [entry["xp"] for entry in today.ranked(self.board, "day")][:3])

    def test_own_row_joins_the_top_five(self):
        rows = today.standings(self.board, "week", "hill")
        self.assertEqual([1, 2, 3, 4, 5, 7], [place for place, _, _ in rows])
        self.assertTrue(rows[-1][2])
        self.assertEqual(5, len(today.standings(self.board, "week", "b")))
        self.assertEqual([], today.standings(None, "week", "hill"))
        self.assertEqual("week", today.period("decade"))


class SessionTests(unittest.TestCase):
    def test_a_session_counts_reviews_from_the_log(self):
        db = database()
        db.executemany("insert into revlog values (?, ?, ?, ?, ?)", [
            (1000, 1, 3, 1, 70_000), (2000, 2, 3, 1, 80_000), (3000, 3, 0, 4, 9_000), (500, 4, 3, 1, 1),
        ])
        col = collection(db, node(0))
        summary = today.session(col, 900, 5000, 12)
        self.assertEqual({"reviews": 2, "minutes": 2, "remaining": 12, "finished_at": 5000}, summary)
        self.assertIsNone(today.session(col, 3500, 5000, 0))
        self.assertIsNone(today.session(col, 0, 5000, 0))
        self.assertIsNone(today.session(col, 1, 1 + today.MAX_SESSION_MS + 1, 0))
        self.assertEqual("2 reviews · 2 minutes · 12 ready at session end", today.session_text(summary))
        self.assertEqual("less than a minute", today.duration(0))
        self.assertEqual("1 minute", today.duration(1))


class PanelTests(unittest.TestCase):
    def view(self, **changes):
        view = {
            "art": "/_addons/1/art/aki_review.png",
            "mood": "A small step starts with one card.",
            "winner": "/_addons/1/art/aki_winner.png",
            "focus": {"id": 7, "name": "Spa<b>nish", "review": 3, "learning": 1, "new": 2,
                      "status": "review_now", "label": "6 cards ready", "ready": 6},
            "goal": {"minimum": "Streak minimum: 5 minutes of review today", "minimum_meter": (2, 5),
                     "daily": "Daily goal: 4 of 50 cards", "daily_meter": (4, 50), "backlog": None},
            "board": {"period": "day", "rows": [(1, {"display": "<img src=x>", "xp": 1200}, False), (6, {"user": "hill", "xp": 3}, True)]},
            "session": "12 reviews · 3 minutes · 0 ready at session end",
        }
        view.update(changes)
        return view

    profile = {"level": 3, "streak": 4, "quests": [
        {"title": "<script>alert(1)</script>", "progress": 3, "target": 10, "done": False, "reward": 20},
        {"title": "Combo", "progress": 5, "target": 5, "done": True, "reward": 15},
    ]}

    def test_the_panel_shows_every_part(self):
        html = board.html(self.profile, 2, 0, today=self.view())
        self.assertIn("aki_review.png", html)
        self.assertIn("A small step starts with one card.", html)
        self.assertIn("4 day streak", html)
        self.assertIn("pycmd('ankiquest:study:7')", html)
        self.assertIn("Study 6 due", html)
        self.assertIn("aria-valuenow=2 aria-valuemax=5", html)
        self.assertIn("aria-valuenow=4 aria-valuemax=50", html)
        self.assertIn("Complete · 15 XP", html)
        self.assertIn("<option value=day selected>", html)
        self.assertIn("<tr class=aq-me><td>#6</td><td>hill</td><td>3 XP</td>", html)
        self.assertIn("1,200 XP", html)
        self.assertIn("Your last study session", html)
        self.assertNotIn("aki_winner.png", html)

    def test_server_and_deck_text_is_escaped(self):
        html = board.html(self.profile, None, 0, today=self.view())
        self.assertNotIn("<script>", html)
        self.assertNotIn("<img src=x>", html)
        self.assertNotIn("Spa<b>nish", html)
        self.assertIn("Spa&lt;b&gt;nish", html)

    def test_every_link_opens_a_known_page(self):
        html = board.html(self.profile, None, 2, today=self.view())
        pages = set(re.findall(r"ankiquest:web:(\w+)", html))
        self.assertTrue({"today", "history", "settings", "reminders", "inbox"} <= pages)
        for page in pages:
            self.assertIn(page, board.PAGES)
        self.assertEqual("/community#reminders", board.PAGES["reminders"])

    def test_missing_parts_are_left_out(self):
        html = board.html(None, None, 0, today=self.view(focus=None, goal=None, board=None, session=None, art=None, mood=None))
        self.assertIn("Your study starts with a deck.", html)
        self.assertNotIn("Today’s goal", html)
        self.assertNotIn("<select", html)
        self.assertNotIn("<img", html)
        done = dict(self.view()["focus"], status="done", label="Done today")
        self.assertNotIn("ankiquest:study", board.html(None, today=self.view(focus=done)))

    def test_all_quests_done_shows_the_winner(self):
        profile = {"quests": [{"title": "Combo", "progress": 5, "target": 5, "done": True, "reward": 15}]}
        self.assertIn("aki_winner.png", board.html(profile, today=self.view()))

    def test_meters_clamp_and_turn_full(self):
        self.assertIn("width:100%", board.meter(12, 10))
        self.assertIn("aq-full", board.meter(12, 10))
        self.assertIn("width:0%", board.meter(-3, 10))
        self.assertNotIn("aq-full", board.meter(0, 0))


class TranslationTests(unittest.TestCase):
    FILES = ("board.py", "goals.py", "today.py", "home.py", "today_ui.py", "moods.py")

    def sources(self):
        found = set()
        for name in self.FILES:
            text = (ADDON / name).read_text("utf-8")
            for call in ast.walk(ast.parse(text)):
                if (
                    isinstance(call, ast.Call) and isinstance(call.func, ast.Name) and call.func.id in ("tr", "translate")
                    and call.args and isinstance(call.args[0], ast.Constant) and isinstance(call.args[0].value, str)
                ):
                    found.add(call.args[0].value)
            if name in ("board.py", "today_ui.py"):
                found.update(re.findall(r'\(\s*[\w."]+,\s*"([^"]*[A-Za-z][^"]*)"\s*\)', text))
        found.update(moods.MESSAGES.values())
        found.add(moods.COMPANION_WELCOME)
        return found

    def test_every_today_string_has_a_spanish_translation(self):
        strings = self.sources()
        self.assertGreater(len(strings), 60)
        missing = sorted(text for text in strings if text not in language.SPANISH)
        self.assertEqual([], missing)

    def test_spanish_keeps_the_placeholders(self):
        for text in self.sources():
            self.assertEqual(re.findall(r"%[ds%]", text), re.findall(r"%[ds%]", language.SPANISH[text]), text)


class HomeTests(unittest.TestCase):
    """The glue in home.py, with Anki replaced by stand-ins."""

    def setUp(self):
        self.db = database()
        self.addCleanup(self.db.close)
        self.cutoff = int(__import__("time").time()) + 3600
        self.tree = node(0, children=[node(5, "Spanish", review=3)])
        self.settings = {"url": "https://example.test", "user": "hill", "token": "secret"}
        self.api = SimpleNamespace(
            configured=True, base="https://example.test", user="hill", name="hill", token="secret",
            leaderboard=Mock(return_value=[{"user": "hill", "display": "Hill", "xp_total": 5, "week_xp": 5}]),
            rank=Mock(return_value={"order": ["hill"]}), notifications=Mock(return_value=[]),
            profile=Mock(return_value={"level": 2, "streak": 1, "today": {"streak_minutes": 0, "reviews": 0}}),
            companion=Mock(return_value="ankilope"),
        )
        col = collection(self.db, self.tree, 100, self.cutoff)
        col.get_config = lambda *args: 4
        col.sched.counts = lambda: (0, 0, 3)
        col.startTimebox = Mock()
        col.decks = SimpleNamespace(
            all_names_and_ids=lambda include_filtered=False: [SimpleNamespace(id=5, name="Spanish")],
            get_current_id=lambda: 5, select=Mock(), get=lambda did, default=False: {"id": did} if did == 5 else None,
        )
        self.mw = SimpleNamespace(
            col=col,
            pm=SimpleNamespace(profile={}),
            taskman=SimpleNamespace(run_in_background=self.run_task),
            form=SimpleNamespace(menuTools=SimpleNamespace(addAction=Mock())),
            addonManager=SimpleNamespace(
                getConfig=lambda _: dict(self.settings), writeConfig=lambda _, values: self.settings.update(values),
                setWebExports=Mock(), addonFromModule=Mock(return_value="ankiquest"),
            ),
            progress=SimpleNamespace(timer=Mock()),
            deckBrowser=SimpleNamespace(refresh=Mock()),
            onOverview=Mock(), moveToState=Mock(),
            state="deckBrowser",
        )
        aqt = ModuleType("aqt")
        aqt.mw = self.mw
        aqt.gui_hooks = SimpleNamespace(**{name: [] for name in (
            "reviewer_did_answer_card", "operation_did_execute", "state_did_change", "sync_did_finish", "profile_did_open",
            "deck_browser_will_render_content", "webview_did_receive_js_message",
        )})
        utils = ModuleType("aqt.utils")
        utils.tooltip, utils.openLink = Mock(), Mock()
        qt = ModuleType("aqt.qt")
        qt.__getattr__ = lambda name: Mock()
        anki = ModuleType("anki")
        anki.lang = ModuleType("anki.lang")
        anki.lang.current_lang = "en"
        modules = patch.dict(sys.modules, {"aqt": aqt, "aqt.utils": utils, "aqt.qt": qt, "anki": anki, "anki.lang": anki.lang})
        modules.start()
        self.addCleanup(modules.stop)
        self.addon = load_module("ankiquest_home_addon", ADDON / "__init__.py", package=True)
        self.addon.client = lambda: self.api
        self.home = sys.modules["ankiquest_home_addon.home"]
        self.home.today_ui = SimpleNamespace(session_dialog=Mock())
        self.hooks = aqt.gui_hooks

    def run_task(self, work, done, uses_collection=False):
        future = Future()
        try:
            future.set_result(work())
        except Exception as error:
            future.set_exception(error)
        done(future)

    def render(self):
        content = SimpleNamespace(
            stats="", tree="<a class=deck href=# onclick=\"return pycmd('open:5')\">Spanish</a>",
        )
        deck_browser = SimpleNamespace(_dueTree=self.tree)
        for hook in self.hooks.deck_browser_will_render_content:
            hook(deck_browser, content)
        return content

    def review(self, at, seconds):
        self.db.execute("insert into revlog values (?, 1, 3, 1, ?)", (at, seconds * 1000))

    def test_the_deck_list_gets_the_today_panel_and_deck_badges(self):
        content = self.render()
        self.assertIn("/_addons/ankiquest/art/ankilope_study.png", content.stats)
        self.assertIn("pycmd('ankiquest:study:5')", content.stats)
        self.assertIn("Today’s goal", content.stats)
        self.assertIn("<tr class=aq-me>", content.stats)
        self.assertIn("ankiquest-review-now", content.tree)
        self.settings["deck_status"] = False
        self.home.local["days"] = None
        self.assertNotIn("ankiquest-day", self.render().tree)

    def test_studying_and_period_links_act_only_from_the_deck_list(self):
        handle = self.hooks.webview_did_receive_js_message[-1]
        self.assertEqual((True, None), handle(False, "ankiquest:study:5", self.mw.deckBrowser))
        self.mw.col.decks.select.assert_called_once_with(5)
        self.mw.onOverview.assert_called_once_with()
        handle(False, "ankiquest:period:month", self.mw.deckBrowser)
        self.assertEqual("month", self.mw.pm.profile[self.home.PERIOD_KEY])
        self.assertFalse(handle(False, "ankiquest:period:day", object()))
        self.assertEqual("month", self.mw.pm.profile[self.home.PERIOD_KEY])

    def session(self, reviews):
        for hook in self.hooks.state_did_change:
            hook("review", "overview")
        self.home.local["session"]["started"] -= 60_000
        started = self.home.local["session"]["started"]
        for index in range(reviews):
            self.review(started + index + 1, 30)
        for hook in self.hooks.state_did_change:
            hook("overview", "review")

    def test_the_first_session_of_the_day_celebrates_then_later_ones_recap(self):
        self.settings.update(goal_mode="cards", goal_cards=100)
        self.session(2)
        dialog = self.home.today_ui.session_dialog
        self.assertEqual("Streak secured! 🔥", dialog.call_args.args[1])
        self.assertIn("2 reviews · 1 minute", dialog.call_args.args[2])
        self.assertTrue(dialog.call_args.args[3].endswith("aki_streak.png"))
        self.session(1)
        self.assertEqual("Your study session", dialog.call_args.args[1])
        dialog.call_args.kwargs["never_again"]()
        self.assertFalse(self.settings["session_summary"])
        self.session(1)
        self.assertEqual(2, dialog.call_count)
        self.assertEqual(3, self.mw.pm.profile[self.home.SESSION_KEY]["remaining"])

    def test_a_session_without_reviews_says_nothing(self):
        self.session(0)
        self.home.today_ui.session_dialog.assert_not_called()


if __name__ == "__main__":
    unittest.main()
