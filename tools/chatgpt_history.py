#!/usr/bin/env python3
"""Owned, local ChatGPT history fetcher for Chat Hub.

This module intentionally does not import Paseo code, mutate Codex auth, or
write under PASEO_HOME. It preserves full provider conversation graphs.
"""
from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import sqlite3
import stat
import sys
import tempfile
import time
from typing import Any, Iterator
from urllib.parse import quote

BASE = "https://chatgpt.com/backend-api/"
OWNER_RE = re.compile(r"^[0-9a-f]{24}$")
MAX_BODY = 60 * 1024 * 1024
MAX_EXPORT_BYTES = 60 * 1024 * 1024
MAX_EXPORT_CONVERSATIONS = 100
SOFT_EXPORT_BYTES = 4 * 1024 * 1024


class HistoryError(Exception):
    def __init__(self, code: str, stage: str, retry_after: int | None = None):
        super().__init__(code)
        self.code = code
        self.stage = stage
        self.retry_after = retry_after


def _claims(token: str) -> dict[str, Any]:
    try:
        segment = token.split(".")[1]
        segment += "=" * (-len(segment) % 4)
        obj = json.loads(base64.urlsafe_b64decode(segment.encode("ascii")))
        return obj if isinstance(obj, dict) else {}
    except Exception:
        return {}


def account_scope(tokens: dict[str, Any]) -> str:
    token = tokens.get("access_token")
    if not isinstance(token, str) or not token:
        raise HistoryError("login_required", "auth")
    claims = _claims(token)
    auth = claims.get("https://api.openai.com/auth")
    account_id = auth.get("chatgpt_account_id") if isinstance(auth, dict) else None
    user_id = auth.get("chatgpt_user_id") if isinstance(auth, dict) else None
    if isinstance(auth, dict):
        if user_id is None:
            user_id = auth.get("user_id")
        for claim_value in (account_id, auth.get("chatgpt_user_id"), auth.get("user_id")):
            if claim_value is not None and not _valid_id(claim_value):
                raise HistoryError("auth_invalid", "auth")
        if account_id is not None and user_id is not None:
            if not _valid_id(account_id) or not _valid_id(user_id):
                raise HistoryError("auth_invalid", "auth")
            identity = ("chatgpt-request-identity-v1", account_id, user_id)
            digest = hashlib.sha256(json.dumps(identity, ensure_ascii=False, separators=(",", ":")).encode()).hexdigest()
            return "chatgpt:observed:" + digest
    fallback_account, subject = tokens.get("account_id"), claims.get("sub")
    if not _valid_id(fallback_account) or not _valid_subject(subject):
        raise HistoryError("auth_invalid", "auth")
    fallback = ("chatgpt-oauth-identity-v1", fallback_account, subject)
    digest = hashlib.sha256(json.dumps(fallback, ensure_ascii=False, separators=(",", ":")).encode()).hexdigest()
    return "chatgpt:oauth:" + digest


def _valid_id(value: Any) -> bool:
    return isinstance(value, str) and bool(value) and len(value) <= 256 and all(
        c.isascii() and (c.isalnum() or c in "-_:") for c in value
    )


def _valid_subject(value: Any) -> bool:
    if not isinstance(value, str) or not value:
        return False
    try:
        return len(value.encode("utf-8")) <= 4096
    except UnicodeError:
        return False


def _owner(tokens: dict[str, Any]) -> str:
    claims = _claims(str(tokens.get("access_token", "")))
    identity = str(tokens.get("account_id", "")) + ":" + str(claims.get("sub", ""))
    return hashlib.sha256(identity.encode()).hexdigest()[:24]


def read_auth(path: Path) -> dict[str, Any]:
    try:
        if path.stat().st_size > 1024 * 1024:
            raise ValueError()
        data = json.loads(path.read_text())
        tokens = data["tokens"]
        if not isinstance(tokens, dict) or not isinstance(tokens.get("access_token"), str):
            raise ValueError()
        return dict(tokens)  # immutable per-job snapshot; never write it back
    except Exception:
        raise HistoryError("login_required", "auth") from None


class RawGraphStore:
    def __init__(self, state_dir: Path, scope: str):
        if state_dir.is_symlink():
            raise HistoryError("state_unavailable", "store")
        state_dir.mkdir(parents=True, exist_ok=True, mode=0o700)
        os.chmod(state_dir, 0o700)
        self.path = state_dir / (hashlib.sha256(scope.encode()).hexdigest() + ".sqlite3")
        if self.path.is_symlink():
            raise HistoryError("state_unavailable", "store")
        try:
            self.db = sqlite3.connect(self.path, timeout=10)
            os.chmod(self.path, 0o600)
            if stat.S_IMODE(self.path.stat().st_mode) != 0o600:
                raise RuntimeError("private database permissions unavailable")
            mode = self.db.execute("PRAGMA journal_mode=WAL").fetchone()[0]
            if str(mode).lower() != "wal":
                raise RuntimeError("private journal mode unavailable")
            for suffix in ("-wal", "-shm"):
                sidecar = Path(str(self.path) + suffix)
                if sidecar.exists():
                    os.chmod(sidecar, 0o600)
                    if stat.S_IMODE(sidecar.stat().st_mode) != 0o600:
                        raise RuntimeError("private journal permissions unavailable")
            self.db.executescript("""
              CREATE TABLE IF NOT EXISTS conversations(
                id TEXT PRIMARY KEY, update_tag TEXT NOT NULL, graph TEXT NOT NULL,
                project_id TEXT NOT NULL DEFAULT '', touched INTEGER NOT NULL
              );
              CREATE TABLE IF NOT EXISTS state(key TEXT PRIMARY KEY, value TEXT NOT NULL);
            """)
            self.db.commit()
            self._secure_sidecars()
        except Exception:
            raise HistoryError("state_unavailable", "store") from None

    def _secure_sidecars(self) -> None:
        try:
            for suffix in ("-wal", "-shm"):
                sidecar = Path(str(self.path) + suffix)
                if sidecar.exists():
                    os.chmod(sidecar, 0o600)
                    if stat.S_IMODE(sidecar.stat().st_mode) != 0o600:
                        raise OSError("private sidecar permissions unavailable")
        except OSError:
            raise HistoryError("state_unavailable", "store") from None

    def get(self, cid: str) -> tuple[str, dict[str, Any], str] | None:
        row = self.db.execute("SELECT update_tag,graph,project_id FROM conversations WHERE id=?", (cid,)).fetchone()
        if not row:
            return None
        try:
            return str(row[0]), json.loads(row[1]), str(row[2])
        except Exception:
            return None

    def put(self, cid: str, tag: str, graph: dict[str, Any], project_id: str = "") -> None:
        try:
            raw = json.dumps(graph, ensure_ascii=False, separators=(",", ":"))
            if len(raw.encode("utf-8")) > MAX_EXPORT_BYTES:
                raise HistoryError("conversation_too_large", "detail")
            with self.db:
                self.db.execute("""INSERT INTO conversations(id,update_tag,graph,project_id,touched)
                    VALUES(?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET
                    update_tag=excluded.update_tag,graph=excluded.graph,
                    project_id=excluded.project_id,touched=excluded.touched""",
                    (cid, tag, raw, project_id, int(time.time())))
            self._secure_sidecars()
        except HistoryError:
            raise
        except Exception:
            raise HistoryError("state_unavailable", "store") from None

    def records(self) -> Iterator[tuple[str, str]]:
        for cid, graph in self.db.execute("SELECT id,graph FROM conversations ORDER BY id"):
            yield str(cid), str(graph)

    def count(self) -> int:
        return int(self.db.execute("SELECT COUNT(*) FROM conversations").fetchone()[0])

    def get_state(self, key: str) -> str:
        row = self.db.execute("SELECT value FROM state WHERE key=?", (key,)).fetchone()
        return str(row[0]) if row else ""

    def set_state(self, key: str, value: str) -> None:
        with self.db:
            self.db.execute("INSERT INTO state(key,value) VALUES(?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value", (key, value))
        self._secure_sidecars()

    def close(self) -> None:
        self.db.close()


class Client:
    def __init__(self, tokens: dict[str, Any], session: Any = None):
        token = tokens.get("access_token", "")
        claims = _claims(token)
        if not isinstance(token, str) or not token or claims.get("exp", 0) <= time.time():
            raise HistoryError("login_required", "auth")
        auth = claims.get("https://api.openai.com/auth")
        headers = {"Authorization": "Bearer " + token, "Accept": "application/json"}
        account = auth.get("chatgpt_account_id") if isinstance(auth, dict) else tokens.get("account_id")
        if account:
            headers["ChatGPT-Account-Id"] = str(account)
        self.headers = headers
        if session is None:
            try:
                from curl_cffi import requests
                session = requests.Session(impersonate="chrome")
            except Exception:
                raise HistoryError("transport_unavailable", "auth") from None
        self.session = session

    def request(self, path: str, params: dict[str, Any] | None = None, body: dict[str, Any] | None = None,
                stage: str = "list") -> Any:
        if path.startswith(("/", "//")) or ".." in path.split("/"):
            raise HistoryError("invalid_request", stage)
        try:
            response = self.session.request(
                "POST" if body is not None else "GET", BASE + path,
                params=params, json=body, headers=self.headers, timeout=60,
                allow_redirects=False, stream=True,
            )
        except Exception:
            raise HistoryError("network_error", stage) from None
        try:
            status = int(getattr(response, "status_code", 0))
            if status == 429:
                try:
                    delay = max(1, min(1800, int(response.headers.get("retry-after", "5"))))
                except Exception:
                    delay = 5
                raise HistoryError("rate_limited", stage, delay)
            if status == 401:
                raise HistoryError("login_required", stage)
            if status != 200:
                raise HistoryError("http_error", stage)
            headers = getattr(response, "headers", {})
            length = int(headers.get("content-length", "0"))
            if length > MAX_BODY:
                raise HistoryError("response_too_large", stage)
            if hasattr(response, "iter_content"):
                chunks: list[bytes] = []
                total = 0
                for chunk in response.iter_content(chunk_size=64 * 1024):
                    if not chunk:
                        continue
                    total += len(chunk)
                    if total > MAX_BODY:
                        raise HistoryError("response_too_large", stage)
                    chunks.append(chunk)
                raw = b"".join(chunks)
                return json.loads(raw)
            raw = response.content
            if len(raw) > MAX_BODY:
                raise HistoryError("response_too_large", stage)
            return response.json()
        except HistoryError:
            raise
        except Exception:
            raise HistoryError("invalid_response", stage) from None
        finally:
            try:
                response.close()
            except Exception:
                pass

    def get(self, path: str, params: dict[str, Any] | None = None, stage: str = "list") -> Any:
        return self.request(path, params=params, stage=stage)

    def batch(self, ids: list[str]) -> Any:
        return self.request("conversations/batch", body={"conversation_ids": ids}, stage="detail")


def pages(client: Client, path: str, params: dict[str, Any], cursor: bool = False,
          stage: str = "list") -> Iterator[dict[str, Any]]:
    seen: set[str] = set()
    offset = int(params.get("offset", 0))
    current = dict(params)
    while True:
        result = client.get(path, current, stage=stage)
        if not isinstance(result, dict) or not isinstance(result.get("items"), list):
            raise HistoryError("invalid_response", stage)
        items = result["items"]
        for item in items:
            if isinstance(item, dict):
                yield item
        if cursor:
            next_cursor = result.get("cursor")
            if not next_cursor:
                break
            marker = "c:" + str(next_cursor)
            if marker in seen:
                raise HistoryError("invalid_pagination", stage)
            seen.add(marker)
            current = dict(params, cursor=next_cursor)
        else:
            if not items:
                break
            offset += len(items)
            if offset >= int(result.get("total", offset)):
                break
            marker = "o:" + str(offset)
            if marker in seen:
                raise HistoryError("invalid_pagination", stage)
            seen.add(marker)
            current = dict(params, offset=offset)


def _conversation_id(row: dict[str, Any]) -> str:
    value = row.get("id") or row.get("conversation_id")
    return str(value) if isinstance(value, str) and value else ""


def fetch_job(client: Client, store: RawGraphStore, scope: str, owner: str,
              output_dir: Path, probe: bool = False) -> dict[str, Any]:
    now = int(time.time())
    not_before = int(store.get_state("not_before") or "0")
    manifest: dict[str, Any] = {"owner": owner, "account_scope": scope, "exports": [],
                                "conversations": 0, "complete": False, "error_code": "",
                                "retry_after": 0}
    if now < not_before:
        manifest.update(error_code="rate_limited", stage="list", retry_after=not_before - now)
        if probe:
            manifest.update(probe=True, listed=0, total=0)
            return manifest
        return _export_cached(store, manifest, output_dir)
    if probe:
        try:
            result = client.get("conversations", {"offset": 0, "limit": 1,
                "order": "updated", "is_archived": "false", "hide_snorlax": "false"}, stage="list")
            if not isinstance(result, dict) or not isinstance(result.get("items"), list):
                raise HistoryError("invalid_response", "list")
            total = result.get("total", 0)
            if not isinstance(total, int) or total < 0:
                raise HistoryError("invalid_response", "list")
            return {"owner": owner, "account_scope": scope, "exports": [], "conversations": 0,
                    "complete": True, "error_code": "", "retry_after": 0,
                    "probe": True, "listed": len(result["items"]), "total": total}
        except HistoryError as exc:
            return {"owner": owner, "account_scope": scope, "exports": [], "conversations": 0,
                    "complete": False, "error_code": exc.code, "stage": exc.stage,
                    "retry_after": exc.retry_after or 0, "probe": True, "listed": 0, "total": 0}
    rows: dict[str, dict[str, Any]] = {}
    project_ids: dict[str, str] = {}
    try:
        for archived in (False, True):
            for row in pages(client, "conversations", {"offset": 0, "limit": 100,
                "order": "updated", "is_archived": str(archived).lower(),
                "hide_snorlax": "false"}, stage="list"):
                cid = _conversation_id(row)
                if cid:
                    rows[cid] = row
    except HistoryError as exc:
        manifest.update(error_code=exc.code, stage=exc.stage,
                        retry_after=exc.retry_after or 0)
        if exc.retry_after:
            store.set_state("not_before", str(int(time.time()) + exc.retry_after))
        return _export_cached(store, manifest, output_dir)

    def fetch_changed(selected: dict[str, dict[str, Any]]) -> bool:
        changed: list[str] = []
        for cid, row in selected.items():
            tag = str(row.get("update_time") or "")
            project_id = project_ids.get(cid, str(row.get("gizmo_id") or ""))
            old = store.get(cid)
            if not old or old[0] != tag or old[2] != project_id:
                changed.append(cid)
        for start in range(0, len(changed), 10):
            ids = changed[start:start + 10]
            try:
                batch = client.batch(ids)
                if not isinstance(batch, list):
                    raise HistoryError("invalid_response", "detail")
                fetched = {_conversation_id(graph): graph for graph in batch if isinstance(graph, dict)}
                for cid in ids:
                    graph = fetched.get(cid)
                    if graph is None:
                        graph = client.get("conversation/" + quote(cid, safe=""), stage="detail")
                    if not isinstance(graph, dict) or _conversation_id(graph) != cid or not isinstance(graph.get("mapping"), dict):
                        raise HistoryError("invalid_response", "detail")
                    project_id = project_ids.get(cid, str(selected[cid].get("gizmo_id") or ""))
                    store.put(cid, str(selected[cid].get("update_time") or ""), graph, project_id)
            except HistoryError as exc:
                manifest.update(error_code=exc.code, stage=exc.stage,
                                retry_after=exc.retry_after or 0)
                if exc.retry_after:
                    store.set_state("not_before", str(int(time.time()) + exc.retry_after))
                return False
        return True

    if not fetch_changed(rows):
        return _export_cached(store, manifest, output_dir)

    # Fetch ordinary account history first; project endpoints are supplemental.
    project_rows: dict[str, dict[str, Any]] = {}
    try:
        projects = list(pages(client, "gizmos/snorlax/sidebar", {
            "conversations_per_gizmo": 0, "limit": 20, "owned_only": "false"},
            cursor=True, stage="project"))
        for item in projects:
            wrapper = item.get("gizmo", {})
            gizmo = wrapper.get("gizmo", wrapper) if isinstance(wrapper, dict) else {}
            pid = gizmo.get("id") if isinstance(gizmo, dict) else None
            if not isinstance(pid, str) or not pid:
                continue
            for row in pages(client, "gizmos/" + quote(pid, safe="") + "/conversations",
                             {"cursor": "0"}, cursor=True, stage="project"):
                cid = _conversation_id(row)
                if cid:
                    row["gizmo_id"] = pid
                    project_rows[cid] = row
                    project_ids[cid] = pid
    except HistoryError as exc:
        manifest.update(error_code=exc.code, stage=exc.stage,
                        retry_after=exc.retry_after or 0)
        if exc.retry_after:
            store.set_state("not_before", str(int(time.time()) + exc.retry_after))
        if exc.code == "rate_limited":
            return _export_cached(store, manifest, output_dir)

    if project_rows and not fetch_changed(project_rows):
        return _export_cached(store, manifest, output_dir)
    if not manifest["error_code"]:
        store.set_state("not_before", "0")
        manifest["complete"] = True
    return _export_cached(store, manifest, output_dir)


def _export_cached(store: RawGraphStore, manifest: dict[str, Any], output_dir: Path) -> dict[str, Any]:
    if output_dir.exists() or output_dir.is_symlink():
        raise HistoryError("output_unavailable", "export")
    parent = output_dir.parent
    parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    staging = Path(tempfile.mkdtemp(prefix=".history-export-", dir=parent))
    os.chmod(staging, 0o700)
    exports: list[str] = []
    chunk: list[bytes] = []
    count = 0
    prefix = (json.dumps({"owner": manifest["owner"], "account_scope": manifest["account_scope"],
                          "format": "chat-hub-history-v1"},
                         separators=(",", ":"), ensure_ascii=False)[:-1] + ',"conversations":[').encode()
    suffix = b"]}"
    size = len(prefix) + len(suffix)
    try:
        def flush() -> None:
            nonlocal chunk, size, count
            if not chunk:
                return
            name = f"export-{len(exports)+1:04d}.json"
            payload = prefix + b",".join(chunk) + suffix
            if len(payload) > MAX_EXPORT_BYTES:
                raise HistoryError("export_too_large", "export")
            fd = os.open(staging / name, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
            with os.fdopen(fd, "wb") as f:
                f.write(payload); f.flush(); os.fsync(f.fileno())
            exports.append(name); chunk = []; size = len(prefix) + len(suffix); count = 0
        for _cid, raw in store.records():
            encoded = raw.encode("utf-8")
            addition = len(encoded) + (1 if chunk else 0)
            if len(prefix) + len(suffix) + len(encoded) > MAX_EXPORT_BYTES:
                raise HistoryError("conversation_too_large", "export")
            if chunk and (count >= MAX_EXPORT_CONVERSATIONS or size + addition > SOFT_EXPORT_BYTES):
                flush(); addition = len(encoded)
            chunk.append(encoded); count += 1; size += addition
        flush()
        os.replace(staging, output_dir)
    except Exception:
        import shutil
        shutil.rmtree(staging, ignore_errors=True)
        raise
    manifest["exports"] = exports
    manifest["conversations"] = store.count()
    return manifest


def run(state_dir: Path, output_dir: Path, auth_file: Path, probe: bool = False,
        session: Any = None) -> dict[str, Any]:
    tokens = read_auth(auth_file)
    scope, owner = account_scope(tokens), _owner(tokens)
    if not OWNER_RE.fullmatch(owner):
        raise HistoryError("auth_invalid", "auth")
    store = RawGraphStore(state_dir, scope)
    try:
        client = Client(tokens, session=session)
        return fetch_job(client, store, scope, owner, output_dir, probe)
    finally:
        store.close()


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--state-dir", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--auth-file", type=Path)
    parser.add_argument("--probe", action="store_true")
    args = parser.parse_args(argv)
    auth = args.auth_file or Path(os.environ.get("CODEX_HOME", str(Path.home() / ".codex"))) / "auth.json"
    try:
        result = run(args.state_dir, args.output_dir, auth, args.probe)
        print(json.dumps(result, separators=(",", ":")))
        return 0
    except HistoryError as exc:
        print(json.dumps({"complete": False, "exports": [], "conversations": 0,
                          "error_code": exc.code, "stage": exc.stage,
                          "retry_after": exc.retry_after or 0}, separators=(",", ":")))
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
