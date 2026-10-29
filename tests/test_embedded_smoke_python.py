"""Execute Python heredocs embedded in release smoke shell scripts with fixtures."""
import ast
import pathlib
import re
import subprocess
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]
SCRIPTS = [
    ROOT / "build/native-launch-smoke.sh",
    ROOT / "build/native-ui-smoke.sh",
    ROOT / "build/updater-smoke/run-macos.sh",
]


class EmbeddedPythonSmokeTest(unittest.TestCase):
    def test_all_embedded_python_blocks_execute_with_fixture_arguments(self):
        exercised = []
        with tempfile.TemporaryDirectory(prefix="smoke fixture with spaces ") as td:
            root = pathlib.Path(td)
            manifest = root / "Cargo.toml"
            manifest.write_text('version = "0.0.1"\n')
            (root / "feed").mkdir()
            (root / "new").mkdir()
            (root / "feed/Muninn.app.tar.gz").write_bytes(b"feed payload")
            (root / "new/Muninn.app.tar.gz").write_bytes(b"signed payload")
            cases = {
                "build/updater-smoke/run-macos.sh": [
                    [str(manifest), "0.2.0"],
                    [str(root), "0.1.0", "old-signature", "0.1.1", "new-signature", "8765"],
                    [str(root), "0.1.1"],
                ]
            }
            for script in SCRIPTS:
                text = script.read_text()
                blocks = re.findall(r"python3\s+-[^\n]*<<'PY'\s*\n(.*?)\nPY\s*$", text, re.M | re.S)
                if not blocks:
                    # The native launcher/UI wrappers delegate Python to standalone drivers.
                    self.assertNotRegex(text, r"python3\s+-[^\n]*<<")
                    continue
                argsets = cases[script.relative_to(ROOT).as_posix()]
                self.assertEqual(len(blocks), len(argsets), script)
                for block, args in zip(blocks, argsets):
                    ast.parse(block)
                    proc = subprocess.run([sys.executable, "-c", block, *args], text=True, capture_output=True)
                    self.assertEqual(proc.returncode, 0, f"{script}: {proc.stderr}")
                    exercised.append((script, block))
            self.assertEqual(len(exercised), 3)
            self.assertEqual((root / "Cargo.toml").read_text(), 'version = "0.2.0"\n')
            self.assertTrue((root / "old.json").is_file())
            self.assertTrue((root / "new.json").is_file())
            self.assertTrue((root / "feed/latest.json").is_file())
            self.assertNotEqual((root / "feed/Muninn.app.tar.gz").read_bytes(), b"feed payload")


if __name__ == "__main__":
    unittest.main()
