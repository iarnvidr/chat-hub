"""Execute production authorization policy and panic hook without desktop dependencies."""
import pathlib
import subprocess
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
        with tempfile.TemporaryDirectory(dir=ROOT) as temp:
            project = pathlib.Path(temp)
            (project / "src").mkdir()
            (project / "tests").mkdir()
            (project / "Cargo.toml").write_text('[package]\nname="url-policy-check"\nversion="0.1.0"\nedition="2021"\n[dependencies]\nurl="=2.5.8"\n')
            (project / "src/lib.rs").write_text(f'#[path = "{POLICY}"] pub mod window_policy;')
            (project / "tests/parsed.rs").write_text('''use url::Url; use url_policy_check::window_policy::{allows_origin,is_provider_origin};\n#[test] fn parsed_origins_are_strict() {\n for (raw, expected) in [("tauri://localhost",true),("http://tauri.localhost",true),("https://tauri.localhost",false),("http://tauri.localhost:8080",false),("http://tauri.localhost:443",false),("http://user@tauri.localhost",false),("http://user:pw@tauri.localhost",false),("http://tauri.localhost.evil",false)] { let u=Url::parse(raw).unwrap(); assert_eq!(allows_origin(u.scheme(),u.host_str(),u.port(),u.username(),u.password()),expected,"{raw}"); }\n for (raw, expected) in [("https://chatgpt.com",true),("https://chatgpt.com/path?q=x",true),("https://chatgpt.com.evil/",false),("https://chatgpt.com.evil@evil/",false),("https://chatgpt.com:444/",false),("https://chatgpt.com:443/",true),("https://chatgpt.com./",false),("http://chatgpt.com/",false)] { let u=Url::parse(raw).unwrap(); assert_eq!(is_provider_origin(u.scheme(),u.host_str(),u.port(),u.username(),u.password()),expected,"{raw}"); }\n for (raw, expected) in [("https://chatgpt.com",true),("https://chatgpt.com/path?q=x",true),("https://chatgpt.com.evil/",false),("https://chatgpt.com.evil@evil/",false),("https://chatgpt.com:444/",false),("https://chatgpt.com:443/",true),("https://chatgpt.com./",false),("http://chatgpt.com/",false)] { let u=Url::parse(raw).unwrap(); assert_eq!(is_provider_origin(u.scheme(),u.host_str(),u.port(),u.username(),u.password()),expected,"{raw}"); }\n}''')
            result = subprocess.run(["cargo", "test", "--offline", "--manifest-path", str(project / "Cargo.toml")], capture_output=True, text=True, timeout=120)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_native_main_ownership_wiring_is_single_and_post_build(self):
        source = (ROOT / "src-tauri/src/main.rs").read_text()
        setup = source[source.index(".setup(|app|"):]
        self.assertEqual(setup.count("WebviewWindowBuilder::from_config(app, &main_config)"), 1)
        self.assertEqual(setup.count("register_after_build()"), 1)
        self.assertLess(setup.index(".build()?;"), setup.index("register_after_build()"))
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
            subprocess.run(["rustc", "--edition=2021", str(harness), "-o", str(binary)], check=True, timeout=60)
            binary.chmod(0o700)
            env = dict(__import__("os").environ, RUST_BACKTRACE="1", CHAT_HUB_CRASH_EVIDENCE=str(temp / "evidence"))
            result = subprocess.run(["bash", "-c", '"$1"', "runner", str(binary)], capture_output=True, env=env, timeout=15)
            report = result.stderr
            self.assertNotEqual(result.returncode, 0)
            self.assertIn(b"Rust panic at", report)
            self.assertIn("UNICODE-λ-".encode(), report)
            self.assertLessEqual(len(report), 40 * 1024)
            for line in report.splitlines():
                self.assertLessEqual(len(line.decode("utf-8")), 200)
            evidence = (temp / "evidence").read_bytes()
            self.assertLessEqual(len(evidence), 40 * 1024)
            self.assertIn(b"Rust panic at", evidence)


if __name__ == "__main__":
    unittest.main()
