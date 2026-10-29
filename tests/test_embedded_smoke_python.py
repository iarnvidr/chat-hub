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
            (root / "app/Contents").mkdir(parents=True)
            (root / "app/Contents/Info.plist").write_bytes(__import__('plistlib').dumps({'CFBundleShortVersionString': '0.2.0'}))
            (root / "feed").mkdir()
            (root / "new").mkdir()
            (root / "feed/Chat-Hub.app.tar.gz").write_bytes(b"feed payload")
            (root / "new/Chat-Hub.app.tar.gz").write_bytes(b"signed payload")
            tls_cert = root / "upstream.pem"
            subprocess.run([sys.executable, str(ROOT / "build/native-tls-cert.py"), str(tls_cert)], check=True)
            tls_addr = root / "upstream.addr"
            tls_process = subprocess.Popen(
                [sys.executable, str(ROOT / "build/native-tls-fixture.py"), str(tls_cert),
                 str(root / "provider-fixture.json"), str(tls_addr)],
                stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True,
            )
            try:
                import time
                for _ in range(100):
                    if tls_addr.exists() and tls_addr.stat().st_size:
                        break
                    if tls_process.poll() is not None:
                        self.fail(f"TLS fixture exited: {tls_process.stderr.read()}")
                    time.sleep(0.05)
                self.assertTrue(tls_addr.exists() and tls_addr.stat().st_size, "TLS fixture startup timed out")
                cases = {
                    "build/native-launch-smoke.sh": [[str(tls_addr), str(tls_cert), "chatgpt.com"]],
                    "build/updater-smoke/run-macos.sh": [
                        [str(root / "app"), "0.2.0"],
                        [str(root), "0.1.0", "old-signature", "0.1.1", "new-signature", "8765"],
                        [str(root), "0.1.1"],
                    ],
                }
                for script in SCRIPTS:
                    text = script.read_text()
                    blocks = re.findall(r"python3\s+-[^\n]*<<'PY'\s*\n(.*?)\nPY\s*$", text, re.M | re.S)
                    if not blocks:
                        # The native launcher/UI wrappers delegate Python to standalone drivers.
                        self.assertNotRegex(text, r"python3\s+-[^\n]*<<")
                        continue
                    relative = script.relative_to(ROOT).as_posix()
                    self.assertIn(relative, cases, f"missing heredoc fixture mapping for {script}")
                    argsets = cases[relative]
                    self.assertEqual(len(blocks), len(argsets), script)
                    for block, args in zip(blocks, argsets):
                        ast.parse(block)
                        proc = subprocess.run([sys.executable, "-c", block, *args], text=True, capture_output=True)
                        self.assertEqual(proc.returncode, 0, f"{script}: {proc.stderr}")
                        exercised.append((script, block))
                self.assertEqual(len(exercised), 4)
                self.assertEqual((root / "Cargo.toml").read_text(), 'version = "0.0.1"\n')
                self.assertTrue((root / "old.json").is_file())
                self.assertTrue((root / "new.json").is_file())
                self.assertTrue((root / "feed/latest.json").is_file())
                self.assertNotEqual((root / "feed/Chat-Hub.app.tar.gz").read_bytes(), b"feed payload")
            finally:
                tls_process.terminate()
                tls_process.wait(timeout=3)


if __name__ == "__main__":
    unittest.main()
