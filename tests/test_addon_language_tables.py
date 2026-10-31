"""Every desktop label has a translation in every language, with the same placeholders."""

import ast
import importlib.util
import re
import sys
import unittest
from pathlib import Path

ADDON = Path(__file__).resolve().parents[1] / "addon"
PLACEHOLDER = re.compile(r"%[sd]")


def load():
    spec = importlib.util.spec_from_file_location("aq_language_tables", ADDON / "language.py")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def literals(node):
    if isinstance(node, ast.Constant) and isinstance(node.value, str):
        return [node.value]
    if isinstance(node, ast.IfExp):
        return literals(node.body) + literals(node.orelse)
    return []


def labels():
    """Every literal passed to `tr` or `translate` anywhere in the add-on."""
    found = set()
    for path in ADDON.glob("*.py"):
        for node in ast.walk(ast.parse(path.read_text("utf-8"))):
            if isinstance(node, ast.Call) and getattr(node.func, "id", None) in ("tr", "translate") and node.args:
                found.update(literals(node.args[0]))
    return found


language = load()


class TranslationTables(unittest.TestCase):
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


if __name__ == "__main__":
    unittest.main()
