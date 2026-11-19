#!/usr/bin/env python3
"""Checksum-sync one immutable Cargo input closure without checkout mtime churn.

A single LOCKED cache owns the mirror, targets and this metadata. Remember only
the LAST real contents of each source path. Workspace skeleton placeholders are
not real source: swapping an unrelated scope in must not forget a compiled
crate's source mtimes. A real edit, including reverting to older contents, always
gets a new mtime. Keeping arbitrary historical content timestamps would be wrong:
Cargo could then reuse a newer, different binary after a source revert.
"""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import stat
import subprocess
import sys
import time

SCHEMA = 1
MAX_FILES = 100_000
MAX_STATE_BYTES = 32 * 1024 * 1024
PLACEHOLDER = (
    b"// chat-hub-ci: workspace skeleton\n"
    b'compile_error!("Dagger source closure omitted a required Cargo target");\n'
)


def fail(message: str) -> None:
    raise ValueError(message)


def files(root: Path) -> dict[str, Path]:
    result: dict[str, Path] = {}
    for directory, directories, names in os.walk(root, followlinks=False):
        for name in directories + names:
            path = Path(directory, name)
            mode = path.lstat().st_mode
            if stat.S_ISLNK(mode):
                fail("Rust source/mirror symlinks are unsupported")
            if not stat.S_ISDIR(mode) and not stat.S_ISREG(mode):
                fail("Rust source/mirror contains a non-regular file")
        for name in names:
            path = Path(directory, name)
            result[path.relative_to(root).as_posix()] = path
            if len(result) > MAX_FILES:
                fail("Rust source exceeds file count limit")
    return result


def digest(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def placeholder(path: Path) -> bool:
    with path.open("rb") as stream:
        # Exact bytes only. A valid Rust file merely starting with our marker
        # must not suppress metadata updates and hide a later source revert.
        return stream.read(len(PLACEHOLDER) + 1) == PLACEHOLDER


def safe_relative(value: str) -> bool:
    path = PurePosixPath(value)
    return bool(value) and not path.is_absolute() and all(
        part not in ("", ".", "..") for part in value.split("/")
    ) and "\\" not in value


def read_state(path: Path, now: int) -> dict[str, dict[str, str | int]]:
    if not path.exists():
        return {}
    if path.is_symlink() or not path.is_file() or path.stat().st_size > MAX_STATE_BYTES:
        fail("Invalid Rust source-state file; use a fresh cache")
    try:
        state = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as error:
        raise ValueError("Invalid Rust source-state JSON; use a fresh cache") from error
    records = state.get("files") if isinstance(state, dict) else None
    if not isinstance(state, dict) or state.get("schema") != SCHEMA or not isinstance(records, dict) or len(records) > MAX_FILES:
        fail("Unsupported Rust source-state schema; use a fresh cache")
    for relative, record in records.items():
        if not isinstance(relative, str) or not safe_relative(relative) or not isinstance(record, dict):
            fail("Invalid Rust source-state path; use a fresh cache")
        fingerprint = record.get("sha256")
        mtime = record.get("mtime_ns")
        if not isinstance(fingerprint, str) or len(fingerprint) != 64 or any(
            char not in "0123456789abcdef" for char in fingerprint
        ) or type(mtime) is not int or mtime < 0 or mtime > now:
            fail("Invalid or future Rust source-state timestamp; use a fresh cache")
    return records


def sync(input_dir: Path, cache_root: Path) -> None:
    worktree = cache_root / "worktree"
    state_dir = cache_root / "source-state"
    if state_dir.is_symlink() or (state_dir.exists() and not state_dir.is_dir()):
        fail("Rust source-state directory must not be a symlink")
    state_dir.mkdir(parents=True, exist_ok=True)
    state_file = state_dir / "last-real.json"
    now = time.time_ns()
    records = read_state(state_file, now)
    current_files = files(worktree)
    source_files = files(input_dir)

    # Existing real mirror contents are the authoritative last state, including
    # when upgrading a cache that predates source-state metadata. Never learn a
    # placeholder as real contents or trust its mtime for a later real crate.
    for relative, path in current_files.items():
        if not placeholder(path):
            mtime = path.stat().st_mtime_ns
            if mtime > now:
                fail("Cached source has future timestamps; use a fresh cache")
            records[relative] = {"sha256": digest(path), "mtime_ns": mtime}

    source_hashes = {
        relative: digest(path)
        for relative, path in source_files.items()
        if not placeholder(path)
    }
    # No --times: incoming checkout timestamps are not authoritative. Removed
    # inputs disappear rather than satisfying a missing source-closure edge.
    subprocess.run([
        "rsync", "--recursive", "--checksum", "--links", "--perms", "--delete",
        f"{input_dir}/", f"{worktree}/",
    ], check=True)

    retained = restored = changed = 0
    for relative, fingerprint in source_hashes.items():
        path = worktree / relative
        previous = records.get(relative)
        if previous and previous["sha256"] == fingerprint:
            prior_mtime = int(previous["mtime_ns"])
            if path.stat().st_mtime_ns != prior_mtime:
                os.utime(path, ns=(path.stat().st_atime_ns, prior_mtime))
                restored += 1
            else:
                retained += 1
        else:
            # This includes reverting to previously seen contents: only the
            # latest real hash is stored, so a revert cannot resurrect an old
            # timestamp that predates a binary compiled from different bytes.
            changed += 1
        records[relative] = {"sha256": fingerprint, "mtime_ns": path.stat().st_mtime_ns}
    if len(records) > MAX_FILES:
        # Dropping inactive metadata sacrifices reuse, never correctness.
        records = {relative: records[relative] for relative in source_hashes}
    temporary = state_dir / "last-real.json.tmp"
    if temporary.is_symlink():
        fail("Rust source-state temporary file must not be a symlink")
    temporary.write_text(json.dumps({"schema": SCHEMA, "files": records}, sort_keys=True) + "\n", encoding="utf-8")
    temporary.replace(state_file)
    print(f"Cargo mirror: {retained} unchanged, {restored} restored unchanged, {changed} changed inputs")


if __name__ == "__main__":
    try:
        if len(sys.argv) != 3:
            fail("usage: stable-worktree.py INPUT_DIR CACHE_ROOT")
        sync(Path(sys.argv[1]), Path(sys.argv[2]))
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        print(f"Rust cache synchronization refused: {error}", file=sys.stderr)
        sys.exit(2)
