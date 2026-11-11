"""Run the real-Qt checks outside the hook tests' Anki stand-ins."""

import os
from pathlib import Path
import subprocess
import sys
import unittest


class MomentWindow(unittest.TestCase):
    def test_lifecycle_actions_and_delayed_replies(self):
        result = subprocess.run([sys.executable, str(Path(__file__).with_name("qt_moments.py"))],
                                env=dict(os.environ, QT_QPA_PLATFORM="offscreen"),
                                capture_output=True, text=True, timeout=60)
        self.assertEqual(0, result.returncode, result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
