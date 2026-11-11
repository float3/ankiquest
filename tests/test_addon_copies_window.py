"""Run copy-dialog lifecycle checks with real Qt in an isolated process."""
import os
from pathlib import Path
import subprocess
import sys
import unittest


class CopiesWindow(unittest.TestCase):
    def test_profile_and_dialog_ownership(self):
        result = subprocess.run([sys.executable, str(Path(__file__).with_name("qt_copies.py"))],
                                env=dict(os.environ, QT_QPA_PLATFORM="offscreen"),
                                capture_output=True, text=True, timeout=90)
        self.assertEqual(0, result.returncode, result.stdout + result.stderr)
