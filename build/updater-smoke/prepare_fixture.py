#!/usr/bin/env python3
"""Make a temporary, internally version-consistent desktop fixture."""
import json
import pathlib
import re
import subprocess
import sys

root = pathlib.Path(sys.argv[1]).resolve()
version = sys.argv[2]
repo = pathlib.Path(__file__).resolve().parents[2]
if root == repo or repo in root.parents:
    raise SystemExit("refusing to mutate the smoke script's checkout; prepare a temporary source copy")
manifest = root / "src-tauri/Cargo.toml"
config = root / "src-tauri/tauri.conf.json"
root_lock = root / "Cargo.lock"
app_manifests = ("Cargo.toml", "src-tauri/Cargo.toml", "crates/chat-hub-api/Cargo.toml")
for relative in app_manifests:
    app_manifest = root / relative
    updated, count = re.subn(r'(?m)^version = "[^"]+"$', f'version = "{version}"',
                            app_manifest.read_text(), count=1)
    if count != 1:
        raise SystemExit(f"missing app package version in disposable fixture: {relative}")
    app_manifest.write_text(updated)
# The API library exposes the app's binary version. Update its local dependency
# requirements too, including runtime's test-support dependency, while leaving
# the independent implementation-library versions untouched.
dependency_manifests = [root / "Cargo.toml", root / "src-tauri/Cargo.toml",
                        *(root / "crates").glob("*/Cargo.toml")]
for dependency_manifest in dependency_manifests:
    source = dependency_manifest.read_text()
    updated = re.sub(r'(?m)^(chat-hub-api\s*=\s*\{[^}\n]*\bversion\s*=\s*")[^"]+(")',
                     lambda match: match[1] + version + match[2], source)
    if updated != source:
        dependency_manifest.write_text(updated)
cfg = json.loads(config.read_text())
cfg["version"] = version
config.write_text(json.dumps(cfg, indent=2) + "\n")
# The CLI, desktop and API's binary-version response share the app version and
# one workspace lockfile. Stable implementation-library versions
# remain independent when the disposable fixture changes the app version.
if not root_lock.is_file():
    raise SystemExit(f"missing workspace lockfile in disposable fixture: {root_lock}")
subprocess.run(["cargo", "generate-lockfile", "--offline", "--manifest-path", str(root / "Cargo.toml")], check=True, stdout=subprocess.DEVNULL)
# The subsequent production build validates the complete graph with --locked.
# metadata without --no-deps downloads target-specific package metadata, so keep
# this preparation step hermetic and validate manifest/lock agreement in tests.
subprocess.run(["cargo", "metadata", "--format-version", "1", "--locked", "--offline", "--no-deps", "--manifest-path", str(manifest)], check=True, stdout=subprocess.DEVNULL)
