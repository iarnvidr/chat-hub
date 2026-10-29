import pathlib
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "build/updater-smoke/prepare_fixture.py"

class FixtureIsolation(unittest.TestCase):
    def test_refuses_checkout(self):
        result = subprocess.run(["python3", str(SCRIPT), str(ROOT), "0.1.0"], capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("refusing to mutate", result.stderr)

if __name__ == "__main__":
    unittest.main()
