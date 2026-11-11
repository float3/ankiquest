"""Load the whole add-on with Anki's real managers, collection, hooks and Qt window."""
import ast
from contextlib import ExitStack
import importlib
from pathlib import Path
import re
import runpy
import sys
import tempfile
import unittest

import anki.lang
from anki.collection import Collection

anki.lang.set_lang("en")

import aqt
from aqt.addons import AddonManager, InstallOk
from aqt.deckbrowser import DeckBrowser, DeckBrowserContent
from aqt.forms.main import Ui_MainWindow
from aqt.main import AnkiQt
from aqt.profiles import ProfileManager
from aqt.progress import ProgressManager
from aqt.qt import QApplication, QMainWindow
from aqt.reviewer import Reviewer
from aqt.taskman import TaskManager

ROOT = Path(__file__).resolve().parents[1]
app = QApplication.instance() or QApplication([])


def members():
    paths, hooks = set(), set()
    for file in (ROOT / "addon").glob("*.py"):
        for node in ast.walk(ast.parse(file.read_text("utf-8"))):
            if isinstance(node, ast.Constant) and isinstance(node.value, str) and re.fullmatch(r"\w+_(did|will)_\w+", node.value):
                hooks.add(node.value)
            if not isinstance(node, ast.Attribute):
                continue
            parts, at = [], node
            while isinstance(at, ast.Attribute):
                parts.append(at.attr)
                at = at.value
            if isinstance(at, ast.Name):
                parts.append(at.id)
            parts.reverse()
            if "mw" in parts:
                paths.add(tuple(parts[parts.index("mw") + 1:]))
            if parts and parts[0] == "gui_hooks":
                hooks.add(parts[1])
    return paths, hooks


class RealAnki(unittest.TestCase):
    def test_load_hooks_and_window_apis(self):
        with ExitStack() as scope:
            folder = scope.enter_context(tempfile.TemporaryDirectory(prefix="ankiquest-smoke-"))
            pm = ProfileManager(Path(folder))
            pm.setupMeta()
            scope.callback(pm.db.close)
            pm.load(pm.profiles()[0])
            mw = AnkiQt.__new__(AnkiQt)
            QMainWindow.__init__(mw)
            mw.app, mw.pm, mw.state = app, pm, "deckBrowser"
            mw.form = Ui_MainWindow()
            mw.form.setupUi(mw)
            mw.col = Collection(str(Path(pm.profileFolder()) / "collection.anki2"))
            scope.callback(mw.col.close)
            mw.backend = mw.col._backend
            mw.taskman, mw.progress = TaskManager(mw), ProgressManager(mw)
            scope.callback(mw.taskman._no_collection_executor.shutdown)
            scope.callback(mw.taskman._collection_executor.shutdown)
            aqt.mw = mw
            # Inspect the real screen APIs without starting Chromium renderer processes.
            mw.deckBrowser = DeckBrowser.__new__(DeckBrowser)
            mw.reviewer = Reviewer.__new__(Reviewer)
            mw.addonManager = AddonManager(mw)
            package = runpy.run_path(str(ROOT / ".github/scripts/package-addon.py"))
            archive = Path(folder) / "ankiquest.ankiaddon"
            package["_build"](ROOT / "addon", archive, {"release": 11, "nightly": 0, "commit": "abcdef0"})
            installed = mw.addonManager.install(str(archive), manifest={"package": "ankiquest_smoke"})
            self.assertIsInstance(installed, InstallOk)
            self.assertTrue(installed.compatible)
            metadata = mw.addonManager.addon_meta("ankiquest_smoke")
            self.assertEqual("addon-11", metadata.human_version)
            self.assertEqual("AnkiQuest", metadata.human_name())
            module = importlib.import_module("ankiquest_smoke")
            try:
                paths, hooks = members()
                for name in hooks:
                    self.assertTrue(hasattr(aqt.gui_hooks, name), name)
                for path in paths:
                    value = mw
                    for part in path:
                        self.assertTrue(hasattr(value, part), ".".join(path))
                        value = getattr(value, part)
                self.assertEqual("AnkiQuest", module._tools.title())
                self.assertEqual(8, len(module._tools.actions()))
                content = DeckBrowserContent(tree="", stats="")
                module.on_deck_browser(mw.deckBrowser, content)
                self.assertIn("AnkiQuest", content.stats)
                # Exercise the real scheduler, timebox and timer signatures used by the add-on.
                mw.col.startTimebox()
                preferences = mw.col.get_preferences()
                preferences.scheduling.rollover = 7
                mw.col.set_preferences(preferences)
                self.assertEqual(7, mw.col.get_config("rollover", 4))
                self.assertGreater(mw.col.sched.day_cutoff, 0)
                timer = mw.progress.timer(10000, lambda: None, repeat=True, parent=mw)
                timer.stop()
                self.assertIsNotNone(mw.addonManager.getConfig(module.__name__))
            finally:
                mw.deleteLater()
                app.processEvents()
                sys.modules.pop("ankiquest_smoke", None)


if __name__ == "__main__":
    unittest.main()
