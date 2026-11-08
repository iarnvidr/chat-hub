#!/usr/bin/env python3
"""Retain separately addressable compiler outputs for each Dagger-tested crate.

These are inert, platform-specific Cargo objects, not installable releases or
proof that tests passed. The complete Cargo snapshot remains the restore unit:
it also contains dependencies, source fingerprints and the stable worktree.
"""
from __future__ import annotations

import argparse
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import tarfile
import tomllib

import oci_cache as cache


SCHEMA = 1
ARCHIVE = "crate.tar.zst"
MANIFEST = "crate-manifest.json"
MAX_CRATE_BYTES = 1024**3
CRATE_NAME = re.compile(r"chat-hub(?:-[a-z]+)*\Z")
HEX = r"[0-9a-f]{8,32}"
OBJECT_EXT = r"(?:rlib|rmeta|so|a|dylib|dll|d)"


def crates(root: Path) -> list[str]:
    workspace = tomllib.loads((root / "Cargo.toml").read_text())
    result = []
    for member in workspace["workspace"]["members"]:
        manifest = tomllib.loads((root / member / "Cargo.toml").read_text())
        name = manifest["package"]["name"]
        if not CRATE_NAME.fullmatch(name) or name in result:
            raise cache.CacheError("invalid or duplicate workspace crate")
        result.append(name)
    return result


def selected(cache_root: Path, crate: str) -> list[Path]:
    """Only outputs owned by this Cargo package, never unrelated deps."""
    if not CRATE_NAME.fullmatch(crate):
        raise cache.CacheError("invalid crate name")
    target = cache_root / "target"
    if target.is_symlink() or not target.is_dir():
        raise cache.CacheError("Cargo target directory is missing or unsafe")
    stem = crate.replace("-", "_")
    deps_name = re.compile(rf"(?:lib)?{re.escape(stem)}-{HEX}(?:\.{OBJECT_EXT})?\Z")
    package_name = re.compile(rf"{re.escape(crate)}-{HEX}\Z")
    outputs = []
    for namespace in target.iterdir():
        debug = namespace / "debug"
        if namespace.is_symlink() or not namespace.is_dir() or not debug.is_dir() or debug.is_symlink():
            continue
        for directory in ("deps", ".fingerprint", "build"):
            location = debug / directory
            if not location.is_dir() or location.is_symlink():
                continue
            for entry in location.iterdir():
                if directory == "deps" and entry.is_file() and deps_name.fullmatch(entry.name):
                    outputs.append(entry)
                elif directory != "deps" and entry.is_dir() and package_name.fullmatch(entry.name):
                    outputs.extend(path for path in entry.rglob("*") if path.is_file())
        # A tested library may have no standalone binary; retain real binaries
        # when Cargo produced them, without promising they are release builds.
        for name in (crate, stem):
            binary = debug / name
            if binary.is_file() and not binary.is_symlink():
                outputs.append(binary)
    outputs = sorted(set(outputs))
    for output in outputs:
        if output.is_symlink() or not output.is_file() or not stat.S_ISREG(output.stat().st_mode):
            raise cache.CacheError("crate output is not an ordinary file")
        if not output.resolve().is_relative_to(target.resolve()):
            raise cache.CacheError("crate output escapes target directory")
    return outputs


def archive_files(source: Path, files: list[Path], archive: Path) -> tuple[str, int, int]:
    """Stream exact owner outputs into a zstd tar with deterministic metadata."""
    total = sum(path.stat().st_size for path in files)
    if total > MAX_CRATE_BYTES:
        raise cache.CacheError("crate output exceeds the per-crate byte limit")
    with archive.open("xb") as output:
        process = subprocess.Popen(["zstd", "-q", "-3", "-T2", "-c"], stdin=subprocess.PIPE,
                                   stdout=output, stderr=subprocess.DEVNULL)
        try:
            with process.stdin, tarfile.open(fileobj=process.stdin, mode="w|", format=tarfile.PAX_FORMAT) as tar:
                for path in files:
                    relative = path.relative_to(source).as_posix()
                    cache.valid_name(relative)
                    metadata = path.lstat()
                    if not stat.S_ISREG(metadata.st_mode):
                        raise cache.CacheError("crate output changed during packaging")
                    member = tarfile.TarInfo(relative)
                    member.size = metadata.st_size
                    member.mode = stat.S_IMODE(metadata.st_mode) & 0o777
                    member.uid = member.gid = 0
                    member.mtime = metadata.st_mtime_ns // 1_000_000_000
                    member.pax_headers = {"mtime": f"{metadata.st_mtime_ns // 1_000_000_000}.{metadata.st_mtime_ns % 1_000_000_000:09d}"}
                    with path.open("rb") as contents:
                        tar.addfile(member, contents)
            if process.wait(timeout=600) != 0:
                raise cache.CacheError("crate compression failed")
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
    digest, size = cache.file_digest(archive, MAX_CRATE_BYTES)
    return digest, size, total


def pack(source: Path, cache_manifest: Path, output: Path, root: Path) -> list[str]:
    if output.exists():
        raise cache.CacheError("crate output directory already exists")
    full = cache.read_json(cache_manifest)
    cache.validate_manifest(full, cache.repository(os.environ.get("GITHUB_REPOSITORY")), cache.compatibility(root))
    names = crates(root)
    output.mkdir(parents=True)
    for crate in names:
        files = selected(source, crate)
        if not files:
            raise cache.CacheError(f"{crate}: no compiled outputs; refusing to publish an empty crate")
        directory = output / crate
        directory.mkdir()
        digest, size, expanded = archive_files(source, files, directory / ARCHIVE)
        metadata = {"schema": SCHEMA, "kind": "crate-compiler-objects", "crate": crate,
                    "compatibility": full["compatibility"], "producer": full["producer"],
                    "parent_cache_sha256": full["archive"]["sha256"],
                    "archive": {"name": ARCHIVE, "sha256": digest, "bytes": size,
                                "expanded_bytes": expanded, "files": len(files)}}
        cache.write_json(directory / MANIFEST, metadata)
        print(f"{crate}: retained {len(files)} compiled outputs ({size} compressed bytes)")
    return names


def validate_bundle(directory: Path, crate: str, parent: dict, repo: str) -> dict:
    if not CRATE_NAME.fullmatch(crate) or directory.is_symlink() or not directory.is_dir():
        raise cache.CacheError("invalid crate bundle")
    if {p.name for p in directory.iterdir()} != {ARCHIVE, MANIFEST}:
        raise cache.CacheError("unexpected crate bundle inventory")
    record = cache.read_json(directory / MANIFEST)
    if (record.get("schema") != SCHEMA or record.get("kind") != "crate-compiler-objects"
            or record.get("crate") != crate or record.get("producer") != parent["producer"]
            or record.get("compatibility") != parent["compatibility"]
            or record.get("parent_cache_sha256") != parent["archive"]["sha256"]):
        raise cache.CacheError("crate bundle is not bound to the validated compiler cache")
    if cache.repository(record["producer"].get("repository")) != repo:
        raise cache.CacheError("crate bundle belongs to a different repository")
    archive = record.get("archive") or {}
    if (archive.get("name") != ARCHIVE or not cache.SHA256.fullmatch(str(archive.get("sha256")))
            or not isinstance(archive.get("files"), int) or archive["files"] < 1
            or not isinstance(archive.get("expanded_bytes"), int)
            or not 0 < archive["expanded_bytes"] <= MAX_CRATE_BYTES):
        raise cache.CacheError("invalid crate archive metadata")
    digest, size = cache.file_digest(directory / ARCHIVE, MAX_CRATE_BYTES)
    if digest != archive["sha256"] or size != archive.get("bytes"):
        raise cache.CacheError("crate archive digest or size mismatch")
    return record


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--cache-manifest", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--root", type=Path, default=Path.cwd())
    args = parser.parse_args()
    try:
        pack(args.source, args.cache_manifest, args.output, args.root)
    except (cache.CacheError, OSError, subprocess.SubprocessError) as exc:
        print(f"crate artifact export failed: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
