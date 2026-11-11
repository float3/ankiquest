"""Run the real leaderboard controls separately from Anki stand-ins."""
import os
from pathlib import Path
import subprocess
import sys
import unittest


class BoardWindow(unittest.TestCase):
    def test_scopes_privacy_retries_and_lifecycle(self):
        result = subprocess.run([sys.executable, str(Path(__file__).with_name("qt_boards.py"))],
                                env=dict(os.environ, QT_QPA_PLATFORM="offscreen"),
                                capture_output=True, text=True, timeout=60)
        self.assertEqual(0, result.returncode, result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
