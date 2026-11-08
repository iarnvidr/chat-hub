"""Structural coverage/source-scope regressions, not a substitute for Dagger CI.

These tests inspect the actual Dang declarations and Cargo manifests. They do not
keep a second handwritten package/dependency list that could silently drift.
"""

from __future__ import annotations

import functools
import importlib.util
import json
import os
from pathlib import Path
import re
import tomllib
import unittest

MODULE_DIR = Path(__file__).resolve().parent
ROOT = MODULE_DIR.parents[2]
MODULE = (MODULE_DIR / "main.dang").read_text()
DIRECTORIES = dict(re.findall(r"\blet (\w+): Directory! \{\n(.*?)\n  \}", MODULE, re.S))
CHECKS = dict(re.findall(r"\bpub (\w+): Void @check \{\n(.*?)\n  \}", MODULE, re.S))
JSON_STRING = r'"(?:\\.|[^"\\])*"'
DIRECTORY_CALL = re.compile(
    rf"ws\.directory\(({JSON_STRING})"
    r"(?:,\s*include:\s*(\[.*?\]))?"
    r"(?:,\s*exclude:\s*(\[.*?\]))?\)", re.S,
)
FILE_CALL = re.compile(rf"\.withFile\(({JSON_STRING}),\s*ws\.file\(({JSON_STRING})\)\)")
NEW_FILE = re.compile(rf"\.withNewFile\(({JSON_STRING}),\s*({JSON_STRING})\)")


def parse_list(value: str | None) -> list[str]:
    if value is None:
        return []
    # Dang and JSON share these literal strings; Dang also allows trailing commas.
    return [json.loads(item) for item in re.findall(JSON_STRING, value)]


def glob_regex(pattern: str) -> re.Pattern:
    result = ""
    index = 0
    while index < len(pattern):
        if pattern[index:index + 3] == "**/":
            result += "(?:[^/]+/)*"
            index += 3
        elif pattern[index:index + 2] == "**":
            result += ".*"
            index += 2
        elif pattern[index] == "*":
            result += "[^/]*"
            index += 1
        elif pattern[index] == "?":
            result += "[^/]"
            index += 1
        else:
            result += re.escape(pattern[index])
            index += 1
    return re.compile("^" + result + "$")


def matches(relative: str, patterns: list[str]) -> bool:
    return any(glob_regex(pattern).match(relative) for pattern in patterns)


@functools.lru_cache
def files_under(relative_dir: str) -> tuple[str, ...]:
    directory = ROOT / relative_dir.lstrip("/")
    found: list[str] = []
    for parent, dirs, names in os.walk(directory):
        dirs[:] = [name for name in dirs if name not in (".git", "target", "__pycache__", "node_modules")]
        for name in names:
            path = Path(parent, name)
            if path.is_file():
                found.append(path.relative_to(directory).as_posix())
    return tuple(found)


@functools.lru_cache
def scope(name: str) -> dict[str, Path | str]:
    """Evaluate only the declarative Directory subset used by this module.

    Real entries map to Path; placeholder entries map to their exact contents.
    Supporting an unfamiliar expression requires explicitly updating this test.
    """
    body = DIRECTORIES[name]
    parent = re.match(r"\s*(\w+)\b", body)[1]
    result = dict(scope(parent)) if parent != "ws" else {}
    for call in DIRECTORY_CALL.finditer(body):
        source_path = json.loads(call[1]).strip("/")
        include, exclude = parse_list(call[2]), parse_list(call[3])
        for relative in files_under(source_path):
            if include and not matches(relative, include):
                continue
            if matches(relative, exclude) or any(
                relative.startswith(pattern.rstrip("/") + "/")
                for pattern in exclude if "*" not in pattern
            ):
                continue
            full = f"{source_path}/{relative}".lstrip("/")
            result[full] = ROOT / full
    for call in FILE_CALL.finditer(body):
        result[json.loads(call[1]).lstrip("/")] = ROOT / json.loads(call[2]).lstrip("/")
    for call in NEW_FILE.finditer(body):
        result[json.loads(call[1]).lstrip("/")] = json.loads(call[2])
    return result


def workspace() -> dict[str, tuple[Path, dict]]:
    root = tomllib.loads((ROOT / "Cargo.toml").read_text())
    result = {}
    for member in root["workspace"]["members"]:
        paths = [ROOT] if member == "." else list(ROOT.glob(member))
        for directory in paths:
            manifest = tomllib.loads((directory / "Cargo.toml").read_text())
            result[manifest["package"]["name"]] = (directory, manifest)
    return result


def path_dependencies(directory: Path, manifest: dict, *, dev: bool) -> set[Path]:
    tables = [manifest] + list(manifest.get("target", {}).values())
    result = set()
    inherited = tomllib.loads((ROOT / "Cargo.toml").read_text()).get("workspace", {}).get("dependencies", {})
    for table in tables:
        for kind in ("dependencies", "build-dependencies", "dev-dependencies"):
            if kind == "dev-dependencies" and not dev:
                continue
            for name, dependency in table.get(kind, {}).items():
                if not isinstance(dependency, dict):
                    continue
                base = directory
                if dependency.get("workspace"):
                    dependency = inherited.get(name, {})
                    base = ROOT
                if isinstance(dependency, dict) and "path" in dependency:
                    result.add((base / dependency["path"]).resolve())
    return result


def dependency_closure(package: str) -> set[str]:
    packages = workspace()
    by_path = {path.resolve(): name for name, (path, _) in packages.items()}
    pending = [(package, True)]
    seen = set()
    while pending:
        name, include_dev = pending.pop()
        if name in seen:
            continue
        seen.add(name)
        path, manifest = packages[name]
        for dependency_path in path_dependencies(path, manifest, dev=include_dev):
            if dependency_path not in by_path:
                raise AssertionError(f"local dependency outside the explicit workspace: {dependency_path}")
            pending.append((by_path[dependency_path], False))
    return seen


def crate_checks() -> dict[str, tuple[str, str]]:
    result = {}
    for check, body in CHECKS.items():
        package = re.search(r'"/run-crate.sh",\s*"([^"]+)"', body)
        if package:
            if package[1] in result:
                raise AssertionError(f"duplicate test check for {package[1]}")
            source = re.search(r'withMountedDirectory\("/input",\s*(\w+)\)', body)
            if not source:
                raise AssertionError(f"missing source scope for {check}")
            result[package[1]] = (check, source[1])
    return result


class DaggerContract(unittest.TestCase):
    def test_native_v1_workspace_and_module_are_pinned(self):
        root = tomllib.loads((ROOT / "dagger.toml").read_text())
        self.assertEqual(root["modules"]["chat-hub-ci"], {"source": "./ci/dagger/check", "main": True})
        manifest = tomllib.loads((MODULE_DIR / "dagger-module.toml").read_text())
        self.assertEqual(manifest["engineVersion"], "v1.0.0-beta.25")
        self.assertEqual(manifest["runtime"]["source"], "dang")
        self.assertFalse((MODULE_DIR / "dagger.json").exists())
        self.assertFalse((MODULE_DIR / "main.go").exists())
        for image in re.findall(r'\.from\("([^"]+)"\)', MODULE):
            self.assertRegex(image, r"@sha256:[0-9a-f]{64}$")

    def test_one_native_test_check_per_workspace_crate(self):
        self.assertEqual(set(crate_checks()), set(workspace()))
        self.assertEqual(len(crate_checks()), 9)
        self.assertEqual(set(CHECKS) - {check for check, _ in crate_checks().values()},
                         {"rustStyle", "javascript", "browser", "contracts"})

    def test_every_test_scope_contains_all_production_and_selected_dev_dependencies(self):
        packages = workspace()
        for package, (_, source) in crate_checks().items():
            inputs = scope(source)
            for required in dependency_closure(package):
                directory, _ = packages[required]
                for path in (directory / "src").rglob("*.rs"):
                    relative = path.relative_to(ROOT).as_posix()
                    self.assertIsInstance(inputs.get(relative), Path,
                                          f"{package}/{source} omitted actual dependency input {relative}")
                manifest = (directory / "Cargo.toml").relative_to(ROOT).as_posix()
                self.assertIsInstance(inputs.get(manifest), Path)

    def test_skeleton_has_real_manifests_but_fail_closed_not_real_entrypoints(self):
        skeleton = scope("workspaceSkeleton")
        helper_spec = importlib.util.spec_from_file_location("stable_worktree", MODULE_DIR / "stable-worktree.py")
        helper = importlib.util.module_from_spec(helper_spec)
        helper_spec.loader.exec_module(helper)
        for directory, manifest in workspace().values():
            cargo_path = (directory / "Cargo.toml").relative_to(ROOT).as_posix()
            self.assertIsInstance(skeleton.get(cargo_path), Path)
            candidates = [directory / "src/lib.rs", directory / "src/main.rs"]
            if manifest.get("package", {}).get("build"):
                candidates.append(directory / manifest["package"]["build"])
            for path in candidates:
                if path.is_file():
                    relative = path.relative_to(ROOT).as_posix()
                    self.assertEqual(skeleton.get(relative), helper.PLACEHOLDER.decode(), relative)

    def test_ui_and_unrelated_rust_edits_do_not_enter_leaf_check_inputs(self):
        leaf_scopes = ["modelSource", "formatsSource", "storeSource", "ingestSource", "captureSource", "apiSource", "runtimeSource"]
        for name in leaf_scopes:
            self.assertFalse(any(path.startswith("desktop-ui/") for path in scope(name)))
            self.assertNotIn("src-tauri/src/browser_history.js", scope(name))
        self.assertNotIn("crates/chat-hub-store/src/store.rs", scope("formatsSource"))
        self.assertNotIn("crates/chat-hub-capture/src/proxy.rs", scope("storeSource"))
        # Package entrypoints are placeholders, not invalidating real input bytes.
        self.assertIsInstance(scope("modelSource")["crates/chat-hub-formats/src/lib.rs"], str)
        self.assertNotIn("desktop-ui/styles.css", scope("cliSource"))
        self.assertIn("desktop-ui/styles.css", scope("desktopSource"))

    def test_explicit_include_files_and_path_modules_are_not_hidden_inputs(self):
        included = re.compile(r'(?:include_(?:str|bytes)!\(\s*|#\[path\s*=\s*)"([^"]+)"')
        for package, (_, source) in crate_checks().items():
            inputs = scope(source)
            for relative, file in inputs.items():
                if not isinstance(file, Path) or file.suffix != ".rs":
                    continue
                for match in included.finditer(file.read_text()):
                    dependency = (file.parent / match[1]).resolve()
                    dependency_relative = dependency.relative_to(ROOT).as_posix()
                    self.assertIsInstance(inputs.get(dependency_relative), Path,
                                          f"{package}: {relative} includes missing {dependency_relative}")
        self.assertIn("build/native_capture_contract.py", scope("cliSource"))
        self.assertIn("tests/fixtures/chatgpt-export/array.json", scope("apiSource"))

    def test_source_scopes_never_consume_compiled_cache_or_git_history(self):
        for _, name in crate_checks().values():
            for relative in scope(name):
                self.assertFalse(relative.startswith(("ci/cache/", ".git/", "target/", "artifacts/")))

    def test_compiler_and_export_use_one_seeded_locked_volume(self):
        cache = re.search(r"let cargoCache: CacheVolume! \{(.*?)\n  \}", MODULE, re.S)[1]
        self.assertIn("source: cargoSeed, sharing: CacheSharingMode.LOCKED", cache)
        self.assertNotRegex(cache, r"(?i)\b(?:nonce|run.?id|commit|sha)\b")
        self.assertEqual(MODULE.count('.withMountedCache("/cache", cargoCache)'), 2)
        export = MODULE.split("pub exportCargoCache(", 1)[1]
        self.assertIn('withEnvVariable("CHAT_HUB_CACHE_EXPORT_NONCE", nonce)', export)
        for directory in ("target", "worktree", "source-state", "check-logs"):
            self.assertIn(directory, export)
        self.assertNotIn("GH_TOKEN", MODULE)
        self.assertNotIn("registryToken", MODULE)
        self.assertNotIn("GITHUB_RUN_ID", MODULE)

    def test_each_crate_has_full_failure_log_and_locked_cargo_commands(self):
        script = (MODULE_DIR / "run-crate.sh").read_text()
        self.assertIn('if [[ "$1" == "chat-hub-desktop" ]]', script)
        self.assertIn('cargo clippy --locked -p "$1" --all-targets --all-features -- -D clippy::correctness', script)
        self.assertIn('cargo clippy --locked -p "$1" --all-targets --all-features -- -D warnings', script)
        self.assertIn('cargo test --locked -p "$1" --all-targets --timings -vv', script)
        for check, _ in crate_checks().values():
            self.assertIn('withEnvVariable("CHAT_HUB_CARGO_LOG", "/cache/check-logs/', CHECKS[check])
        self.assertIn('"CARGO_BUILD_JOBS", "2"', MODULE)
        self.assertIn('"CARGO_PROFILE_DEV_DEBUG", "0"', MODULE)
        self.assertIn('"CARGO_PROFILE_TEST_DEBUG", "0"', MODULE)

    def test_no_node_and_existing_bun_browser_contracts_remain_checks(self):
        self.assertNotIn('.from("node:', MODULE)
        self.assertIn("Node executable is prohibited", MODULE)
        self.assertIn('"bash", "build/test-desktop-js.sh"', CHECKS["javascript"])
        for test in ("desktop_ui_browser.cjs", "desktop_workspace.cjs", "desktop_chat_features.cjs", "desktop_ui_graph_browser.cjs"):
            self.assertIn(f'"bun", "tests/{test}"', CHECKS["browser"])
        self.assertIn('"ci/dagger/check", "-p", "test_*.py"', CHECKS["contracts"])
        self.assertIn('"cargo", "fmt", "--all", "--", "--check"', CHECKS["rustStyle"])


if __name__ == "__main__":
    unittest.main()
