import importlib.util
import json
import os
from pathlib import Path
import stat
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location(
    "paseo_history_bridge", ROOT / "tools" / "paseo_history_bridge.py"
)
bridge = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(bridge)


OWNER = "0123456789abcdef01234567"


def write_fake_plugin(path: Path, *, fail: bool = False) -> None:
    failure = "    raise HistoryError('sensitive details must not leak')" if fail else "    return {'started': True}"
    path.write_text(
        """from pathlib import Path
import os

OWNER = '0123456789abcdef01234567'
GRAPHS = [
    {'id': 'conversation-a', 'conversation_id': 'conversation-a', 'mapping': {
        'user': {'parent': None, 'message': {'id': 'user', 'author': {'role': 'user'}, 'content': {'parts': ['question']}}},
        'answer': {'parent': 'user', 'message': {'id': 'answer', 'author': {'role': 'assistant'}, 'content': {'parts': ['answer']}}},
    }, 'current_node': 'answer'},
    {'id': 'conversation-b', 'conversation_id': 'conversation-b', 'mapping': {
        'root': {'parent': None, 'message': {'id': 'root', 'author': {'role': 'user'}, 'content': {'parts': ['branch']}}},
    }, 'current_node': 'root'},
]

class HistoryError(Exception):
    pass

class _Db:
    def close(self):
        pass

class Archive:
    def __init__(self):
        self.owner = OWNER
        self.root = Path(os.environ['PASEO_HOME']) / 'chatgpt-history' / OWNER
        self.root.mkdir(parents=True, exist_ok=True, mode=0o700)
        self.db = _Db()
        self.tokens = {'access_token': 'do-not-export'}
    def state(self, key):
        return ''

def render(conversation, project=''):
    return [{'rendered': conversation['id'], 'project': project}]

def synchronize(archive):
    for graph in GRAPHS:
        docs = render(graph, 'project-1')
        assert docs == [{'rendered': graph['id'], 'project': 'project-1'}]
"""
        + failure
        + "\n",
        encoding="utf-8",
    )


class PaseoHistoryBridgeTests(unittest.TestCase):
    def test_wraps_original_render_and_exports_full_graphs_with_owner(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            plugin = root / "history.py"
            write_fake_plugin(plugin)
            shared = root / "shared-paseo"
            output = root / "out"
            previous = os.environ.get("PASEO_HOME")

            manifest = bridge.run_bridge(
                plugin_path=plugin,
                state_dir=root / "private-state",
                shared_paseo_home=shared,
                output_dir=output,
            )

            self.assertEqual(manifest, {
                "owner": OWNER,
                "exports": ["export-0001.json"],
                "conversations": 2,
            })
            payload = json.loads((output / manifest["exports"][0]).read_text())
            self.assertEqual(payload["owner"], OWNER)
            self.assertEqual([g["id"] for g in payload["conversations"]], [
                "conversation-a", "conversation-b"
            ])
            self.assertIn("mapping", payload["conversations"][0])
            encoded = (output / manifest["exports"][0]).read_text()
            self.assertNotIn("do-not-export", encoded)
            private_root = root / "private-state" / "chatgpt-history" / OWNER
            self.assertTrue((private_root / "refresh.lock").is_symlink())
            self.assertEqual(
                (private_root / "refresh.lock").resolve(),
                (shared / "chatgpt-history" / OWNER / "refresh.lock").resolve(),
            )
            self.assertEqual(os.environ.get("PASEO_HOME"), previous)
            self.assertEqual(stat.S_IMODE((output / "export-0001.json").stat().st_mode), 0o600)

    def test_refresh_link_validates_target_and_never_replaces_regular_lock(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            private = root / "private"
            private.mkdir()
            canonical = root / "shared" / "refresh.lock"
            canonical.parent.mkdir()
            canonical.write_text("active lock inode")
            bridge._link_private_refresh_lock(private, canonical.resolve())
            self.assertEqual((private / "refresh.lock").resolve(), canonical.resolve())
            bridge._link_private_refresh_lock(private, canonical.resolve())
            self.assertEqual(canonical.read_text(), "active lock inode")

            other = root / "other.lock"
            other.write_text("do not replace")
            (private / "refresh.lock").unlink()
            (private / "refresh.lock").symlink_to(other)
            with self.assertRaisesRegex(bridge.BridgeError, "unexpected target"):
                bridge._link_private_refresh_lock(private, canonical.resolve())
            self.assertEqual(other.read_text(), "do not replace")

            (private / "refresh.lock").unlink()
            (private / "refresh.lock").write_text("still active")
            with self.assertRaises(bridge.BridgeError):
                bridge._link_private_refresh_lock(private, other.resolve())
            self.assertEqual((private / "refresh.lock").read_text(), "still active")

    def test_sync_failure_emits_no_manifest_or_export_directory(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            plugin = root / "history.py"
            write_fake_plugin(plugin, fail=True)
            output = root / "out"
            with self.assertRaisesRegex(bridge.BridgeError, "synchronization failed"):
                bridge.run_bridge(
                    plugin_path=plugin,
                    state_dir=root / "private-state",
                    shared_paseo_home=root / "shared-paseo",
                    output_dir=output,
                )
            self.assertFalse(output.exists())

    def test_exports_respect_conversation_count_and_byte_caps(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            owner = OWNER
            spool = bridge.RawGraphSpool(root / "spool.sqlite3", owner)
            try:
                for i in range(3):
                    spool.put({"id": f"conversation-{i}", "mapping": {"root": i}})
                old_count = bridge.MAX_EXPORT_CONVERSATIONS
                old_bytes = bridge.MAX_EXPORT_BYTES
                bridge.MAX_EXPORT_CONVERSATIONS = 1
                bridge.MAX_EXPORT_BYTES = 512
                try:
                    files = bridge._write_exports(spool, owner, root / "exports")
                finally:
                    bridge.MAX_EXPORT_CONVERSATIONS = old_count
                    bridge.MAX_EXPORT_BYTES = old_bytes
                self.assertEqual(files, ["export-0001.json", "export-0002.json", "export-0003.json"])
                self.assertTrue(all(json.loads((root / "exports" / name).read_text())["owner"] == owner for name in files))
            finally:
                spool.close()

    def test_oversized_graph_is_rejected_without_truncation_or_partial_export(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            spool = bridge.RawGraphSpool(root / "spool.sqlite3", OWNER)
            try:
                spool.put({"id": "large", "body": "x" * 1024})
                old_bytes = bridge.MAX_EXPORT_BYTES
                bridge.MAX_EXPORT_BYTES = 512
                try:
                    with self.assertRaisesRegex(bridge.BridgeError, "one conversation exceeds"):
                        bridge._write_exports(spool, OWNER, root / "exports")
                finally:
                    bridge.MAX_EXPORT_BYTES = old_bytes
                self.assertFalse((root / "exports").exists())
            finally:
                spool.close()


if __name__ == "__main__":
    unittest.main()
