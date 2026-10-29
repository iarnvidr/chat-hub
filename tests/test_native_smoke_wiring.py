"""Keep native smoke checks isolated; never disguise fail-closed shell gaps."""
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


class NativeSmokeWiring(unittest.TestCase):
    def test_existing_native_smokes_remain_debug_only(self):
        workflow = (ROOT / ".github/workflows/macos.yml").read_text()
        self.assertIn("Native signed updater integration smoke (debug bundle)", workflow)
        self.assertIn("Native UI interaction smoke (debug bundle only)", workflow)
        self.assertIn("build/updater-smoke/run-macos.sh", workflow)
        self.assertIn("build/native-ui-smoke.sh", workflow)
        harness = (ROOT / "build/native-ui-smoke.sh").read_text()
        self.assertIn("native-ui-driver.py", harness)
        self.assertIn("MUNINN_NATIVE_UI_SMOKE=1", harness)
        self.assertIn("native-smoke", workflow)
        self.assertIn("chat-hub-desktop", workflow)
        self.assertNotIn("BLOCKED: native UI", harness)
        driver = (ROOT / "build/native-ui-driver.py").read_text()
        self.assertIn("screencapture", driver)
        self.assertIn("native-ui.png", driver)
        self.assertIn("window_visible", driver)
        interaction = (ROOT / "build/native-ui-interactions.js").read_text()
        self.assertIn("button.click()", interaction)
        self.assertIn("update_status", interaction)

    def test_no_smoke_claim_in_artifact_summary(self):
        workflow = (ROOT / ".github/workflows/macos.yml").read_text()
        self.assertIn("ad-hoc signed and not notarized", workflow)
        self.assertIn("chat-hub-debug-ui-evidence", workflow)


if __name__ == "__main__":
    unittest.main()
