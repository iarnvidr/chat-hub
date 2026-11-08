#!/usr/bin/env python3
"""Inert, timestamp-preserving Cargo snapshots for disposable CI engines.

The PR job may *read* its own cache and a green-main cache. Only trusted
workflow_run code publishes. A compiler cache is never passing-check evidence.
The publisher never extracts the Cargo tar or executes any producer content.
"""
from __future__ import annotations

import argparse
from contextlib import contextmanager
from decimal import Decimal, InvalidOperation
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import time
import zipfile


SCHEMA = 1
CACHE_FORMAT = "chat-hub-cargo-mirror-v1"
ARCHIVE = "cargo-cache.tar.zst"
MANIFEST = "cache-manifest.json"
CHECK_RECEIPT = "check-receipt.json"
WORKFLOW = ".github/workflows/check.yml"
ROOTS = frozenset({"cargo", "target", "worktree", "source-state"})
MAX_ARCHIVE_BYTES = 4 * 1024**3
MAX_EXPANDED_BYTES = 20 * 1024**3
MAX_ENTRIES = 500_000
MAX_JSON_BYTES = 64 * 1024
SHA = re.compile(r"[a-f0-9]{40}\Z")
SHA256 = re.compile(r"[a-f0-9]{64}\Z")


class CacheError(Exception):
    """An invalid/unavailable optimization must not bypass required checks."""


def repository(value: object) -> str:
    if not isinstance(value, str) or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]*/[A-Za-z0-9][A-Za-z0-9_.-]*", value):
        raise CacheError("invalid repository")
    return value.lower()


def positive_integer(value: object, name: str) -> int:
    if isinstance(value, bool) or not re.fullmatch(r"[1-9][0-9]*", str(value)):
        raise CacheError(f"invalid {name}")
    return int(str(value))


def sha(value: object, name: str) -> str:
    if not isinstance(value, str) or not SHA.fullmatch(value):
        raise CacheError(f"invalid {name}")
    return value


def file_digest(path: Path, limit: int = MAX_ARCHIVE_BYTES) -> tuple[str, int]:
    digest = hashlib.sha256()
    size = 0
    if path.is_symlink() or not path.is_file():
        raise CacheError("artifact member is not a regular file")
    with path.open("rb") as source:
        while chunk := source.read(1024 * 1024):
            size += len(chunk)
            if size > limit:
                raise CacheError("artifact exceeds size limit")
            digest.update(chunk)
    return digest.hexdigest(), size


def read_json(path: Path, limit: int = MAX_JSON_BYTES) -> dict:
    if path.is_symlink() or not path.is_file() or path.stat().st_size > limit:
        raise CacheError("missing, unsafe or oversized JSON metadata")
    try:
        result = json.loads(path.read_text(encoding="utf-8"))
    except (UnicodeError, json.JSONDecodeError) as exc:
        raise CacheError("malformed JSON metadata") from exc
    if not isinstance(result, dict):
        raise CacheError("JSON metadata must be an object")
    return result


def write_json(path: Path, value: dict) -> None:
    data = (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode()
    if len(data) > MAX_JSON_BYTES:
        raise CacheError("metadata exceeds size limit")
    path.write_bytes(data)


def command(args: list[str], *, cwd: Path | None = None, env: dict | None = None,
            input_text: str | None = None, timeout: int = 60) -> str:
    try:
        result = subprocess.run(args, cwd=cwd, env=env, input=input_text,
                                text=True, capture_output=True, check=True, timeout=timeout)
    except (OSError, subprocess.SubprocessError) as exc:
        # Never echo argv/environment/stdin from registry authentication failures.
        raise CacheError(f"{Path(args[0]).name} operation failed") from exc
    return result.stdout.strip()


def compatibility(root: Path) -> dict:
    module = root / "ci/dagger/check/main.dang"
    images = set(re.findall(r"rust:[A-Za-z0-9_.-]+@sha256:[a-f0-9]{64}", module.read_text()))
    if len(images) != 1:
        raise CacheError("expected one digest-pinned Rust image in Dagger module")
    scripts = [root / "ci/dagger/check/run-rust.sh"]
    helper = root / "ci/dagger/check/stable-worktree.py"
    if helper.exists():
        scripts.append(helper)
    script_hashes = {str(path.relative_to(root)): file_digest(path, MAX_JSON_BYTES)[0]
                     for path in scripts}
    return {"format": CACHE_FORMAT, "platform": "linux-x86_64",
            "rust_image": images.pop(), "runners": script_hashes}


def provenance(root: Path, environ: dict | None = None) -> dict:
    env = os.environ if environ is None else environ
    repo = repository(env.get("GITHUB_REPOSITORY"))
    tested_sha = sha(command(["git", "rev-parse", "HEAD"], cwd=root), "checkout SHA")
    if tested_sha != env.get("GITHUB_SHA"):
        raise CacheError("checkout differs from the immutable workflow SHA")
    tested_tree = sha(command(["git", "rev-parse", "HEAD^{tree}"], cwd=root), "checkout tree")
    event_name = env.get("GITHUB_EVENT_NAME")
    if event_name not in ("pull_request", "push", "workflow_dispatch"):
        raise CacheError("unsupported producer event")
    result = {"repository": repo,
              "run_id": positive_integer(env.get("GITHUB_RUN_ID"), "run ID"),
              "run_attempt": positive_integer(env.get("GITHUB_RUN_ATTEMPT"), "run attempt"),
              "event": event_name, "ref": env.get("GITHUB_REF", ""),
              "tested_sha": tested_sha, "tested_tree": tested_tree,
              "head_sha": tested_sha, "head_repository": repo}
    if event_name == "pull_request":
        event = read_json(Path(env.get("GITHUB_EVENT_PATH", "")), 5 * 1024**2)
        pr = event.get("pull_request") or {}
        head, base = pr.get("head") or {}, pr.get("base") or {}
        result.update({"pr_number": positive_integer(pr.get("number"), "PR number"),
                       "head_sha": sha(head.get("sha"), "PR head"),
                       "base_sha": sha(base.get("sha"), "PR base"),
                       "head_repository": repository((head.get("repo") or {}).get("full_name"))})
    return result


def valid_name(name: str) -> PurePosixPath:
    path = PurePosixPath(name)
    if (not name or "\\" in name or "\x00" in name or path.is_absolute()
            or ".." in path.parts or "." in name.split("/")
            or not path.parts or path.parts[0] not in ROOTS
            or str(path) != name.rstrip("/")):
        raise CacheError("unsafe cache archive path")
    return path


def valid_link(path: PurePosixPath, target: str) -> None:
    if not target or "\\" in target or "\x00" in target or target.startswith("/"):
        raise CacheError("unsafe cache symlink")
    stack = list(path.parent.parts)
    for part in PurePosixPath(target).parts:
        if part == "..":
            if not stack:
                raise CacheError("cache symlink escapes snapshot")
            stack.pop()
        elif part not in ("", "."):
            stack.append(part)
    if not stack or stack[0] not in ROOTS:
        raise CacheError("cache symlink escapes snapshot")
    if path.parts[0] in ("worktree", "source-state"):
        raise CacheError("source mirrors cannot contain symlinks")


def mtime_ns(member: tarfile.TarInfo) -> int:
    try:
        stamp = Decimal(member.pax_headers.get("mtime", str(member.mtime)))
        nanoseconds = stamp * 1_000_000_000
        if not stamp.is_finite() or nanoseconds != nanoseconds.to_integral_value():
            raise CacheError("invalid timestamp precision")
        result = int(nanoseconds)
    except (ValueError, TypeError, InvalidOperation, OverflowError) as exc:
        raise CacheError("invalid cache timestamp") from exc
    if result < 0 or result > time.time_ns():
        raise CacheError("cache contains a future or invalid timestamp")
    return result


def tar_members(source: Path):
    """Walk without following links; exclude logs and the seed placeholder."""
    if not {"cargo", "target", "worktree"}.issubset({p.name for p in source.iterdir()}):
        raise CacheError("cache export must preserve cargo, target and source mirror together")
    for entry in sorted(source.iterdir()):
        if entry.name in ("check-logs", ".gitignore"):
            continue
        if entry.name not in ROOTS or not entry.is_dir() or entry.is_symlink():
            raise CacheError("unexpected cache export root")
        yield entry
        for current, directories, files in os.walk(entry, followlinks=False):
            for name in sorted(directories + files):
                yield Path(current) / name


def write_tar(source: Path, stream) -> dict:
    count, total = 0, 0
    with tarfile.open(fileobj=stream, mode="w|", format=tarfile.PAX_FORMAT) as archive:
        for path in tar_members(source):
            count += 1
            if count > MAX_ENTRIES:
                raise CacheError("cache has too many entries")
            relative = valid_name(path.relative_to(source).as_posix())
            details = path.lstat()
            member = tarfile.TarInfo(str(relative))
            member.mode = stat.S_IMODE(details.st_mode) & 0o777
            member.uid = member.gid = 0
            member.mtime = details.st_mtime_ns // 1_000_000_000
            member.pax_headers = {"mtime": f"{details.st_mtime_ns // 1_000_000_000}.{details.st_mtime_ns % 1_000_000_000:09d}"}
            mtime_ns(member)
            if stat.S_ISDIR(details.st_mode):
                member.type = tarfile.DIRTYPE
                archive.addfile(member)
            elif stat.S_ISLNK(details.st_mode):
                member.type = tarfile.SYMTYPE
                member.linkname = os.readlink(path)
                valid_link(relative, member.linkname)
                archive.addfile(member)
            elif stat.S_ISREG(details.st_mode):
                total += details.st_size
                if total > MAX_EXPANDED_BYTES:
                    raise CacheError("expanded cache exceeds size limit")
                member.size = details.st_size
                # Do not accidentally follow a replaced link during packaging.
                descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
                with os.fdopen(descriptor, "rb") as data:
                    before = os.fstat(data.fileno())
                    if (before.st_dev, before.st_ino, before.st_mtime_ns, before.st_size) != (
                            details.st_dev, details.st_ino, details.st_mtime_ns, details.st_size):
                        raise CacheError("cache changed during packaging")
                    archive.addfile(member, data)
                    after = os.fstat(data.fileno())
                    if (after.st_mtime_ns, after.st_size) != (before.st_mtime_ns, before.st_size):
                        raise CacheError("cache changed during packaging")
            else:
                raise CacheError("cache contains a device, socket or FIFO")
    if not count:
        raise CacheError("cache export is empty")
    return {"entries": count, "expanded_bytes": total}


def unpack_tar(stream, destination: Path) -> dict:
    """Extract manually: no owners, hardlinks, traversal or writes through links."""
    if destination.is_symlink() or not destination.is_dir() or any(destination.iterdir()):
        raise CacheError("cache extraction requires a new empty directory")
    seen, metadata = set(), []
    count, total = 0, 0
    with tarfile.open(fileobj=stream, mode="r|*") as archive:
        for member in archive:
            path = valid_name(member.name)
            if str(path) in seen:
                raise CacheError("duplicate cache archive path")
            seen.add(str(path))
            count += 1
            total += member.size
            if count > MAX_ENTRIES or member.size < 0 or total > MAX_EXPANDED_BYTES:
                raise CacheError("cache extraction exceeds limits")
            stamp = mtime_ns(member)
            if (not (member.isfile() or member.isdir() or member.issym())
                    or member.mode & 0o7000 or member.sparse is not None):
                raise CacheError("unsupported cache archive entry")
            output = destination.joinpath(*path.parts)
            for parent in output.parents:
                if parent == destination:
                    break
                if parent.is_symlink():
                    raise CacheError("cache writes through a symlink")
            output.parent.mkdir(parents=True, exist_ok=True)
            if member.isdir():
                if output.is_symlink() or (output.exists() and not output.is_dir()):
                    raise CacheError("cache directory conflicts with a file")
                output.mkdir(exist_ok=True)
            elif member.issym():
                if len(path.parts) == 1:
                    raise CacheError("cache root cannot be a symlink")
                valid_link(path, member.linkname)
                os.symlink(member.linkname, output)
            else:
                if len(path.parts) == 1:
                    raise CacheError("cache root must be a directory")
                data = archive.extractfile(member)
                if data is None:
                    raise CacheError("cache file data is missing")
                with data, output.open("xb") as target:
                    shutil.copyfileobj(data, target, length=1024 * 1024)
                if output.stat().st_size != member.size:
                    raise CacheError("truncated cache file")
            metadata.append((output, member.mode & 0o777, stamp, member.issym()))
    # Set directory mtimes last, after creation of their children.
    for output, mode, stamp, is_link in reversed(metadata):
        if not is_link:
            output.chmod(mode)
        os.utime(output, ns=(stamp, stamp), follow_symlinks=False)
    return {"entries": count, "expanded_bytes": total}


def pack(source: Path, output: Path, root: Path) -> None:
    source = source.absolute()
    if source.is_symlink() or not source.is_dir() or output.exists():
        raise CacheError("pack needs an existing cache and a new output directory")
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".cargo-pack-", dir=output.parent) as temporary:
        stage = Path(temporary)
        archive = stage / ARCHIVE
        with archive.open("xb") as target, tempfile.TemporaryFile() as errors:
            process = subprocess.Popen(["zstd", "-q", "-3", "-T2", "-c"], stdin=subprocess.PIPE,
                                       stdout=target, stderr=errors)
            try:
                with process.stdin:
                    inventory = write_tar(source, process.stdin)
                if process.wait(timeout=1200):
                    raise CacheError("cache compression failed")
            finally:
                if process.poll() is None:
                    process.kill()
                    process.wait()
        digest, size = file_digest(archive)
        manifest = {"schema": SCHEMA, "kind": "compiler-cache", "compatibility": compatibility(root),
                    "producer": provenance(root), "archive": {"name": ARCHIVE, "sha256": digest,
                    "bytes": size, **inventory}}
        write_json(stage / MANIFEST, manifest)
        # Never overlay a source mirror from one snapshot onto a different target.
        stage.rename(output)
    print(f"Cargo cache packed: {inventory['expanded_bytes']} expanded bytes, {size} compressed bytes; not check evidence")


def validate_manifest(value: dict, repo: str, expected_compatibility: dict | None = None) -> dict:
    if value.get("schema") != SCHEMA or value.get("kind") != "compiler-cache":
        raise CacheError("unsupported cache schema or kind")
    producer, comp, archive = value.get("producer"), value.get("compatibility"), value.get("archive")
    if not all(isinstance(item, dict) for item in (producer, comp, archive)):
        raise CacheError("incomplete cache metadata")
    if repository(producer.get("repository")) != repo:
        raise CacheError("cache belongs to another repository")
    if (comp.get("format") != CACHE_FORMAT or comp.get("platform") != "linux-x86_64"
            or not re.fullmatch(r"rust:[A-Za-z0-9_.-]+@sha256:[a-f0-9]{64}", str(comp.get("rust_image")))
            or not isinstance(comp.get("runners"), dict) or not comp["runners"]
            or any(not SHA256.fullmatch(str(digest)) for digest in comp["runners"].values())):
        raise CacheError("invalid cache compatibility contract")
    if expected_compatibility is not None and comp != expected_compatibility:
        raise CacheError("incompatible compiler cache")
    positive_integer(producer.get("run_id"), "producer run")
    positive_integer(producer.get("run_attempt"), "producer attempt")
    for field in ("tested_sha", "tested_tree", "head_sha"):
        sha(producer.get(field), field)
    repository(producer.get("head_repository"))
    if producer.get("event") == "pull_request":
        positive_integer(producer.get("pr_number"), "producer PR")
        sha(producer.get("base_sha"), "producer base")
    elif producer.get("event") not in ("push", "workflow_dispatch"):
        raise CacheError("invalid producer event")
    expanded = archive.get("expanded_bytes")
    if (archive.get("name") != ARCHIVE or not SHA256.fullmatch(str(archive.get("sha256")))
            or not 0 < positive_integer(archive.get("bytes"), "archive bytes") <= MAX_ARCHIVE_BYTES
            or isinstance(expanded, bool) or not isinstance(expanded, int)
            or not 0 <= expanded <= MAX_EXPANDED_BYTES
            or not 0 < positive_integer(archive.get("entries"), "archive entries") <= MAX_ENTRIES):
        raise CacheError("invalid archive inventory")
    return value


def validate_bundle(directory: Path, repo: str, expected_compatibility: dict | None = None) -> dict:
    if directory.is_symlink() or {p.name for p in directory.iterdir()} != {ARCHIVE, MANIFEST}:
        raise CacheError("unexpected cache bundle inventory")
    manifest = validate_manifest(read_json(directory / MANIFEST), repo, expected_compatibility)
    digest, size = file_digest(directory / ARCHIVE)
    if digest != manifest["archive"]["sha256"] or size != manifest["archive"]["bytes"]:
        raise CacheError("compiler cache checksum or size mismatch")
    return manifest


@contextmanager
def registry_session(oras: str):
    with tempfile.TemporaryDirectory(prefix="chat-hub-oci-auth-") as temporary:
        config = Path(temporary) / "config.json"
        env = dict(os.environ)
        token = env.pop("GH_TOKEN", "")
        env.pop("GITHUB_TOKEN", None)
        env["DOCKER_CONFIG"] = temporary

        def run(args: list[str], *, cwd: Path | None = None, input_text: str | None = None) -> str:
            prefix_length = 2 if args[0] in ("manifest", "blob", "repo") else 1
            return command([oras, *args[:prefix_length], "--registry-config", str(config), *args[prefix_length:]],
                           cwd=cwd, env=env, input_text=input_text, timeout=1200)

        if token:
            run(["login", "ghcr.io", "--username", os.environ.get("GITHUB_ACTOR", "token"), "--password-stdin"],
                input_text=token + "\n")
        try:
            yield run
        finally:
            # The isolated directory (not the runner's shared Docker config) is removed.
            token = ""


def restore_refs(repo: str, event_name: str, event: dict) -> list[str]:
    refs = []
    if event_name == "pull_request":
        pr = event.get("pull_request") or {}
        head = pr.get("head") or {}
        if (head.get("repo") or {}).get("full_name", "").lower() == repo:
            refs.append(f"pr-{positive_integer(pr.get('number'), 'PR number')}")
    return [*refs, "main"]


def validate_restore_scope(manifest: dict, tag: str, repo: str) -> None:
    producer, publication = manifest["producer"], manifest.get("publication") or {}
    expected = {"repository": repo, "run_id": producer["run_id"], "run_attempt": producer["run_attempt"],
                "event": producer["event"], "head_sha": producer["head_sha"], "workflow_path": WORKFLOW}
    if any(publication.get(key) != value for key, value in expected.items()):
        raise CacheError("cache lacks trusted publisher provenance")
    if producer["head_repository"] != repo:
        raise CacheError("fork cache cannot be restored")
    if tag == "main":
        if (producer["event"] != "push" or producer.get("ref") != "refs/heads/main"
                or publication.get("head_branch") != "main" or publication.get("conclusion") != "success"):
            raise CacheError("main cache is not from successful main checks")
    elif (producer["event"] != "pull_request" or tag != f"pr-{producer.get('pr_number')}"):
        raise CacheError("cache belongs to a different PR")


def destination_is_empty(destination: Path) -> bool:
    return (not destination.is_symlink() and (not destination.exists() or
            (destination.is_dir() and all(p.name == ".gitignore" and p.is_file() and not p.is_symlink()
                                         for p in destination.iterdir()))))


def registry_descriptors(value: dict, expected_files: dict[str, int]) -> dict:
    descriptors = value.get("layers") or value.get("blobs") or []
    if not isinstance(descriptors, list) or len(descriptors) != len(expected_files):
        raise CacheError("unexpected OCI bundle inventory")
    indexed = {}
    for entry in descriptors:
        if not isinstance(entry, dict):
            raise CacheError("invalid OCI descriptor")
        name = (entry.get("annotations") or {}).get("org.opencontainers.image.title")
        if (name not in expected_files or name in indexed
                or not re.fullmatch(r"sha256:[a-f0-9]{64}", str(entry.get("digest")))
                or not 0 < positive_integer(entry.get("size"), "OCI blob size") <= expected_files[name]):
            raise CacheError("unsafe OCI blob descriptor")
        indexed[name] = entry
    return indexed


def fetch_registry_bundle(run_oras, image: str, digest: str, destination: Path) -> None:
    raw = run_oras(["manifest", "fetch", f"{image}@{digest}"])
    if len(raw.encode()) > MAX_JSON_BYTES:
        raise CacheError("OCI manifest exceeds limit")
    try:
        descriptors = registry_descriptors(json.loads(raw), {ARCHIVE: MAX_ARCHIVE_BYTES, MANIFEST: MAX_JSON_BYTES})
    except (json.JSONDecodeError, AttributeError) as exc:
        raise CacheError("invalid OCI manifest") from exc
    # Explicit output paths: never let an OCI title choose a filesystem path.
    for name, descriptor in descriptors.items():
        output = destination / name
        run_oras(["blob", "fetch", "--output", str(output.absolute()), f"{image}@{descriptor['digest']}"])
        content_digest, size = file_digest(output, descriptor["size"])
        if "sha256:" + content_digest != descriptor["digest"] or size != descriptor["size"]:
            raise CacheError("OCI blob digest or size mismatch")


def restore(destination: Path, oras: str, root: Path) -> bool:
    if not destination_is_empty(destination):
        raise CacheError("refusing to overlay an existing compiler cache")
    repo = repository(os.environ.get("GITHUB_REPOSITORY"))
    event_path = os.environ.get("GITHUB_EVENT_PATH")
    event = read_json(Path(event_path), 5 * 1024**2) if event_path else {}
    tags = restore_refs(repo, os.environ.get("GITHUB_EVENT_NAME", ""), event)
    expected = compatibility(root)
    image = f"ghcr.io/{repo}-ci-cache"
    destination.parent.mkdir(parents=True, exist_ok=True)
    with registry_session(oras) as run:
        for tag in tags:
            with tempfile.TemporaryDirectory(prefix=".cargo-restore-", dir=destination.parent) as temporary:
                stage = Path(temporary)
                bundle, unpacked = stage / "bundle", stage / "cache"
                bundle.mkdir()
                unpacked.mkdir()
                try:
                    # Resolve a mutable alias once, then fetch exactly that immutable object.
                    digest = run(["resolve", f"{image}:{tag}"])
                    if not re.fullmatch(r"sha256:[a-f0-9]{64}", digest):
                        raise CacheError("registry did not resolve an immutable digest")
                    fetch_registry_bundle(run, image, digest, bundle)
                    manifest = validate_bundle(bundle, repo, expected)
                    validate_restore_scope(manifest, tag, repo)
                    with tempfile.TemporaryFile() as errors:
                        process = subprocess.Popen(["zstd", "-q", "-dc", str(bundle / ARCHIVE)],
                                                   stdout=subprocess.PIPE, stderr=errors)
                        try:
                            with process.stdout:
                                inventory = unpack_tar(process.stdout, unpacked)
                            if process.wait(timeout=1200):
                                raise CacheError("cache decompression failed")
                        finally:
                            if process.poll() is None:
                                process.kill()
                                process.wait()
                    if any(inventory[key] != manifest["archive"][key] for key in inventory):
                        raise CacheError("expanded cache inventory mismatch")
                    # An empty tracked seed placeholder is the only thing replaced.
                    if not destination_is_empty(destination):
                        raise CacheError("cache destination changed during restoration")
                    if destination.exists():
                        placeholder = destination / ".gitignore"
                        placeholder.unlink(missing_ok=True)
                        destination.rmdir()
                    unpacked.rename(destination)
                    print(f"Restored compiler cache {image}@{digest} ({tag}); required checks still run")
                    return True
                except (CacheError, OSError, tarfile.TarError, ValueError) as exc:
                    print(f"Compiler cache {tag} unavailable: {exc}; trying next seed")
    print("No compatible compiler cache; required checks will build cold")
    return False


def github_json(path: str) -> object:
    try:
        return json.loads(command(["gh", "api", "--method", "GET", path]))
    except json.JSONDecodeError as exc:
        raise CacheError("GitHub returned malformed metadata") from exc


def validated_run(event: dict, repo: str, api=github_json) -> dict:
    candidate = event.get("workflow_run") or {}
    run_id = positive_integer(candidate.get("id"), "workflow run ID")
    attempt = positive_integer(candidate.get("run_attempt"), "workflow run attempt")
    run = api(f"repos/{repo}/actions/runs/{run_id}/attempts/{attempt}")
    if (not isinstance(run, dict) or run.get("id") != run_id or run.get("run_attempt") != attempt
            or run.get("name") != "Checks" or run.get("path") != WORKFLOW
            or run.get("status") != "completed"
            or not isinstance(run.get("head_branch"), str) or not run["head_branch"]
            or (run.get("repository") or {}).get("full_name", "").lower() != repo
            or (run.get("head_repository") or {}).get("full_name", "").lower() != repo
            or run.get("event") not in ("pull_request", "push")
            or run.get("head_sha") != candidate.get("head_sha")):
        raise CacheError("producer is not an eligible completed same-repository Checks run")
    sha(run.get("head_sha"), "workflow head")
    if run["event"] == "push" and run.get("head_branch") != "main":
        raise CacheError("only main push runs can publish branch caches")
    return run


def validate_producer(manifest: dict, run: dict, repo: str, api=github_json) -> dict:
    producer = manifest["producer"]
    expected = {"repository": repo, "run_id": run["id"], "run_attempt": run["run_attempt"],
                "event": run["event"], "head_sha": run["head_sha"], "head_repository": repo}
    if any(producer.get(key) != value for key, value in expected.items()):
        raise CacheError("cache producer identity differs from the immutable workflow run")
    if run["event"] == "push":
        if producer.get("ref") != "refs/heads/main" or producer["tested_sha"] != run["head_sha"]:
            raise CacheError("main cache does not identify the tested main commit")
    else:
        number = positive_integer(producer.get("pr_number"), "producer PR")
        linked = run.get("pull_requests") or []
        if linked:
            if (len(linked) != 1 or linked[0].get("number") != number
                    or (linked[0].get("head") or {}).get("sha") != producer["head_sha"]
                    or (linked[0].get("base") or {}).get("sha") != producer["base_sha"]):
                raise CacheError("cache PR association differs from its workflow run")
        else:
            # Some rerun payloads omit pull_requests. Recover association through
            # the run's exact head, never a PR number provided only by an artifact.
            associated = api(f"repos/{repo}/commits/{run['head_sha']}/pulls?per_page=100")
            if not isinstance(associated, list) or len(associated) >= 100:
                raise CacheError("PR association listing is unavailable or truncated")
            matches = [pr for pr in associated if pr.get("number") == number
                       and (pr.get("base", {}).get("repo") or {}).get("full_name", "").lower() == repo
                       and (pr.get("head", {}).get("repo") or {}).get("full_name", "").lower() == repo]
            if len(matches) != 1:
                raise CacheError("cache PR could not be tied to the producing commit")
    commit = api(f"repos/{repo}/git/commits/{producer['tested_sha']}")
    if (not isinstance(commit, dict) or commit.get("sha") != producer["tested_sha"]
            or (commit.get("tree") or {}).get("sha") != producer["tested_tree"]):
        raise CacheError("cache source tree differs from the tested commit")
    if run["event"] == "pull_request":
        parents = [parent.get("sha") for parent in commit.get("parents", [])]
        if parents != [producer["base_sha"], producer["head_sha"]]:
            raise CacheError("cache does not identify the PR's tested merge candidate")
    return {**expected, "workflow_path": WORKFLOW, "head_branch": run["head_branch"],
            "conclusion": run.get("conclusion"), "pr_number": producer.get("pr_number")}


def artifact_download(repo: str, run: dict, name: str, output: Path,
                      expected_files: dict[str, int], api=github_json) -> None:
    listed = api(f"repos/{repo}/actions/runs/{run['id']}/artifacts?per_page=100")
    if not isinstance(listed, dict) or listed.get("total_count", 0) > 100:
        raise CacheError("artifact listing is unavailable or truncated")
    found = [item for item in listed.get("artifacts", []) if item.get("name") == name and not item.get("expired")]
    if len(found) != 1:
        raise CacheError(f"exact artifact {name} is missing or ambiguous")
    item = found[0]
    artifact_id = positive_integer(item.get("id"), "artifact ID")
    expected_digest = item.get("digest", "")
    limit = sum(expected_files.values()) + 10 * 1024**2
    if (not re.fullmatch(r"sha256:[a-f0-9]{64}", expected_digest)
            or not 0 < positive_integer(item.get("size_in_bytes"), "artifact size") <= limit):
        raise CacheError("artifact has no bounded immutable checksum")
    with tempfile.TemporaryDirectory(prefix="chat-hub-artifact-") as temporary:
        zipped = Path(temporary) / "artifact.zip"
        with zipped.open("xb") as target:
            with tempfile.TemporaryFile() as errors:
                process = subprocess.Popen(["gh", "api", "--method", "GET",
                                            f"repos/{repo}/actions/artifacts/{artifact_id}/zip"],
                                           stdout=subprocess.PIPE, stderr=errors)
                size = 0
                try:
                    with process.stdout:
                        while chunk := process.stdout.read(1024 * 1024):
                            size += len(chunk)
                            if size > limit:
                                raise CacheError("workflow artifact exceeds download limit")
                            target.write(chunk)
                    if process.wait(timeout=1200):
                        raise CacheError("workflow artifact download failed")
                finally:
                    if process.poll() is None:
                        process.kill()
                        process.wait()
        digest, _ = file_digest(zipped, limit)
        if "sha256:" + digest != expected_digest:
            raise CacheError("GitHub artifact checksum mismatch")
        output.mkdir()
        with zipfile.ZipFile(zipped) as archive:
            entries = archive.infolist()
            if len(entries) != len(expected_files) or {entry.filename for entry in entries} != set(expected_files):
                raise CacheError("unexpected workflow artifact inventory")
            for entry in entries:
                mode = entry.external_attr >> 16
                if (entry.flag_bits & 1 or stat.S_IFMT(mode) not in (0, stat.S_IFREG)
                        or entry.file_size > expected_files[entry.filename]):
                    raise CacheError("unsafe workflow artifact member")
                with archive.open(entry) as source, (output / entry.filename).open("xb") as target:
                    shutil.copyfileobj(source, target, length=1024 * 1024)


def manifest_descriptor_matches(manifest: dict, files: dict[str, tuple[str, int]]) -> bool:
    descriptors = manifest.get("layers") or manifest.get("blobs") or []
    if len(descriptors) != len(files):
        return False
    indexed = {entry.get("annotations", {}).get("org.opencontainers.image.title"): entry
               for entry in descriptors}
    return set(indexed) == set(files) and all(
        indexed[name].get("digest") == "sha256:" + digest and indexed[name].get("size") == size
        for name, (digest, size) in files.items())


def publish_bundle(run_oras, image: str, tag: str, directory: Path,
                   media_types: dict[str, str], repo: str, revision: str) -> str:
    files = {name: file_digest(directory / name) for name in media_types}
    ref = f"{image}:{tag}"
    try:
        existing = json.loads(run_oras(["manifest", "fetch", ref]))
    except CacheError:
        existing = None
    except json.JSONDecodeError as exc:
        raise CacheError("registry manifest is malformed") from exc
    if existing is not None:
        if not manifest_descriptor_matches(existing, files):
            raise CacheError("immutable run tag already exists with different artifact bytes")
    else:
        run_oras(["push", "--artifact-type", "application/vnd.chat-hub.ci.v1",
                  "--annotation", f"org.opencontainers.image.source=https://github.com/{repo}",
                  "--annotation", f"org.opencontainers.image.revision={revision}", ref,
                  *[f"{name}:{media}" for name, media in media_types.items()]], cwd=directory)
    digest = run_oras(["resolve", ref])
    if not re.fullmatch(r"sha256:[a-f0-9]{64}", digest):
        raise CacheError("published artifact has no immutable digest")
    # Source association is create-once; exact run/attempt tags are always retained.
    source_tag = f"sha-{revision}"
    try:
        run_oras(["resolve", f"{image}:{source_tag}"])
    except CacheError:
        run_oras(["tag", f"{image}@{digest}", source_tag])
    return digest


def moving_tag(run: dict, producer: dict) -> str | None:
    if run["event"] == "pull_request":
        return f"pr-{positive_integer(producer.get('pr_number'), 'producer PR')}"
    if run["event"] == "push" and run["head_branch"] == "main" and run.get("conclusion") == "success":
        return "main"
    return None


def validate_evidence(record: dict, run: dict, manifest: dict) -> None:
    if run.get("conclusion") != "success" or run["event"] != "pull_request":
        raise CacheError("only successful PR checks may publish check evidence")
    producer = manifest["producer"]
    expected = {"schema": 1, "repository": producer["repository"], "workflow_path": WORKFLOW,
                "run_id": run["id"], "run_attempt": run["run_attempt"], "event": "pull_request",
                "head_sha": producer["head_sha"], "base_sha": producer["base_sha"],
                "pr_number": producer["pr_number"], "tested_tree": producer["tested_tree"]}
    if any(record.get(key) != value for key, value in expected.items()):
        raise CacheError("check receipt differs from the exact successful candidate")
    sha(record.get("workflow_sha"), "receipt workflow SHA")
    sha(record.get("helper_sha"), "receipt helper SHA")


def publish(event_path: Path, oras: str) -> None:
    repo = repository(os.environ.get("GITHUB_REPOSITORY"))
    if os.environ.get("GITHUB_EVENT_NAME") != "workflow_run":
        raise CacheError("package publication is restricted to trusted workflow_run jobs")
    event = read_json(event_path, 5 * 1024**2)
    run = validated_run(event, repo)
    with tempfile.TemporaryDirectory(prefix="chat-hub-publish-") as temporary:
        stage = Path(temporary)
        bundle = stage / "cache"
        artifact_name = f"chat-hub-cargo-cache-{run['id']}-{run['run_attempt']}"
        artifact_download(repo, run, artifact_name, bundle,
                          {ARCHIVE: MAX_ARCHIVE_BYTES, MANIFEST: MAX_JSON_BYTES})
        manifest = validate_bundle(bundle, repo)
        manifest["publication"] = validate_producer(manifest, run, repo)
        write_json(bundle / MANIFEST, manifest)
        immutable = f"run-{run['id']}-{run['run_attempt']}"
        revision = manifest["producer"]["tested_sha"]
        with registry_session(oras) as run_oras:
            image = f"ghcr.io/{repo}-ci-cache"
            digest = publish_bundle(run_oras, image, immutable, bundle,
                                    {ARCHIVE: "application/vnd.chat-hub.cargo-cache.v1.tar+zstd",
                                     MANIFEST: "application/json"}, repo, revision)
            alias = moving_tag(run, manifest["producer"])
            if alias:
                run_oras(["tag", f"{image}@{digest}", alias])
            print(f"Retained compiler cache {image}@{digest}; producer conclusion: {run.get('conclusion')}; not check evidence")
            if run.get("conclusion") == "success":
                # Dagger's full snapshot is still the atomic Cargo restore unit.
                # Publish the real compiled outputs for each workspace member
                # separately, without running untrusted PR code or unpacking
                # any downloaded compiler objects in this privileged job.
                import crate_artifacts

                for crate in crate_artifacts.crates(Path.cwd()):
                    crate_bundle = stage / f"crate-{crate}"
                    artifact_download(
                        repo, run, f"chat-hub-crate-{crate}-{run['id']}-{run['run_attempt']}",
                        crate_bundle,
                        {crate_artifacts.ARCHIVE: crate_artifacts.MAX_CRATE_BYTES,
                         crate_artifacts.MANIFEST: MAX_JSON_BYTES},
                    )
                    crate_artifacts.validate_bundle(crate_bundle, crate, manifest, repo)
                    crate_image = f"ghcr.io/{repo}-crate-{crate}"
                    crate_digest = publish_bundle(
                        run_oras, crate_image, immutable, crate_bundle,
                        {crate_artifacts.ARCHIVE: "application/vnd.chat-hub.rust-crate-objects.v1.tar+zstd",
                         crate_artifacts.MANIFEST: "application/json"},
                        repo, revision,
                    )
                    if alias:
                        run_oras(["tag", f"{crate_image}@{crate_digest}", alias])
                    print(f"Retained {crate} compiled objects at {crate_image}@{crate_digest}")
            if run.get("conclusion") == "success" and run["event"] == "pull_request":
                try:
                    evidence = stage / "evidence"
                    artifact_download(repo, run, f"chat-hub-check-receipt-{run['id']}-{run['run_attempt']}",
                                      evidence, {CHECK_RECEIPT: MAX_JSON_BYTES})
                    validate_evidence(read_json(evidence / CHECK_RECEIPT), run, manifest)
                    evidence_image = f"ghcr.io/{repo}-check-evidence"
                    evidence_digest = publish_bundle(run_oras, evidence_image, immutable, evidence,
                                                    {CHECK_RECEIPT: "application/json"}, repo, revision)
                    print(f"Retained exact check receipt {evidence_image}@{evidence_digest}; release qualification remains separate")
                except (CacheError, OSError, zipfile.BadZipFile) as exc:
                    print(f"Check evidence not published: {exc}; compiler cache remains retained")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="operation", required=True)
    packing = sub.add_parser("pack")
    packing.add_argument("--source", required=True, type=Path)
    packing.add_argument("--output", required=True, type=Path)
    packing.add_argument("--root", type=Path, default=Path.cwd())
    restoring = sub.add_parser("restore")
    restoring.add_argument("--destination", required=True, type=Path)
    restoring.add_argument("--oras", required=True)
    restoring.add_argument("--root", type=Path, default=Path.cwd())
    publishing = sub.add_parser("publish")
    publishing.add_argument("--event", required=True, type=Path)
    publishing.add_argument("--oras", required=True)
    args = parser.parse_args(argv)
    try:
        if args.operation == "pack":
            pack(args.source, args.output, args.root)
        elif args.operation == "restore":
            restore(args.destination, args.oras, args.root)
        else:
            publish(args.event, args.oras)
    except (CacheError, OSError, tarfile.TarError, zipfile.BadZipFile) as exc:
        if args.operation == "restore":
            print(f"Compiler cache unavailable: {exc}; required checks will build cold")
            return 0
        print(f"Cargo cache {args.operation} failed: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
