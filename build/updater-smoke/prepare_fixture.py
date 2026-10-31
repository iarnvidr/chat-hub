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
root_manifest = root / "Cargo.toml"
config = root / "src-tauri/tauri.conf.json"
lock = root / "src-tauri/Cargo.lock"
root_lock = root / "Cargo.lock"
s = manifest.read_text()
rs = root_manifest.read_text()
rs, rn = re.subn(r'(?m)^version = "[^"]+"$', f'version = "{version}"', rs, count=1)
assert rn == 1
root_manifest.write_text(rs)
s, n = re.subn(r'(?m)^version = "[^"]+"$', f'version = "{version}"', s, count=1)
assert n == 1
manifest.write_text(s)
cfg = json.loads(config.read_text())
cfg["version"] = version
config.write_text(json.dumps(cfg, indent=2) + "\n")
# Regenerate both workspace lockfiles in this disposable copy. Both packages
# are path dependencies, so changing either version invalidates locked records.
for lockfile in (root_lock, lock):
    if not lockfile.is_file():
        raise SystemExit(f"missing lockfile in disposable fixture: {lockfile}")
    subprocess.run(["cargo", "generate-lockfile", "--offline", "--manifest-path", str(manifest)], check=True, stdout=subprocess.DEVNULL)
    # One generation updates the selected workspace lock; keep the other
    # lockfile coherent too where this repository maintains both.
    break
subprocess.run(["cargo", "generate-lockfile", "--offline", "--manifest-path", str(root / "Cargo.toml")], check=True, stdout=subprocess.DEVNULL)
# The subsequent production build validates the complete graph with --locked.
# metadata without --no-deps downloads target-specific package metadata, so keep
# this preparation step hermetic and validate manifest/lock agreement in tests.
subprocess.run(["cargo", "metadata", "--format-version", "1", "--locked", "--offline", "--no-deps", "--manifest-path", str(manifest)], check=True, stdout=subprocess.DEVNULL)
