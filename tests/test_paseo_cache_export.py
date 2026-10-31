import importlib.util
import contextlib
import json
import sqlite3
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location(
    "paseo_cache_export", ROOT / "tools" / "paseo_cache_export.py"
)
exporter = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(exporter)


OWNER = "0123456789abcdef01234567"
COLUMNS = (
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


def make_snapshot(path: Path) -> None:
    with contextlib.closing(sqlite3.connect(path)) as db, db:
        db.execute(
            "CREATE TABLE docs (id TEXT,kind TEXT,title TEXT,project TEXT,"
            "conversation TEXT,updated TEXT,text TEXT,url TEXT,meta TEXT,source TEXT)"
        )
        rows = [
            (
                "conversation:chat-1", "conversation", "A title", "project-7",
                "chat-1", "1730000000", "# A title\n\n## You\n\nhello\n\n## ChatGPT\n\nworld",
                "https://chatgpt.com/c/chat-1", '{"branch":"A","attachments":[1]}', "chatgpt",
            ),
            (
                "conversation:thread-1", "conversation", "Codex local", "",
                "thread-1", "1730000001", "## User\n\nquestion\n\n## Assistant\n\nanswer",
                "", '{"thread_id":"thread-1"}', "codex_local",
            ),
            (
                "conversation:thread-2", "conversation", "Codex cloud", "",
                "thread-2", "1730000002", "cached cloud text — preserve exactly",
                "", '{"cloud":true}', "codex_cloud",
            ),
            (
                "artifact:chat-1:file", "artifact", "File", "project-7", "chat-1",
                "1730000000", "not a conversation", "", "{}", "chatgpt",
            ),
        ]
        placeholders = ",".join("?" for _ in COLUMNS)
        db.executemany(f"INSERT INTO docs VALUES ({placeholders})", rows)


class PaseoCacheExportTests(unittest.TestCase):
    def test_exports_all_sources_without_rewriting_transcript_or_metadata(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / "snapshot.sqlite3"
            make_snapshot(source)
            before = source.read_bytes()

            manifest = exporter.export_cache(source, OWNER, root / "out")

            self.assertEqual(manifest, {
                "owner": OWNER,
                "exports": ["export-0001.json"],
                "conversations": 3,
            })
            envelope = json.loads((root / "out" / manifest["exports"][0]).read_text())
            self.assertEqual(envelope["format"], "paseo-cached-transcripts-v1")
            self.assertEqual(envelope["owner"], OWNER)
            self.assertEqual([row["source"] for row in envelope["conversations"]], [
                "chatgpt", "codex_cloud", "codex_local"
            ])
            chatgpt = envelope["conversations"][0]
            self.assertEqual(chatgpt["text"], "# A title\n\n## You\n\nhello\n\n## ChatGPT\n\nworld")
            self.assertEqual(chatgpt["meta"], '{"branch":"A","attachments":[1]}')
            self.assertEqual(source.read_bytes(), before)

    def test_source_connection_is_query_only_and_excludes_non_conversations(self):
        with tempfile.TemporaryDirectory() as tmp:
            source = Path(tmp) / "snapshot.sqlite3"
            make_snapshot(source)
            db = exporter._connect_read_only(source)
            try:
                with self.assertRaises(sqlite3.OperationalError):
                    db.execute("DELETE FROM docs")
                rows = list(exporter._rows(db))
                self.assertEqual(len(rows), 3)
                self.assertTrue(all(row["kind"] == "conversation" for row in rows))
            finally:
                db.close()

    def test_chunks_are_bounded_by_item_count_and_actual_json_bytes(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / "snapshot.sqlite3"
            make_snapshot(source)
            old_count = exporter.MAX_EXPORT_CONVERSATIONS
            old_bytes = exporter.MAX_EXPORT_BYTES
            exporter.MAX_EXPORT_CONVERSATIONS = 1
            exporter.MAX_EXPORT_BYTES = 1024
            try:
                manifest = exporter.export_cache(source, OWNER, root / "out")
                self.assertEqual(manifest["exports"], [
                    "export-0001.json", "export-0002.json", "export-0003.json"
                ])
                for name in manifest["exports"]:
                    path = root / "out" / name
                    self.assertLessEqual(path.stat().st_size, 1024)
                    self.assertEqual(len(json.loads(path.read_text())["conversations"]), 1)
            finally:
                exporter.MAX_EXPORT_CONVERSATIONS = old_count
                exporter.MAX_EXPORT_BYTES = old_bytes

    def test_oversized_single_row_is_rejected_without_partial_output(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / "snapshot.sqlite3"
            make_snapshot(source)
            with contextlib.closing(sqlite3.connect(source)) as db, db:
                db.execute(
                    "INSERT INTO docs VALUES(?,?,?,?,?,?,?,?,?,?)",
                    ("conversation:large", "conversation", "large", "", "large", "1", "x" * 2048, "", "{}", "chatgpt"),
                )
            old_bytes = exporter.MAX_EXPORT_BYTES
            exporter.MAX_EXPORT_BYTES = 1024
            try:
                with self.assertRaisesRegex(exporter.ExportError, "one cached conversation exceeds"):
                    exporter.export_cache(source, OWNER, root / "out")
            finally:
                exporter.MAX_EXPORT_BYTES = old_bytes
            self.assertFalse((root / "out").exists())

    def test_invalid_owner_and_incomplete_schema_are_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / "snapshot.sqlite3"
            make_snapshot(source)
            with self.assertRaises(exporter.ExportError):
                exporter.export_cache(source, "not-an-owner", root / "invalid-owner")
            with contextlib.closing(sqlite3.connect(source)) as db, db:
                db.execute("ALTER TABLE docs DROP COLUMN source")
            with self.assertRaisesRegex(exporter.ExportError, "provenance"):
                exporter.export_cache(source, OWNER, root / "out")
            self.assertFalse((root / "out").exists())


if __name__ == "__main__":
    unittest.main()
