#!/usr/bin/env python3
"""Publish and restore independent, source-bound Cargo crate compiler objects.

Each package contains only its own compiled objects and the associated source
timestamp records. The shared cache still supplies third-party dependencies;
Cargo always checks whether a restored compiler object is actually fresh.
"""
from __future__ import annotations

import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import time
import tomllib

import oci_cache as cache
import crate_sources


SCHEMA = 2
ARCHIVE = "crate.tar.zst"
MANIFEST = "crate-manifest.json"
SOURCE_TIMES = "source-state/crate-seed.json"
MAX_CRATE_BYTES = 1024**3
MAX_SOURCE_STATE_BYTES = 8 * 1024**2
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


def archive_files(source: Path, files: list[Path], archive: Path,
                  metadata: bytes = b'{}') -> tuple[str, int, int]:
    """Stream exact owner outputs into a zstd tar with deterministic metadata."""
    if len(metadata) > MAX_SOURCE_STATE_BYTES:
        raise cache.CacheError("crate source-state exceeds limit")
    total = sum(path.stat().st_size for path in files) + len(metadata)
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
                    details = path.lstat()
                    if not stat.S_ISREG(details.st_mode):
                        raise cache.CacheError("crate output changed during packaging")
                    member = tarfile.TarInfo(relative)
                    member.size = details.st_size
                    member.mode = stat.S_IMODE(details.st_mode) & 0o777
                    member.uid = member.gid = 0
                    member.mtime = details.st_mtime_ns // 1_000_000_000
                    member.pax_headers = {"mtime": f"{details.st_mtime_ns // 1_000_000_000}.{details.st_mtime_ns % 1_000_000_000:09d}"}
                    with path.open("rb") as contents:
                        tar.addfile(member, contents)
                member = tarfile.TarInfo(SOURCE_TIMES)
                member.size = len(metadata)
                member.mode = 0o644
                member.uid = member.gid = 0
                member.mtime = int(time.time())
                tar.addfile(member, io.BytesIO(metadata))
            if process.wait(timeout=600) != 0:
                raise cache.CacheError("crate compression failed")
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
    digest, size = cache.file_digest(archive, MAX_CRATE_BYTES)
    return digest, size, total


def source_state(source: Path, root: Path, crate: str,
                 fingerprint: dict) -> bytes:
    """Save source mtimes for the exact Dagger closure, not another crate."""
    state_file = source / "source-state/last-real.json"
    if state_file.is_file() and not state_file.is_symlink():
        raw = cache.read_json(state_file, MAX_SOURCE_STATE_BYTES)
        if raw.get("schema") != 1 or not isinstance(raw.get("files"), dict):
            raise cache.CacheError("invalid Cargo source-state metadata")
        records = raw["files"]
    elif state_file.exists():
        raise cache.CacheError("unsafe Cargo source-state metadata")
    else:
        records = {}
    actual = crate_sources.scope(fingerprint["scope"], root)
    selected = {}
    for relative, content in actual.items():
        if not isinstance(content, Path) or relative not in records:
            continue
        candidate = records[relative]
        if (not isinstance(candidate, dict) or
                candidate.get("sha256") != hashlib.sha256(content.read_bytes()).hexdigest() or
                type(candidate.get("mtime_ns")) is not int or
                not 0 <= candidate["mtime_ns"] <= time.time_ns()):
            raise cache.CacheError(f"invalid cached timestamp for {relative}")
        selected[relative] = candidate
    return (json.dumps({"schema": 1, "source": fingerprint, "files": selected},
                       sort_keys=True, separators=(",", ":")) + "\n").encode()


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
        fingerprint = crate_sources.fingerprint(root, crate)
        records = source_state(source, root, crate, fingerprint)
        digest, size, expanded = archive_files(source, files, directory / ARCHIVE, records)
        metadata = {"schema": SCHEMA, "kind": "crate-compiler-objects", "crate": crate,
                    "compatibility": full["compatibility"], "producer": full["producer"],
                    "source": fingerprint,
                    "parent_cache_sha256": full["archive"]["sha256"],
                    "archive": {"name": ARCHIVE, "sha256": digest, "bytes": size,
                                "expanded_bytes": expanded, "files": len(files) + 1}}
        cache.write_json(directory / MANIFEST, metadata)
        print(f"{crate}: retained {len(files)} compiled outputs ({size} compressed bytes, closure {fingerprint['sha256'][:12]})")
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
    source = record.get("source")
    if (not isinstance(source, dict) or
            not re.fullmatch(r"[A-Za-z][A-Za-z0-9]*Source", str(source.get("scope"))) or
            not cache.SHA256.fullmatch(str(source.get("sha256"))) or
            type(source.get("files")) is not int or source["files"] < 1):
        raise cache.CacheError("crate bundle has no valid source-closure identity")
    if cache.repository(record["producer"].get("repository")) != repo:
        raise cache.CacheError("crate bundle belongs to a different repository")
    archive = record.get("archive") or {}
    if (archive.get("name") != ARCHIVE or not cache.SHA256.fullmatch(str(archive.get("sha256")))
            or type(archive.get("files")) is not int or archive["files"] < 2
            or not isinstance(archive.get("expanded_bytes"), int)
            or not 0 < archive["expanded_bytes"] <= MAX_CRATE_BYTES):
        raise cache.CacheError("invalid crate archive metadata")
    digest, size = cache.file_digest(directory / ARCHIVE, MAX_CRATE_BYTES)
    if digest != archive["sha256"] or size != archive.get("bytes"):
        raise cache.CacheError("crate archive digest or size mismatch")
    return record


def merge_source_state(destination: Path, seed: dict, root: Path, source: dict) -> int:
    """Add only missing, content-verified records; never overwrite newer state."""
    if (seed.get("schema") != 1 or seed.get("source") != source or
            not isinstance(seed.get("files"), dict)):
        raise cache.CacheError("crate source-state does not match validated source closure")
    real = {relative: value for relative, value in crate_sources.scope(source["scope"], root).items()
            if isinstance(value, Path)}
    now = time.time_ns()
    for relative, record in seed["files"].items():
        if (relative not in real or not isinstance(record, dict) or
                record.get("sha256") != hashlib.sha256(real[relative].read_bytes()).hexdigest() or
                type(record.get("mtime_ns")) is not int or
                not 0 <= record["mtime_ns"] <= now):
            raise cache.CacheError("crate source-state contains an invalid input or timestamp")
    location = destination / "source-state/last-real.json"
    if location.exists():
        existing = cache.read_json(location, 32 * 1024**2)
        if existing.get("schema") != 1 or not isinstance(existing.get("files"), dict):
            raise cache.CacheError("existing Cargo source-state is invalid")
        values = existing["files"]
    else:
        values = {}
    added = 0
    for relative, record in seed["files"].items():
        if relative not in values:
            values[relative] = record
            added += 1
    location.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(mode="w", encoding="utf-8", dir=location.parent,
                                     prefix=".crate-state-", delete=False) as temporary:
        json.dump({"schema": 1, "files": values}, temporary, sort_keys=True)
        temporary.write("\n")
        temporary_path = Path(temporary.name)
    temporary_path.replace(location)
    return added


def install_objects(stage: Path, destination: Path, crate: str, expected: dict,
                    root: Path) -> tuple[int, int]:
    """Install verified owner files without replacing other crates' outputs."""
    if any(p.is_symlink() or not (p.is_file() or p.is_dir()) for p in stage.rglob("*")):
        raise cache.CacheError("crate package contains symlinks or special files")
    outputs = selected(stage, crate)
    expected_paths = {p.relative_to(stage).as_posix() for p in outputs}
    extracted = {p.relative_to(stage).as_posix() for p in stage.rglob("*") if p.is_file()}
    if (extracted != expected_paths | {SOURCE_TIMES} or
            len(expected_paths) + 1 != expected["archive"]["files"]):
        raise cache.CacheError("crate archive contains non-owner outputs")
    seed = cache.read_json(stage / SOURCE_TIMES, MAX_SOURCE_STATE_BYTES)
    source = expected["source"]
    if seed.get("source") != source:
        raise cache.CacheError("crate source-state is not bound to the package")
    # Validate the entire seed before publishing any files into the live cache.
    real = {relative: value for relative, value in crate_sources.scope(source["scope"], root).items()
            if isinstance(value, Path)}
    if not isinstance(seed.get("files"), dict):
        raise cache.CacheError("missing source-state records")
    now = time.time_ns()
    for relative, record in seed["files"].items():
        if (relative not in real or not isinstance(record, dict) or
                record.get("sha256") != hashlib.sha256(real[relative].read_bytes()).hexdigest() or
                type(record.get("mtime_ns")) is not int or not 0 <= record["mtime_ns"] <= now):
            raise cache.CacheError("unsafe crate source-state input")
    replaced, already = 0, 0
    for path in outputs:
        relative = path.relative_to(stage)
        output = destination / relative
        if any(parent.is_symlink() for parent in [output, *output.parents] if parent != Path("/")):
            raise cache.CacheError("crate restore would write through a symlink")
        output.parent.mkdir(parents=True, exist_ok=True)
        if output.exists() and output.is_file() and not output.is_symlink():
            current_hash = cache.file_digest(output, MAX_CRATE_BYTES)
            old_hash = cache.file_digest(path, MAX_CRATE_BYTES)
            if current_hash == old_hash and output.stat().st_mtime_ns == path.stat().st_mtime_ns:
                already += 1
                continue
        with tempfile.NamedTemporaryFile(dir=output.parent, prefix=".crate-object-", delete=False) as temp:
            temp_path = Path(temp.name)
        try:
            shutil.copy2(path, temp_path)
            temp_path.replace(output)
        finally:
            temp_path.unlink(missing_ok=True)
        replaced += 1
    merge_source_state(destination, seed, root, source)
    return replaced, already


def restore(destination: Path, oras: str, root: Path) -> dict[str, str]:
    """Read published per-crate OCI packages independently of the base cache.

    A mismatched source, format, runner or trusted publisher is a cache MISS.
    Never let untrusted object reuse make a failed check look successful.
    """
    repo = cache.repository(os.environ.get("GITHUB_REPOSITORY"))
    event_path = os.environ.get("GITHUB_EVENT_PATH")
    event = cache.read_json(Path(event_path), 5 * 1024**2) if event_path else {}
    tags = cache.restore_refs(repo, os.environ.get("GITHUB_EVENT_NAME", ""), event)
    expected_compat = cache.compatibility(root)
    destination.mkdir(parents=True, exist_ok=True)
    if destination.is_symlink():
        raise cache.CacheError("unsafe crate restore destination")
    results: dict[str, str] = {}
    with cache.registry_session(oras) as run:
        for crate in crates(root):
            local_source = crate_sources.fingerprint(root, crate)
            image = f"ghcr.io/{repo}-crate-{crate}"
            for tag in tags:
                with tempfile.TemporaryDirectory(prefix=".crate-restore-", dir=destination.parent) as temporary:
                    stage = Path(temporary)
                    bundle, objects = stage / "bundle", stage / "objects"
                    bundle.mkdir()
                    objects.mkdir()
                    try:
                        digest = run(["resolve", f"{image}:{tag}"])
                        if not re.fullmatch(r"sha256:[a-f0-9]{64}", digest):
                            raise cache.CacheError("crate tag did not resolve to an immutable digest")
                        cache.fetch_registry_bundle(
                            run, image, digest, bundle,
                            {ARCHIVE: MAX_CRATE_BYTES, MANIFEST: cache.MAX_JSON_BYTES},
                        )
                        record = cache.read_json(bundle / MANIFEST)
                        parent = {"schema": cache.SCHEMA, "kind": "compiler-cache",
                                  "producer": record.get("producer"),
                                  "compatibility": record.get("compatibility"),
                                  "archive": {"name": cache.ARCHIVE,
                                              "sha256": record.get("parent_cache_sha256"),
                                              "bytes": 1, "expanded_bytes": 0, "entries": 1}}
                        cache.validate_manifest(parent, repo, expected_compat)
                        # Validate producer, source and compatibility before decompression.
                        validated = validate_bundle(bundle, crate, parent, repo)
                        if validated["compatibility"] != expected_compat:
                            raise cache.CacheError("crate compiler ABI/runner differs")
                        cache.validate_restore_scope(validated, tag, repo)
                        if validated["source"] != local_source:
                            raise cache.CacheError("crate source/dependency closure changed")
                        with tempfile.TemporaryFile() as errors:
                            process = subprocess.Popen(
                                ["zstd", "-q", "-dc", str(bundle / ARCHIVE)],
                                stdout=subprocess.PIPE, stderr=errors,
                            )
                            try:
                                with process.stdout:
                                    inventory = cache.unpack_tar(process.stdout, objects)
                                if process.wait(timeout=1200):
                                    raise cache.CacheError("crate decompression failed")
                            finally:
                                if process.poll() is None:
                                    process.kill()
                                    process.wait()
                        if (inventory["entries"] != validated["archive"]["files"] or
                                inventory["expanded_bytes"] != validated["archive"]["expanded_bytes"]):
                            raise cache.CacheError("crate archive inventory mismatch")
                        restored, already = install_objects(objects, destination, crate, validated, root)
                        results[crate] = f"{tag}: {restored} installed, {already} already present"
                        print(f"Reused published {crate} package {image}@{digest} ({results[crate]})")
                        break
                    except (cache.CacheError, OSError, tarfile.TarError, ValueError) as exc:
                        print(f"Crate {crate} from {tag} unavailable: {exc}; trying next package")
            else:
                results[crate] = "miss: Cargo will compile as needed"
                print(f"Crate {crate}: no compatible published package; Cargo will compile as needed")
    return results


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=("pack", "restore"), nargs="?", default="pack")
    parser.add_argument("--source", type=Path)
    parser.add_argument("--cache-manifest", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--destination", type=Path)
    parser.add_argument("--oras")
    parser.add_argument("--root", type=Path, default=Path.cwd())
    args = parser.parse_args()
    try:
        if args.operation == "pack":
            if any(arg is None for arg in (args.source, args.cache_manifest, args.output)):
                parser.error("pack needs --source, --cache-manifest and --output")
            pack(args.source, args.cache_manifest, args.output, args.root)
        else:
            if not args.destination or not args.oras:
                parser.error("restore needs --destination and --oras")
            restore(args.destination, args.oras, args.root)
    except (cache.CacheError, OSError, subprocess.SubprocessError, ValueError) as exc:
        if args.operation == "restore":
            print(f"Crate package restoration unavailable: {exc}; Cargo will check and compile inputs as needed")
            return 0
        print(f"crate artifact export failed: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
