"""Static and source-bundle contracts for protected release tooling."""
import hashlib
import json
import os
import re
import subprocess
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

    def test_release_review_transport_binds_the_tag_in_its_own_step(self):
        step = self.workflow.split("- name: Stage protected release review artifacts\n", 1)[1].split("      - id:", 1)[0]
        self.assertIn("INPUT_TAG: ${{ inputs.tag }}", step.split("run: |", 1)[0])
        script = re.search(r"<<'PY'\n(.*?)\n          PY", step, re.S).group(1)
        script = "\n".join(line[10:] for line in script.splitlines())
        with tempfile.TemporaryDirectory() as tmp:
            output = Path(tmp) / "transport.json"
            env = {**os.environ, "GITHUB_REPOSITORY": "iarnvidr/chat-hub", "GITHUB_RUN_ID": "123", "GITHUB_RUN_ATTEMPT": "1", "GITHUB_SHA": "a" * 40, "INPUT_TAG": "v0.2.1"}
            subprocess.run([sys.executable, "-c", script, str(output)], env=env, check=True, capture_output=True)
            self.assertEqual(json.loads(output.read_text()), {"repository": "iarnvidr/chat-hub", "run_id": "123", "run_attempt": "1", "sha": "a" * 40, "tag": "v0.2.1"})

    def test_signing_and_dmg_evidence_are_required(self):
        self.assertIn("TAURI_SIGNING_PRIVATE_KEY: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}", self.workflow)
        self.assertIn("TAURI_SIGNING_PRIVATE_KEY_PASSWORD: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY_PASSWORD }}", self.workflow)
        self.assertIn("signer sign", self.workflow)
        self.assertIn("minisign -Vm", self.workflow)
        self.assertIn("latest.json", self.workflow)
        self.assertIn("updater_release.py generate", self.workflow)
        self.assertIn("updater_release.py validate", self.workflow)
        self.assertIn('cp artifacts/Chat_Hub_*.dmg', self.workflow)
        native = (ROOT / 'ci/actions/macos-native-build/action.yml').read_text()
        self.assertIn('Build and mount-verify the DMG', native)
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
        native = (ROOT / "ci/actions/macos-native-build/action.yml").read_text()
        compile_step = native.index("Compile native-smoke feature (required)")
        build_step = native.index("Build debug binary for native UI interactions")
        self.assertLess(compile_step, build_step)
        self.assertNotIn("continue-on-error", native[compile_step:native.index("Prepare isolated debug bundle")])
        self.assertGreaterEqual(native.count("continue-on-error: true"), 2)
        release = (ROOT / ".github/workflows/release.yml").read_text()
        self.assertIn("release_artifact.py retrieve", release)
        self.assertNotIn('cargo check', release)
        self.assertNotIn('cargo build', release)
        self.assertIn("Build real Tauri app, ad-hoc signed", native)
        self.assertIn("Build and mount-verify the DMG", native)

    def test_dmg_packager_reports_missing_bundle_path(self):
        self.assertIn('app_input="$1"', self.dmg)
        self.assertIn('DMG packaging app bundle does not exist: $app_input', self.dmg)
        self.assertIn('DMG packaging source bundle does not exist: $sources_input', self.dmg)

    def test_source_manifest_digest_and_no_overwrite(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "root"
            root.mkdir()
            for name in ("LICENSE", "NOTICE", "README.md", "Cargo.toml", "Cargo.lock", "rust-toolchain.toml"):
                (root / name).write_text(name)
            for directory, files in {
                "src": ["main.rs"], "src-tauri": ["Cargo.toml", "tauri.conf.json", "Info.plist", "src/main.rs", "icons/icon.png"],
                "crates/chat-hub-model": ["Cargo.toml", "src/lib.rs"],
                "crates/chat-hub-formats": ["Cargo.toml", "src/lib.rs"],
                "desktop-ui": ["index.html"], "build": ["build-native-macos.sh"],
                "ci/actions/macos-native-build": ["action.yml"],
                "ci/actions/gha-artifact-upload": ["action.yml", "main.go"],
                ".github/workflows": ["macos.yml", "accept-native.yml"],
            }.items():
                for name in files:
                    path = root / directory / name
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_text(name)
            for directory in ("target", ".tools", ".git"):
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
                "src/main.rs", "src-tauri/Cargo.toml",
                "crates/chat-hub-model/Cargo.toml", "crates/chat-hub-model/src/lib.rs",
                "crates/chat-hub-formats/Cargo.toml", "crates/chat-hub-formats/src/lib.rs",
                "src-tauri/tauri.conf.json", "src-tauri/Info.plist",
                "src-tauri/src/main.rs", "src-tauri/icons/icon.png",
                "desktop-ui/index.html", "build/build-native-macos.sh",
                "ci/actions/macos-native-build/action.yml",
                "ci/actions/gha-artifact-upload/action.yml",
                "ci/actions/gha-artifact-upload/main.go",
                ".github/workflows/macos.yml", ".github/workflows/accept-native.yml",
                "Cargo.toml", "Cargo.lock", "rust-toolchain.toml",
                "LICENSE", "NOTICE",
            }.issubset(paths))
            self.assertFalse(any(path.startswith((".git/", "target/", ".tools/")) for path in paths))
            self.assertNotIn("src/escape", paths)
            for item in manifest["files"]:
                self.assertEqual(hashlib.sha256((out / item["path"]).read_bytes()).hexdigest(), item["sha256"])
            with self.assertRaises(FileExistsError):
                release_sources.prepare(out, root)


if __name__ == "__main__":
    unittest.main()
