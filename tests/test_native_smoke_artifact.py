"""Cargo metadata artifact resolution regression tests."""
import importlib.util
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("native_smoke_artifact", ROOT / "build/native_smoke_artifact.py")
resolver = importlib.util.module_from_spec(spec)
spec.loader.exec_module(resolver)


class NativeSmokeArtifact(unittest.TestCase):
    def test_shared_cargo_target_directory_and_declared_binary_name(self):
        metadata = {
            "target_directory": "/runner/work/repo/target",
            "packages": [{"name": "chat-hub-desktop", "targets": [
                {"name": "chat-hub-desktop", "kind": ["bin"]},
                {"name": "build-script-build", "kind": ["custom-build"]},
            ]}],
        }
        self.assertEqual(
            resolver.artifact_path(metadata),
            "/runner/work/repo/target/aarch64-apple-darwin/debug/chat-hub-desktop",
        )

    def test_workflows_use_metadata_artifact_and_isolated_bundle(self):
        action = (ROOT / "ci/actions/macos-native-build/action.yml").read_text()
        prepare = action.index("Prepare isolated debug bundle")
        build = action.rindex("cargo build --locked", 0, prepare)
        self.assertLess(build, prepare)
        self.assertIn("native_smoke_artifact.py", action)
        self.assertIn('"$RUNNER_TEMP/chat-hub-debug-smoke/Chat Hub.app"', action)
        self.assertLess(action.index("Compile native-smoke feature"), prepare)
        self.assertIn("cmp -s", action)
        self.assertIn("codesign --verify --deep --strict", action)
        self.assertIn('"$RUNNER_TEMP/chat-hub/Chat Hub.app"', action)
        self.assertIn('cp -R "$RUNNER_TEMP/chat-hub/Chat Hub.app" "$root/chat-hub-macos-arm64/"', action)
        macos = (ROOT / ".github/workflows/macos.yml").read_text()
        self.assertIn('name: Upload downloadable macOS build', macos)
        release = (ROOT / ".github/workflows/release.yml").read_text()
        self.assertIn('release_artifact.py retrieve', release)
        self.assertNotIn('cargo build', release)
        self.assertIn("release-assets/Chat-Hub.app.tar.gz", release)


if __name__ == "__main__":
    unittest.main()
