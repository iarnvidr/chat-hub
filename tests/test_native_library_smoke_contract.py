from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[1]
MAIN = (ROOT / "src-tauri/src/main.rs").read_text()


class NativeLibrarySmokeContractTests(unittest.TestCase):
 def test_fixture_import_is_compile_time_fixed_and_calls_authenticated_native_api(self):
    self.assertIn('"import_library_fixture" =>', MAIN)
    self.assertIn('include_bytes!("../../tests/fixtures/chatgpt-export/array.json")', MAIN)
    self.assertIn("desktop_api::import(", MAIN)
    self.assertIn("&state.api, &state.token, bytes, count", MAIN)
    self.assertIn('"imported":r.imported', MAIN)
    self.assertIn('"updated":r.updated', MAIN)
    self.assertIn('"unchanged":r.unchanged', MAIN)
    self.assertIn('"failed":r.failed', MAIN)


 def test_fixture_import_requires_debug_smoke_and_disposable_default_data_opt_in(self):
     op = MAIN.split('"import_library_fixture" => {', 1)[1].split('"provider_denial" => {', 1)[0]
     self.assertIn("cfg!(debug_assertions)", op)
     self.assertIn("super::smoke()", op)
     self.assertIn("dir().is_some_and(|p| p.is_absolute() && p.is_dir())", op)
     self.assertIn('MUNINN_E2E_ALLOW_DEFAULT_DATA_DIR', op)
     self.assertIn('v == "1"', op)
     self.assertIn('MUNINN_DATA_DIR', op)
     self.assertIn('MUNINN_E2E_DATA_DIR', op)
     self.assertIn("has_data_override", op)
     self.assertNotIn("isolated_data", op)
     # The setup path used by smoke is the same platform default as production.
     setup = MAIN.split("fn main()", 1)[1]
     self.assertIn("if smoke() {\n                    app.path().app_data_dir()?", setup)
     self.assertNotIn('PathBuf::from(path)', setup)


 def test_default_data_evidence_reports_actual_setup_directory_and_platform_resolver(self):
     op = MAIN.split('"default_data_info" => {', 1)[1].split('"state" => {', 1)[0]
     self.assertIn("cfg!(debug_assertions)", op)
     self.assertIn("super::smoke()", op)
     self.assertIn("e2e_dir_is_valid", op)
     self.assertIn("has_data_override", op)
     self.assertIn("allow_default_data_dir", op)
     self.assertIn("app.path().app_data_dir()", op)
     self.assertIn("state.smoke_data_dir", op)
     self.assertIn('"data_dir_source":"platform_default"', op)
     self.assertIn('"configured_matches_platform_default"', op)
     self.assertIn('#[cfg(all(feature = "native-smoke", debug_assertions))]', MAIN)
     self.assertIn("smoke_data_dir: dir.clone()", MAIN)


 def test_fixture_import_is_terminal_dispatch_operation_and_does_not_accept_input_bytes(self):
     self.assertIn('op == "eval"', MAIN)
     self.assertIn('DispatchReply::Terminal(result)', MAIN)
     op = MAIN.split('"import_library_fixture" => {', 1)[1].split('"provider_denial" => {', 1)[0]
     self.assertNotIn("c[\"path\"]", op)
     self.assertNotIn("c[\"bytes\"]", op)
     self.assertNotIn("c[\"token\"]", op)
     self.assertIn("include_bytes!", op)

if __name__ == "__main__":
 unittest.main()
