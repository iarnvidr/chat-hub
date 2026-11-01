import io
import importlib.util
import json
import os
import pathlib
import subprocess
import sys
import tarfile
import tempfile
import unittest
from types import SimpleNamespace
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

    def test_composite_oras_installer_receives_scoped_release_download_token(self):
        action = (ROOT / "ci/actions/cargo-sccache-cache/action.yml").read_text()
        installer_step = action.split("    - name: Install pinned ORAS client\n", 1)[1]
        installer_step = installer_step.split("    - name: Restore protected-main compiler cache\n", 1)[0]
        self.assertIn("      env:\n        GH_TOKEN: ${{ inputs.github-token }}\n", installer_step)

    def test_composite_retries_with_verified_shared_oras_and_passes_selected_path(self):
        action = (ROOT / "ci/actions/cargo-sccache-cache/action.yml").read_text()
        installer_step = action.split("    - name: Install pinned ORAS client\n", 1)[1]
        installer_step = installer_step.split("    - name: Restore protected-main compiler cache\n", 1)[0]
        self.assertIn('cache_dir="$RUNNER_TEMP/chat-hub-oras"', installer_step)
        self.assertIn('shared_dir="$RUNNER_TEMP/oras"', installer_step)
        self.assertIn('elif path="$(bash "$installer" "$shared_dir"', installer_step)
        self.assertIn('echo "path=$path" >> "$GITHUB_OUTPUT"', installer_step)
        self.assertIn('ORAS installer diagnostic (sanitized)', installer_step)
        self.assertIn('echo "path=" >> "$GITHUB_OUTPUT"', installer_step)
        self.assertIn('--oras "${{ steps.oras.outputs.path }}"', action)

    def test_pinned_oras_installer_reuses_only_binary_matching_verified_digest(self):
        installer = (ROOT / "build/install-oras.sh").read_text()
        self.assertIn('binary_digest_file="$destination/.oras.sha256"', installer)
        self.assertIn('actual_digest="$(shasum -a 256 "$binary" | awk', installer)
        self.assertIn('[[ "$actual_digest" == "$recorded_digest" ]]', installer)
        self.assertIn("grep -Eq '^Version:[[:space:]]+1\\.2\\.3([[:space:]]|$)'", installer)
        self.assertLess(installer.index('shasum -a 256 -c'), installer.index('tar -xzf'))
        self.assertLess(installer.index('tar -xzf'), installer.index('printf \'%s\\n\' "$binary_digest"'))

    def test_rustc_wrapper_preserves_hyphenated_cargo_env_and_exact_arguments(self):
        with tempfile.TemporaryDirectory() as temp:
            root = pathlib.Path(temp)
            wrapper = root / "rustc-cache-wrapper"
            binary = root / "fake-sccache"
            marker = root / "sccache-enabled"
            binary.write_text(
                f"#!{sys.executable}\n"
                "import json, os, sys\n"
                "print(json.dumps({'args':sys.argv[1:],"
                "'cargo_bin':os.environ.get('CARGO_BIN_EXE_chat-hub')}))\n",
                encoding="utf-8",
            )
            binary.chmod(0o755)
            marker.write_text("enabled\n", encoding="ascii")
            compiled_cache.write_rustc_wrapper(wrapper, binary, marker)

            compiler_args = ["/fixture/rustc", "--arg with spaces", 'quote"inside']
            env = os.environ.copy()
            env["CARGO_BIN_EXE_chat-hub"] = "/fixture/path with spaces/chat-hub"
            result = subprocess.run([str(wrapper), *compiler_args], env=env,
                                    check=True, capture_output=True, text=True)
            payload = json.loads(result.stdout)
            self.assertEqual(payload["cargo_bin"], env["CARGO_BIN_EXE_chat-hub"])
            self.assertEqual(payload["args"], compiler_args)

    def test_rustc_wrapper_falls_back_to_compiler_without_cache_marker(self):
        with tempfile.TemporaryDirectory() as temp:
            root = pathlib.Path(temp)
            wrapper = root / "rustc-cache-wrapper"
            compiler = root / "fake-rustc"
            compiler.write_text(
                f"#!{sys.executable}\n"
                "import json, os, sys\n"
                "print(json.dumps({'args':sys.argv[1:],"
                "'cargo_bin':os.environ.get('CARGO_BIN_EXE_chat-hub')}))\n",
                encoding="utf-8",
            )
            compiler.chmod(0o755)
            compiled_cache.write_rustc_wrapper(wrapper, root / "missing-sccache",
                                                root / "missing-marker")

            compiler_args = [str(compiler), "--crate-name", "chat-hub"]
            env = os.environ.copy()
            env["CARGO_BIN_EXE_chat-hub"] = "fixture-value"
            result = subprocess.run([str(wrapper), *compiler_args], env=env,
                                    check=True, capture_output=True, text=True)
            payload = json.loads(result.stdout)
            self.assertEqual(payload["cargo_bin"], "fixture-value")
            self.assertEqual(payload["args"], compiler_args[1:])

    def test_tag_and_summary_use_compatibility_contract_shape(self):
        tag = compiled_cache.cache_tag(self.contract)
        self.assertRegex(tag, r"^cargo-sccache-v1-macos-[a-f0-9]{20}$")
        self.assertLessEqual(len(tag), 128)
        with tempfile.TemporaryDirectory() as temp:
            root = pathlib.Path(temp)
            summary = root / "summary.md"
            args = SimpleNamespace(root=str(root), platform="macos-arm64", platform_id="macos-15-arm64",
                                   target="aarch64-apple-darwin", cache_dir=str(root / "cache"),
                                   state_dir=str(root / "state"), github_env=None,
                                   repository="example/chat-hub", summary=str(summary))
            with mock.patch.dict("os.environ", {}, clear=True), \
                 mock.patch.object(compiled_cache, "contract_for", return_value=self.contract), \
                 mock.patch.object(compiled_cache, "install_sccache", return_value=root / "sccache"):
                compiled_cache.set_up(args)
            state = compiled_cache.read_state(pathlib.Path(args.state_dir))
            self.assertTrue(state["enabled"])
            self.assertRegex(state["tag"], r"^cargo-sccache-v1-macos-[a-f0-9]{20}$")
            self.assertIn("Contract:", summary.read_text())

    def test_prepare_and_report_summaries_remain_nonfatal_when_cache_setup_is_disabled(self):
        with tempfile.TemporaryDirectory() as temp:
            root = pathlib.Path(temp)
            summary = root / "summary.md"
            args = SimpleNamespace(root=str(root), platform="macos-arm64", platform_id="macos-15-arm64",
                                   target="aarch64-apple-darwin", cache_dir=str(root / "cache"),
                                   state_dir=str(root / "state"), github_env=None,
                                   repository="example/chat-hub", summary=str(summary))
            with mock.patch.dict("os.environ", {}, clear=True), \
                 mock.patch.object(compiled_cache, "contract_for", side_effect=compiled_cache.CacheError("unavailable")):
                compiled_cache.set_up(args)
            state = compiled_cache.read_state(pathlib.Path(args.state_dir))
            self.assertFalse(state["enabled"])
            compiled_cache.report(SimpleNamespace(state_dir=args.state_dir, summary=str(summary)))
            self.assertIn("without cache", summary.read_text())

    def test_sibling_retention_import_works_under_isolated_module_loading(self):
        path = ROOT / "ci/scripts/compiled_cache.py"
        spec = importlib.util.spec_from_file_location("compiled_cache_isolated", path)
        self.assertIsNotNone(spec)
        self.assertIsNotNone(spec.loader)
        module = importlib.util.module_from_spec(spec)
        restricted_path = [entry for entry in sys.path if pathlib.Path(entry or ".").resolve() != path.parent]
        with mock.patch.object(sys, "path", restricted_path), mock.patch.dict(sys.modules, {"cache_retention": None}):
            spec.loader.exec_module(module)
        self.assertEqual(module.cache_retention.platform_prefix("linux"), "cargo-sccache-v1-linux-")


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

        manual_ci = (ROOT / ".github/workflows/ci.yml").read_text()
        rust_job = manual_ci.split("  rust:", 1)[1].split("  browser:", 1)[0]
        self.assertIn("packages: read", rust_job)
        self.assertIn("cached-linux-checks", rust_job)
        self.assertIn("--registry-token env:GH_TOKEN", rust_job)

    def test_dagger_removes_cache_secret_before_any_cargo_command(self):
        dagger_source = (ROOT / "ci/dagger/check/main.go").read_text()
        cargo_start = dagger_source.index('WithExec([]string{"cargo", "fmt"')
        pull_secret_removal = dagger_source.index('WithoutSecretVariable("GH_TOKEN")')
        guard = dagger_source.index('${GH_TOKEN+x}')
        self.assertLess(pull_secret_removal, cargo_start)
        self.assertLess(guard, cargo_start)
        self.assertIn('WithoutEnvVariable("GH_TOKEN")', dagger_source[pull_secret_removal:cargo_start])


class CachePublisherTests(unittest.TestCase):
    def test_linux_publish_uses_prepared_oras_and_staged_bundle_working_directory(self):
        with tempfile.TemporaryDirectory() as temp:
            root = pathlib.Path(temp)
            summary = root / "summary.md"
            state = {
                "enabled": True,
                "repository": "example/chat-hub",
                "contract": {"compatibility": {"schema": 1, "target": "x86_64-unknown-linux-gnu"},
                             "provenance": {}},
                "contract_key": "a" * 64,
                "image": "ghcr.io/example/chat-hub-rust-cache-chat-hub",
                "tag": "cargo-sccache-v1-linux-" + "b" * 20,
                "oras": "/pinned/linux/oras",
                "cache_dir": str(root / "cache"),
            }
            args = SimpleNamespace(state_dir=str(root / "state"), summary=str(summary), oras="")

            def write_bundle(_state, _repository, destination):
                destination.write_bytes(b"fixture archive")
                return len(b"fixture archive")

            def staged_push(oras, image, operation, _summary, *, cwd=None):
                self.assertEqual(oras, state["oras"])
                self.assertEqual(image, state["image"])
                self.assertIsNotNone(cwd)
                archive_ref = next(item for item in operation if item.startswith("chat-hub-sccache-cache.tar.gz:"))
                self.assertTrue((cwd / archive_ref.split(":", 1)[0]).is_file())
                return "sha256:" + "c" * 64

            env = {"GITHUB_REF": "refs/heads/main", "GITHUB_EVENT_NAME": "push",
                   "GITHUB_RUN_ID": "123", "GITHUB_REPOSITORY": "example/chat-hub", "GITHUB_SHA": "d" * 40}
            with mock.patch.dict(os.environ, env, clear=True), \
                 mock.patch.object(compiled_cache, "read_state", return_value=state), \
                 mock.patch.object(compiled_cache, "report"), \
                 mock.patch.object(compiled_cache, "create_bundle", side_effect=write_bundle), \
                 mock.patch.object(compiled_cache.cache_retention, "preflight_publish", return_value={"before": 0, "deleted": 0, "remaining": 0}), \
                 mock.patch.object(compiled_cache.cache_retention, "prune_after_publish", return_value={"before": 1, "deleted": 0, "remaining": 1}), \
                 mock.patch.object(compiled_cache, "authenticate_and_call", side_effect=staged_push):
                compiled_cache.publish_cache(args)
            self.assertIn("published trusted-main cache", summary.read_text())


if __name__ == "__main__":
    unittest.main()
