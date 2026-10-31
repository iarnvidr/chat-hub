#!/usr/bin/env python3
"""Run the installed Paseo ChatGPT history sync and spool faithful raw graphs.

This bridge deliberately delegates OAuth, paging, project discovery, retries,
batching, and normalized cache updates to the configured history plugin. It
wraps only ``render`` to retain each full conversation graph in a separate,
private SQLite spool for Chat Hub.
"""

from __future__ import annotations

import argparse
import copy
import fcntl
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import stat
import sys
import tempfile
from typing import Any, Iterator


OWNER_RE = re.compile(r"^[0-9a-f]{24}$")
MAX_EXPORT_BYTES = 60 * 1024 * 1024
MAX_EXPORT_CONVERSATIONS = 1000
EXPORT_FORMAT = "chat-hub-paseo-history-v1"


class BridgeError(Exception):
    """Safe bridge failures; never include credentials or conversation content."""


def _resolved(path: str | Path) -> Path:
    return Path(path).expanduser().resolve(strict=False)


def _ensure_private_dir(path: Path) -> None:
    if path.is_symlink():
        raise BridgeError("refusing a symlink for private state")
    try:
        old_umask = os.umask(0o077)
        try:
            path.mkdir(parents=True, exist_ok=True, mode=0o700)
        finally:
            os.umask(old_umask)
        if not path.is_dir():
            raise BridgeError("private state path is not a directory")
        os.chmod(path, 0o700)
    except OSError:
        raise BridgeError("unable to prepare private state") from None


def _canonical_refresh_lock(shared_home: Path, owner: str) -> Path:
    if not OWNER_RE.fullmatch(owner):
        raise BridgeError("Paseo returned an invalid account owner hash")
    owner_root = shared_home / "chatgpt-history" / owner
    try:
        _ensure_private_dir(owner_root)
    except BridgeError:
        raise
    lock_path = owner_root / "refresh.lock"
    if lock_path.is_symlink():
        raise BridgeError("refusing a symlink for the shared refresh lock")
    flags = os.O_CREAT | os.O_RDWR
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    try:
        fd = os.open(lock_path, flags, 0o600)
        try:
            if not stat.S_ISREG(os.fstat(fd).st_mode):
                raise BridgeError("shared refresh lock is not a regular file")
            os.fchmod(fd, 0o600)
        finally:
            os.close(fd)
    except BridgeError:
        raise
    except OSError:
        raise BridgeError("unable to prepare the shared refresh lock") from None
    return lock_path.resolve(strict=True)


def _link_private_refresh_lock(private_root: Path, canonical_lock: Path) -> None:
    private_lock = private_root / "refresh.lock"
    if private_lock.is_symlink():
        try:
            if private_lock.resolve(strict=True) != canonical_lock.resolve(strict=True):
                raise BridgeError("private refresh lock points to an unexpected target")
        except OSError:
            raise BridgeError("private refresh lock target is unavailable") from None
        return
    if private_lock.exists():
        # A regular lock may be held by a previous bridge/plugin process. Never
        # replace or rename it; the caller must resolve the conflicting setup.
        raise BridgeError("private refresh lock already exists; refusing to replace it")
    try:
        private_lock.symlink_to(canonical_lock)
    except FileExistsError:
        _link_private_refresh_lock(private_root, canonical_lock)
    except OSError:
        raise BridgeError("unable to link the private refresh lock") from None


def _load_plugin(plugin_path: Path):
    if not plugin_path.is_file():
        raise BridgeError("configured Paseo history plugin file is unavailable")
    name = "_chat_hub_paseo_history_" + hashlib.sha256(
        str(plugin_path).encode("utf-8")
    ).hexdigest()[:16]
    spec = importlib.util.spec_from_file_location(name, plugin_path)
    if spec is None or spec.loader is None:
        raise BridgeError("unable to load the configured Paseo history plugin")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    try:
        spec.loader.exec_module(module)
    except Exception:
        sys.modules.pop(name, None)
        raise BridgeError("configured Paseo history plugin could not be loaded") from None
    return module


class RawGraphSpool:
    """Owner-bound private raw-graph cache separate from the plugin text index."""

    def __init__(self, path: Path, owner: str) -> None:
        if not OWNER_RE.fullmatch(owner):
            raise BridgeError("Paseo returned an invalid account owner hash")
        self.owner = owner
        self.path = path
        try:
            import sqlite3

            self.db = sqlite3.connect(path, timeout=10)
            self.db.execute("PRAGMA journal_mode=WAL")
            self.db.execute(
                "CREATE TABLE IF NOT EXISTS raw_conversations ("
                "owner TEXT NOT NULL, conversation_id TEXT NOT NULL, "
                "graph_json TEXT NOT NULL, PRIMARY KEY(owner, conversation_id))"
            )
            self.db.commit()
            os.chmod(path, 0o600)
            for suffix in ("-wal", "-shm"):
                sidecar = Path(str(path) + suffix)
                if sidecar.exists():
                    os.chmod(sidecar, 0o600)
        except Exception:
            raise BridgeError("unable to open the private raw-conversation spool") from None

    def put(self, conversation: Any) -> None:
        if not isinstance(conversation, dict):
            raise BridgeError("Paseo returned an invalid conversation graph")
        conversation_id = conversation.get("conversation_id") or conversation.get("id")
        if not isinstance(conversation_id, str) or not conversation_id.strip():
            raise BridgeError("Paseo returned a conversation without an id")
        try:
            graph_json = json.dumps(
                conversation, ensure_ascii=False, separators=(",", ":")
            )
            encoded = graph_json.encode("utf-8")
        except (TypeError, ValueError, UnicodeError):
            raise BridgeError("Paseo returned an unencodable conversation graph") from None
        if len(encoded) > MAX_EXPORT_BYTES:
            raise BridgeError("one conversation exceeds the safe export size limit")
        try:
            with self.db:
                self.db.execute(
                    "INSERT INTO raw_conversations(owner,conversation_id,graph_json) "
                    "VALUES(?,?,?) ON CONFLICT(owner,conversation_id) "
                    "DO UPDATE SET graph_json=excluded.graph_json",
                    (self.owner, conversation_id, graph_json),
                )
        except Exception:
            raise BridgeError("unable to retain a conversation in the private spool") from None

    def records(self) -> Iterator[tuple[str, str]]:
        try:
            rows = self.db.execute(
                "SELECT conversation_id,graph_json FROM raw_conversations "
                "WHERE owner=? ORDER BY conversation_id",
                (self.owner,),
            )
            for conversation_id, graph_json in rows:
                yield str(conversation_id), str(graph_json)
        except Exception:
            raise BridgeError("unable to read the private raw-conversation spool") from None

    def count(self) -> int:
        try:
            return int(
                self.db.execute(
                    "SELECT count(*) FROM raw_conversations WHERE owner=?",
                    (self.owner,),
                ).fetchone()[0]
            )
        except Exception:
            raise BridgeError("unable to count private conversations") from None

    def close(self) -> None:
        self.db.close()


def _write_exports(spool: RawGraphSpool, owner: str, output_dir: Path) -> list[str]:
    if not OWNER_RE.fullmatch(owner):
        raise BridgeError("Paseo returned an invalid account owner hash")
    if output_dir.expanduser().is_symlink():
        raise BridgeError("refusing a symlink for private export output")
    output_dir = _resolved(output_dir)
    if output_dir.exists():
        raise BridgeError("output directory already exists; choose a new private path")
    parent = output_dir.parent
    try:
        parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        staging = Path(tempfile.mkdtemp(prefix=f".{output_dir.name}.staging-", dir=parent))
        os.chmod(staging, 0o700)
    except OSError:
        raise BridgeError("unable to create private export output") from None

    owner_json = json.dumps(owner, separators=(",", ":")).encode("ascii")
    prefix = b'{"owner":' + owner_json + b',"conversations":['
    suffix = b"]}"
    exports: list[str] = []
    chunk: list[bytes] = []
    size = len(prefix) + len(suffix)
    try:
        def flush() -> None:
            nonlocal chunk, size
            if not chunk:
                return
            index = len(exports) + 1
            basename = f"export-{index:04d}.json"
            payload = prefix + b",".join(chunk) + suffix
            if len(payload) > MAX_EXPORT_BYTES:
                raise BridgeError("conversation export exceeded the safe size limit")
            path = staging / basename
            flags = os.O_CREAT | os.O_EXCL | os.O_WRONLY
            if hasattr(os, "O_NOFOLLOW"):
                flags |= os.O_NOFOLLOW
            fd = os.open(path, flags, 0o600)
            with os.fdopen(fd, "wb") as stream:
                stream.write(payload)
                stream.flush()
                os.fsync(stream.fileno())
            exports.append(basename)
            chunk = []
            size = len(prefix) + len(suffix)

        for _conversation_id, graph_json in spool.records():
            graph = graph_json.encode("utf-8")
            addition = len(graph) + (1 if chunk else 0)
            if len(graph) + len(prefix) + len(suffix) > MAX_EXPORT_BYTES:
                raise BridgeError("one conversation exceeds the safe export size limit")
            if chunk and (
                len(chunk) >= MAX_EXPORT_CONVERSATIONS
                or size + addition > MAX_EXPORT_BYTES
            ):
                flush()
                addition = len(graph)
            chunk.append(graph)
            size += addition
        flush()
        if output_dir.exists():
            raise BridgeError("output directory appeared during export; refusing to overwrite it")
        os.replace(staging, output_dir)
        try:
            dir_fd = os.open(parent, os.O_RDONLY)
            try:
                os.fsync(dir_fd)
            finally:
                os.close(dir_fd)
        except OSError:
            # The export has been atomically published; the files remain valid.
            pass
        return exports
    except Exception:
        shutil.rmtree(staging, ignore_errors=True)
        raise


def run_bridge(
    *,
    plugin_path: str | Path,
    state_dir: str | Path,
    shared_paseo_home: str | Path,
    output_dir: str | Path,
) -> dict[str, Any]:
    if Path(state_dir).expanduser().is_symlink():
        raise BridgeError("refusing a symlink for private state")
    if Path(output_dir).expanduser().is_symlink():
        raise BridgeError("refusing a symlink for private export output")
    plugin_path = _resolved(plugin_path)
    if plugin_path.is_dir():
        plugin_path = plugin_path / "server" / "history.py"
    state_home = _resolved(state_dir)
    shared_home = _resolved(shared_paseo_home)
    if state_home == shared_home:
        raise BridgeError("private state must be separate from the shared Paseo home")

    old_paseo_home = os.environ.get("PASEO_HOME")
    plugin = archive = spool = original_render = None
    try:
        _ensure_private_dir(state_home)
        os.environ["PASEO_HOME"] = str(state_home)
        plugin = _load_plugin(plugin_path)
        # The installed plugin owns credential loading and identity derivation.
        # This call happens only when the bridge is explicitly run.
        old_umask = os.umask(0o077)
        try:
            archive = plugin.Archive()
        finally:
            os.umask(old_umask)
        owner = getattr(archive, "owner", None)
        if not isinstance(owner, str) or not OWNER_RE.fullmatch(owner):
            raise BridgeError("Paseo returned an invalid account owner hash")

        private_root = _resolved(archive.root)
        if private_root == shared_home / "chatgpt-history" / owner:
            raise BridgeError("Paseo cache must use the app-private state directory")
        canonical_lock = _canonical_refresh_lock(shared_home, owner)
        _link_private_refresh_lock(private_root, canonical_lock)
        spool = RawGraphSpool(private_root / "chat-hub-raw-graphs.sqlite3", owner)

        original_render = plugin.render

        def render_and_spool(conversation: Any, project: str = "", *args, **kwargs):
            rendered = original_render(conversation, project, *args, **kwargs)
            spool.put(copy.deepcopy(conversation))
            return rendered

        plugin.render = render_and_spool
        result = plugin.synchronize(archive)
        if not isinstance(result, dict) or result.get("started") is not True:
            raise BridgeError("Paseo history synchronization did not start")
        error = archive.state("error")
        if error:
            raise BridgeError("Paseo history synchronization did not complete")
        exports = _write_exports(spool, owner, _resolved(output_dir))
        return {
            "owner": owner,
            "exports": exports,
            "conversations": spool.count(),
        }
    except BridgeError:
        raise
    except Exception as failure:
        if failure.__class__.__name__ == "HistoryError":
            # Plugin HistoryError strings are intentionally user-safe, but avoid
            # leaking arbitrary exception text from a configured plugin module.
            raise BridgeError("Paseo history synchronization failed; cached data was retained") from None
        raise BridgeError("Paseo history bridge failed; private state was retained") from None
    finally:
        if plugin is not None and original_render is not None:
            plugin.render = original_render
        for resource in (spool, getattr(archive, "db", None)):
            if resource is not None:
                try:
                    resource.close()
                except Exception:
                    pass
        if old_paseo_home is None:
            os.environ.pop("PASEO_HOME", None)
        else:
            os.environ["PASEO_HOME"] = old_paseo_home


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--plugin", required=True, type=Path, help="plugin directory or installed server/history.py source")
    parser.add_argument("--state-dir", required=True, type=Path, help="Chat Hub private Paseo state directory")
    parser.add_argument("--shared-paseo-home", required=True, type=Path, help="existing/default Paseo home used to coordinate OAuth refresh")
    parser.add_argument("--output-dir", required=True, type=Path, help="new private directory for bounded owner-tagged JSON exports")
    return parser


def main(argv: list[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    try:
        manifest = run_bridge(
            plugin_path=args.plugin,
            state_dir=args.state_dir,
            shared_paseo_home=args.shared_paseo_home,
            output_dir=args.output_dir,
        )
    except BridgeError as error:
        print(f"paseo-history-bridge: {error}", file=sys.stderr)
        return 1
    print(json.dumps(manifest, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
