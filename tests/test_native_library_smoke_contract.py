from pathlib import Path
import re
import unittest


ROOT = Path(__file__).resolve().parents[1]
MAIN = (ROOT / "src-tauri/src/main.rs").read_text()


def arm(name, next_name):
    return MAIN.split(f'"{name}" => {{', 1)[1].split(f'"{next_name}" => {{', 1)[0]


def call_arguments(source, callee):
    """Extract a balanced Rust call argument span without whitespace sensitivity."""
    start = source.index(callee) + len(callee)
    while source[start].isspace():
        start += 1
    assert source[start] == "("
    depth, in_string, escaped = 0, False, False
    for end in range(start, len(source)):
        ch = source[end]
        if in_string:
            if escaped:
                escaped = False
            elif ch == "\\":
                escaped = True
            elif ch == '"':
                in_string = False
            continue
        if ch == '"':
            in_string = True
        elif ch == "(":
            depth += 1
        elif ch == ")":
            depth -= 1
            if depth == 0:
                return source[start + 1:end]
    raise AssertionError(f"unterminated call {callee}")


class NativeLibrarySmokeContractTests(unittest.TestCase):
    def test_fixture_import_is_compile_time_fixed_and_calls_authenticated_native_api(self):
        op = arm("import_library_fixture", "provider_denial")
        self.assertIn('include_bytes!("../../tests/fixtures/chatgpt-export/array.json")', op)
        args = re.sub(r"\s+", "", call_arguments(op, "super::desktop_api::import"))
        for token in ("&state.api", "&state.token", "bytes", "count"):
            self.assertIn(token, args)
        self.assertIn('"imported":r.imported', op)
        self.assertIn('"updated":r.updated', op)
        self.assertIn('"unchanged":r.unchanged', op)
        self.assertIn('"failed":r.failed', op)

    def test_fixture_import_requires_debug_smoke_and_disposable_default_data_opt_in(self):
        op = arm("import_library_fixture", "provider_denial")
        self.assertIn("cfg!(debug_assertions)", op)
        self.assertIn("super::smoke()", op)
        self.assertIn("dir().is_some_and(|p| p.is_absolute() && p.is_dir())", op)
        self.assertIn('MUNINN_E2E_ALLOW_DEFAULT_DATA_DIR', op)
        self.assertIn('v == "1"', op)
        self.assertIn('MUNINN_DATA_DIR', op)
        self.assertIn('MUNINN_E2E_DATA_DIR', op)
        self.assertIn("has_data_override", op)
        self.assertNotIn("isolated_data", op)

    def test_default_data_evidence_release_arm_does_not_access_debug_only_field(self):
        op = arm("default_data_info", "state")
        self.assertIn("#[cfg(not(debug_assertions))]", op)
        self.assertIn("native smoke default data evidence disabled", op)
        debug = op.split("#[cfg(debug_assertions)]", 1)[1]
        self.assertIn("state.smoke_data_dir", debug)
        self.assertIn("app.path().app_data_dir()", debug)
        self.assertIn('"data_dir_source":"platform_default"', debug)
        self.assertIn('"configured_matches_platform_default"', debug)
        self.assertIn('#[cfg(all(feature = "native-smoke", debug_assertions))]', MAIN)
        self.assertIn("smoke_data_dir: dir.clone()", MAIN)

    def test_legacy_smoke_data_dir_override_is_selected_only_for_smoke(self):
        setup = MAIN.split("fn main()", 1)[1]
        self.assertIn('std::env::var_os("MUNINN_E2E_DATA_DIR")', setup)
        smoke_branch = setup.split("if smoke() {", 1)[1].split("} else {", 1)[0]
        normal_branch = setup.split("} else {", 1)[1].split("#[cfg(not(feature", 1)[0]
        self.assertIn("PathBuf::from", smoke_branch)
        self.assertIn("app.path().app_data_dir()?", smoke_branch)
        self.assertNotIn("MUNINN_E2E_DATA_DIR", normal_branch)
        self.assertIn("app.path().app_data_dir()?", normal_branch)
        self.assertIn("smoke_data_dir: dir.clone()", MAIN)

    def test_fixture_import_is_terminal_and_does_not_accept_input_bytes(self):
        self.assertIn('op == "eval"', MAIN)
        self.assertIn("DispatchReply::Terminal(result)", MAIN)
        op = arm("import_library_fixture", "provider_denial")
        self.assertNotIn('c["path"]', op)
        self.assertNotIn('c["bytes"]', op)
        self.assertNotIn('c["token"]', op)
        self.assertIn("include_bytes!", op)


if __name__ == "__main__":
    unittest.main()
