"""Every desktop label has a translation in every language, with the same placeholders."""

import ast
import json
import re
import unittest

from addon_loader import ADDON, load_in_package, load_module

PLACEHOLDER = re.compile(r"%[sd]")


def literals(node):
    if isinstance(node, ast.Constant) and isinstance(node.value, str):
        return [node.value]
    if isinstance(node, ast.IfExp):
        return literals(node.body) + literals(node.orelse)
    return []


def labels():
    """Every literal label, including both forms passed to the plural helper."""
    found = set()
    for path in ADDON.glob("*.py"):
        for node in ast.walk(ast.parse(path.read_text("utf-8"))):
            if isinstance(node, ast.Call):
                name = getattr(node.func, "id", None)
                count = 2 if name == "_plural" else 1 if name in ("tr", "translate") else 0
                for argument in node.args[:count]:
                    found.update(literals(argument))
    return found


def code_strings():
    """Every string literal in the add-on outside the tables. A label can reach `tr` through
    a variable, as moods.py and inbox.py do, so this is wider than labels()."""
    found = set()
    for path in ADDON.glob("*.py"):
        if path.name != "language.py":
            for node in ast.walk(ast.parse(path.read_text("utf-8"))):
                if isinstance(node, ast.Constant) and isinstance(node.value, str):
                    found.add(node.value)
    return found


language = load_module("aq_language_tables", "language.py")


class TranslationTables(unittest.TestCase):
    def test_shared_screens_match_the_reviewed_android_wording(self):
        fixture = json.loads((ADDON.parent / "tests/fixtures/desktop-wording.json").read_text("utf-8"))
        for row in fixture["strings"]:
            for code, expected in row["translations"].items():
                with self.subTest(resource=row["android_resource"], language=code):
                    self.assertEqual(expected, language.tr(row["source"], code))

    def test_languages(self):
        self.assertEqual(sorted(language.TABLES), ["de", "es", "fr", "pt"])

    def test_every_language_has_every_key(self):
        keys = set(language.SPANISH)
        for code, table in language.TABLES.items():
            with self.subTest(code=code):
                self.assertEqual(sorted(keys - set(table)), [])
                self.assertEqual(sorted(set(table) - keys), [])

    def test_every_label_in_the_code_is_translated(self):
        for code, table in language.TABLES.items():
            with self.subTest(code=code):
                self.assertEqual(sorted(labels() - set(table)), [])

    def test_every_translated_label_is_still_in_the_code(self):
        self.assertEqual(sorted(set(language.SPANISH) - code_strings()), [])

    def test_placeholders_survive_translation(self):
        for code, table in language.TABLES.items():
            for source, text in table.items():
                with self.subTest(code=code, source=source):
                    self.assertEqual(PLACEHOLDER.findall(text), PLACEHOLDER.findall(source))
                    self.assertTrue(text.strip())

    def test_lookup_follows_the_primary_language_and_falls_back_to_english(self):
        self.assertEqual(language.tr("Save", "de_DE"), "Speichern")
        self.assertEqual(language.tr("Save", "pt-BR"), "Guardar")
        self.assertEqual(language.tr("Save", "fr"), "Enregistrer")
        self.assertEqual(language.tr("Save", "es-AR"), "Guardar")
        self.assertEqual(language.tr("Save", "ja"), "Save")
        self.assertEqual(language.tr("Not a label", "de"), "Not a label")

    def test_labels_are_translated_before_names_are_filled_in(self):
        board = load_in_package("aq_language_board", "board")
        spanish = lambda value: language.tr(value, "es-AR")
        self.assertIn("Clasificación", board.html({"level": 4, "streak": 3}, 2, 1, spanish))
        self.assertIn("Puesto 2 esta semana", board.html({}, 2, 0, spanish))
        self.assertEqual("Conectado como Friends::{0}, nivel 2.", spanish("Connected as %s, level %d.") % ("Friends::{0}", 2))


if __name__ == "__main__":
    unittest.main()
