"""Static and source-bundle contracts for protected release tooling."""
import hashlib
import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "build"))
import release_sources


class ReleaseContracts(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.workflow = (ROOT / ".github/workflows/release.yml").read_text()
        cls.dmg = (ROOT / "build/package-dmg.sh").read_text()

    def test_manual_fail_closed_reviewed_publish(self):
        self.assertIn("workflow_dispatch:", self.workflow)
        self.assertIn("default: false", self.workflow)
        self.assertIn("runs-on: macos-15", self.workflow)
        self.assertIn("test \"$INPUT_TAG\" = \"v$INPUT_VERSION\"", self.workflow)
        self.assertIn('refs/tags/$INPUT_TAG^{commit}', self.workflow)
        self.assertIn("refs/remotes/origin/main^{commit}", self.workflow)
        self.assertIn("if: github.ref == 'refs/heads/main' && inputs.publish", self.workflow)
        self.assertIn("contents: write", self.workflow)
        self.assertIn("needs: prepare", self.workflow)
        self.assertIn("name: chat-hub-release", self.workflow)
        self.assertNotIn("contents: write", self.workflow.split("publish:", 1)[0])

    def test_signing_and_dmg_evidence_are_required(self):
        self.assertIn("TAURI_SIGNING_PRIVATE_KEY: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}", self.workflow)
        self.assertIn("TAURI_SIGNING_PRIVATE_KEY_PASSWORD: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY_PASSWORD }}", self.workflow)
        self.assertIn("signer sign", self.workflow)
        self.assertIn("Create and mount-verify DMG", self.workflow)
        self.assertIn("hdiutil verify", self.dmg)
        self.assertIn("hdiutil attach", self.dmg)
        self.assertIn("codesign --verify", self.dmg)
        self.assertIn("Corresponding Source", self.dmg)
        self.assertIn("NOTICE", self.dmg)

    def test_source_manifest_digest_and_no_overwrite(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "root"
            root.mkdir()
            for name in ("LICENSE", "NOTICE", "README.md", "Cargo.toml", "Cargo.lock"):
                (root / name).write_text(name)
            (root / "src-tauri").mkdir()
            (root / "src-tauri/Cargo.toml").write_text("tauri")
            (root / "src-tauri/tauri.conf.json").write_text("{}")
            out = Path(tmp) / "source"
            release_sources.prepare(out, root)
            manifest = json.loads((out / "manifest.json").read_text())
            self.assertEqual(manifest["format"], release_sources.FORMAT)
            self.assertTrue(any(item["path"] == "NOTICE" for item in manifest["files"]))
            for item in manifest["files"]:
                self.assertEqual(hashlib.sha256((out / item["path"]).read_bytes()).hexdigest(), item["sha256"])
            with self.assertRaises(FileExistsError):
                release_sources.prepare(out, root)


if __name__ == "__main__":
    unittest.main()
