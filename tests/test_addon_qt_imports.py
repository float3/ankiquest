"""Every Qt name the add-on imports exists in Anki's build; skipped without aqt installed."""

import ast
import importlib
import importlib.util
import unittest
from pathlib import Path

ADDON = Path(__file__).resolve().parents[1] / "addon"


def qt_imports():
    found = set()
    for path in ADDON.glob("*.py"):
        for node in ast.walk(ast.parse(path.read_text("utf-8"))):
            if isinstance(node, ast.ImportFrom) and node.module and (
                node.module == "aqt.qt" or node.module.startswith("PyQt6")
            ):
                found.update((path.name, node.module, alias.name) for alias in node.names)
    return found


@unittest.skipUnless(importlib.util.find_spec("aqt"), "aqt is not installed")
class QtImports(unittest.TestCase):
    def test_every_name_exists(self):
        missing = sorted(
            "%s: from %s import %s" % (file, module, name)
            for file, module, name in qt_imports()
            if not hasattr(importlib.import_module(module), name)
        )
        self.assertEqual(missing, [])


if __name__ == "__main__":
    unittest.main()
