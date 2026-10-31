import base64
import importlib.util
import json
import hashlib
import os
import stat
import tempfile
import time
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("chatgpt_history", ROOT / "tools" / "chatgpt_history.py")
history = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(history)


def token(claims):
    enc = lambda value: base64.urlsafe_b64encode(json.dumps(value).encode()).decode().rstrip("=")
    return "a." + enc(claims) + ".sig"


class FakeClient:
    def __init__(self):
        self.batch_calls = []
        self.raise_archived = False
        self.get_calls = []

    def get(self, path, params=None, stage="list"):
        self.get_calls.append((path, dict(params or {})))
        if path == "conversations":
            if params["is_archived"] == "true" and self.raise_archived:
                raise history.HistoryError("rate_limited", "list", 23)
            items = [{"id": "c1", "update_time": "u1"}] if params["is_archived"] == "false" else []
            return {"items": items, "total": len(items)}
        if path == "gizmos/snorlax/sidebar":
            return {"items": []}
        raise AssertionError(path)

    def batch(self, ids):
        self.batch_calls.append(ids)
        return [{"id": "c1", "mapping": {"root": {"message": {"content": {"parts": ["full transcript"]}}}}}]


class ChatgptHistoryTests(unittest.TestCase):
    def setUp(self):
        self.claims = {"exp": int(time.time()) + 3600, "sub": "subject", "https://api.openai.com/auth": {
            "chatgpt_account_id": "account", "chatgpt_user_id": "user"}}
        self.tokens = {"access_token": token(self.claims), "account_id": "account"}

    def test_scope_is_account_and_user_bound_with_provisional_fallback(self):
        original = history.account_scope(self.tokens)
        expected = hashlib.sha256(b'["chatgpt-request-identity-v1","account","user"]').hexdigest()
        self.assertEqual(original, "chatgpt:observed:" + expected)
        other = dict(self.tokens, access_token=token({**self.claims, "https://api.openai.com/auth": {
            "chatgpt_account_id": "other", "chatgpt_user_id": "user"}}))
        self.assertNotEqual(original, history.account_scope(other))
        fallback = {"access_token": token({"exp": 123, "sub": "auth0|subject"}), "account_id": "a"}
        self.assertTrue(history.account_scope(fallback).startswith("chatgpt:oauth:"))
        with self.assertRaisesRegex(history.HistoryError, "auth_invalid"):
            history.account_scope({"access_token": token({"sub": ""}), "account_id": ""})

    def test_fetch_keeps_full_graph_and_skips_unchanged_details(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            scope = history.account_scope(self.tokens)
            store = history.RawGraphStore(root / "state", scope)
            client = FakeClient()
            manifest = history.fetch_job(client, store, scope, "0" * 24, root / "out")
            self.assertTrue(manifest["complete"])
            self.assertEqual(manifest["conversations"], 1)
            self.assertEqual(client.batch_calls, [["c1"]])
            data = json.loads((root / "out" / manifest["exports"][0]).read_text())
            self.assertEqual(data["format"], "chat-hub-history-v1")
            self.assertEqual(data["account_scope"], scope)
            self.assertEqual(data["conversations"][0]["mapping"]["root"]["message"]["content"]["parts"], ["full transcript"])
            next_client = FakeClient()
            second = history.fetch_job(next_client, store, scope, "0" * 24, root / "out2")
            self.assertTrue(second["complete"])
            self.assertEqual(next_client.batch_calls, [])
            store.close()

    def test_429_persists_backoff_and_exports_previously_cached_graph(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            scope = history.account_scope(self.tokens)
            store = history.RawGraphStore(root / "state", scope)
            graph = {"id": "old", "mapping": {"root": None}}
            store.put("old", "old-tag", graph)
            client = FakeClient()
            client.raise_archived = True
            manifest = history.fetch_job(client, store, scope, "0" * 24, root / "partial")
            self.assertFalse(manifest["complete"])
            self.assertEqual(manifest["error_code"], "rate_limited")
            self.assertEqual(manifest["stage"], "list")
            self.assertGreaterEqual(manifest["retry_after"], 1)
            cached = json.loads((root / "partial" / manifest["exports"][0]).read_text())
            self.assertEqual([g["id"] for g in cached["conversations"]], ["old"])
            blocked = history.fetch_job(FakeClient(), store, scope, "0" * 24, root / "blocked")
            self.assertEqual(blocked["error_code"], "rate_limited")
            store.close()

    def test_expired_access_token_fails_without_refresh_or_mutation(self):
        expired = {"access_token": token({"exp": 1, "sub": "s"}), "refresh_token": "untouched"}
        with self.assertRaisesRegex(history.HistoryError, "login_required"):
            history.Client(expired, session=object())
        self.assertEqual(expired["refresh_token"], "untouched")

    def test_probe_makes_only_normal_list_requests(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            scope = history.account_scope(self.tokens)
            store = history.RawGraphStore(root / "state", scope)
            client = FakeClient()
            result = history.fetch_job(client, store, scope, "0" * 24, root / "probe", probe=True)
            self.assertTrue(result["complete"])
            self.assertEqual(result["listed"], 1)
            self.assertEqual(result["total"], 1)
            self.assertTrue(result["probe"])
            self.assertEqual(len(client.get_calls), 1)
            self.assertEqual(client.get_calls[0][1]["limit"], 1)
            self.assertEqual(client.batch_calls, [])
            self.assertFalse((root / "probe").exists())
            store.close()

    def test_response_is_closed_after_rate_limit(self):
        class Response:
            status_code = 429
            headers = {"retry-after": "9"}
            closed = False
            def close(self): self.closed = True

        class Session:
            def __init__(self): self.response = Response()
            def request(self, *args, **kwargs): return self.response

        session = Session()
        client = object.__new__(history.Client)
        client.session = session
        client.headers = {}
        with self.assertRaisesRegex(history.HistoryError, "rate_limited"):
            client.request("conversations", stage="list")
        self.assertTrue(session.response.closed)

    def test_invalid_graph_is_not_cached_as_complete(self):
        class BadGraphClient(FakeClient):
            def batch(self, ids): return [{"id": ids[0], "title": "missing mapping"}]

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            scope = history.account_scope(self.tokens)
            store = history.RawGraphStore(root / "state", scope)
            result = history.fetch_job(BadGraphClient(), store, scope, "0" * 24, root / "bad")
            self.assertFalse(result["complete"])
            self.assertEqual(result["error_code"], "invalid_response")
            self.assertEqual(store.count(), 0)
            store.close()

    def test_exports_soft_chunk_by_document_count_and_refuses_database_symlink(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            scope = history.account_scope(self.tokens)
            store = history.RawGraphStore(root / "state", scope)
            for i in range(101):
                store.put(f"c{i:03}", str(i), {"id": f"c{i:03}", "mapping": {}})
            manifest = history._export_cached(store, {
                "owner": "0" * 24, "account_scope": scope, "exports": [], "conversations": 0,
                "complete": True, "error_code": "", "retry_after": 0,
            }, root / "exports")
            self.assertEqual(manifest["conversations"], 101)
            envelopes = [json.loads((root / "exports" / name).read_text()) for name in manifest["exports"]]
            self.assertEqual([len(e["conversations"]) for e in envelopes], [100, 1])
            self.assertEqual(stat.S_IMODE(store.path.stat().st_mode), 0o600)
            store.close()

            state = root / "linked-state"
            state.mkdir(mode=0o700)
            link_path = state / (hashlib.sha256(scope.encode()).hexdigest() + ".sqlite3")
            target = root / "target.sqlite3"
            target.touch()
            link_path.symlink_to(target)
            with self.assertRaisesRegex(history.HistoryError, "state_unavailable"):
                history.RawGraphStore(state, scope)


if __name__ == "__main__":
    unittest.main()
