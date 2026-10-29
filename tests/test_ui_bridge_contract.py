import json
import pathlib
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
        self.assertIn("service_info(s: State<'_, AppState>)", tauri)

    def test_provider_webview_is_external_and_not_local_command_window(self):
        tauri = (ROOT / "src-tauri/src/main.rs").read_text()
        self.assertIn('"provider",', tauri)
        self.assertIn('WebviewUrl::External("https://chatgpt.com"', tauri)
        config = json.loads((ROOT / "src-tauri/tauri.conf.json").read_text())
        self.assertEqual(config["app"]["windows"][0]["title"], "Chat Hub")


if __name__ == "__main__":
    unittest.main()
