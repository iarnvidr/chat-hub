"""Exercise the stable-worktree cache with real, offline Cargo builds.

These fixtures have no registry dependencies. Cargo's JSON ``fresh`` field, not
elapsed time, is the evidence that a compiler invocation was reused. Native
application and Dagger-engine execution remain separate hosted acceptance gates.
"""

from __future__ import annotations

import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
import unittest


RUNNER = Path(__file__).with_name("run-rust.sh")
MODEL = "cache_fixture_model"
APP = "cache_fixture_app"
SKELETON_SOURCE = (
    '// chat-hub-ci: workspace skeleton\n'
    'compile_error!("Dagger source closure omitted a required Cargo target");\n'
)


class CacheRunnerCase(unittest.TestCase):
    """Use only fixture-owned paths for cache and path-rejection tests."""

    @classmethod
    def setUpClass(cls) -> None:
        missing = [name for name in ("bash", "rsync", "realpath") if not shutil.which(name)]
        if missing:
            raise unittest.SkipTest("cache runner needs " + ", ".join(missing))
        if not RUNNER.is_file():
            raise AssertionError(f"cache runner is missing: {RUNNER}")

    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="chat-hub-cargo-cache-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.source = self.root / "input"
        self.cache = self.root / "cache"
        self.source.mkdir()
        self.cache.mkdir()
        self.env = os.environ.copy()
        # Host compilation settings must not decide the fixture's cache keys or
        # accidentally direct the test's output into another project.
        for key in (
            "CARGO_TARGET_DIR", "CARGO_ENCODED_RUSTFLAGS", "RUSTFLAGS",
            "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER", "CHAT_HUB_CARGO_LOG",
        ):
            self.env.pop(key, None)
        self.env.update({
            "CARGO_HOME": str(self.root / "cargo-home"),
            "CARGO_TARGET_DIR": str(self.cache / "target" / "default"),
            "CARGO_NET_OFFLINE": "true",
            "CARGO_INCREMENTAL": "0",
            "CARGO_BUILD_JOBS": "1",
            "CARGO_TERM_COLOR": "never",
        })

    def run_cached(
        self,
        *command: str,
        source: Path | None = None,
        cache: Path | None = None,
        env: dict[str, str] | None = None,
    ) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            ["bash", str(RUNNER), str(source or self.source), str(cache or self.cache), *command],
            env=env or self.env,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
            timeout=120,
        )

    def assert_ok(self, result: subprocess.CompletedProcess[str]) -> None:
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


class CacheRunnerSafetyTests(CacheRunnerCase):
    def test_changed_bytes_sync_even_with_same_size_and_source_mtime(self) -> None:
        path = self.source / "value.txt"
        path.write_text("before\n")
        original_mtime = path.stat().st_mtime_ns
        self.assert_ok(self.run_cached("true"))
        path.write_text("after!\n")
        os.utime(path, ns=(original_mtime, original_mtime))
        self.assert_ok(self.run_cached("true"))
        self.assertEqual((self.cache / "worktree" / "value.txt").read_text(), "after!\n")

    def test_mtime_only_changes_do_not_touch_the_mirror(self) -> None:
        path = self.source / "value.txt"
        path.write_text("same bytes\n")
        self.assert_ok(self.run_cached("true"))
        mirrored = self.cache / "worktree" / "value.txt"
        before = mirrored.stat().st_mtime_ns
        changed = time.time_ns() + 10_000_000_000
        os.utime(path, ns=(changed, changed))
        self.assert_ok(self.run_cached("true"))
        self.assertEqual(mirrored.stat().st_mtime_ns, before)

    def test_deleted_sources_disappear_from_the_mirror(self) -> None:
        path = self.source / "removed.txt"
        path.write_text("must not become a hidden build input\n")
        self.assert_ok(self.run_cached("true"))
        path.unlink()
        self.assert_ok(self.run_cached("true"))
        self.assertFalse((self.cache / "worktree" / "removed.txt").exists())

    def test_source_file_symlinks_are_rejected(self) -> None:
        outside = self.root / "outside.txt"
        outside.write_text("not a declared input\n")
        (self.source / "hidden-input").symlink_to(outside)
        result = self.run_cached("true")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("symlink", (result.stdout + result.stderr).lower())
        self.assertFalse((self.cache / "worktree" / "hidden-input").exists())

    def test_source_directory_symlink_is_rejected(self) -> None:
        linked_source = self.root / "linked-input"
        linked_source.symlink_to(self.source, target_is_directory=True)
        result = self.run_cached("true", source=linked_source)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("symlink", (result.stdout + result.stderr).lower())

    def test_cache_cannot_be_the_source_or_its_child(self) -> None:
        for destination in (self.source, self.source / "nested-cache"):
            with self.subTest(destination=destination):
                environment = self.env | {"CARGO_TARGET_DIR": str(destination / "target" / "default")}
                result = self.run_cached("true", cache=destination, env=environment)
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse((destination / "worktree").exists())

    def test_cached_worktree_symlink_is_rejected_without_writing_through_it(self) -> None:
        outside = self.root / "outside"
        outside.mkdir()
        sentinel = outside / "sentinel.txt"
        sentinel.write_text("untouched\n")
        (self.cache / "worktree").symlink_to(outside, target_is_directory=True)
        (self.source / "sentinel.txt").write_text("must not replace outside data\n")
        result = self.run_cached("true")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("symlink", (result.stdout + result.stderr).lower())
        self.assertEqual(sentinel.read_text(), "untouched\n")

    def test_target_directory_must_be_inside_the_cache_target_namespace(self) -> None:
        outside = self.root / "unrelated-target"
        result = self.run_cached("true", env=self.env | {"CARGO_TARGET_DIR": str(outside)})
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("CARGO_TARGET_DIR", result.stdout + result.stderr)
        self.assertFalse(outside.exists())

    def test_target_symlink_cannot_escape_the_cache_namespace(self) -> None:
        outside = self.root / "outside-target"
        outside.mkdir()
        (self.cache / "target").symlink_to(outside, target_is_directory=True)
        result = self.run_cached("true")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((outside / "default").exists())

    def test_future_target_timestamps_fail_closed(self) -> None:
        target = self.cache / "target" / "default"
        target.mkdir(parents=True)
        future = target / "future-artifact"
        future.write_text("stale fingerprint hazard\n")
        timestamp = time.time() + 3600
        os.utime(future, (timestamp, timestamp))
        result = self.run_cached("true")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("future", (result.stdout + result.stderr).lower())

    def test_full_log_retains_both_streams_and_the_original_failure(self) -> None:
        log = self.cache / "check-logs" / "failed-check.log"
        result = self.run_cached(
            "bash", "-c", "printf 'compiler stdout\\n'; printf 'compiler stderr\\n' >&2; exit 37",
            env=self.env | {"CHAT_HUB_CARGO_LOG": str(log)},
        )
        self.assertEqual(result.returncode, 37, result.stdout + result.stderr)
        self.assertEqual(log.read_text(), "compiler stdout\ncompiler stderr\n")
        self.assertIn("compiler stderr", result.stdout + result.stderr)

    def test_log_destination_must_be_cache_owned(self) -> None:
        outside = self.root / "outside.log"
        result = self.run_cached("true", env=self.env | {"CHAT_HUB_CARGO_LOG": str(outside)})
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("CHAT_HUB_CARGO_LOG", result.stdout + result.stderr)
        self.assertFalse(outside.exists())

    def test_log_path_cannot_traverse_out_of_the_log_directory(self) -> None:
        path = self.cache / "check-logs" / ".." / "escaped.log"
        result = self.run_cached("true", env=self.env | {"CHAT_HUB_CARGO_LOG": str(path)})
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.cache / "escaped.log").exists())

    def test_log_parent_symlink_cannot_redirect_output(self) -> None:
        outside = self.root / "outside-logs"
        outside.mkdir()
        (self.cache / "check-logs").mkdir()
        (self.cache / "check-logs" / "linked").symlink_to(outside, target_is_directory=True)
        path = self.cache / "check-logs" / "linked" / "outside.log"
        result = self.run_cached("true", env=self.env | {"CHAT_HUB_CARGO_LOG": str(path)})
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((outside / "outside.log").exists())

    def test_malformed_source_state_fails_closed(self) -> None:
        self.assert_ok(self.run_cached("true"))
        state = self.cache / "source-state" / "last-real.json"
        for malformed in ("{broken", "[]", '{"schema":99,"files":{}}'):
            with self.subTest(malformed=malformed):
                state.write_text(malformed)
                result = self.run_cached("true")
                self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
                self.assertIn("source-state", result.stdout + result.stderr)

    def test_future_source_state_timestamp_fails_closed(self) -> None:
        (self.source / "value.txt").write_text("unchanged source\n")
        self.assert_ok(self.run_cached("true"))
        state = self.cache / "source-state" / "last-real.json"
        data = json.loads(state.read_text())
        data["files"]["value.txt"]["mtime_ns"] = time.time_ns() + 3_600_000_000_000
        state.write_text(json.dumps(data))
        result = self.run_cached("true")
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn("future", (result.stdout + result.stderr).lower())

    def test_source_state_paths_cannot_escape_the_workspace(self) -> None:
        self.assert_ok(self.run_cached("true"))
        state = self.cache / "source-state" / "last-real.json"
        data = {
            "schema": 1,
            "files": {"../escape": {"sha256": "0" * 64, "mtime_ns": 1}},
        }
        state.write_text(json.dumps(data))
        result = self.run_cached("true")
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn("source-state path", result.stdout + result.stderr)
        self.assertFalse((self.cache / "escape").exists())

    def test_source_state_directory_cannot_be_a_symlink(self) -> None:
        outside = self.root / "outside-state"
        outside.mkdir()
        (self.cache / "source-state").symlink_to(outside, target_is_directory=True)
        result = self.run_cached("true")
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertFalse((outside / "last-real.json").exists())

    def test_source_state_file_cannot_be_a_symlink(self) -> None:
        outside = self.root / "outside-state.json"
        outside.write_text('{"schema":1,"files":{}}')
        (self.cache / "source-state").mkdir()
        (self.cache / "source-state" / "last-real.json").symlink_to(outside)
        result = self.run_cached("true")
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertEqual(outside.read_text(), '{"schema":1,"files":{}}')


class CargoFingerprintTests(CacheRunnerCase):
    @classmethod
    def setUpClass(cls) -> None:
        super().setUpClass()
        missing = [name for name in ("cargo", "rustc") if not shutil.which(name)]
        if missing:
            raise unittest.SkipTest("real offline Cargo fixtures need " + ", ".join(missing))

    def setUp(self) -> None:
        super().setUp()
        self.env["RUSTC"] = str(shutil.which("rustc"))
        (self.source / "model" / "src").mkdir(parents=True)
        (self.source / "app" / "src").mkdir(parents=True)
        (self.source / "Cargo.toml").write_text(
            '[workspace]\nmembers = ["model", "app"]\nresolver = "2"\n'
        )
        (self.source / "model" / "Cargo.toml").write_text(
            '[package]\nname = "cache-fixture-model"\nversion = "0.1.0"\nedition = "2021"\n'
        )
        (self.source / "app" / "Cargo.toml").write_text(
            '[package]\nname = "cache-fixture-app"\nversion = "0.1.0"\nedition = "2021"\n'
            '[dependencies]\ncache-fixture-model = { path = "../model" }\n'
        )
        (self.source / "model" / "src" / "lib.rs").write_text("pub fn value() -> u8 { 1 }\n")
        (self.source / "app" / "src" / "lib.rs").write_text(
            "pub fn doubled() -> u8 { cache_fixture_model::value() * 2 }\n"
        )
        generated = subprocess.run(
            ["cargo", "generate-lockfile", "--offline", "--manifest-path", str(self.source / "Cargo.toml")],
            env=self.env, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            timeout=30, check=False,
        )
        self.assert_ok(generated)

    def build(self, package: str | None = None) -> subprocess.CompletedProcess[str]:
        selection = ["-p", package] if package else ["--workspace"]
        return self.run_cached(
            "cargo", "build", "--offline", "--locked", *selection, "--message-format=json", "-vv",
        )

    def artifact_states(self, result: subprocess.CompletedProcess[str]) -> dict[str, bool]:
        states = {}
        for line in result.stdout.splitlines():
            try:
                message = json.loads(line)
            except json.JSONDecodeError:
                continue
            if message.get("reason") == "compiler-artifact":
                states[message["target"]["name"]] = message["fresh"]
        return states

    def freshness(
        self, result: subprocess.CompletedProcess[str], expected: set[str] | None = None,
    ) -> dict[str, bool]:
        self.assert_ok(result)
        states = self.artifact_states(result)
        self.assertEqual(set(states), expected or {MODEL, APP}, result.stdout + result.stderr)
        return states

    def test_cold_then_identical_warm_then_fresh_checkout_mtimes(self) -> None:
        self.assertEqual(self.freshness(self.build()), {MODEL: False, APP: False})
        self.assertEqual(self.freshness(self.build()), {MODEL: True, APP: True})
        source_times = {
            path.relative_to(self.source): (self.cache / "worktree" / path.relative_to(self.source)).stat().st_mtime_ns
            for path in self.source.rglob("*") if path.is_file()
        }
        timestamp = time.time_ns() + 10_000_000_000
        for path in self.source.rglob("*"):
            if path.is_file():
                os.utime(path, ns=(timestamp, timestamp))
        self.assertEqual(self.freshness(self.build()), {MODEL: True, APP: True})
        for relative, previous in source_times.items():
            self.assertEqual((self.cache / "worktree" / relative).stat().st_mtime_ns, previous, str(relative))

    def test_same_size_changed_leaf_rebuilds_only_that_leaf(self) -> None:
        self.freshness(self.build())
        path = self.source / "app" / "src" / "lib.rs"
        before = path.read_text()
        original_mtime = path.stat().st_mtime_ns
        changed = before.replace("* 2", "* 3")
        self.assertNotEqual(changed, before)
        self.assertEqual(len(changed), len(before))
        path.write_text(changed)
        os.utime(path, ns=(original_mtime, original_mtime))
        self.assertEqual(self.freshness(self.build()), {MODEL: True, APP: False})
        self.assertEqual(self.freshness(self.build()), {MODEL: True, APP: True})

    def test_changed_dependency_rebuilds_only_its_dependency_chain(self) -> None:
        self.freshness(self.build())
        path = self.source / "model" / "src" / "lib.rs"
        before = path.read_text()
        original_mtime = path.stat().st_mtime_ns
        path.write_text(before.replace("{ 1 }", "{ 7 }"))
        os.utime(path, ns=(original_mtime, original_mtime))
        self.assertEqual(self.freshness(self.build()), {MODEL: False, APP: False})
        self.assertEqual(self.freshness(self.build()), {MODEL: True, APP: True})

    def test_edit_then_revert_must_recompile_not_restore_an_old_content_timestamp(self) -> None:
        self.freshness(self.build())
        path = self.source / "model" / "src" / "lib.rs"
        original = path.read_text()
        original_mtime = path.stat().st_mtime_ns
        path.write_text(original.replace("{ 1 }", "{ 8 }"))
        os.utime(path, ns=(original_mtime, original_mtime))
        self.assertEqual(self.freshness(self.build()), {MODEL: False, APP: False})
        # A content-addressed timestamp catalog must not give the old A bytes a
        # time older than the target last compiled from B. Cargo uses mtimes.
        path.write_text(original)
        os.utime(path, ns=(original_mtime, original_mtime))
        self.assertEqual(self.freshness(self.build()), {MODEL: False, APP: False})
        self.assertEqual(self.freshness(self.build()), {MODEL: True, APP: True})

    def test_scope_switch_restores_freshness_for_unchanged_real_sources(self) -> None:
        app_source = self.source / "app" / "src" / "lib.rs"
        app_text = "mod helper;\npub fn doubled() -> u8 { cache_fixture_model::value() * helper::factor() }\n"
        helper = self.source / "app" / "src" / "helper.rs"
        helper_text = "pub fn factor() -> u8 { 2 }\n"
        app_source.write_text(app_text)
        helper.write_text(helper_text)
        self.assertEqual(self.freshness(self.build()), {MODEL: False, APP: False})
        original_times = {
            name: (self.cache / "worktree" / "app" / "src" / name).stat().st_mtime_ns
            for name in ("lib.rs", "helper.rs")
        }
        # A model-only Dagger source closure has real workspace manifests but
        # only a marked placeholder entrypoint for the unselected app package.
        app_source.write_text(SKELETON_SOURCE)
        helper.unlink()
        self.assertEqual(
            self.freshness(self.build("cache-fixture-model"), expected={MODEL}), {MODEL: True},
        )
        self.assertFalse((self.cache / "worktree" / "app" / "src" / "helper.rs").exists())
        app_source.write_text(app_text)
        helper.write_text(helper_text)
        self.assertEqual(self.freshness(self.build()), {MODEL: True, APP: True})
        for name, previous in original_times.items():
            self.assertEqual((self.cache / "worktree" / "app" / "src" / name).stat().st_mtime_ns, previous)

    def test_skeleton_marker_does_not_hide_real_code_or_a_revert(self) -> None:
        self.freshness(self.build())
        path = self.source / "model" / "src" / "lib.rs"
        original = path.read_text()
        # Only the exact, un-compilable sentinel is a placeholder. A marker
        # comment on ordinary source must not suppress the last-real update.
        path.write_text("// chat-hub-ci: workspace skeleton\n" + original.replace("{ 1 }", "{ 9 }"))
        self.assertEqual(self.freshness(self.build()), {MODEL: False, APP: False})
        path.write_text(original)
        self.assertEqual(self.freshness(self.build()), {MODEL: False, APP: False})
        self.assertEqual(self.freshness(self.build()), {MODEL: True, APP: True})

    def test_failed_leaf_retains_successfully_compiled_dependencies(self) -> None:
        path = self.source / "app" / "src" / "lib.rs"
        original = path.read_text()
        path.write_text('compile_error!("intentional cache fixture failure");\n')
        failed = self.build()
        self.assertNotEqual(failed.returncode, 0)
        self.assertIn("intentional cache fixture failure", failed.stdout + failed.stderr)
        self.assertEqual(self.artifact_states(failed).get(MODEL), False)
        path.write_text(original)
        self.assertEqual(self.freshness(self.build()), {MODEL: True, APP: False})

    def test_deleted_module_is_not_satisfied_by_stale_mirror_content(self) -> None:
        (self.source / "app" / "src" / "lib.rs").write_text(
            "mod helper;\npub fn doubled() -> u8 { cache_fixture_model::value() * helper::factor() }\n"
        )
        helper = self.source / "app" / "src" / "helper.rs"
        helper.write_text("pub fn factor() -> u8 { 2 }\n")
        self.freshness(self.build())
        helper.unlink()
        result = self.build()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("file not found for module", result.stdout + result.stderr)
        self.assertFalse((self.cache / "worktree" / "app" / "src" / "helper.rs").exists())


if __name__ == "__main__":
    unittest.main()
