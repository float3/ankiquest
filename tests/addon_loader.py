"""Loads add-on modules straight from addon/, so the tests run without Anki.

Each test file names its modules itself, so two files never share a copy one of them patched."""

import importlib.util
import sys
from pathlib import Path
from types import ModuleType

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
