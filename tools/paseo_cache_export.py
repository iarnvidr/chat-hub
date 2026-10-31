#!/usr/bin/env python3
"""Export the existing Paseo history cache as bounded Chat Hub envelopes.

The SQLite source is opened read-only. Transcript Markdown and plugin metadata
are copied as stored; this exporter never reconstructs message graphs from text.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import shutil
import sqlite3
import sys
import tempfile
from typing import Any, Iterator


OWNER_RE = re.compile(r"^[0-9a-f]{24}$")
EXPORT_FORMAT = "paseo-cached-transcripts-v1"
MAX_EXPORT_BYTES = 60 * 1024 * 1024
MAX_EXPORT_CONVERSATIONS = 1000
DOC_COLUMNS = (
    "id",
    "kind",
    "title",
    "project",
    "conversation",
    "updated",
    "text",
    "url",
    "meta",
    "source",
)


class ExportError(Exception):
    """Safe exporter errors; never include transcript contents or DB diagnostics."""


def _private_output_dir(path: Path) -> Path:
    if path.expanduser().is_symlink():
        raise ExportError("refusing a symlink for private export output")
    target = path.expanduser().resolve(strict=False)
    if target.exists():
        raise ExportError("output directory already exists; choose a new private path")
    return target


def _connect_read_only(database: Path) -> sqlite3.Connection:
    source = database.expanduser()
    if source.is_symlink() or not source.is_file():
        raise ExportError("snapshot must be a regular SQLite file")
    try:
        uri = source.resolve(strict=True).as_uri() + "?mode=ro"
        db = sqlite3.connect(uri, uri=True, timeout=5)
        db.row_factory = sqlite3.Row
        db.execute("PRAGMA query_only=ON")
        return db
    except Exception:
        raise ExportError("unable to open the SQLite snapshot read-only") from None


def _validate_schema(db: sqlite3.Connection) -> None:
    try:
        columns = {row["name"] for row in db.execute("PRAGMA table_info(docs)")}
    except sqlite3.Error:
        raise ExportError("SQLite snapshot has no readable docs table") from None
    missing = set(DOC_COLUMNS) - columns
    if missing:
        # A missing source column means the snapshot predates the plugin's
        # cross-source provenance schema; guessing would relabel cached rows.
        raise ExportError("SQLite snapshot is missing required provenance columns")


def _rows(db: sqlite3.Connection) -> Iterator[dict[str, Any]]:
    query = (
        "SELECT id,kind,title,project,conversation,updated,text,url,meta,source "
        "FROM docs WHERE kind='conversation' ORDER BY source,conversation,id"
    )
    try:
        cursor = db.execute(query)
        for row in cursor:
            yield {column: row[column] for column in DOC_COLUMNS}
    except sqlite3.Error:
        raise ExportError("unable to read cached conversation rows") from None


def _row_json(row: dict[str, Any]) -> bytes:
    try:
        return json.dumps(row, ensure_ascii=True, separators=(",", ":")).encode("ascii")
    except (TypeError, ValueError, UnicodeError):
        raise ExportError("a cached conversation row cannot be encoded safely") from None


def _write_exports(rows: Iterator[dict[str, Any]], owner: str, output_dir: Path) -> list[str]:
    if not OWNER_RE.fullmatch(owner):
        raise ExportError("account owner hash must be 24 lowercase hexadecimal characters")
    output_dir = _private_output_dir(output_dir)
    parent = output_dir.parent
    try:
        parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        staging = Path(tempfile.mkdtemp(prefix=f".{output_dir.name}.staging-", dir=parent))
        os.chmod(staging, 0o700)
    except OSError:
        raise ExportError("unable to create private export output") from None

    owner_json = json.dumps(owner, separators=(",", ":")).encode("ascii")
    format_json = json.dumps(EXPORT_FORMAT, separators=(",", ":")).encode("ascii")
    prefix = b'{"owner":' + owner_json + b',"format":' + format_json + b',"conversations":['
    suffix = b"]}"
    exports: list[str] = []
    chunk: list[bytes] = []
    size = len(prefix) + len(suffix)

    def flush() -> None:
        nonlocal chunk, size
        if not chunk:
            return
        basename = f"export-{len(exports) + 1:04d}.json"
        payload = prefix + b",".join(chunk) + suffix
        if len(payload) > MAX_EXPORT_BYTES:
            raise ExportError("conversation export exceeded the safe size limit")
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

    try:
        for row in rows:
            encoded = _row_json(row)
            if len(prefix) + len(suffix) + len(encoded) > MAX_EXPORT_BYTES:
                raise ExportError("one cached conversation exceeds the safe export size limit")
            addition = len(encoded) + (1 if chunk else 0)
            if chunk and (
                len(chunk) >= min(100, MAX_EXPORT_CONVERSATIONS)
                or size + addition > min(4 * 1024 * 1024, MAX_EXPORT_BYTES)
            ):
                flush()
                addition = len(encoded)
            chunk.append(encoded)
            size += addition
        flush()
        if output_dir.exists():
            raise ExportError("output directory appeared during export; refusing to overwrite it")
        os.replace(staging, output_dir)
        try:
            dir_fd = os.open(parent, os.O_RDONLY)
            try:
                os.fsync(dir_fd)
            finally:
                os.close(dir_fd)
        except OSError:
            pass
        return exports
    except Exception:
        shutil.rmtree(staging, ignore_errors=True)
        raise


def export_cache(database: str | Path, owner: str, output_dir: str | Path) -> dict[str, Any]:
    if not OWNER_RE.fullmatch(owner):
        raise ExportError("account owner hash must be 24 lowercase hexadecimal characters")
    db = _connect_read_only(Path(database))
    try:
        db.execute("BEGIN")
        _validate_schema(db)
        count = int(
            db.execute("SELECT count(*) FROM docs WHERE kind='conversation'").fetchone()[0]
        )
        exports = _write_exports(_rows(db), owner, Path(output_dir))
        return {"owner": owner, "exports": exports, "conversations": count}
    except ExportError:
        raise
    except sqlite3.Error:
        raise ExportError("unable to export the SQLite snapshot") from None
    finally:
        db.close()


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--database", required=True, type=Path, help="read-only SQLiteSnapshot of the Paseo cache")
    parser.add_argument("--owner", required=True, help="24-character Paseo Archive owner hash")
    parser.add_argument("--output-dir", required=True, type=Path, help="new private directory for bounded exports")
    return parser


def main(argv: list[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    try:
        manifest = export_cache(args.database, args.owner, args.output_dir)
    except ExportError as error:
        print(f"paseo-cache-export: {error}", file=sys.stderr)
        return 1
    print(json.dumps(manifest, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
