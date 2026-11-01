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
EXCLUDED_DIRS = {".git", "target", ".tools", "__pycache__", ".pytest_cache", ".mypy_cache", ".ruff_cache", ".venv", "venv", "dist", "build-cache"}


def _source_files(root):
    """Enumerate allowlisted source inputs without following symlinks."""
    roots = ["src", "src-tauri", "crates", "desktop-ui", "build", "ci/actions", "ci/scripts", ".github/workflows"]
    root_files = ["Cargo.toml", "Cargo.lock", "LICENSE", "NOTICE", "rust-toolchain", "rust-toolchain.toml", "rust-toolchain.yaml", "README.md"]
    candidates = set(root_files)
    for directory in roots:
        base = root / directory
        if base.is_dir() and not base.is_symlink():
            for path in base.rglob("*"):
                if path.is_symlink() or not path.is_file():
                    continue
                relative = path.relative_to(root)
                if any(part in EXCLUDED_DIRS or part.startswith(".") and part not in {".github"} for part in relative.parts):
                    continue
                candidates.add(relative.as_posix())
    result = []
    for relative in sorted(candidates):
        path = root / relative
        if path.is_symlink() or not path.is_file():
            continue
        resolved = path.resolve()
        if not resolved.is_relative_to(root.resolve()):
            continue
        result.append((relative, path))
    return result


def prepare(destination, root=ROOT):
    root, destination = Path(root).resolve(), Path(destination)
    if destination.exists() or destination.is_symlink():
        raise FileExistsError(f"destination already exists: {destination}")
    destination.parent.mkdir(parents=True, exist_ok=True)
    stage = Path(tempfile.mkdtemp(prefix=f".{destination.name}.staging-", dir=destination.parent))
    try:
        entries = []
        for relative, source in _source_files(root):
            target = stage / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, target)
            entries.append({"path": relative, "sha256": hashlib.sha256(target.read_bytes()).hexdigest()})
        if not {"LICENSE", "NOTICE"}.issubset({entry["path"] for entry in entries}):
            raise FileNotFoundError("required corresponding source missing: LICENSE or NOTICE")
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
