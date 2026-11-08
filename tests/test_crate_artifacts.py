"""Per-crate compiler objects must remain separate and bound to the cache run."""
from __future__ import annotations

import copy
import io
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "ci/scripts"))
import crate_artifacts as crates
import oci_cache as cache

REPO = "iarnvidr/chat-hub"
COMPAT = {"format": cache.CACHE_FORMAT, "platform": "linux-x86_64",
          "rust_image": "rust:1.99.1-bookworm@sha256:" + "a" * 64,
          "runners": {"ci/dagger/check/run-rust.sh": "b" * 64}}
PRODUCER = {"repository": REPO, "run_id": 123, "run_attempt": 1,
            "event": "pull_request", "ref": "refs/pull/39/merge",
            "tested_sha": "a" * 40, "tested_tree": "b" * 40,
            "head_sha": "c" * 40, "base_sha": "d" * 40,
            "head_repository": REPO, "pr_number": 39}
PARENT = {"schema": 1, "kind": "compiler-cache", "compatibility": COMPAT,
          "producer": PRODUCER, "archive": {"name": cache.ARCHIVE,
          "sha256": "e" * 64, "bytes": 100, "expanded_bytes": 300, "entries": 5}}


class CrateArtifactTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory(prefix="chat-hub-crate-objects-")
        self.addCleanup(temp.cleanup)
        self.path = Path(temp.name)
        self.source = self.path / "cache"
        self.deps = self.source / "target" / "abi-123" / "debug" / "deps"
        self.deps.mkdir(parents=True)
        self.meta = self.path / "cache-manifest.json"
        cache.write_json(self.meta, PARENT)

    def write_output(self, crate: str, value: bytes = b"compiled"):
        stem = crate.replace("-", "_")
        output = self.deps / f"lib{stem}-12345678.rlib"
        output.write_bytes(value)
        return output

    def test_crate_selection_never_captures_similarly_prefixed_dependency(self):
        self.write_output("chat-hub", b"root")
        self.write_output("chat-hub-model", b"model")
        self.write_output("chat-hub-store", b"store")
        names = [path.name for path in crates.selected(self.source, "chat-hub")]
        self.assertEqual(names, ["libchat_hub-12345678.rlib"])
        self.assertEqual([path.name for path in crates.selected(self.source, "chat-hub-model")],
                         ["libchat_hub_model-12345678.rlib"])

    @unittest.skipUnless(shutil.which("zstd"), "zstd is unavailable")
    def test_each_workspace_crate_has_real_separate_verified_compiler_objects(self):
        names = crates.crates(ROOT)
        self.assertEqual(len(names), 9)
        for crate in names:
            self.write_output(crate, crate.encode())
        output = self.path / "bundles"
        with mock.patch.dict(os.environ, {"GITHUB_REPOSITORY": REPO}), \
             mock.patch.object(cache, "compatibility", return_value=COMPAT):
            self.assertEqual(crates.pack(self.source, self.meta, output, ROOT), names)
        for crate in names:
            bundle = output / crate
            record = crates.validate_bundle(bundle, crate, PARENT, REPO)
            self.assertEqual(record["archive"]["files"], 1)
            decompressed = subprocess.check_output(["zstd", "-dc", str(bundle / crates.ARCHIVE)])
            with tarfile.open(fileobj=io.BytesIO(decompressed), mode="r:") as archive:
                members = archive.getmembers()
                self.assertEqual(len(members), 1)
                self.assertEqual(archive.extractfile(members[0]).read(), crate.encode())

        tampered = output / names[0] / crates.ARCHIVE
        tampered.write_bytes(b"tampered")
        with self.assertRaisesRegex(cache.CacheError, "digest or size"):
            crates.validate_bundle(output / names[0], names[0], PARENT, REPO)

    def test_no_empty_or_cross_run_crate_packages(self):
        self.write_output("chat-hub-model")
        with mock.patch.dict(os.environ, {"GITHUB_REPOSITORY": REPO}), \
             mock.patch.object(cache, "compatibility", return_value=COMPAT):
            with self.assertRaisesRegex(cache.CacheError, "no compiled outputs"):
                crates.pack(self.source, self.meta, self.path / "incomplete", ROOT)

    def test_workflow_requires_nine_individual_uploads_and_pr_app_bundle(self):
        check = (ROOT / ".github/workflows/check.yml").read_text()
        for crate in crates.crates(ROOT):
            self.assertIn(f"chat-hub-crate-{crate}-${{{{ github.run_id }}}}", check)
        self.assertIn("crate_artifacts.py --source", check)
        self.assertIn("steps.crate-pack.outcome == 'success'", check)
        mac = (ROOT / ".github/workflows/macos.yml").read_text()
        pr = mac.split("  package-pr:", 1)[1].split("  package-main:", 1)[0]
        self.assertIn("Upload installable Apple Silicon PR preview", pr)
        self.assertIn("Chat_Hub_${version}_macos-arm64.dmg", pr)
        self.assertIn("actions/upload-artifact@v4", pr)
        self.assertNotIn("packages: write", pr)


if __name__ == "__main__":
    unittest.main()
