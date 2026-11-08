import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]

def load(name):
    spec = importlib.util.spec_from_file_location(name, ROOT / "build" / (name + ".py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module

preview = load("merge_preview")
drafts = load("release_drafts")
SHA = "e8120a4e826704fcf81266cdccfde5a2e17bda90"
TREE = "7e7c3ce5b78dfa22e78e3a53eb12f1ac3da9a520"

class PreviewTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / "src-tauri").mkdir()
        (self.root / "src-tauri/tauri.conf.json").write_text('{"version":"0.2.5"}')
        self.artifacts = self.root / "artifacts"
        self.artifacts.mkdir()
        names = ["Chat_Hub_0.2.5_macos-arm64" + ext for ext in (".zip", ".zip.sha256", ".dmg")]
        for name in names:
            (self.artifacts / name).write_bytes(name.encode())
        self.source = {"accepted_commit": SHA, "accepted_tree": TREE, "version": "0.2.5",
                       "source_run_id": 123, "acceptance": "trusted-main-build",
                       "repository": "iarnvidr/chat-hub", "source_artifact_digest": "sha256:accepted",
                       "files": {n: preview.digest(self.artifacts / n) for n in names}}
        self.save_source()
        self.git = patch.object(preview.subprocess, "check_output", return_value=TREE + "\n")
        self.git.start()
        self.addCleanup(self.git.stop)

    def save_source(self):
        (self.artifacts / "release-input.json").write_text(json.dumps(self.source))

    def prepared(self):
        return preview.prepare(self.root, SHA, 123)

    def release(self, info):
        names = set(info["files"]) | {"SHA256SUMS", "build-info.json"}
        return {"tag_name": "merge-" + SHA, "draft": False, "prerelease": True,
                "assets": [{"name": n, "size": (self.artifacts / n).stat().st_size,
                            "digest": "sha256:" + preview.digest(self.artifacts / n)} for n in names]}

    def test_unique_names_and_unchanged_native_bytes(self):
        info = self.prepared()
        self.assertEqual(info["app_version"], "0.2.5")
        self.assertEqual(info["display_version"], "0.2.5-preview.e8120a4e8267")
        self.assertEqual(info["channel"], "merge-preview")
        for name, digest in info["files"].items():
            ext = Path(name).suffix
            self.assertEqual(digest, self.source["files"]["Chat_Hub_0.2.5_macos-arm64" + ext])
            self.assertIn(name, (self.artifacts / "SHA256SUMS").read_text())
        self.assertNotEqual(preview.preview_version("0.2.5", SHA), preview.preview_version("0.2.5", "f" * 40))

    def test_wrong_source_binding_is_rejected(self):
        for key, value in [("accepted_commit", "f" * 40), ("accepted_tree", "f" * 40),
                           ("source_run_id", 124), ("acceptance", "pull-request")]:
            original = self.source[key]
            self.source[key] = value
            self.save_source()
            with self.assertRaises(ValueError):
                self.prepared()
            self.source[key] = original

    def test_tampered_artifact_is_rejected(self):
        (self.artifacts / "Chat_Hub_0.2.5_macos-arm64.zip").write_bytes(b"tampered")
        with self.assertRaises(ValueError):
            self.prepared()

    def test_idempotent_rerun_and_asset_verification(self):
        info = self.prepared()
        release = self.release(info)
        self.prepared()
        preview.verify_existing(self.root, release, SHA)
        for field, value in [("digest", None), ("digest", "sha256:wrong"), ("size", 1)]:
            changed = json.loads(json.dumps(release))
            changed["assets"][0][field] = value
            with self.assertRaises(ValueError):
                preview.verify_existing(self.root, changed, SHA)

    def test_published_stable_or_draft_is_not_overwritten(self):
        release = self.release(self.prepared())
        for field, value in [("draft", True), ("prerelease", False), ("tag_name", "v0.2.5")]:
            changed = dict(release, **{field: value})
            with self.assertRaises(ValueError):
                preview.verify_existing(self.root, changed, SHA)

    def test_invalid_identity(self):
        for version, sha in [("../version", SHA), ("0.2.5", "short"), ("0.2.5", SHA.upper())]:
            with self.assertRaises(ValueError):
                preview.preview_version(version, sha)

class DraftTests(unittest.TestCase):
    def test_only_explicit_empty_obsolete_draft_can_be_deleted(self):
        draft = {"id": 12, "tag_name": "v0.2.0", "draft": True, "assets": []}
        latest = {"tag_name": "v0.2.5", "draft": False, "prerelease": False}
        self.assertEqual(drafts.validate_cleanup(draft, latest, 12, "v0.2.0"), 12)
        for field, value in [("draft", False), ("assets", [{}]), ("id", 13), ("tag_name", "v0.2.6")]:
            with self.assertRaises(ValueError):
                drafts.validate_cleanup(dict(draft, **{field: value}), latest, 12, "v0.2.0")
        for tag in ("v0.1.0", "v0.2.0", "v0.3.0-rc.1"):
            with self.assertRaises(ValueError):
                drafts.validate_cleanup(draft, dict(latest, tag_name=tag), 12, "v0.2.0")

if __name__ == "__main__":
    unittest.main()
