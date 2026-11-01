import io
import json
import pathlib
import sys
import tarfile
import tempfile
import unittest
from unittest import mock


ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "ci/scripts"))
import compiled_cache


class CompiledCacheContractTests(unittest.TestCase):
    def setUp(self):
        self.contract = {
            "compatibility": {
                "schema": 1,
                "sccache_version": "0.19.0",
                "platform_id": "macos-15-arm64",
                "host_system": "darwin",
                "host_machine": "arm64",
                "target": "aarch64-apple-darwin",
                "sdk": "15.4",
                "xcode_sha256": "abc",
            },
            "provenance": {"cargo_inputs": {"Cargo.lock": "one"}, "rustc_sha256": "one"},
        }

    def test_source_and_dependency_provenance_does_not_partition_shared_cache(self):
        changed = json.loads(json.dumps(self.contract))
        changed["provenance"]["cargo_inputs"]["Cargo.lock"] = "two"
        changed["provenance"]["rustc_sha256"] = "two"
        self.assertEqual(compiled_cache.contract_key(self.contract), compiled_cache.contract_key(changed))

    def test_compiler_target_and_sdk_changes_partition_cache(self):
        for key, value in (("sccache_version", "0.20.0"), ("target", "x86_64-apple-darwin"),
                           ("sdk", "15.5"), ("platform_id", "macos-26-arm64")):
            changed = json.loads(json.dumps(self.contract))
            changed["compatibility"][key] = value
            self.assertNotEqual(compiled_cache.contract_key(self.contract), compiled_cache.contract_key(changed))

    def test_sccache_rust_hit_counter_is_read_from_pinned_json_schema(self):
        stats = {"cache_hits": {"counts": {"Rust": 3, "C/C++": 99}}}
        self.assertEqual(compiled_cache.rust_hits_from_stats(stats), 3)
        with self.assertRaises(compiled_cache.CacheError):
            compiled_cache.rust_hits_from_stats({"cache_hits": {"counts": {"Rust": "three"}}})


class CompiledCacheArchiveTests(unittest.TestCase):
    def setUp(self):
        self.contract = {"compatibility": {"schema": 1, "target": "aarch64-apple-darwin"},
                         "provenance": {"Cargo.lock": "not-a-cache-key"}}
        self.key = compiled_cache.contract_key(self.contract)

    def _archive(self, path, entries, *, repository="example/chat-hub", contract=None, key=None):
        manifest = {
            "schema": 1,
            "repository": repository,
            "compatibility": (contract or self.contract)["compatibility"],
            "provenance": (contract or self.contract)["provenance"],
            "contract_key": key or self.key,
            "writer_ref": "refs/heads/main",
        }
        with tarfile.open(path, "w:gz") as archive:
            encoded = json.dumps(manifest, sort_keys=True).encode()
            info = tarfile.TarInfo("manifest.json")
            info.size = len(encoded)
            archive.addfile(info, io.BytesIO(encoded))
            for name, content in entries:
                info = tarfile.TarInfo(name)
                if content is None:
                    info.type = tarfile.SYMTYPE
                    info.linkname = "../../outside"
                    archive.addfile(info)
                else:
                    info.size = len(content)
                    archive.addfile(info, io.BytesIO(content))

    def test_manifest_first_safe_archive_restores_cache_objects(self):
        with tempfile.TemporaryDirectory() as temp:
            archive = pathlib.Path(temp) / "cache.tar.gz"
            dest = pathlib.Path(temp) / "cache"
            self._archive(archive, [("cache/ab/object", b"compiled-output")])
            size = compiled_cache.restore_archive(archive, dest, self.contract, self.key, "example/chat-hub")
            self.assertEqual(size, len(b"compiled-output"))
            self.assertEqual((dest / "ab/object").read_bytes(), b"compiled-output")

    def test_traversal_symlink_duplicate_and_contract_mismatch_are_rejected(self):
        cases = [
            [("cache/../outside", b"bad")],
            [("cache/link", None)],
            [("cache/item", b"one"), ("cache/item", b"two")],
        ]
        for entries in cases:
            with self.subTest(entries=entries), tempfile.TemporaryDirectory() as temp:
                archive = pathlib.Path(temp) / "cache.tar.gz"
                self._archive(archive, entries)
                with self.assertRaises(compiled_cache.CacheError):
                    compiled_cache.restore_archive(archive, pathlib.Path(temp) / "cache",
                                                   self.contract, self.key, "example/chat-hub")
        for repository, writer_ref in (("other/repo", "refs/heads/main"),
                                       ("example/chat-hub", "refs/pull/3/merge")):
            with self.subTest(repository=repository, writer_ref=writer_ref), tempfile.TemporaryDirectory() as temp:
                archive = pathlib.Path(temp) / "cache.tar.gz"
                self._archive(archive, [("cache/item", b"bad")], repository=repository)
                if writer_ref != "refs/heads/main":
                    # Rewrite the fixture manifest through a dedicated archive.
                    manifest = {"schema": 1, "repository": repository,
                                "compatibility": self.contract["compatibility"],
                                "provenance": self.contract["provenance"],
                                "contract_key": self.key, "writer_ref": writer_ref}
                    with tarfile.open(archive, "w:gz") as bundle:
                        encoded = json.dumps(manifest).encode()
                        info = tarfile.TarInfo("manifest.json"); info.size = len(encoded)
                        bundle.addfile(info, io.BytesIO(encoded))
                        info = tarfile.TarInfo("cache/item"); info.size = 3
                        bundle.addfile(info, io.BytesIO(b"bad"))
                with self.assertRaises(compiled_cache.CacheError):
                    compiled_cache.restore_archive(archive, pathlib.Path(temp) / "cache",
                                                   self.contract, self.key, "example/chat-hub")


class TrustedWriterTests(unittest.TestCase):
    def test_only_protected_main_push_or_dispatch_may_publish(self):
        with mock.patch.dict("os.environ", {"GITHUB_REF": "refs/heads/main", "GITHUB_EVENT_NAME": "push",
                                             "GITHUB_RUN_ID": "123"}, clear=True):
            self.assertTrue(compiled_cache.trusted_main_writer())
        for event, ref in (("pull_request", "refs/pull/2/merge"), ("push", "refs/heads/feature"),
                           ("workflow_dispatch", "refs/heads/feature")):
            with self.subTest(event=event, ref=ref), mock.patch.dict(
                    "os.environ", {"GITHUB_REF": ref, "GITHUB_EVENT_NAME": event, "GITHUB_RUN_ID": "123"}, clear=True):
                self.assertFalse(compiled_cache.trusted_main_writer())

    def test_workflows_make_cache_readable_to_prs_but_writable_only_from_main_jobs(self):
        macos = (ROOT / ".github/workflows/macos.yml").read_text()
        pr_job = macos.split("  package-pr:", 1)[1].split("  package-main:", 1)[0]
        main_job = macos.split("  package-main:", 1)[1].split("  artifact-transfer:", 1)[0]
        self.assertIn("packages: read", pr_job)
        self.assertNotIn("packages: write", pr_job)
        self.assertIn("github.ref == 'refs/heads/main'", main_job)
        self.assertIn("packages: write", main_job)
        self.assertIn("mode: publish", main_job)

        accept = (ROOT / ".github/workflows/accept-native.yml").read_text()
        seed_job = accept.split("  linux-cache-seed:", 1)[1].split("  artifact-transfer:", 1)[0]
        self.assertIn("if: github.ref == 'refs/heads/main'", seed_job)
        self.assertIn("packages: write", seed_job)
        self.assertIn("seed-linux-cache", seed_job)


if __name__ == "__main__":
    unittest.main()
