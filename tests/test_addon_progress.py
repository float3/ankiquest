"""Keep the real Progress window outside tests that replace Anki with stand-ins."""
import os
from pathlib import Path
import subprocess
import sys
import unittest


class ProgressWindow(unittest.TestCase):
    def test_years_states_retries_and_account_boundaries(self):
        result = subprocess.run([sys.executable, str(Path(__file__).with_name("qt_progress.py"))],
                                env=dict(os.environ, QT_QPA_PLATFORM="offscreen"),
                                capture_output=True, text=True, timeout=60)
        self.assertEqual(0, result.returncode, result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
