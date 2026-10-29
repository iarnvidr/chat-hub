"""Production build-chain contracts: real bundle, ad-hoc mode, evidence, gated publish."""
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


class ProductionContract(unittest.TestCase):
    def test_ad_hoc_real_tauri_build(self):
        build = (ROOT / "build/build-native-macos.sh").read_text()
        config = (ROOT / "src-tauri/tauri.conf.json").read_text()
        self.assertIn("npm run tauri -- build", build)
        self.assertIn("--bundles app", build)
        self.assertIn("codesign --force --deep --sign -", build)
        self.assertIn("codesign --verify --deep --strict", build)
        self.assertIn('"productName": "Chat Hub"', config)
        self.assertIn('"infoPlist": "Info.plist"', config)
        self.assertNotIn("notarized; ticket", build)

    def test_artifact_must_contain_real_outputs(self):
        workflow = (ROOT / ".github/workflows/macos.yml").read_text()
        self.assertIn("runs-on: macos-15", workflow)
        self.assertIn("name: chat-hub-macos-arm64", workflow)
        self.assertIn("if-no-files-found: error", workflow)
        for entry in ("Chat Hub.app", "Chat_Hub_*.dmg", "verify-macos.txt"):
            self.assertIn(entry, workflow)

    def test_protected_publish_stays_separate(self):
        workflow = (ROOT / ".github/workflows/release.yml").read_text()
        self.assertIn("default: false", workflow)
        self.assertIn("if: github.ref == 'refs/heads/main' && inputs.publish", workflow)
        self.assertIn("name: release-review-${{ inputs.tag }}", workflow)
        self.assertNotIn("APPLE_DEVELOPER_ID", workflow)


if __name__ == "__main__":
    unittest.main()
