"""Run inbox lifecycle checks without the suite's Anki stand-ins."""
import os
from pathlib import Path
import subprocess
import sys
import unittest


class ActivityWindow(unittest.TestCase):
    def test_pagination_actions_and_lifecycle(self):
        result = subprocess.run([sys.executable, str(Path(__file__).with_name("qt_activity.py"))],
                                env=dict(os.environ, QT_QPA_PLATFORM="offscreen"),
                                capture_output=True, text=True, timeout=60)
        self.assertEqual(0, result.returncode, result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
