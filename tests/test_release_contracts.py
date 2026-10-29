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
        self.assertIn("minisign -Vm", self.workflow)
        self.assertIn("latest.json", self.workflow)
        self.assertIn("updater_release.py generate", self.workflow)
        self.assertIn("updater_release.py validate", self.workflow)
        self.assertIn("Create and mount-verify DMG", self.workflow)
        self.assertIn("hdiutil verify", self.dmg)
        self.assertIn("hdiutil attach", self.dmg)
        self.assertIn("codesign --verify", self.dmg)
        self.assertIn("Corresponding Source", self.dmg)
        self.assertIn("NOTICE", self.dmg)
        native = (ROOT / "build/build-native-macos.sh").read_text()
        self.assertLess(native.index('cp "$file" "$resources/$file"'), native.index('codesign --force'))
        self.assertIn('for file in NOTICE LICENSE', native)
        self.assertIn('macOS app packaging copy missing:', native)

    def test_native_smoke_compilation_is_blocking_but_runtime_diagnostics_are_nonblocking(self):
        workflow = (ROOT / ".github/workflows/macos.yml").read_text()
        compile_step = workflow.index("Compile native-smoke feature (required)")
        build_step = workflow.index("Build debug binary for native UI interactions")
        self.assertLess(compile_step, build_step)
        self.assertNotIn("continue-on-error", workflow[compile_step:workflow.index("Prepare isolated debug bundle")])
        self.assertGreaterEqual(workflow.count("continue-on-error: true"), 2)
        release = (ROOT / ".github/workflows/release.yml").read_text()
        self.assertIn("Required native-smoke feature compilation", release)
        self.assertIn("Build real Tauri app, ad-hoc signed", workflow)
        self.assertIn("Build and mount-verify the DMG", workflow)

    def test_dmg_packager_reports_missing_bundle_path(self):
        self.assertIn('app_input="$1"', self.dmg)
        self.assertIn('DMG packaging app bundle does not exist: $app_input', self.dmg)
        self.assertIn('DMG packaging source bundle does not exist: $sources_input', self.dmg)

    def test_source_manifest_digest_and_no_overwrite(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "root"
            root.mkdir()
            for name in ("LICENSE", "NOTICE", "README.md", "Cargo.toml", "Cargo.lock", "package.json", "rust-toolchain.toml"):
                (root / name).write_text(name)
            for directory, files in {
                "src": ["main.rs"], "src-tauri": ["Cargo.toml", "Cargo.lock", "tauri.conf.json", "Info.plist", "src/main.rs", "icons/icon.png"],
                "desktop-ui": ["index.html"], "build": ["build-native-macos.sh"],
            }.items():
                for name in files:
                    path = root / directory / name
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_text(name)
            for directory in ("target", "node_modules", ".tools", ".git"):
                path = root / directory
                path.mkdir()
                (path / "secret.tmp").write_text("exclude")
            (root / "src" / "escape").symlink_to(root / "NOTICE")
            out = Path(tmp) / "source"
            release_sources.prepare(out, root)
            manifest = json.loads((out / "manifest.json").read_text())
            self.assertEqual(manifest["format"], release_sources.FORMAT)
            paths = {item["path"] for item in manifest["files"]}
            self.assertTrue({
                "src/main.rs", "src-tauri/Cargo.toml", "src-tauri/Cargo.lock",
                "src-tauri/tauri.conf.json", "src-tauri/Info.plist",
                "src-tauri/src/main.rs", "src-tauri/icons/icon.png",
                "desktop-ui/index.html", "build/build-native-macos.sh",
                "Cargo.toml", "Cargo.lock", "package.json", "rust-toolchain.toml",
                "LICENSE", "NOTICE",
            }.issubset(paths))
            self.assertFalse(any(path.startswith((".git/", "target/", "node_modules/", ".tools/")) for path in paths))
            self.assertNotIn("src/escape", paths)
            for item in manifest["files"]:
                self.assertEqual(hashlib.sha256((out / item["path"]).read_bytes()).hexdigest(), item["sha256"])
            with self.assertRaises(FileExistsError):
                release_sources.prepare(out, root)


if __name__ == "__main__":
    unittest.main()
