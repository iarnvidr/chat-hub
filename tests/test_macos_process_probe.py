import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

MODULE = Path(__file__).resolve().parents[1] / "build" / "macos-process-probe.py"
spec = importlib.util.spec_from_file_location("macos_process_probe", MODULE)
probe = importlib.util.module_from_spec(spec)
spec.loader.exec_module(probe)


class ProbeTests(unittest.TestCase):
    def test_natural_nonzero_exit_is_recorded_not_terminated(self):
        with tempfile.TemporaryDirectory() as td:
            app = Path(td) / "fake app.py"
            app.write_text("import time\nprint('started', flush=True)\ntime.sleep(.05)\nraise SystemExit(7)\n")
            output = subprocess.run([sys.executable, str(MODULE), sys.executable, td, "--duration", "0.2", "-c", app.read_text()],
                                    capture_output=True, text=True, check=True)
            result = json.loads(output.stdout)
            self.assertEqual(result["status"], "natural_exit")
            self.assertEqual(result["exit_code"], 7)
            self.assertFalse(result["cleanup_requested"])
            self.assertGreaterEqual(result["elapsed_ms"], 30)
            self.assertIn("started", (Path(td) / "probe.stdout.log").read_text())

    def test_survival_then_intentional_termination_is_distinct(self):
        with tempfile.TemporaryDirectory() as td:
            app = Path(td) / "loop application"
            app.write_text("#!/usr/bin/env python3\nimport time\nwhile True: time.sleep(.02)\n")
            app.chmod(0o644)
            output = subprocess.run([sys.executable, str(MODULE), sys.executable, td, "--duration", "0.2", str(app)], capture_output=True, text=True, check=True)
            result = json.loads(output.stdout)
            self.assertEqual(result["status"], "survived_duration")
            self.assertTrue(result["survived_duration"])
            self.assertTrue(result["cleanup_requested"])
            self.assertIn(result["cleanup_signal"], ("SIGTERM", "SIGKILL"))
            self.assertNotIn("exit_code", result)
            self.assertGreaterEqual(result["elapsed_ms"], 150)


if __name__ == "__main__":
    unittest.main()
