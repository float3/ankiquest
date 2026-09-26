import importlib.util
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1] / "addon"


def load(name):
    spec = importlib.util.spec_from_file_location("aq_language_" + name, ROOT / (name + ".py"))
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


class LanguageTests(unittest.TestCase):
    def test_desktop_labels_follow_anki_and_format_user_names_after_translation(self):
        language, board = load("language"), load("board")
        spanish = lambda value: language.tr(value, "es-AR")
        self.assertIn("Clasificación", board.html({"level": 4, "streak": 3}, 2, 1, spanish))
        self.assertIn("Puesto 2 esta semana", board.html({}, 2, 0, spanish))
        self.assertEqual("Conectado como Friends::{0}, nivel 2.", spanish("Connected as %s, level %d.") % ("Friends::{0}", 2))
        self.assertEqual("Friends", language.tr("Friends", "fr"))
