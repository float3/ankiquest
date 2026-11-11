"""The full Anki smoke runs separately from tests that replace aqt with stand-ins."""
import os
from pathlib import Path
import subprocess
import sys
import unittest


class Compatibility(unittest.TestCase):
    def test_the_addon_loads_into_real_anki(self):
        result = subprocess.run(
            [sys.executable, str(Path(__file__).with_name("qt_addon_smoke.py"))],
            env=dict(os.environ, QT_QPA_PLATFORM="offscreen"),
            capture_output=True, text=True, timeout=60,
        )
        self.assertEqual(0, result.returncode, result.stdout + result.stderr)
