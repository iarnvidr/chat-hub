"""Collector state machine, trust_ca, profile permissions and status endpoint, without a browser."""
import asyncio
import json
import os
import pathlib
import stat
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1] / "server-browser"))
import browser as browser_module  # noqa: E402
from collector import Collector  # noqa: E402
from main import loopback_addr, status_server  # noqa: E402

CA = "-----BEGIN CERTIFICATE-----\nMIIBsynthetic\n-----END CERTIFICATE-----\n"


class FakeBrowser:
    def __init__(self, links, signed_out_after=None):
        self.links, self.signed_out_after, self.visits = links, signed_out_after, []

    async def run_job(self, job):
        return await job(None)

    async def goto(self, url):
        self.visits.append(url)

    async def signed_out(self):
        return self.signed_out_after is not None and len(self.visits) > self.signed_out_after

    async def sidebar_links(self):
        return list(self.links)

    async def scroll_sidebar(self):
        pass


def collector(browser, **kw):
    async def no_sleep(_):
        return None
    return Collector(browser, settle=0, sleep=no_sleep, **kw)


class CollectorTests(unittest.IsolatedAsyncioTestCase):
    async def test_sweep_visits_recent_then_backfills_once(self):
        links = [f"/c/id-{i}" for i in range(8)]
        fake = FakeBrowser(links)
        c = collector(fake, recent=2, backfill=3)
        await c.sweep()
        self.assertEqual(fake.visits[0], "https://chatgpt.com/")
        self.assertEqual([v[-4:] for v in fake.visits[1:]], ["id-0", "id-1", "id-2", "id-3", "id-4"])
        self.assertEqual((c.state, c.counters["visited"], c.counters["sweeps"]), ("idle", 5, 1))
        fake.visits.clear()
        await c.sweep()
        self.assertEqual([v[-4:] for v in fake.visits[1:]], ["id-0", "id-1", "id-5", "id-6", "id-7"])

    async def test_signed_out_idles_without_visiting(self):
        fake = FakeBrowser(["/c/id-1"], signed_out_after=0)
        c = collector(fake)
        await c.sweep()
        self.assertEqual((c.state, fake.visits, c.counters["visited"]), ("needs_sign_in", ["https://chatgpt.com/"], 0))

    async def test_sign_out_mid_sweep_stops(self):
        fake = FakeBrowser(["/c/id-1", "/c/id-2"], signed_out_after=1)
        c = collector(fake)
        await c.sweep()
        self.assertEqual(c.state, "needs_sign_in")
        self.assertEqual(len(fake.visits), 2)

    async def test_failure_sets_error_and_loop_continues(self):
        fake = FakeBrowser([])
        fake.goto = mock.AsyncMock(side_effect=RuntimeError("synthetic"))
        c = collector(fake)
        await c._guarded_sweep()
        self.assertEqual((c.state, c.counters["errors"]), ("error", 1))
        self.assertNotIn("synthetic", json.dumps(c.status()))

    async def test_status_endpoint_is_get_status_only(self):
        c = collector(FakeBrowser([]))
        server = await status_server(c, "127.0.0.1", 0)
        port = server.sockets[0].getsockname()[1]

        async def request(line):
            r, w = await asyncio.open_connection("127.0.0.1", port)
            w.write(f"{line} HTTP/1.1\r\nHost: x\r\n\r\n".encode())
            data = await r.read()
            w.close()
            return data.decode()

        ok = await request("GET /status")
        self.assertIn("200 OK", ok)
        self.assertEqual(set(json.loads(ok.split("\r\n\r\n", 1)[1])), {"state", "last_sweep", "counters"})
        self.assertIn("404", await request("GET /evaluate"))
        self.assertIn("405", await request("POST /status"))
        server.close()
        await server.wait_closed()


class SetupTests(unittest.TestCase):
    def test_trust_ca_scopes_import_to_profile_nssdb(self):
        with tempfile.TemporaryDirectory() as home:
            ca = pathlib.Path(home) / "ca.pem"
            ca.write_text(CA)
            calls = []
            with mock.patch.object(browser_module.subprocess, "run", side_effect=lambda cmd, **kw: calls.append(cmd)):
                browser_module.trust_ca(pathlib.Path(home) / "root", ca, certutil="fake-certutil")
            db = f"sql:{home}/root/.pki/nssdb"
            self.assertEqual(calls[0], ["fake-certutil", "-N", "-d", db, "--empty-password"])
            self.assertEqual(calls[-1], ["fake-certutil", "-A", "-d", db, "-n", "chat-hub-capture-ca", "-t", "C,,", "-i", str(ca)])
            self.assertEqual(stat.S_IMODE(os.stat(f"{home}/root/.pki/nssdb").st_mode), 0o700)
            self.assertTrue(all("/etc/ssl" not in " ".join(c) and "/usr/local/share" not in " ".join(c) for c in calls))

    def test_trust_ca_rejects_non_pem(self):
        with tempfile.TemporaryDirectory() as home:
            bad = pathlib.Path(home) / "ca.pem"
            bad.write_bytes(b"\x30\x82binary")
            with self.assertRaises(ValueError):
                browser_module.trust_ca(pathlib.Path(home), bad)
            with self.assertRaises(ValueError):
                browser_module.trust_ca(pathlib.Path(home), pathlib.Path(home) / "missing.pem")

    def test_profile_root_is_private(self):
        with tempfile.TemporaryDirectory() as parent:
            root = pathlib.Path(parent) / "state"
            root.mkdir(mode=0o755)
            browser_module.HeadlessBrowser(root, 8788)
            self.assertEqual(stat.S_IMODE(root.stat().st_mode), 0o700)

    def test_loopback_only_addresses(self):
        self.assertEqual(loopback_addr("127.0.0.1:8788"), ("127.0.0.1", 8788))
        for bad in ["0.0.0.0:8788", "10.0.0.5:1", "127.0.0.1:0", "nonsense"]:
            with self.assertRaises(ValueError):
                loopback_addr(bad)


if __name__ == "__main__":
    unittest.main()
