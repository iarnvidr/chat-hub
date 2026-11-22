"""Prompt sending in the headless sidecar, with a fake page and no browser."""
import asyncio
import json
import pathlib
import sys
import tempfile
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1] / "server-browser"))
import browser as browser_module  # noqa: E402
from collector import Collector  # noqa: E402
from main import authorized, load_secret, status_server  # noqa: E402
from policy import MAX_PROMPT_BYTES, prompt_target_url, prompt_text_ok  # noqa: E402
from prompts import PromptJobs  # noqa: E402

CID = "0a1b2c3d-0000-4000-8000-0123456789ab"
SECRET = "synthetic-sidecar-secret"
TEXT = "synthetic prompt text"


class FakeLocator:
    def __init__(self, page, selector):
        self.page, self.selector = page, selector

    @property
    def first(self):
        return self

    async def wait_for(self, state, timeout):
        self.page.waits.append((self.selector, state))
        if self.page.fail_composer and state == "visible" and "prompt-textarea" in self.selector:
            raise RuntimeError("synthetic timeout")
        if self.page.fast_reply and self.selector == browser_module.GENERATING_INDICATOR and state == "visible":
            raise RuntimeError("synthetic: indicator never seen")

    async def count(self):
        return 1 if self.page.login_button and self.selector == browser_module.LOGIN_BUTTON else 0

    async def click(self, timeout=None):
        self.page.events.append("click")

    async def fill(self, text, timeout=None):
        self.page.events.append("fill")
        self.page.typed.append(text)


class FakePage:
    def __init__(self, login_button=False, fail_composer=False, fast_reply=False):
        self.url = "about:blank"
        self.login_button, self.fail_composer, self.fast_reply = login_button, fail_composer, fast_reply
        self.visited, self.waits, self.events, self.typed = [], [], [], []
        self.keyboard = self

    async def goto(self, url, wait_until=None, timeout=None):
        self.url = url
        self.visited.append(url)

    def locator(self, selector):
        return FakeLocator(self, selector)

    async def press(self, key):
        self.events.append(f"press:{key}")


def browser(page):
    b = browser_module.HeadlessBrowser(pathlib.Path(tempfile.mkdtemp()) / "root", 8788)
    b.page = page
    return b


class PolicyTests(unittest.TestCase):
    def test_target_urls_and_text_limits(self):
        self.assertEqual(prompt_target_url(None), "https://chatgpt.com/")
        self.assertEqual(prompt_target_url(CID), f"https://chatgpt.com/c/{CID}")
        for bad in ["", "../x", f"{CID}/../..", "https://evil.example/", CID[:-1], 5, "g" * 36]:
            with self.assertRaises(ValueError):
                prompt_target_url(bad)
        self.assertTrue(prompt_text_ok("x" * MAX_PROMPT_BYTES))
        for bad in ["", "  \n", "x" * (MAX_PROMPT_BYTES + 1), "é" * (MAX_PROMPT_BYTES // 2 + 1), None, 5]:
            self.assertFalse(prompt_text_ok(bad))

    def test_secret_loading_and_comparison(self):
        with tempfile.TemporaryDirectory() as directory:
            path = pathlib.Path(directory) / "secret"
            path.write_text(SECRET + "\n")
            self.assertEqual(load_secret(str(path)), SECRET)
            path.write_text("short")
            with self.assertRaises(ValueError):
                load_secret(str(path))
        self.assertIsNone(load_secret(None))
        self.assertTrue(authorized(f"Bearer {SECRET}", SECRET))
        for bad in ["", SECRET, "Bearer wrong", f"Basic {SECRET}", f"Bearer {SECRET}x"]:
            self.assertFalse(authorized(bad, SECRET))


class SendPromptTests(unittest.IsolatedAsyncioTestCase):
    async def test_types_into_composer_and_submits_with_enter(self):
        page = FakePage()
        self.assertEqual(await browser(page).send_prompt(CID, TEXT), "done")
        self.assertEqual(page.visited, [f"https://chatgpt.com/c/{CID}"])
        self.assertEqual(page.typed, [TEXT])
        self.assertEqual(page.events, ["click", "fill", "press:Enter"])
        self.assertIn((browser_module.GENERATING_INDICATOR, "hidden"), page.waits)

    async def test_new_chat_and_fast_reply(self):
        page = FakePage(fast_reply=True)
        self.assertEqual(await browser(page).send_prompt(None, TEXT), "done")
        self.assertEqual(page.visited, ["https://chatgpt.com/"])

    async def test_signed_out_and_missing_composer_report_states_without_the_text(self):
        page = FakePage(login_button=True, fail_composer=True)
        self.assertEqual(await browser(page).send_prompt(None, TEXT), "needs_sign_in")
        self.assertEqual(page.typed, [])
        page = FakePage(fail_composer=True)
        self.assertEqual(await browser(page).send_prompt(None, TEXT), "error")
        self.assertEqual(page.typed, [])

    async def test_invalid_conversation_never_navigates(self):
        page = FakePage()
        with self.assertRaises(ValueError):
            await browser(page).send_prompt("../etc", TEXT)
        self.assertEqual(page.visited, [])

    async def test_jobs_are_serialized_and_the_sweep_yields_to_a_waiting_prompt(self):
        b = browser(FakePage())
        order, release = [], asyncio.Event()

        async def slow(_page):
            order.append("slow-start")
            await release.wait()
            order.append("slow-end")

        first = asyncio.create_task(b.run_job(slow))
        await asyncio.sleep(0)
        prompt = asyncio.create_task(b.send_prompt(None, TEXT))
        await asyncio.sleep(0.01)
        self.assertEqual(b.prompt_waiting, 1)
        self.assertEqual(order, ["slow-start"])
        self.assertFalse(prompt.done())

        class Sweeper:
            prompt_waiting = 1
            visits = []

            async def goto(self, url):
                self.visits.append(url)

            async def signed_out(self):
                return False

            async def sidebar_links(self):
                return ["/c/id-1", "/c/id-2"]

            async def scroll_sidebar(self):
                pass

        sweeper = Sweeper()

        async def no_sleep(_):
            return None

        collector = Collector(sweeper, settle=0, sleep=no_sleep)
        await collector.sweep()
        self.assertEqual(sweeper.visits, ["https://chatgpt.com/"])  # stopped before any conversation page
        self.assertEqual((collector.state, collector.counters["sweeps"]), ("idle", 0))
        release.set()
        self.assertEqual(await prompt, "done")
        await first
        self.assertEqual((order, b.prompt_waiting), (["slow-start", "slow-end"], 0))


class FakeSender:
    def __init__(self):
        self.calls, self.gate, self.result = [], asyncio.Event(), "done"

    async def send_prompt(self, conversation_id, text):
        self.calls.append((conversation_id, text))
        await self.gate.wait()
        return self.result


class StubCollector:
    state = "idle"

    def status(self):
        return {"state": self.state, "last_sweep": None, "counters": {"sweeps": 0}}


class PromptServerTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.sender = FakeSender()
        self.collector = StubCollector()
        self.prompts = PromptJobs(self.sender)
        self.server = await status_server(self.collector, "127.0.0.1", 0, self.prompts, SECRET)
        self.port = self.server.sockets[0].getsockname()[1]

    async def asyncTearDown(self):
        self.sender.gate.set()
        await self.prompts.stop()
        self.server.close()
        await self.server.wait_closed()

    async def call(self, method, path, body=None, token=SECRET, length=None):
        r, w = await asyncio.open_connection("127.0.0.1", self.port)
        data = b"" if body is None else (body if isinstance(body, bytes) else json.dumps(body).encode())
        head = f"{method} {path} HTTP/1.1\r\nHost: x\r\n"
        if token is not None:
            head += f"Authorization: Bearer {token}\r\n"
        head += f"Content-Length: {len(data) if length is None else length}\r\n\r\n"
        w.write(head.encode() + data)
        raw = (await r.read()).decode()
        w.close()
        head, _, payload = raw.partition("\r\n\r\n")
        return int(head.split()[1]), json.loads(payload or "{}")

    async def test_requires_the_bearer_secret(self):
        body = {"conversation_id": None, "text": TEXT}
        for token in (None, "wrong", SECRET + "x"):
            self.assertEqual((await self.call("POST", "/prompt", body, token=token))[0], 401)
        self.assertEqual(self.sender.calls, [])
        self.assertEqual((await self.call("GET", "/prompt"))[0], 405)

    async def test_accepts_one_prompt_then_reports_busy_and_states(self):
        status, body = await self.call("POST", "/prompt", {"conversation_id": CID, "text": TEXT})
        self.assertEqual((status, body["accepted"]), (202, True))
        self.assertRegex(body["job_id"], r"^[0-9a-f]{16}$")
        await asyncio.sleep(0.01)
        self.assertEqual(self.sender.calls, [(CID, TEXT)])
        self.assertEqual((await self.call("POST", "/prompt", {"conversation_id": None, "text": "second"}))[0], 409)
        status, state = await self.call("GET", "/status")
        self.assertEqual(state["live"], "busy")
        self.assertNotIn(TEXT, json.dumps(state))
        self.sender.gate.set()
        await self.prompts.task
        self.assertEqual((await self.call("GET", "/status"))[1]["live"], "idle")
        self.assertEqual((await self.call("POST", "/prompt", {"conversation_id": None, "text": "again"}))[0], 202)

    async def test_failure_and_sign_in_states_are_metadata_only(self):
        self.sender.gate.set()
        self.sender.result = "needs_sign_in"
        await self.call("POST", "/prompt", {"conversation_id": None, "text": TEXT})
        await self.prompts.task
        state = (await self.call("GET", "/status"))[1]
        self.assertEqual(state["live"], "needs_sign_in")
        self.assertNotIn(TEXT, json.dumps(state))
        self.sender.result = "error"
        await self.call("POST", "/prompt", {"conversation_id": None, "text": TEXT})
        await self.prompts.task
        self.assertEqual((await self.call("GET", "/status"))[1]["live"], "error")
        self.collector.state = "needs_sign_in"
        self.assertEqual((await self.call("GET", "/status"))[1]["live"], "needs_sign_in")

    async def test_length_cap_and_validation(self):
        big = {"conversation_id": None, "text": "x" * (MAX_PROMPT_BYTES + 1)}
        self.assertEqual((await self.call("POST", "/prompt", big))[0], 400)
        self.assertEqual((await self.call("POST", "/prompt", b"x" * 10, length=10_000_000))[0], 413)
        self.assertEqual((await self.call("POST", "/prompt", {"conversation_id": None, "text": TEXT}, length=None))[0], 202)
        self.sender.gate.set()
        await self.prompts.task
        for bad in [b"not json", {"text": TEXT}, {"conversation_id": "../x", "text": TEXT},
                    {"conversation_id": None, "text": ""}, {"conversation_id": 5, "text": TEXT}, [1]]:
            self.assertEqual((await self.call("POST", "/prompt", bad))[0], 400, bad)
        r, w = await asyncio.open_connection("127.0.0.1", self.port)
        w.write(f"POST /prompt HTTP/1.1\r\nAuthorization: Bearer {SECRET}\r\n\r\n".encode())
        self.assertIn(b"411", await r.read())
        w.close()
        self.assertEqual(len(self.sender.calls), 1)


if __name__ == "__main__":
    unittest.main()
