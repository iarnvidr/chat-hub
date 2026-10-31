"""Execute production authorization policy and panic hook without desktop dependencies."""
import pathlib
import subprocess
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]
POLICY = ROOT / "src-tauri/src/window_policy.rs"
CRASH = ROOT / "src-tauri/src/crash_diagnostics.rs"


class WindowPolicyTests(unittest.TestCase):
    def test_production_rust_policy_and_ownership_tests(self):
        with tempfile.TemporaryDirectory(dir=ROOT) as temp:
            binary = pathlib.Path(temp) / "window-policy-tests"
            subprocess.run(["rustc", "--test", "--edition=2021", str(POLICY), "-o", str(binary)], check=True, timeout=60)
            binary.chmod(0o700)
            result = subprocess.run(["bash", "-c", '"$1"', "runner", str(binary)], text=True, capture_output=True, check=True, timeout=30)
            self.assertIn("2 passed", result.stdout)

    def test_parsed_urls_execute_production_policy(self):
        result = subprocess.run(
            ["cargo", "test", "--locked", "--manifest-path", str(ROOT / "Cargo.toml"), "--test", "window_policy"],
            capture_output=True,
            text=True,
            timeout=180,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_native_main_ownership_wiring_is_single_and_post_build(self):
        source = (ROOT / "src-tauri/src/main.rs").read_text()
        setup = source[source.index(".setup(|app|"):]
        self.assertEqual(setup.count("WebviewWindowBuilder::from_config(app, &main_config)"), 1)
        self.assertEqual(setup.count("register_after_build()"), 1)
        build_end = setup.index(".map_err(|error|", setup.index("WebviewWindowBuilder::from_config(app, &main_config)"))
        self.assertLess(build_end, setup.index("main.on_window_event("))
        self.assertLess(setup.index("main.on_window_event("), setup.index("register_after_build()"))
        self.assertNotIn(".on_window_event(", setup[:setup.index("WebviewWindowBuilder::from_config(app, &main_config)" )])
        self.assertIn("window_policy::allows_origin(", setup)
        self.assertIn("ownership.revoke()", setup)
        self.assertIn("move |event|", setup)
        self.assertNotIn("move |_, event|", setup)
        self.assertNotIn("register_after_build()", source[:source.index(".setup(|app|")])

    def test_panic_hook_runs_in_subprocess_and_bounds_complete_report(self):
        with tempfile.TemporaryDirectory(dir=ROOT) as temp:
            temp = pathlib.Path(temp)
            harness = temp / "panic.rs"
            harness.write_text(f'''#[path = "{CRASH}"] mod crash_diagnostics;
fn main() {{ crash_diagnostics::install(); panic!("{{}}", format!("UNICODE-λ-{{}}", "x".repeat(50000))); }}
''')
            binary = temp / "panic"
            command = ["rustc", "--edition=2021", str(harness), "-o", str(binary)]
            if sys.platform == "darwin":
                command.extend(["-l", "framework=Foundation", "-l", "objc"])
            subprocess.run(command, check=True, timeout=60)
            binary.chmod(0o700)
            env = dict(__import__("os").environ, RUST_BACKTRACE="1", CHAT_HUB_CRASH_EVIDENCE=str(temp / "evidence"))
            result = subprocess.run(["bash", "-c", '"$1"', "runner", str(binary)], capture_output=True, env=env, timeout=15)
            report = result.stderr
            self.assertNotEqual(result.returncode, 0)
            self.assertIn(b"Rust panic at", report)
            self.assertIn("UNICODE-λ-".encode(), report)
            self.assertLessEqual(len(report), 40 * 1024)
            lines = report.splitlines()
            self.assertLessEqual(len(lines[0].decode("utf-8")), 1030)
            for line in lines[1:]:
                self.assertLessEqual(len(line.decode("utf-8")), 200)
            evidence = (temp / "evidence").read_bytes()
            self.assertLessEqual(len(evidence), 40 * 1024)
            self.assertIn(b"Rust panic at", evidence)


if __name__ == "__main__":
    unittest.main()
