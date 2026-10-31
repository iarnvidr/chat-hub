import ast
import pathlib
import unittest


DRIVER = pathlib.Path(__file__).with_name("native-ui-driver.py")


class NativeUiDriverHistorySmokeContract(unittest.TestCase):
    def test_opt_in_smoke_is_gated_bounded_and_uses_only_safe_status_fields(self):
        source = DRIVER.read_text()
        ast.parse(source)
        self.assertIn("MUNINN_HISTORY_SESSION_SMOKE", source)
        self.assertIn('"get_chatgpt_connection"', source)
        self.assertIn('document.getElementById("sync-history")', source)
        self.assertIn('"chatgpt_history_status"', source)
        sync_ui = DRIVER.parents[1] / "desktop-ui" / "history-sync.js"
        self.assertIn("sync_chatgpt_history", sync_ui.read_text())
        self.assertIn("time.monotonic()+15", source)
        self.assertIn("timeout_seconds=3", source)
        self.assertIn("safe_status['error_code'] not in {'signed_out','login_required'}", source)
        self.assertIn("safe_status['processed_conversations'] != 0", source)
        # Status evidence is whitelisted; no account identity or content fields.
        block = source.split("if os.environ.get('MUNINN_HISTORY_SESSION_SMOKE') == '1':", 1)[1]
        self.assertNotIn("status.get('message')", block)
        self.assertNotIn("status.get('email')", block)
        self.assertNotIn("status.get('conversation')", block)

    def test_provider_readiness_accepts_signed_out_or_connected_probe_states(self):
        source = DRIVER.read_text()
        self.assertIn("ChatGPT is open|Sign in to ChatGPT|Connected to ChatGPT|ChatGPT is connected", source)


if __name__ == "__main__":
    unittest.main()
