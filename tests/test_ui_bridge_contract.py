import json
import pathlib
import re
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]


class UiBridgeContract(unittest.TestCase):
    def test_local_ui_uses_tauri_internal_invoke_and_calls_real_service(self):
        config = json.loads((ROOT / "src-tauri/tauri.conf.json").read_text())
        self.assertFalse(config["app"]["withGlobalTauri"])
        self.assertEqual(config["build"]["frontendDist"], "../desktop-ui")
        windows = config["app"]["windows"]
        self.assertEqual([window.get("label", "main") for window in windows], ["main"])
        ui = (ROOT / "desktop-ui/index.html").read_text()
        self.assertIn("window.__TAURI_INTERNALS__?.invoke", ui)
        self.assertIn("await invoke('service_info')", ui)
        self.assertIn("window.ChatHub={invoke}", ui)
        tauri = (ROOT / "src-tauri/src/main.rs").read_text()
        self.assertIn("service_info,", tauri)
        self.assertRegex(tauri, r"fn service_info\(\s*window: tauri::WebviewWindow")

    def test_provider_webview_is_external_and_not_local_command_window(self):
        tauri = (ROOT / "src-tauri/src/main.rs").read_text()
        self.assertIn('"provider",', tauri)
        self.assertIn('WebviewUrl::External("https://chatgpt.com"', tauri)
        config = json.loads((ROOT / "src-tauri/tauri.conf.json").read_text())
        self.assertEqual(config["app"]["windows"][0]["title"], "Chat Hub")

    def test_sensitive_commands_receive_native_window_and_guard(self):
        source = (ROOT / "src-tauri/src/main.rs").read_text()
        sensitive = ("update_status", "service_info", "check_for_updates", "install_update", "e2e_result")
        for command in sensitive:
            with self.subTest(command=command):
                matches = list(re.finditer(
                    rf"fn {command}\s*\((.*?)\)\s*(?:->[^{{]+)?\{{(.*?)\n\}}",
                    source,
                    re.S,
                ))
                self.assertTrue(matches, f"missing command {command}")
                signature, body = matches[-1].groups()
                self.assertRegex(signature, r"window:\s*tauri::WebviewWindow")
                self.assertIn("authorize_local_main(&window)?;", body)

    def test_guard_allows_only_main_packaged_local_origins(self):
        source = (ROOT / "src-tauri/src/main.rs").read_text()
        match = re.search(r"fn authorize_local_main\(.*?\n\}", source, re.S)
        self.assertIsNotNone(match)
        guard = match.group()
        self.assertIn('window.label() != "main"', guard)
        self.assertIn('url.scheme() == "tauri" && url.host_str() == Some("localhost")', guard)
        self.assertIn('url.scheme() == "http" && url.host_str() == Some("tauri.localhost")', guard)
        self.assertIn('if !local', guard)
        self.assertNotIn("https://", guard)
        handlers = source.split(".invoke_handler", 1)[-1]
        self.assertRegex(handlers, r"e2e_result\s*,?")
        production_handlers = handlers.split("#[cfg(not(feature = \"native-smoke\"))]", 1)[-1]
        self.assertNotIn("e2e_result", production_handlers)
        self.assertNotIn("e2e_provider_result", production_handlers)


if __name__ == "__main__":
    unittest.main()
