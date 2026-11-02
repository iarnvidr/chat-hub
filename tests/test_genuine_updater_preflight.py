import hashlib
import http.server
import importlib.util
import json
from pathlib import Path
import tempfile
import threading
import unittest

MODULE = Path(__file__).resolve().parents[1] / "build/genuine-updater-preflight/preflight.py"
spec = importlib.util.spec_from_file_location("genuine_updater_preflight", MODULE)
preflight_module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(preflight_module)


class HealthHandler(http.server.BaseHTTPRequestHandler):
    token = "a" * 64

    def do_GET(self):
        if self.path != "/api/health" or self.headers.get("Authorization") != f"Bearer {self.token}":
            self.send_response(401)
            payload = b'{"error":"unauthorized"}'
        else:
            self.send_response(200)
            payload = b'{"status":"ok"}'
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def log_message(self, *_args):
        pass


class PreflightTests(unittest.TestCase):
    def setUp(self):
        self.server = http.server.HTTPServer(("127.0.0.1", 0), HealthHandler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.home = self.root / "home"
        self.data = self.home / preflight_module.APP_DATA_SUFFIX
        self.data.mkdir(parents=True)
        self.dmg = self.root / "Chat_Hub_0.1.1_macos-arm64.dmg"
        self.dmg.write_bytes(b"exact published disk image bytes")
        self.app = self.root / "Chat Hub.app"
        self.app.mkdir()
        self.pid = 4312
        self.token = HealthHandler.token
        (self.data / "service.token").write_text(self.token)
        (self.data / "service.json").write_text(json.dumps({
            "service": "chat-hub",
            "version": "0.1.1",
            "api": f"http://127.0.0.1:{self.server.server_port}",
            "process_id": self.pid,
        }))
        self.executable_digest = hashlib.sha256(b"baseline executable").hexdigest()
        self.ax_record = {
            "status": "ready", "reason": None, "pid": self.pid,
            "identity_matches": True, "trusted": True, "check_button_count": 1,
            "check_button_enabled": True, "check_button_press_supported": True,
        }
        self.commands = []

    def tearDown(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=2)
        self.temp.cleanup()

    def args(self, **changes):
        values = {
            "dmg": str(self.dmg),
            "expected_dmg_sha256": preflight_module.file_sha256(self.dmg),
            "app": str(self.app),
            "expected_executable_sha256": self.executable_digest,
        }
        values.update(changes)
        return type("Args", (), values)()

    def runner(self, argv, **kwargs):
        self.commands.append(list(argv))
        if argv[0] == "/usr/sbin/lsof":
            return type("Completed", (), {
                "returncode": 0,
                "stdout": f"COMMAND PID USER FD TYPE DEVICE SIZE/OFF NODE NAME\nChat Hub {self.pid} user 6u IPv4 0 0 TCP 127.0.0.1:{self.server.server_port} (LISTEN)\n",
                "stderr": "",
            })()
        if argv[0] == "/usr/bin/codesign" or argv[0] == "/usr/bin/swiftc":
            return type("Completed", (), {"returncode": 0, "stdout": "", "stderr": ""})()
        if argv[0].endswith("/ax-preflight"):
            return type("Completed", (), {
                "returncode": 0, "stdout": json.dumps(self.ax_record), "stderr": "",
            })()
        self.fail(f"unexpected command: {argv[0]}")

    def test_ready_result_requires_owned_pid_listener_health_signature_and_ax(self):
        result = preflight_module.preflight(
            self.args(), platform="darwin", home=self.home, runner=self.runner)
        self.assertEqual(result["status"], "ready")
        self.assertEqual(result["process_id"], self.pid)
        self.assertTrue(result["codesign_integrity_verified"])
        self.assertTrue(result["health_status_ok"])
        self.assertTrue(result["accessibility_trusted"])
        self.assertFalse(result["checked_feed"])
        self.assertNotIn(self.token, json.dumps(result))
        self.assertTrue(any(command[0] == "/usr/sbin/lsof" and str(self.pid) in command for command in self.commands))
        helper_args = next(command for command in self.commands if command[0].endswith("/ax-preflight"))
        self.assertEqual(helper_args[1], str(self.pid))
        self.assertEqual(helper_args[2], "io.github.iarnvidr.chat-hub")
        self.assertEqual(helper_args[3], "0.1.1")

    def test_wrong_published_digest_fails_before_touching_process(self):
        with self.assertRaisesRegex(preflight_module.PreflightBlocked, "published_dmg_digest_mismatch"):
            preflight_module.preflight(
                self.args(expected_dmg_sha256="0" * 64), platform="darwin",
                home=self.home, runner=self.runner)
        self.assertEqual(self.commands, [])

    def test_unexpected_service_discovery_field_fails_closed(self):
        record_path = self.data / "service.json"
        record = json.loads(record_path.read_text())
        record["proxy"] = "http://127.0.0.1:1234"
        record_path.write_text(json.dumps(record))
        with self.assertRaisesRegex(preflight_module.PreflightBlocked, "service_record_schema_mismatch"):
            preflight_module.preflight(self.args(), platform="darwin", home=self.home, runner=self.runner)

    def test_wrong_token_does_not_claim_old_health(self):
        (self.data / "service.token").write_text("b" * 64)
        with self.assertRaisesRegex(preflight_module.PreflightBlocked, "baseline_health_contract_mismatch"):
            preflight_module.preflight(self.args(), platform="darwin", home=self.home, runner=self.runner)

    def test_wrong_baseline_version_fails_before_listener_or_ax(self):
        record_path = self.data / "service.json"
        record = json.loads(record_path.read_text())
        record["version"] = "0.2.0"
        record_path.write_text(json.dumps(record))
        with self.assertRaisesRegex(preflight_module.PreflightBlocked, "baseline_service_version_mismatch"):
            preflight_module.preflight(self.args(), platform="darwin", home=self.home, runner=self.runner)
        self.assertFalse(any(command[0] == "/usr/sbin/lsof" for command in self.commands))

    def test_listener_owned_by_other_pid_fails_closed(self):
        original = self.runner
        def other_pid_runner(argv, **kwargs):
            result = original(argv, **kwargs)
            if argv[0] == "/usr/sbin/lsof":
                result.stdout = "COMMAND PID USER FD TYPE DEVICE SIZE/OFF NODE NAME\n"
            return result
        with self.assertRaisesRegex(preflight_module.PreflightBlocked, "baseline_listener_identity_mismatch"):
            preflight_module.preflight(self.args(), platform="darwin", home=self.home, runner=other_pid_runner)

    def test_ax_permission_failure_is_preserved_as_blocked(self):
        self.ax_record = dict(self.ax_record, status="blocked", reason="accessibility_trust_missing",
                              trusted=False, check_button_count=0, check_button_enabled=False,
                              check_button_press_supported=False)
        with self.assertRaisesRegex(preflight_module.PreflightBlocked, "accessibility_trust_missing"):
            preflight_module.preflight(self.args(), platform="darwin", home=self.home, runner=self.runner)

    def test_wrong_platform_stops_before_any_io_or_execution(self):
        with self.assertRaisesRegex(preflight_module.PreflightBlocked, "macos_required"):
            preflight_module.preflight(self.args(), platform="linux", home=self.home, runner=self.runner)
        self.assertEqual(self.commands, [])


if __name__ == "__main__":
    unittest.main()
