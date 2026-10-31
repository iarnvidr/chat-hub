"""Exercise the real image-creation wrapper without mounting or launching anything."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class DmgCreateRetry(unittest.TestCase):
    def run_create(self, mode):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            executable = root / "hdiutil"
            executable.write_text("""#!/usr/bin/env python3
import json, os, pathlib, sys
counter = pathlib.Path(os.environ['COUNTER'])
count = int(counter.read_text()) + 1 if counter.exists() else 1
counter.write_text(str(count))
pathlib.Path(os.environ['ARGS']).write_text(json.dumps(sys.argv[1:]))
mode = os.environ['MODE']
if mode == 'fatal':
    print('hdiutil: create failed - Permission denied', file=sys.stderr)
    sys.exit(7)
if mode == 'busy' or (mode == 'transient' and count < 3):
    print('hdiutil: create failed - Resource busy', file=sys.stderr)
    sys.exit(16)
print('created image')
""")
            executable.chmod(0o755)
            sleep = root / "sleep"
            sleep.write_text("#!/bin/sh\nexit 0\n")
            sleep.chmod(0o755)
            env = {**os.environ, "PATH": str(root) + os.pathsep + os.environ['PATH'], "COUNTER": str(root / "counter"), "ARGS": str(root / "args"), "MODE": mode}
            args = ["-srcfolder", "folder with spaces", "-ov", "image.dmg"]
            result = subprocess.run(["bash", str(ROOT / "build/create-dmg-image.sh"), *args], env=env, capture_output=True, text=True)
            self.assertEqual(json.loads((root / "args").read_text()), ["create", *args])
            return result, int((root / "counter").read_text())

    def test_success_does_not_retry(self):
        result, count = self.run_create('success')
        self.assertEqual((result.returncode, count), (0, 1))

    def test_transient_busy_retries_and_preserves_arguments(self):
        result, count = self.run_create('transient')
        self.assertEqual((result.returncode, count), (0, 3))

    def test_persistent_busy_is_bounded_and_fails_closed(self):
        result, count = self.run_create('busy')
        self.assertEqual((result.returncode, count), (16, 3))

    def test_other_errors_are_not_retried(self):
        result, count = self.run_create('fatal')
        self.assertEqual((result.returncode, count), (7, 1))


if __name__ == '__main__':
    unittest.main()
