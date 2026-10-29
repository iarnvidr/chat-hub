#!/usr/bin/env python3
"""Create a deterministic corresponding-source bundle for Chat Hub releases."""
import argparse
import hashlib
import json
import shutil
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
FORMAT = "chat-hub-corresponding-source-v1"


def prepare(destination, root=ROOT):
    root, destination = Path(root), Path(destination)
    if destination.exists() or destination.is_symlink():
        raise FileExistsError(f"destination already exists: {destination}")
    destination.parent.mkdir(parents=True, exist_ok=True)
    stage = Path(tempfile.mkdtemp(prefix=f".{destination.name}.staging-", dir=destination.parent))
    try:
        files = ["LICENSE", "NOTICE", "README.md", "Cargo.toml", "Cargo.lock",
                 "src-tauri/Cargo.toml", "src-tauri/tauri.conf.json"]
        entries = []
        for relative in files:
            source = root / relative
            if not source.is_file():
                raise FileNotFoundError(f"required corresponding source missing: {relative}")
            target = stage / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source, target)
            entries.append({"path": relative, "sha256": hashlib.sha256(target.read_bytes()).hexdigest()})
        (stage / "manifest.json").write_text(json.dumps({
            "format": FORMAT,
            "scope": "Corresponding Chat Hub source, build manifests, license and notice; not a complete third-party compliance determination.",
            "files": entries,
        }, indent=2, sort_keys=True) + "\n")
        if destination.exists() or destination.is_symlink():
            raise FileExistsError(f"destination already exists: {destination}")
        stage.rename(destination)
    finally:
        if stage.exists():
            shutil.rmtree(stage)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    command = sub.add_parser("prepare")
    command.add_argument("destination", type=Path)
    args = parser.parse_args()
    prepare(args.destination)


if __name__ == "__main__":
    main()
