#!/usr/bin/env python3
"""Make a temporary, internally version-consistent desktop fixture."""
import json
import pathlib
import re
import subprocess
import sys

root = pathlib.Path(sys.argv[1]).resolve()
version = sys.argv[2]
manifest = root / "src-tauri/Cargo.toml"
root_manifest = root / "Cargo.toml"
config = root / "src-tauri/tauri.conf.json"
lock = root / "src-tauri/Cargo.lock"
package = root / "package.json"
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
pkg = json.loads(package.read_text())
pkg["version"] = version
package.write_text(json.dumps(pkg, separators=(",", ":")) + "\n")
s = lock.read_text()
pattern = r'(?ms)(\[\[package\]\]\nname = "chat-hub-desktop"\nversion = ")[^"]+("\n)'
s, n = re.subn(pattern, rf'\g<1>{version}\g<2>', s, count=1)
assert n == 1, "desktop package absent from lockfile"
lock.write_text(s)
subprocess.run(["cargo", "metadata", "--format-version", "1", "--locked", "--offline", "--no-deps", "--manifest-path", str(manifest)], check=True, stdout=subprocess.DEVNULL)
