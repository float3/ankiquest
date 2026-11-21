"""Loads add-on modules straight from addon/, so the tests run without Anki, and the stand-ins they run with.

Each test file names its modules itself, so two files never share a copy one of them patched."""

import importlib.util
import sys
from pathlib import Path
from types import ModuleType, SimpleNamespace
from unittest.mock import Mock, patch

ADDON = Path(__file__).resolve().parents[1] / "addon"


def load_module(name, file, package=False):
    """addon/`file` as module `name`. As a package (for __init__.py), its relative imports load from addon/."""
    spec = importlib.util.spec_from_file_location(
        name, ADDON / file, submodule_search_locations=[str(ADDON)] if package else None
    )
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def load_in_package(package, name):
    """addon/`name`.py inside a stand-in package, so its relative imports work without running
    __init__.py. A module loads once per package."""
    if package not in sys.modules:
        stand_in = ModuleType(package)
        stand_in.__path__ = [str(ADDON)]
        sys.modules[package] = stand_in
    full = package + "." + name
    return sys.modules.get(full) or load_module(full, name + ".py")


class _Hooks(SimpleNamespace):
    """aqt.gui_hooks where every hook is a list, made the first time the add-on names it."""

    def __getattr__(self, name):
        hook = []
        setattr(self, name, hook)
        return hook


# The Anki version anki.utils.int_version() reports under fake_anki: the add-on's minimum.
ANKI_VERSION = 231000


def fake_anki(test, mw):
    """Replaces aqt, aqt.utils, aqt.qt, anki, anki.lang and anki.utils with stand-ins until `test` ends.

    aqt.mw is `mw`, Qt names are mocks, the language is English and Anki is ANKI_VERSION.
    Returns the aqt stand-in."""
    aqt = ModuleType("aqt")
    aqt.mw, aqt.gui_hooks = mw, _Hooks()
    utils = ModuleType("aqt.utils")
    utils.tooltip, utils.openLink = Mock(), Mock()
    qt = ModuleType("aqt.qt")
    qt.__getattr__ = lambda name: Mock()
    anki = ModuleType("anki")
    anki.lang = ModuleType("anki.lang")
    anki.lang.current_lang = "en"
    anki.utils = ModuleType("anki.utils")
    anki.utils.int_version = lambda: ANKI_VERSION

    modules = patch.dict(sys.modules, {
        "aqt": aqt, "aqt.utils": utils, "aqt.qt": qt, "anki": anki, "anki.lang": anki.lang, "anki.utils": anki.utils,
    })
    modules.start()
    test.addCleanup(modules.stop)
    return aqt


class FakeApi:
    """A signed-in client.Client whose _request answers by path from `answers`; an exception answer is raised.

    `sent` lists each request as (path, body, method); `features` is what capabilities() reports."""

    user = "hill"
    base = "https://quest.example"
    token = "token"

    def __init__(self, answers=None, features=()):
        self.answers, self.features, self.sent = dict(answers or {}), frozenset(features), []

    def capabilities(self):
        return self.features

    def _request(self, path, body=None, method=None):
        self.sent.append((path, body, method))
        answer = self.answers[path]
        if isinstance(answer, Exception):
            raise answer
        return answer
