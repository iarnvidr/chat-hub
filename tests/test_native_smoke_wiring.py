"""Keep native smoke checks isolated; never disguise fail-closed shell gaps."""
import unittest
from pathlib import Path
import json, os, pathlib, subprocess, sys, tempfile, urllib.request
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]


class NativeSmokeWiring(unittest.TestCase):
    def test_existing_native_smokes_remain_debug_only(self):
        workflow = (ROOT / ".github/workflows/macos.yml").read_text()
        self.assertIn("Native signed updater integration smoke (debug bundle)", workflow)
        self.assertIn("Optional updater UI interaction smoke (debug bundle only)", workflow)
        self.assertIn("build/updater-smoke/run-macos.sh", workflow)
        self.assertIn("build/native-ui-smoke.sh", workflow)
        harness = (ROOT / "build/native-ui-smoke.sh").read_text()
        self.assertIn("native-ui-driver.py", harness)
        self.assertIn("MUNINN_NATIVE_UI_SMOKE=1", harness)
        self.assertIn("native-smoke", workflow)
        self.assertIn("native_smoke_artifact.py", workflow)
        self.assertNotIn("BLOCKED: native UI", harness)
        driver = (ROOT / "build/native-ui-driver.py").read_text()
        self.assertIn("screencapture", driver)
        self.assertIn("native-ui.png", driver)
        self.assertIn("window_visible", driver)
        interaction = (ROOT / "build/native-ui-interactions.js").read_text()
        self.assertIn("button.click()", interaction)
        self.assertIn("update_status", interaction)

    def test_release_smoke_discovers_and_authenticates_real_service(self):
        script = (ROOT / "build/smoke-native-macos.sh").read_text()
        main = (ROOT / "src-tauri/src/main.rs").read_text()
        self.assertIn("service.json", script)
        self.assertIn("service.token", script)
        self.assertIn("Authorization", script)
        # Assert production behavior in the embedded Python runner, not a
        # brittle spelling of a URL literal.
        self.assertIn("health_path='/api/health'", script)
        self.assertIn("subprocess.run(['open','-a',str(app)],check=True,timeout=15)", script)
        self.assertIn('python3 - "$evidence" "$app_data" "$app_pid" "$app" <<\'PY\'', script)
        self.assertIn('evidence,data,pid,app=Path(sys.argv[1]),Path(sys.argv[2]),int(sys.argv[3]),sys.argv[4]', script)
        self.assertIn("tell application id", script)
        self.assertIn("service PID changed", script)
        self.assertIn("for _ in range(15):", script)
        self.assertIn("after startup poke", script)
        self.assertNotIn("47871", script)
        self.assertNotIn("/health", script.replace("/api/health", ""))
        self.assertIn('dir.join("service.json")', main)
        self.assertNotIn('"token": token', main)

    def test_embedded_runner_executes_with_space_path_and_detects_pid_change(self):
        script = (ROOT / "build/smoke-native-macos.sh").read_text()
        runner = script.split("python3 - \"$evidence\" \"$app_data\" \"$app_pid\" \"$app\" <<'PY'\n", 1)[1].split("\nPY\n", 1)[0]
        self.assertNotIn("NameError", runner)
        for fail_pid in (False, True):
            with tempfile.TemporaryDirectory() as tmp:
                base = pathlib.Path(tmp); evidence=base/'evidence'; data=base/'data'; app=base/'Chat Hub.app'
                evidence.mkdir(); data.mkdir()
                (data/'service.json').write_text(json.dumps({'api':'http://127.0.0.1:1234','bundle_id':'example.app','process_id':124 if fail_pid else 123}))
                (data/'service.token').write_text('x'*64)
                # urllib's context manager must return JSON through json.load.
                from io import BytesIO
                def urlopen(*a, **kw): return BytesIO(b'{"status":"ok"}')
                with mock.patch.dict(os.environ, {}), mock.patch.object(sys, 'argv', ['runner',str(evidence),str(data),'123',str(app)]), mock.patch.object(os,'kill',side_effect=lambda pid,sig: None), mock.patch.object(subprocess,'run') as run, mock.patch.object(urllib.request,'urlopen',side_effect=urlopen), mock.patch('time.sleep'):
                    code=0
                    try: exec(compile(runner,'embedded-smoke','exec'), {})
                    except SystemExit as exc: code=exc.code
                    self.assertEqual(code != 0, fail_pid)
                    self.assertEqual(run.call_args_list[0].args[0], ['open','-a',str(app)])
                    self.assertEqual(run.call_args_list[0].args[0][-1], str(app))
                    if not fail_pid: self.assertEqual(len(urllib.request.urlopen.call_args_list), 16)

    def test_release_startup_evidence_is_retained_and_uploaded(self):
        script = (ROOT / "build/smoke-native-macos.sh").read_text()
        workflow = (ROOT / ".github/workflows/macos.yml").read_text()
        self.assertIn('"$evidence/app.stdout.log"', script)
        self.assertIn('"$evidence/app.stderr.log"', script)
        self.assertIn("trap on_exit EXIT", script)
        self.assertIn("diagnostics.json", script)
        self.assertIn("service_json_path", script)
        self.assertIn("app_alive", script)
        self.assertIn("app_exitcode", script)
        self.assertIn("chat-hub-release-evidence/**", workflow)
        self.assertIn("release-evidence/**", workflow)
        self.assertIn("if-no-files-found: error", workflow)
        self.assertNotIn("service.token", workflow)

    def test_no_smoke_claim_in_artifact_summary(self):
        workflow = (ROOT / ".github/workflows/macos.yml").read_text()
        self.assertIn("ad-hoc signed and not notarized", workflow)
        self.assertIn("chat-hub-debug-ui-evidence", workflow)


if __name__ == "__main__":
    unittest.main()
