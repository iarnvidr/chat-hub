#!/usr/bin/env python3
"""Node-free, bounded sccache transport for trusted Chat Hub CI builds."""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import os
import pathlib
import platform
import re
import shutil
import shlex
import stat
import subprocess
import sys
import tarfile
import tempfile
import time
import urllib.error
import urllib.request


SCHEMA = 1
SCCACHE_VERSION = "0.19.0"
MAX_INSTALL_BYTES = 80 * 1024 * 1024
MAX_BUNDLE_BYTES = 2 * 1024 * 1024 * 1024
MAX_CACHE_BYTES = 1536 * 1024 * 1024
MAX_CACHE_ENTRIES = 100_000
CACHE_SIZE = "1G"
MATRIX_ID = "native-macos-default-check-smoke-check-release-build-smoke-build-v1"
ORAS_VERSION = "1.2.3"
ORAS_LINUX_SHA256 = "b4efc97a91f471f323f193ea4b4d63d8ff443ca3aab514151a30751330852827"
ASSETS = {
    "macos-arm64": (
        "sccache-v0.19.0-aarch64-apple-darwin.tar.gz",
        "a35a0b7d622618774bc1bb8102d72562dfb21e619f88b153b370d7f89bc286af",
    ),
    "linux-x86_64": (
        "sccache-v0.19.0-x86_64-unknown-linux-musl.tar.gz",
        "aa73c7270c997894038c0d8f0d297865f57569e1687883322bc431713d4bb9dd",
    ),
}


class CacheError(Exception):
    pass


def digest_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def canonical_json(value: object) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode("utf-8")


def command_output(argv: list[str], *, cwd: pathlib.Path | None = None, timeout: int = 20) -> str:
    try:
        result = subprocess.run(argv, cwd=cwd, check=True, stdout=subprocess.PIPE,
                                stderr=subprocess.PIPE, text=True, timeout=timeout)
    except (OSError, subprocess.SubprocessError) as exc:
        raise CacheError("required toolchain metadata is unavailable") from exc
    return result.stdout.strip()


def compiler_metadata(root: pathlib.Path, platform_id: str) -> dict[str, str]:
    rustc = command_output(["rustc", "-Vv"], cwd=root)
    sdk = "not-applicable"
    xcode = "not-applicable"
    if platform.system() == "Darwin":
        sdk = command_output(["xcrun", "--sdk", "macosx", "--show-sdk-version"], cwd=root)
        xcode = command_output(["xcodebuild", "-version"], cwd=root)
    return {
        "platform_id": platform_id,
        "host_system": platform.system().lower(),
        "host_machine": platform.machine().lower(),
        "host_release": platform.release(),
        "rustc_sha256": digest_bytes(rustc.encode("utf-8")),
        "sdk": sdk,
        "xcode_sha256": digest_bytes(xcode.encode("utf-8")),
    }


def source_fingerprints(root: pathlib.Path) -> dict[str, str]:
    paths = ["Cargo.toml", "Cargo.lock", "src-tauri/Cargo.toml"]
    optional = [".cargo/config.toml", "src-tauri/build.rs"]
    hashes: dict[str, str] = {}
    for relative in paths + optional:
        path = root / relative
        if path.is_file():
            hashes[relative] = digest_bytes(path.read_bytes())
        elif relative in paths:
            raise CacheError("required Cargo manifest or lockfile is missing")
    return hashes


def contract_for(root: pathlib.Path, platform_id: str, target: str) -> dict[str, object]:
    if not re.fullmatch(r"[A-Za-z0-9_.-]{1,80}", platform_id):
        raise CacheError("invalid platform identifier")
    if not re.fullmatch(r"[A-Za-z0-9_.-]{1,80}", target):
        raise CacheError("invalid Rust target triple")
    compiler = compiler_metadata(root, platform_id)
    compatibility = {
        "schema": SCHEMA,
        "sccache_version": SCCACHE_VERSION,
        "platform_id": platform_id,
        "host_system": compiler["host_system"],
        "host_machine": compiler["host_machine"],
        "target": target,
        "sdk": compiler["sdk"],
        "xcode_sha256": compiler["xcode_sha256"],
    }
    # Keep changing project inputs as provenance only. sccache keys each object
    # by rustc, source, args, environment and cfg/features, so these must not
    # invalidate the entire shared crate cache.
    provenance = {
        "rustc_sha256": compiler["rustc_sha256"],
        "cargo_inputs": source_fingerprints(root),
        "invocation_matrix": MATRIX_ID,
        "cache_key_model": "sccache-rustc-arguments-source-environment-v1",
    }
    return {"compatibility": compatibility, "provenance": provenance}


def contract_key(contract: dict[str, object]) -> str:
    return digest_bytes(canonical_json(contract["compatibility"]))


def registry_image(repository: str) -> str:
    if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repository):
        raise CacheError("invalid repository identifier")
    owner, name = repository.lower().split("/", 1)
    return f"ghcr.io/{owner}/chat-hub-rust-cache-{name}"


def cache_tag(contract: dict[str, object]) -> str:
    compiler = contract["compiler"]
    platform_id = re.sub(r"[^a-z0-9-]+", "-", compiler["platform_id"].lower()).strip("-")
    target = re.sub(r"[^a-z0-9-]+", "-", str(contract["target"]).lower()).strip("-")
    return f"cargo-sccache-v{SCHEMA}-{platform_id}-{target}"


def safe_append_env(path_text: str, values: dict[str, str]) -> None:
    path = pathlib.Path(path_text)
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a", encoding="utf-8") as stream:
        for key, value in values.items():
            if not re.fullmatch(r"[A-Z_][A-Z0-9_]*", key) or "\n" in value or "\r" in value:
                raise CacheError("unsafe environment value")
            stream.write(f"{key}={value}\n")


def write_summary(path_text: str | None, outcome: str, contract: dict[str, object] | None) -> None:
    if not path_text:
        print(f"Rust compile cache: {outcome}")
        return
    key = contract_key(contract)[:16] if contract else "unavailable"
    path = pathlib.Path(path_text)
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a", encoding="utf-8") as stream:
        stream.write("\n### Rust compile cache\n\n")
        stream.write(f"- Status: {outcome}\n- Contract: `{key}`\n")


def install_sccache(platform_name: str, destination: pathlib.Path) -> pathlib.Path:
    if platform_name not in ASSETS:
        raise CacheError("unsupported sccache platform")
    filename, expected_sha = ASSETS[platform_name]
    url = f"https://github.com/mozilla/sccache/releases/download/v{SCCACHE_VERSION}/{filename}"
    destination.mkdir(parents=True, exist_ok=True)
    archive_path = destination / filename
    hasher = hashlib.sha256()
    size = 0
    request = urllib.request.Request(url, headers={"User-Agent": "chat-hub-compiled-cache"})
    try:
        with urllib.request.urlopen(request, timeout=45) as response, archive_path.open("wb") as out:
            while True:
                chunk = response.read(1024 * 1024)
                if not chunk:
                    break
                size += len(chunk)
                if size > MAX_INSTALL_BYTES:
                    raise CacheError("sccache release archive exceeds limit")
                hasher.update(chunk)
                out.write(chunk)
    except (OSError, urllib.error.URLError, CacheError) as exc:
        archive_path.unlink(missing_ok=True)
        raise CacheError("pinned sccache download failed") from exc
    if hasher.hexdigest() != expected_sha:
        archive_path.unlink(missing_ok=True)
        raise CacheError("pinned sccache digest mismatch")
    binary = destination / "sccache"
    try:
        with tarfile.open(archive_path, "r:gz") as archive:
            matches = [member for member in archive.getmembers()
                       if pathlib.PurePosixPath(member.name).name == "sccache"]
            if len(matches) != 1 or not matches[0].isreg() or matches[0].size > 100 * 1024 * 1024:
                raise CacheError("unexpected sccache release archive")
            source = archive.extractfile(matches[0])
            if source is None:
                raise CacheError("sccache binary missing from release archive")
            with source, binary.open("wb") as out:
                shutil.copyfileobj(source, out)
    except (OSError, tarfile.TarError) as exc:
        raise CacheError("cannot read pinned sccache release archive") from exc
    binary.chmod(0o755)
    archive_path.unlink(missing_ok=True)
    version = command_output([str(binary), "--version"], timeout=15)
    if SCCACHE_VERSION not in version:
        binary.unlink(missing_ok=True)
        raise CacheError("installed sccache version does not match pin")
    return binary


def install_linux_oras(destination: pathlib.Path) -> pathlib.Path:
    filename = f"oras_{ORAS_VERSION}_linux_amd64.tar.gz"
    url = f"https://github.com/oras-project/oras/releases/download/v{ORAS_VERSION}/{filename}"
    destination.mkdir(parents=True, exist_ok=True)
    archive_path = destination / filename
    hasher = hashlib.sha256()
    size = 0
    request = urllib.request.Request(url, headers={"User-Agent": "chat-hub-compiled-cache"})
    try:
        with urllib.request.urlopen(request, timeout=45) as response, archive_path.open("wb") as out:
            while True:
                chunk = response.read(1024 * 1024)
                if not chunk:
                    break
                size += len(chunk)
                if size > MAX_INSTALL_BYTES:
                    raise CacheError("pinned ORAS archive exceeds limit")
                hasher.update(chunk)
                out.write(chunk)
    except (OSError, urllib.error.URLError, CacheError) as exc:
        archive_path.unlink(missing_ok=True)
        raise CacheError("pinned ORAS download failed") from exc
    if hasher.hexdigest() != ORAS_LINUX_SHA256:
        archive_path.unlink(missing_ok=True)
        raise CacheError("pinned ORAS digest mismatch")
    binary = destination / "oras"
    try:
        with tarfile.open(archive_path, "r:gz") as archive:
            matches = [member for member in archive.getmembers() if member.name == "oras"]
            if len(matches) != 1 or not matches[0].isreg() or matches[0].size > 100 * 1024 * 1024:
                raise CacheError("unexpected ORAS release archive")
            source = archive.extractfile(matches[0])
            if source is None:
                raise CacheError("ORAS binary missing from pinned release")
            with source, binary.open("wb") as out:
                shutil.copyfileobj(source, out)
    except (OSError, tarfile.TarError) as exc:
        raise CacheError("cannot read pinned ORAS release archive") from exc
    binary.chmod(0o755)
    archive_path.unlink(missing_ok=True)
    if ORAS_VERSION not in command_output([str(binary), "version"], timeout=15):
        binary.unlink(missing_ok=True)
        raise CacheError("installed ORAS version does not match pin")
    return binary


def set_up(args: argparse.Namespace) -> None:
    root = pathlib.Path(args.root).resolve()
    state_dir = pathlib.Path(args.state_dir).resolve()
    cache_dir = pathlib.Path(args.cache_dir).resolve()
    state_dir.mkdir(parents=True, exist_ok=True, mode=0o700)
    binary_dir = state_dir / "bin"
    binary_dir.mkdir(parents=True, exist_ok=True, mode=0o700)
    binary_path = binary_dir / "sccache"
    wrapper_path = binary_dir / "rustc-cache-wrapper"
    marker_path = binary_dir / "sccache-enabled"
    marker_path.unlink(missing_ok=True)
    wrapper_path.write_text(
        "#!/bin/sh\n"
        f"if [ -x {shlex.quote(str(binary_path))} ] && [ -f {shlex.quote(str(marker_path))} ]; then\n"
        f"  exec {shlex.quote(str(binary_path))} \"$@\"\n"
        "fi\n"
        "exec \"$@\"\n", encoding="utf-8")
    wrapper_path.chmod(0o755)
    cache_dir.mkdir(parents=True, exist_ok=True, mode=0o700)
    state_file = state_dir / "state.json"
    state: dict[str, object] = {"enabled": False, "platform": args.platform,
                                "wrapper": str(wrapper_path), "cache_dir": str(cache_dir)}
    outcome = "disabled; full compile remains required"
    try:
        contract = contract_for(root, args.platform_id, args.target)
        binary = install_sccache(args.platform, binary_dir)
        probe = cache_dir / ".write-probe"
        probe.write_bytes(b"ok")
        probe.unlink()
        repository = args.repository or os.environ.get("GITHUB_REPOSITORY", "")
        state = {"enabled": True, "binary": str(binary), "wrapper": str(wrapper_path),
                 "cache_dir": str(cache_dir),
                 "contract": contract, "contract_key": contract_key(contract),
                 "tag": cache_tag(contract), "image": registry_image(repository),
                 "repository": repository.lower()}
        if args.platform == "linux-x86_64":
            state["oras"] = str(install_linux_oras(state_dir / "bin"))
        marker_path.write_text("enabled\n", encoding="ascii")
        marker_path.chmod(0o600)
        outcome = "ready; cache lookup follows"
    except (OSError, CacheError, subprocess.SubprocessError) as exc:
        marker_path.unlink(missing_ok=True)
        state["enabled"] = False
        state["reason"] = str(exc)[:120]
        outcome = "unavailable; running required compile without cache"
    env_file = args.github_env or os.environ.get("GITHUB_ENV")
    if env_file:
        try:
            safe_append_env(env_file, {
                "RUSTC_WRAPPER": str(wrapper_path),
                "SCCACHE_DIR": str(cache_dir),
                "SCCACHE_CACHE_SIZE": CACHE_SIZE,
                "CARGO_INCREMENTAL": "0",
            })
        except (OSError, CacheError) as exc:
            marker_path.unlink(missing_ok=True)
            state["enabled"] = False
            state["reason"] = str(exc)[:120]
            outcome = "unavailable; running required compile without cache"
    state_file.write_text(json.dumps(state, sort_keys=True) + "\n", encoding="utf-8")
    write_summary(args.summary, outcome, state.get("contract") if state.get("enabled") else None)


def read_state(state_dir: pathlib.Path) -> dict[str, object]:
    path = state_dir / "state.json"
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise CacheError("compile cache state is unavailable") from exc
    if not isinstance(value, dict):
        raise CacheError("compile cache state is malformed")
    return value


def run_oras(oras: str, args: list[str], *, token: str | None = None, config: pathlib.Path) -> str:
    env = dict(os.environ)
    env.pop("GH_TOKEN", None)
    env["DOCKER_CONFIG"] = str(config)
    if not args:
        raise CacheError("OCI cache operation is missing")
    args = [args[0], "--registry-config", str(config / "config.json"), *args[1:]]
    try:
        result = subprocess.run([oras, *args], input=(token + "\n") if token else None,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                                env=env, timeout=300, check=True)
    except (OSError, subprocess.SubprocessError) as exc:
        raise CacheError("OCI cache operation failed") from exc
    return result.stdout


def authenticate_and_call(oras: str, image: str, operation: list[str], summary: str | None) -> str:
    token = os.environ.get("GH_TOKEN", "")
    actor = os.environ.get("GITHUB_ACTOR", "")
    if not token or not actor:
        raise CacheError("registry credentials are unavailable")
    with tempfile.TemporaryDirectory(prefix="chat-hub-oras-") as temp:
        config = pathlib.Path(temp)
        run_oras(oras, ["login", "ghcr.io", "--username", actor, "--password-stdin"],
                 token=token, config=config)
        try:
            output = run_oras(oras, operation, config=config)
        finally:
            try:
                run_oras(oras, ["logout", "ghcr.io"], config=config)
            except CacheError:
                pass
    return output


def valid_member_path(name: str) -> pathlib.PurePosixPath:
    path = pathlib.PurePosixPath(name)
    raw_parts = name.rstrip("/").split("/")
    if (path.is_absolute() or any(part in {"", ".", ".."} for part in raw_parts) or
            "\\" in name or not path.parts):
        raise CacheError("unsafe cache archive path")
    return path


def restore_archive(archive_path: pathlib.Path, cache_dir: pathlib.Path,
                    expected_contract: dict[str, object], expected_key: str,
                    expected_repository: str, expected_writer_ref: str = "refs/heads/main") -> int:
    cache_dir.mkdir(parents=True, exist_ok=True, mode=0o700)
    seen: set[str] = set()
    total = 0
    count = 0
    with tarfile.open(archive_path, "r:gz") as archive:
        members = iter(archive)
        first = next(members, None)
        if first is None or first.name != "manifest.json" or not first.isreg() or first.size > 128 * 1024:
            raise CacheError("cache manifest must be first and bounded")
        manifest_source = archive.extractfile(first)
        if manifest_source is None:
            raise CacheError("cache manifest is unreadable")
        with manifest_source:
            manifest = json.loads(manifest_source.read(128 * 1024 + 1))
        if (not isinstance(manifest, dict) or manifest.get("schema") != SCHEMA or
                manifest.get("compatibility") != expected_contract.get("compatibility") or
                manifest.get("contract_key") != expected_key or
                manifest.get("repository") != expected_repository.lower() or
                manifest.get("writer_ref") != expected_writer_ref):
            raise CacheError("cache contract does not match this build")
        seen.add("manifest.json")
        for member in members:
            count += 1
            if count > MAX_CACHE_ENTRIES:
                raise CacheError("cache archive has too many entries")
            path = valid_member_path(member.name)
            if member.name in seen:
                raise CacheError("cache archive contains duplicate paths")
            seen.add(member.name)
            if member.name == "cache" and member.isdir():
                continue
            if not member.name.startswith("cache/") or not (member.isdir() or member.isreg()):
                raise CacheError("cache archive contains unsupported entry type")
            if member.isdir():
                relative = pathlib.PurePosixPath(*path.parts[1:])
                destination = cache_dir.joinpath(*relative.parts)
                destination.mkdir(parents=True, exist_ok=True, mode=0o700)
                continue
            total += member.size
            if member.size < 0 or total > MAX_CACHE_BYTES:
                raise CacheError("cache archive exceeds size limit")
            relative = pathlib.PurePosixPath(*path.parts[1:])
            destination = cache_dir.joinpath(*relative.parts)
            destination.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
            source = archive.extractfile(member)
            if source is None:
                raise CacheError("cache object is unreadable")
            copied = 0
            with source, destination.open("xb") as output:
                while True:
                    chunk = source.read(1024 * 1024)
                    if not chunk:
                        break
                    copied += len(chunk)
                    if copied > member.size:
                        raise CacheError("cache object exceeds declared size")
                    output.write(chunk)
                if copied != member.size:
                    raise CacheError("cache object size does not match archive")
            destination.chmod(0o600)
    return total


def pull_cache(args: argparse.Namespace) -> None:
    state_dir = pathlib.Path(args.state_dir)
    state = read_state(state_dir)
    contract = state.get("contract")
    outcome = "cache miss; compiling normally"
    if not state.get("enabled"):
        write_summary(args.summary, "cache disabled; compiling normally", None)
        return
    try:
        image = str(state["image"])
        tag = str(state["tag"])
        cache_dir = pathlib.Path(str(state["cache_dir"]))
        if any(cache_dir.iterdir()):
            write_summary(args.summary, "warm local sccache store reused", contract)
            return
        oras = args.oras or state.get("oras")
        if not oras:
            raise CacheError("pinned ORAS client is unavailable")
        with tempfile.TemporaryDirectory(prefix="chat-hub-cache-pull-") as temp:
            output = pathlib.Path(temp)
            authenticate_and_call(str(oras), image, ["pull", f"{image}:{tag}", "--output", str(output)], args.summary)
            archive_path = output / "chat-hub-sccache-cache.tar.gz"
            if not archive_path.is_file() or archive_path.is_symlink() or archive_path.stat().st_size > MAX_BUNDLE_BYTES:
                raise CacheError("cache bundle missing or exceeds size limit")
            with tempfile.TemporaryDirectory(prefix="chat-hub-cache-stage-") as stage:
                stage_dir = pathlib.Path(stage) / "cache"
                restored_bytes = restore_archive(archive_path, stage_dir, contract,
                                                 str(state["contract_key"]), str(state["repository"]))
                for root, dirs, files in os.walk(stage_dir, followlinks=False):
                    base = pathlib.Path(root)
                    for name in dirs:
                        source = base / name
                        if source.is_symlink():
                            raise CacheError("staged cache contains a symlink")
                        (cache_dir / source.relative_to(stage_dir)).mkdir(parents=True, exist_ok=True, mode=0o700)
                    for name in files:
                        source = base / name
                        if source.is_symlink() or not source.is_file():
                            raise CacheError("staged cache contains a non-regular file")
                        destination = cache_dir / source.relative_to(stage_dir)
                        if destination.exists():
                            if destination.is_symlink() or not destination.is_file():
                                raise CacheError("existing cache path is unsafe")
                            continue
                        destination.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
                        shutil.copyfile(source, destination)
                        destination.chmod(0o600)
        state["restored_bytes"] = restored_bytes
        state_dir.joinpath("state.json").write_text(json.dumps(state, sort_keys=True) + "\n", encoding="utf-8")
        outcome = f"restored ({restored_bytes} cache bytes)"
    except (OSError, CacheError, tarfile.TarError, json.JSONDecodeError, KeyError, TypeError):
        outcome = "cache miss or invalid bundle; compiling normally"
    write_summary(args.summary, outcome, contract if isinstance(contract, dict) else None)


def stats_text(binary: str) -> str:
    stats = stats_payload(binary)
    if stats is None:
        return "sccache statistics format unsupported; compilation proceeded"
    hits = stats["cache_hits"]["counts"].get("Rust", 0)
    misses = stats["cache_misses"]["counts"].get("Rust", 0)
    writes = stats.get("cache_writes", 0)
    requests = stats.get("compile_requests", 0)
    errors = stats.get("cache_errors", {}).get("counts", {}).get("Rust", 0)
    return f"Rust compile requests={requests}; cache hits={hits}; cache misses={misses}; cache writes={writes}; cache errors={errors}"


def stats_payload(binary: str) -> dict[str, object] | None:
    env = dict(os.environ)
    env.pop("GH_TOKEN", None)
    try:
        result = subprocess.run([binary, "--show-stats", "--stats-format", "json"],
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                                timeout=20, check=False, env=env)
    except (OSError, subprocess.SubprocessError):
        return None
    try:
        payload = json.loads(result.stdout)
        stats = payload["stats"]
        if not isinstance(stats["cache_hits"]["counts"], dict):
            return None
        return stats
    except (KeyError, TypeError, ValueError):
        return None


def rust_hits_from_stats(stats: dict[str, object]) -> int:
    try:
        count = stats["cache_hits"]["counts"].get("Rust", 0)
    except (KeyError, TypeError, AttributeError) as exc:
        raise CacheError("pinned sccache Rust hit counter is unavailable") from exc
    if not isinstance(count, int) or count < 0:
        raise CacheError("pinned sccache Rust hit counter is malformed")
    return count


def rust_hit_count(binary: str) -> int:
    stats = stats_payload(binary)
    if stats is None:
        raise CacheError("pinned sccache JSON statistics are unavailable")
    return rust_hits_from_stats(stats)


def warm_probe(args: argparse.Namespace) -> None:
    state = read_state(pathlib.Path(args.state_dir))
    if not state.get("enabled"):
        raise CacheError("sccache setup unavailable; compiler reuse could not be verified")
    binary = str(state["binary"])
    root = pathlib.Path(args.root).resolve()
    packages = ["chat-hub-model", "chat-hub-formats"]
    build = ["cargo", "build", "--locked", "-p", packages[0], "-p", packages[1]]
    clean = ["cargo", "clean", "-p", packages[0], "-p", packages[1]]
    for label, command in [("clean before probe", clean), ("first probe build", build)]:
        print(f"Rust cache probe: {label}")
        subprocess.run(command, cwd=root, check=True)
    before_second = rust_hit_count(binary)
    print("Rust cache probe: clean package outputs")
    subprocess.run(clean, cwd=root, check=True)
    print("Rust cache probe: rebuild same leaf crates")
    subprocess.run(build, cwd=root, check=True)
    after_second = rust_hit_count(binary)
    delta = after_second - before_second
    print(f"Rust cache probe counters: before_second={before_second}, after_second={after_second}, second_pass_hits={delta}")
    if delta < 1:
        raise CacheError("no Rust cache hit observed on the repeated leaf-crate build")
    if args.summary:
        with pathlib.Path(args.summary).open("a", encoding="utf-8") as stream:
            stream.write("\n### Rust compile cache warm-reuse probe\n\n")
            stream.write(f"- Rust cache hits on repeated build: `{delta}`\n")


def report(args: argparse.Namespace, *, stop: bool = False) -> None:
    state = read_state(pathlib.Path(args.state_dir))
    if not state.get("enabled"):
        write_summary(args.summary, "sccache unavailable; required compile ran without cache", None)
        return
    binary = str(state["binary"])
    text = stats_text(binary)
    if stop:
        env = dict(os.environ)
        env.pop("GH_TOKEN", None)
        subprocess.run([binary, "--stop-server"], stdout=subprocess.DEVNULL,
                       stderr=subprocess.DEVNULL, timeout=20, check=False, env=env)
    summary = pathlib.Path(args.summary) if args.summary else None
    if summary:
        with summary.open("a", encoding="utf-8") as stream:
            stream.write("\n### Rust compile cache statistics\n\n```text\n")
            stream.write(text + "\n```\n")
    print(text)


def create_bundle(state: dict[str, object], repository: str, destination: pathlib.Path) -> int:
    cache_dir = pathlib.Path(str(state["cache_dir"]))
    entries: list[pathlib.Path] = []
    total = 0
    for root, dirs, files in os.walk(cache_dir, followlinks=False):
        base = pathlib.Path(root)
        for name in dirs:
            path = base / name
            if path.is_symlink():
                raise CacheError("cache contains a symlink")
        for name in files:
            path = base / name
            mode = path.lstat().st_mode
            if not stat.S_ISREG(mode):
                raise CacheError("cache contains a non-regular file")
            total += path.stat().st_size
            if total > MAX_CACHE_BYTES:
                raise CacheError("cache exceeds configured size limit")
            entries.append(path)
            if len(entries) > MAX_CACHE_ENTRIES:
                raise CacheError("cache has too many entries")
    if not entries:
        raise CacheError("cache is empty")
    contract = state["contract"]
    manifest = {"schema": SCHEMA, "repository": repository.lower(),
                "compatibility": contract["compatibility"],
                "provenance": contract["provenance"],
                "contract_key": state["contract_key"],
                "created_run_id": os.environ.get("GITHUB_RUN_ID", ""),
                "created_run_attempt": os.environ.get("GITHUB_RUN_ATTEMPT", ""),
                "writer_ref": os.environ.get("GITHUB_REF", "")}
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tarfile.open(destination, "w:gz", compresslevel=1) as bundle:
        encoded = canonical_json(manifest) + b"\n"
        info = tarfile.TarInfo("manifest.json")
        info.size = len(encoded)
        info.mode = 0o600
        info.mtime = 0
        bundle.addfile(info, io.BytesIO(encoded))
        cache_info = tarfile.TarInfo("cache")
        cache_info.type = tarfile.DIRTYPE
        cache_info.mode = 0o700
        cache_info.mtime = 0
        bundle.addfile(cache_info)
        for path in entries:
            relative = path.relative_to(cache_dir).as_posix()
            info = tarfile.TarInfo("cache/" + relative)
            info.size = path.stat().st_size
            info.mode = 0o600
            info.mtime = 0
            with path.open("rb") as source:
                bundle.addfile(info, source)
    if destination.stat().st_size > MAX_BUNDLE_BYTES:
        destination.unlink(missing_ok=True)
        raise CacheError("compressed cache bundle exceeds size limit")
    return total


def trusted_main_writer() -> bool:
    return (os.environ.get("GITHUB_REF") == "refs/heads/main" and
            os.environ.get("GITHUB_EVENT_NAME") in {"push", "workflow_dispatch"} and
            bool(os.environ.get("GITHUB_RUN_ID")))


def publish_cache(args: argparse.Namespace) -> None:
    state_dir = pathlib.Path(args.state_dir)
    state = read_state(state_dir)
    contract = state.get("contract")
    if not state.get("enabled"):
        write_summary(args.summary, "cache not published; sccache unavailable", None)
        return
    if not trusted_main_writer():
        write_summary(args.summary, "cache publish refused outside protected main workflow", contract)
        return
    repository = os.environ.get("GITHUB_REPOSITORY", "")
    if not repository or repository.lower() != state.get("repository"):
        write_summary(args.summary, "cache publish refused for repository mismatch", contract)
        return
    try:
        # Capture hit/miss evidence before stopping the daemon and sealing its store.
        report(args, stop=True)
        with tempfile.TemporaryDirectory(prefix="chat-hub-cache-publish-") as temp:
            bundle = pathlib.Path(temp) / "chat-hub-sccache-cache.tar.gz"
            cache_bytes = create_bundle(state, repository, bundle)
            image = str(state["image"])
            tag = str(state["tag"])
            output = authenticate_and_call(args.oras, image,
                ["push", f"{image}:{tag}", "--artifact-type", "application/vnd.chat-hub.sccache-cache.v1",
                 "--annotation", f"org.opencontainers.image.revision={os.environ.get('GITHUB_SHA','')}",
                 "chat-hub-sccache-cache.tar.gz:application/vnd.chat-hub.sccache-cache.v1.tar+gzip",
                 "--format", "json"],
                args.summary)
            digest_match = re.search(r"sha256:[a-f0-9]{64}", output)
            outcome = f"published trusted-main cache ({cache_bytes} bytes; {digest_match.group(0) if digest_match else 'digest recorded by registry'})"
    except (OSError, CacheError, tarfile.TarError, KeyError, TypeError):
        outcome = "cache publish failed; required native build result remains unchanged"
    write_summary(args.summary, outcome, contract if isinstance(contract, dict) else None)


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    commands = result.add_subparsers(dest="command", required=True)
    prepare = commands.add_parser("prepare")
    prepare.add_argument("--root", required=True)
    prepare.add_argument("--platform", choices=sorted(ASSETS), required=True)
    prepare.add_argument("--platform-id", required=True)
    prepare.add_argument("--target", required=True)
    prepare.add_argument("--cache-dir", required=True)
    prepare.add_argument("--state-dir", required=True)
    prepare.add_argument("--github-env")
    prepare.add_argument("--repository")
    prepare.add_argument("--summary")
    pull = commands.add_parser("pull")
    pull.add_argument("--state-dir", required=True)
    pull.add_argument("--oras", default="")
    pull.add_argument("--summary")
    stats = commands.add_parser("report")
    stats.add_argument("--state-dir", required=True)
    stats.add_argument("--summary")
    publish = commands.add_parser("publish")
    publish.add_argument("--state-dir", required=True)
    publish.add_argument("--oras", default="")
    publish.add_argument("--summary")
    probe = commands.add_parser("warm-probe")
    probe.add_argument("--state-dir", required=True)
    probe.add_argument("--root", required=True)
    probe.add_argument("--summary")
    contract = commands.add_parser("contract")
    contract.add_argument("--root", required=True)
    contract.add_argument("--platform-id", required=True)
    contract.add_argument("--target", required=True)
    return result


def main() -> int:
    args = parser().parse_args()
    try:
        if args.command == "prepare":
            set_up(args)
        elif args.command == "pull":
            pull_cache(args)
        elif args.command == "report":
            report(args)
        elif args.command == "publish":
            publish_cache(args)
        elif args.command == "warm-probe":
            warm_probe(args)
        elif args.command == "contract":
            print(json.dumps(contract_for(pathlib.Path(args.root).resolve(), args.platform_id,
                                          args.target), sort_keys=True))
        return 0
    except CacheError as exc:
        print(f"Rust compile cache skipped: {exc}")
        return 1 if args.command == "warm-probe" else 0


if __name__ == "__main__":
    raise SystemExit(main())
