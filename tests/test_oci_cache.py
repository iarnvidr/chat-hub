"""Cargo snapshot data is reusable optimization, never a green-check receipt."""
from contextlib import contextmanager
import copy
import hashlib
import io
import json
import os
from pathlib import Path
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import time
import unittest
from unittest import mock
import zipfile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "ci/scripts"))
import oci_cache as cache


REPOSITORY = "iarnvidr/chat-hub"
IMAGE = "rust:1.99.1-bookworm@sha256:" + "1" * 64
CONTRACT = {"format": cache.CACHE_FORMAT, "platform": "linux-x86_64", "rust_image": IMAGE,
            "runners": {"ci/dagger/check/run-rust.sh": "2" * 64}}
PRODUCER = {"repository": REPOSITORY, "run_id": 123, "run_attempt": 2,
            "event": "pull_request", "ref": "refs/pull/42/merge", "pr_number": 42,
            "tested_sha": "a" * 40, "tested_tree": "b" * 40,
            "head_sha": "c" * 40, "base_sha": "d" * 40, "head_repository": REPOSITORY}


def manifest(data=b"opaque compiled output"):
    return {"schema": 1, "kind": "compiler-cache", "compatibility": copy.deepcopy(CONTRACT),
            "producer": copy.deepcopy(PRODUCER), "archive": {"name": cache.ARCHIVE,
            "sha256": hashlib.sha256(data).hexdigest(), "bytes": len(data),
            "expanded_bytes": 300, "entries": 5}}


def workflow_run():
    return {"id": 123, "run_attempt": 2, "name": "Checks", "path": cache.WORKFLOW,
            "status": "completed", "event": "pull_request", "conclusion": "failure",
            "repository": {"full_name": REPOSITORY}, "head_repository": {"full_name": REPOSITORY},
            "head_sha": "c" * 40, "head_branch": "feature/crates",
            "pull_requests": [{"number": 42, "head": {"sha": "c" * 40}, "base": {"sha": "d" * 40}}]}


def publication():
    return {"repository": REPOSITORY, "run_id": 123, "run_attempt": 2,
            "event": "pull_request", "head_sha": "c" * 40, "head_branch": "feature/crates",
            "workflow_path": cache.WORKFLOW, "conclusion": "failure", "pr_number": 42}


def evidence():
    return {"schema": 1, "repository": REPOSITORY, "workflow_path": cache.WORKFLOW,
            "run_id": 123, "run_attempt": 2, "event": "pull_request", "head_sha": "c" * 40,
            "base_sha": "d" * 40, "pr_number": 42, "tested_tree": "b" * 40,
            "workflow_sha": "a" * 40, "helper_sha": "e" * 40}


def fixture_source(root):
    for name in ("cargo/registry", "cargo/git", "target/debug", "worktree", "source-state", "check-logs"):
        (root / name).mkdir(parents=True)
    files = {"cargo/registry/crate": b"download", "target/debug/bin": b"compiled binary",
             "worktree/Cargo.toml": b"[workspace]\n", "source-state/last-real.json": b"{}"}
    stamp = 1_700_000_000_123_456_789
    for name, value in files.items():
        path = root / name
        path.write_bytes(value)
        path.chmod(0o755 if name.endswith("/bin") else 0o644)
        os.utime(path, ns=(stamp, stamp))
    (root / "check-logs/failed-crate.log").write_text("failed check output, not a reusable object")
    (root / "target/debug/current").symlink_to("bin")
    return stamp


def malicious_tar(entries):
    """Entries are (path, type, data/link); directories need no payload."""
    stream = io.BytesIO()
    with tarfile.open(fileobj=stream, mode="w", format=tarfile.PAX_FORMAT) as archive:
        for path, kind, value in entries:
            member = tarfile.TarInfo(path)
            member.mtime = 1_700_000_000
            member.mode = 0o644
            member.type = kind
            if kind == tarfile.REGTYPE:
                member.size = len(value)
                archive.addfile(member, io.BytesIO(value))
            else:
                if kind in (tarfile.SYMTYPE, tarfile.LNKTYPE):
                    member.linkname = value
                archive.addfile(member)
    stream.seek(0)
    return stream


class ArchiveTests(unittest.TestCase):
    def test_pax_roundtrip_preserves_nanosecond_mtimes_modes_and_internal_symlinks(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source, output = root / "source", root / "output"
            source.mkdir(); output.mkdir()
            stamp = fixture_source(source)
            tar = io.BytesIO()
            before = cache.write_tar(source, tar)
            tar.seek(0)
            self.assertEqual(cache.unpack_tar(tar, output), before)
            for name in ("cargo/registry/crate", "target/debug/bin", "worktree/Cargo.toml"):
                self.assertEqual((output / name).stat().st_mtime_ns, stamp)
                self.assertEqual((output / name).read_bytes(), (source / name).read_bytes())
            self.assertEqual(stat.S_IMODE((output / "target/debug/bin").stat().st_mode), 0o755)
            self.assertEqual(os.readlink(output / "target/debug/current"), "bin")
            self.assertFalse((output / "check-logs").exists())

    @unittest.skipUnless(shutil.which("zstd"), "zstd unavailable")
    def test_pack_uses_real_zstd_and_an_independent_manifest(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source, packed, output = root / "source", root / "bundle", root / "unpacked"
            source.mkdir(); output.mkdir()
            stamp = fixture_source(source)
            with mock.patch.object(cache, "compatibility", return_value=CONTRACT), \
                    mock.patch.object(cache, "provenance", return_value=PRODUCER):
                cache.pack(source, packed, root)
            meta = cache.validate_bundle(packed, REPOSITORY, CONTRACT)
            data = subprocess.run(["zstd", "-q", "-dc", str(packed / cache.ARCHIVE)],
                                  check=True, capture_output=True).stdout
            actual = cache.unpack_tar(io.BytesIO(data), output)
            self.assertEqual(actual["entries"], meta["archive"]["entries"])
            self.assertEqual((output / "worktree/Cargo.toml").stat().st_mtime_ns, stamp)
            self.assertNotIn("publication", meta)
            self.assertEqual(meta["kind"], "compiler-cache")

    def test_source_mirror_and_target_must_travel_together(self):
        with tempfile.TemporaryDirectory() as temporary:
            source = Path(temporary)
            (source / "target").mkdir()
            with self.assertRaisesRegex(cache.CacheError, "together"):
                cache.write_tar(source, io.BytesIO())

    def test_rejects_paths_links_special_files_and_duplicates(self):
        fixtures = [
            [("../outside", tarfile.REGTYPE, b"bad")],
            [("/target/outside", tarfile.REGTYPE, b"bad")],
            [("cargo/../outside", tarfile.REGTYPE, b"bad")],
            [("cargo\\outside", tarfile.REGTYPE, b"bad")],
            [("cargo", tarfile.SYMTYPE, "target")],
            [("target/link", tarfile.SYMTYPE, "/etc/passwd")],
            [("target/link", tarfile.SYMTYPE, "../../outside")],
            [("worktree/link", tarfile.SYMTYPE, "../target/bin")],
            [("target/link", tarfile.LNKTYPE, "target/bin")],
            [("target/fifo", tarfile.FIFOTYPE, None)],
            [("target/device", tarfile.CHRTYPE, None)],
            [("target/file", tarfile.REGTYPE, b"a"), ("target/file", tarfile.REGTYPE, b"b")],
            [("target/link", tarfile.SYMTYPE, "../cargo"), ("target/link/file", tarfile.REGTYPE, b"bad")],
        ]
        for entries in fixtures:
            with self.subTest(entries=entries), tempfile.TemporaryDirectory() as temporary:
                with self.assertRaises(cache.CacheError):
                    cache.unpack_tar(malicious_tar(entries), Path(temporary))

    def test_rejects_future_timestamp_without_touching_file(self):
        member = tarfile.TarInfo("target/file")
        member.pax_headers["mtime"] = str(time.time() + 3600)
        with self.assertRaisesRegex(cache.CacheError, "future"):
            cache.mtime_ns(member)

    def test_rejects_nonfinite_fractional_nanosecond_or_negative_timestamps(self):
        for value in ("NaN", "Infinity", "-1", "1700000000.1234567891"):
            with self.subTest(value=value):
                member = tarfile.TarInfo("target/file")
                member.pax_headers["mtime"] = value
                with self.assertRaises(cache.CacheError):
                    cache.mtime_ns(member)

    def test_bounded_extraction(self):
        with tempfile.TemporaryDirectory() as temporary, mock.patch.object(cache, "MAX_EXPANDED_BYTES", 1):
            with self.assertRaisesRegex(cache.CacheError, "limits"):
                cache.unpack_tar(malicious_tar([("target/item", tarfile.REGTYPE, b"large")]), Path(temporary))

    def test_will_not_overlay_existing_snapshot(self):
        with tempfile.TemporaryDirectory() as temporary:
            destination = Path(temporary)
            (destination / "target").mkdir()
            with self.assertRaisesRegex(cache.CacheError, "overlay"):
                cache.restore(destination, "/unused/oras", ROOT)


class MetadataTests(unittest.TestCase):
    def test_manifest_identity_and_compatibility(self):
        self.assertEqual(cache.validate_manifest(manifest(), REPOSITORY, CONTRACT)["schema"], 1)
        for field, value in (("schema", 2), ("kind", "passing-checks")):
            invalid = manifest(); invalid[field] = value
            with self.assertRaises(cache.CacheError):
                cache.validate_manifest(invalid, REPOSITORY, CONTRACT)
        wrong = copy.deepcopy(CONTRACT); wrong["rust_image"] = IMAGE.replace("1.99.1", "1.99.2")
        with self.assertRaisesRegex(cache.CacheError, "incompatible"):
            cache.validate_manifest(manifest(), REPOSITORY, wrong)
        with self.assertRaisesRegex(cache.CacheError, "another repository"):
            cache.validate_manifest(manifest(), "another/repo")

    def test_malformed_limits_fail_closed(self):
        for value in (-1, True, "not a size", None):
            data = manifest(); data["archive"]["expanded_bytes"] = value
            with self.subTest(value=value), self.assertRaises(cache.CacheError):
                cache.validate_manifest(data, REPOSITORY)

    def test_manifest_hash_is_verified(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / cache.ARCHIVE).write_bytes(b"corrupted")
            cache.write_json(root / cache.MANIFEST, manifest())
            with self.assertRaisesRegex(cache.CacheError, "checksum"):
                cache.validate_bundle(root, REPOSITORY)

    def test_source_edits_do_not_partition_cache_but_runner_changes_do(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            module = root / "ci/dagger/check"
            module.mkdir(parents=True)
            (module / "main.dang").write_text(f'let image = "{IMAGE}"')
            (module / "run-rust.sh").write_text("cargo test\n")
            (module / "run-crate.sh").write_text("cargo test -p model\n")
            (module / "stable-worktree.py").write_text("# stable timestamps\n")
            scripts = root / "ci/scripts"
            scripts.mkdir(parents=True)
            (scripts / "crate_sources.py").write_text("# closure identity\n")
            first = cache.compatibility(root)
            (root / "Cargo.lock").write_text("new dependencies\n")
            self.assertEqual(cache.compatibility(root), first)
            (module / "stable-worktree.py").write_text("# changed mirror algorithm\n")
            self.assertNotEqual(cache.compatibility(root), first)
            (module / "run-crate.sh").write_text("cargo test -p new-model\n")
            self.assertNotEqual(cache.compatibility(root), first)

    def test_restore_uses_same_pr_then_main_and_never_another_fork_cache(self):
        event = {"pull_request": {"number": 42, "head": {"repo": {"full_name": REPOSITORY}}}}
        self.assertEqual(cache.restore_refs(REPOSITORY, "pull_request", event), ["pr-42", "main"])
        event["pull_request"]["head"]["repo"]["full_name"] = "fork/chat-hub"
        self.assertEqual(cache.restore_refs(REPOSITORY, "pull_request", event), ["main"])
        self.assertEqual(cache.restore_refs(REPOSITORY, "push", {}), ["main"])

    def test_restore_scope_cannot_confuse_pr_failed_cache_and_green_main(self):
        data = manifest(); data["publication"] = publication()
        cache.validate_restore_scope(data, "pr-42", REPOSITORY)
        for tag in ("pr-43", "main"):
            with self.assertRaises(cache.CacheError):
                cache.validate_restore_scope(data, tag, REPOSITORY)
        data["producer"].update({"event": "push", "ref": "refs/heads/main"})
        data["publication"].update({"event": "push", "head_branch": "main", "conclusion": "success"})
        cache.validate_restore_scope(data, "main", REPOSITORY)
        data["publication"]["conclusion"] = "failure"
        with self.assertRaisesRegex(cache.CacheError, "successful"):
            cache.validate_restore_scope(data, "main", REPOSITORY)

    def test_restoration_requires_trusted_publisher_provenance(self):
        with self.assertRaisesRegex(cache.CacheError, "publisher"):
            cache.validate_restore_scope(manifest(), "pr-42", REPOSITORY)


class PublicationTests(unittest.TestCase):
    def test_green_publisher_retains_all_nine_crates_individually_without_extracting(self):
        import crate_artifacts

        run = workflow_run()
        run["conclusion"] = "success"
        crate_names = crate_artifacts.crates(ROOT)
        downloaded = []

        def download(repo, source_run, name, output, expected_files):
            downloaded.append(name)
            output.mkdir()
            if name.startswith("chat-hub-cargo-cache-"):
                payload = b"opaque, never extracted"
                (output / cache.ARCHIVE).write_bytes(payload)
                cache.write_json(output / cache.MANIFEST, manifest(payload))
            elif name.startswith("chat-hub-crate-"):
                (output / crate_artifacts.ARCHIVE).write_bytes(b"opaque compiled object")
                (output / crate_artifacts.MANIFEST).write_text("{}")
            else:
                cache.write_json(output / cache.CHECK_RECEIPT, evidence())

        @contextmanager
        def session(_):
            yield mock.Mock()

        with tempfile.TemporaryDirectory() as temporary:
            event = Path(temporary) / "event.json"
            cache.write_json(event, {"workflow_run": run})
            env = {"GITHUB_REPOSITORY": REPOSITORY, "GITHUB_EVENT_NAME": "workflow_run"}
            with mock.patch.dict(os.environ, env), \
                 mock.patch.object(cache, "validated_run", return_value=run), \
                 mock.patch.object(cache, "artifact_download", side_effect=download), \
                 mock.patch.object(cache, "validate_producer", return_value={**publication(), "conclusion": "success"}), \
                 mock.patch.object(cache, "registry_session", session), \
                 mock.patch.object(cache, "publish_bundle", return_value="sha256:" + "9" * 64) as publisher, \
                 mock.patch.object(cache, "validate_evidence"), \
                 mock.patch.object(crate_artifacts, "validate_bundle") as validate_crate, \
                 mock.patch.object(cache, "unpack_tar") as unpack:
                cache.publish(event, "/tools/oras")
        self.assertEqual(validate_crate.call_count, len(crate_names))
        self.assertEqual(len(downloaded), len(crate_names) + 2)  # cache + 9 crates + receipt
        self.assertEqual(publisher.call_count, len(crate_names) + 2)
        self.assertEqual({call.args[1] for call in publisher.call_args_list[1:-1]},
                         {f"ghcr.io/{REPOSITORY}-crate-{name}" for name in crate_names})
        unpack.assert_not_called()

    def test_run_is_refetched_and_forks_or_unexpected_workflows_are_rejected(self):
        original = workflow_run()
        api = mock.Mock(return_value=original)
        self.assertEqual(cache.validated_run({"workflow_run": original}, REPOSITORY, api), original)
        api.assert_called_once_with(f"repos/{REPOSITORY}/actions/runs/123/attempts/2")
        for change in ({"head_repository": {"full_name": "fork/chat-hub"}},
                       {"path": ".github/workflows/other.yml"}, {"status": "in_progress"},
                       {"run_attempt": 1}, {"event": "workflow_dispatch"}):
            wrong = {**original, **change}
            with self.subTest(change=change), self.assertRaises(cache.CacheError):
                cache.validated_run({"workflow_run": original}, REPOSITORY, mock.Mock(return_value=wrong))

    def test_pr_tree_and_merge_parents_are_verified(self):
        commit = {"sha": "a" * 40, "tree": {"sha": "b" * 40},
                  "parents": [{"sha": "d" * 40}, {"sha": "c" * 40}]}
        api = mock.Mock(return_value=commit)
        result = cache.validate_producer(manifest(), workflow_run(), REPOSITORY, api)
        self.assertEqual(result["conclusion"], "failure")
        self.assertEqual(result["pr_number"], 42)
        for change in ({"tree": {"sha": "f" * 40}}, {"parents": [{"sha": "c" * 40}]}):
            with self.subTest(change=change), self.assertRaises(cache.CacheError):
                cache.validate_producer(manifest(), workflow_run(), REPOSITORY, mock.Mock(return_value={**commit, **change}))

    def test_mismatched_pr_head_or_base_or_run_is_rejected(self):
        for key, value in (("base_sha", "e" * 40), ("head_sha", "e" * 40), ("run_attempt", 1), ("pr_number", 99)):
            data = manifest(); data["producer"][key] = value
            with self.subTest(key=key), self.assertRaises(cache.CacheError):
                cache.validate_producer(data, workflow_run(), REPOSITORY, mock.Mock())

    def test_missing_pr_links_use_authenticated_commit_association(self):
        run = workflow_run(); run["pull_requests"] = []
        commit = {"sha": "a" * 40, "tree": {"sha": "b" * 40},
                  "parents": [{"sha": "d" * 40}, {"sha": "c" * 40}]}
        linked = [{"number": 42, "head": {"repo": {"full_name": REPOSITORY}},
                   "base": {"repo": {"full_name": REPOSITORY}}}]
        api = mock.Mock(side_effect=[linked, commit])
        self.assertEqual(cache.validate_producer(manifest(), run, REPOSITORY, api)["pr_number"], 42)
        self.assertIn("/commits/" + "c" * 40 + "/pulls", api.call_args_list[0].args[0])
        with self.assertRaises(cache.CacheError):
            cache.validate_producer(manifest(), run, REPOSITORY, mock.Mock(return_value=[]))

    def test_failed_test_cache_can_update_only_its_own_pr_alias(self):
        run = workflow_run()
        self.assertEqual(cache.moving_tag(run, PRODUCER), "pr-42")
        run.update({"event": "push", "head_branch": "main"})
        self.assertIsNone(cache.moving_tag(run, PRODUCER))
        run["conclusion"] = "success"
        self.assertEqual(cache.moving_tag(run, PRODUCER), "main")

    def test_check_evidence_requires_green_exact_candidate(self):
        run = workflow_run()
        with self.assertRaisesRegex(cache.CacheError, "successful"):
            cache.validate_evidence(evidence(), run, manifest())
        run["conclusion"] = "success"
        cache.validate_evidence(evidence(), run, manifest())
        for key in ("tested_tree", "head_sha", "base_sha"):
            record = evidence(); record[key] = "f" * 40
            with self.subTest(key=key), self.assertRaises(cache.CacheError):
                cache.validate_evidence(record, run, manifest())

    def test_publisher_retains_failed_cache_without_unpacking_or_claiming_evidence(self):
        def download(repo, run, name, output, expected_files):
            output.mkdir()
            payload = b"not even a tar: publisher must not inspect or execute compiled content"
            (output / cache.ARCHIVE).write_bytes(payload)
            cache.write_json(output / cache.MANIFEST, manifest(payload))

        run_oras = mock.Mock()
        @contextmanager
        def session(_):
            yield run_oras

        with tempfile.TemporaryDirectory() as temporary:
            event = Path(temporary) / "event.json"
            cache.write_json(event, {"workflow_run": workflow_run()})
            env = {"GITHUB_REPOSITORY": REPOSITORY, "GITHUB_EVENT_NAME": "workflow_run"}
            with mock.patch.dict(os.environ, env), \
                    mock.patch.object(cache, "validated_run", return_value=workflow_run()), \
                    mock.patch.object(cache, "artifact_download", side_effect=download) as downloader, \
                    mock.patch.object(cache, "validate_producer", return_value=publication()), \
                    mock.patch.object(cache, "registry_session", session), \
                    mock.patch.object(cache, "publish_bundle", return_value="sha256:" + "9" * 64) as publisher, \
                    mock.patch.object(cache, "unpack_tar") as unpack:
                cache.publish(event, "/tools/oras")
                self.assertEqual(downloader.call_count, 1)
                self.assertEqual(publisher.call_count, 1)
                unpack.assert_not_called()
                run_oras.assert_called_once_with(["tag", "ghcr.io/iarnvidr/chat-hub-ci-cache@sha256:" + "9" * 64, "pr-42"])


class RegistryTests(unittest.TestCase):
    def test_auth_is_isolated_and_never_in_child_environment(self):
        invocations = []
        def fake(args, **kwargs):
            invocations.append((args, kwargs))
            self.assertNotIn("GH_TOKEN", kwargs["env"])
            self.assertNotIn("GITHUB_TOKEN", kwargs["env"])
            return ""
        with mock.patch.dict(os.environ, {"GH_TOKEN": "do-not-leak", "GITHUB_TOKEN": "other-secret"}), \
                mock.patch.object(cache, "command", side_effect=fake):
            with cache.registry_session("/tools/oras") as run:
                run(["manifest", "fetch", "ghcr.io/example/cache:main"])
                config = Path(invocations[0][1]["env"]["DOCKER_CONFIG"])
                self.assertTrue(config.is_dir())
        self.assertFalse(config.exists())
        self.assertEqual(invocations[0][1]["input_text"], "do-not-leak\n")
        self.assertEqual(invocations[1][0][:4], ["/tools/oras", "manifest", "fetch", "--registry-config"])
        self.assertIsNone(invocations[1][1]["input_text"])

    def test_registry_blob_names_are_checked_before_any_download(self):
        expected = {cache.ARCHIVE: 100, cache.MANIFEST: 100}
        layers = [{"digest": "sha256:" + "1" * 64, "size": 10,
                   "annotations": {"org.opencontainers.image.title": name}} for name in expected]
        self.assertEqual(set(cache.registry_descriptors({"layers": layers}, expected)), set(expected))
        for name in ("../../escape", "/etc/config", "unrelated"):
            changed = copy.deepcopy(layers)
            changed[0]["annotations"]["org.opencontainers.image.title"] = name
            with self.subTest(name=name), self.assertRaises(cache.CacheError):
                cache.registry_descriptors({"layers": changed}, expected)

    def test_immutable_existing_tag_must_have_identical_artifact_bytes(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "file.json").write_bytes(b"{}")
            wrong = {"layers": [{"digest": "sha256:" + "0" * 64, "size": 2,
                                 "annotations": {"org.opencontainers.image.title": "file.json"}}]}
            run = mock.Mock(return_value=json.dumps(wrong))
            with self.assertRaisesRegex(cache.CacheError, "immutable"):
                cache.publish_bundle(run, "ghcr.io/example/cache", "run-1-1", root,
                                     {"file.json": "application/json"}, REPOSITORY, "a" * 40)
            self.assertEqual(run.call_count, 1)

    def test_artifact_zip_cannot_choose_paths_or_symlinks(self):
        for bad_name, symlink in (("../../escape", False), ("file.json", True)):
            with self.subTest(name=bad_name, symlink=symlink), tempfile.TemporaryDirectory() as temporary:
                buffer = io.BytesIO()
                with zipfile.ZipFile(buffer, "w") as archive:
                    info = zipfile.ZipInfo(bad_name)
                    info.external_attr = ((stat.S_IFLNK | 0o777) if symlink else (stat.S_IFREG | 0o644)) << 16
                    archive.writestr(info, b"{}");
                data = buffer.getvalue()
                listed = {"total_count": 1, "artifacts": [{"id": 5, "name": "candidate", "expired": False,
                          "size_in_bytes": len(data), "digest": "sha256:" + hashlib.sha256(data).hexdigest()}]}
                process = mock.Mock()
                process.stdout = io.BytesIO(data); process.wait.return_value = 0; process.poll.return_value = 0
                with mock.patch.object(cache.subprocess, "Popen", return_value=process), self.assertRaises(cache.CacheError):
                    cache.artifact_download(REPOSITORY, workflow_run(), "candidate", Path(temporary) / "output",
                                            {"file.json": 100}, mock.Mock(return_value=listed))


if __name__ == "__main__":
    unittest.main()
