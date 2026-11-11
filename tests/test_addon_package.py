"""Release and PR archives must be installable and exclude local state."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from zipfile import ZipFile

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("addon_package", ROOT / ".github/scripts/package-addon.py")
packaging = importlib.util.module_from_spec(spec)
spec.loader.exec_module(packaging)


class Package(unittest.TestCase):
    def test_github_archive_conflicts_with_the_published_ankiweb_id(self):
        with tempfile.TemporaryDirectory() as folder:
            output = Path(folder) / "addon.ankiaddon"
            packaging._build(ROOT / "addon", output, {"release": 13, "nightly": 0, "commit": "abc1234"})
            with ZipFile(output) as archive:
                manifest = json.loads(archive.read("manifest.json"))
                self.assertEqual(["414948448"], manifest.get("conflicts"))
                self.assertNotIn(manifest["package"], manifest["conflicts"])

    def test_ankiweb_archive_uses_its_numeric_id_and_conflicts_only_with_github(self):
        with tempfile.TemporaryDirectory() as folder:
            output = Path(folder) / "addon.ankiaddon"
            packaging._build(ROOT / "addon", output, {"release": 13, "nightly": 0, "commit": "abc1234"}, ankiweb=True)
            with ZipFile(output) as archive:
                manifest = json.loads(archive.read("manifest.json"))
                self.assertEqual("414948448", manifest["package"])
                self.assertEqual(["ankiquest"], manifest["conflicts"])
                self.assertNotIn(manifest["package"], manifest["conflicts"])

    def test_archive_excludes_local_state_and_shows_its_installed_version(self):
        with tempfile.TemporaryDirectory() as folder:
            source = Path(folder) / "source"
            source.mkdir()
            (source / "manifest.json").write_text(json.dumps({"package": "ankiquest", "name": "ankiquest", "min_point_version": 231000}), "utf-8")
            for name in ("__init__.py", "config.md", "art/aki.png", "meta.json", "art/meta.json", "tests/test_private.py",
                         "__pycache__/cached.pyc", "art/__pycache__/cached.pyc", ".hidden.json", "debug.log"):
                path = source / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(b"test")
            output = Path(folder) / "addon.ankiaddon"
            version = {"release": 11, "nightly": 2, "commit": "abcdef0123456789"}
            packaging._build(source, output, version)
            with ZipFile(output) as archive:
                self.assertEqual({"__init__.py", "config.md", "art/aki.png", "manifest.json", "version.json"}, set(archive.namelist()))
                self.assertIsNone(archive.testzip())
                manifest = json.loads(archive.read("manifest.json"))
                self.assertEqual("addon-11 nightly 2 (abcdef0)", manifest["human_version"])
                self.assertEqual(231000, manifest["min_point_version"])
                self.assertEqual(version, json.loads(archive.read("version.json")))
            first = output.read_bytes()
            packaging._build(source, output, version)
            self.assertEqual(first, output.read_bytes())
