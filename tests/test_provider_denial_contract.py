import pathlib
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]


class ProviderDenialContract(unittest.TestCase):
    def test_native_driver_runs_provider_invoke_and_requires_denial_evidence(self):
        driver = (ROOT / "build/native-ui-driver.py").read_text()
        source = (ROOT / "src-tauri/src/main.rs").read_text()
        self.assertIn("provider_denial", driver)
        self.assertIn("denial.get('denied') is not True", driver)
        self.assertIn("invoke('service_info')", source)
        self.assertIn("e2e_provider_result", source)
        self.assertIn('window.label() != "provider"', source)
        self.assertIn('url.host_str() != Some("chatgpt.com")', source)
        self.assertIn("authorize_local_main(&window)?;", source[source.index("fn service_info("):])
        handlers = source.split(".invoke_handler", 1)[-1]
        self.assertIn("e2e_provider_result", handlers)


if __name__ == "__main__":
    unittest.main()
