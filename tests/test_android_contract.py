"""The server guard rejects breaking fixture shapes without coupling new endpoints."""
import json
from pathlib import Path
import runpy
import tempfile
import unittest

guard = runpy.run_path(str(Path(__file__).resolve().parents[1] / ".github/scripts/android-contract.py"))


class Shape(unittest.TestCase):
    def test_values_new_keys_and_new_array_variants_are_allowed(self):
        before = {"items": [{"id": 1, "name": "old"}], "count": 1}
        after = {"items": [{"id": 2.0, "name": "new", "avatar": None}, {"id": 3}], "count": 20, "next": None}
        self.assertEqual([], guard["_changes"](before, after))

    def test_removed_nested_keys_and_number_to_boolean_are_rejected(self):
        errors = guard["_changes"]({"items": [{"id": 1, "label": "word"}]}, {"items": [{"id": True}]})
        self.assertEqual(2, len(errors))
        self.assertTrue(any('$["items"][*]["id"]' in error and "number" in error for error in errors))
        self.assertTrue(any('$["items"][*]["label"] was removed' in error for error in errors))

    def test_null_and_container_types_are_distinct(self):
        for before, after in ((None, "text"), ([], {}), ({"x": 1}, []), ([{"id": 1}], [])):
            with self.subTest(before=before, after=after):
                self.assertTrue(guard["_changes"](before, after))


class Catalog(unittest.TestCase):
    def setUp(self):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        self.root = Path(folder.name)
        self.server, self.android = self.root / "server", self.root / "android"
        self.server.mkdir()
        (self.android / "ankiquest-api").mkdir(parents=True)
        (self.android / "ankiquest-server.sha").write_text("a" * 40 + "\n", "utf-8")

    def write(self, parent, name, value):
        (parent / (name + ".json")).write_text(json.dumps(value), "utf-8")

    def catalog(self, **groups):
        self.write(self.android / "ankiquest-api", "names", groups)

    def test_only_absent_names_skip_the_guard(self):
        self.assertIsNone(guard["_check"](self.server, self.android))
        self.catalog(unused=[])
        with self.assertRaisesRegex(ValueError, "consumed"):
            guard["_check"](self.server, self.android)

    def test_all_consumed_groups_are_guarded_but_unread_names_are_not(self):
        self.catalog(consumed=["profile"], boards=["leaderboard"], challenges=["challenges"],
                     unused=["unused"], not_yet=["future"])
        for name in ("profile", "leaderboard", "challenges"):
            self.write(self.android / "ankiquest-api", name, {"id": 1})
            self.write(self.server, name, {"id": 2, "new": True})
        self.write(self.server, "new_endpoint", {"anything": []})
        self.assertEqual([], guard["_check"](self.server, self.android))
        (self.server / "profile.json").unlink()
        self.assertEqual(["Consumed example profile.json was removed"], guard["_check"](self.server, self.android))

    def test_missing_baseline_and_invalid_names_cannot_silently_pass(self):
        self.catalog(consumed=["profile"])
        self.assertIn("vendored example is missing", guard["_check"](self.server, self.android)[0])
        self.catalog(consumed=["../secret"])
        with self.assertRaisesRegex(ValueError, "invalid"):
            guard["_check"](self.server, self.android)
        self.catalog(consumed=[])
        (self.android / "ankiquest-server.sha").write_text("master", "utf-8")
        with self.assertRaisesRegex(ValueError, "full commit SHA"):
            guard["_check"](self.server, self.android)


if __name__ == "__main__":
    unittest.main()
