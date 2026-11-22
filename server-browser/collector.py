"""Background history sweep: ordinary page navigation makes ChatGPT's own page fetch
`GET /backend-api/conversation/<id>`, the only history response the capture lane
stores (see docs/headless-capture.md). Nothing here calls the API or reads content.
"""
from __future__ import annotations
import asyncio
import random
import time
from policy import START_URL


class Collector:
    def __init__(self, browser, interval: float = 1800.0, recent: int = 10, backfill: int = 10,
                 settle: float = 4.0, rng: random.Random | None = None, sleep=asyncio.sleep):
        self.browser, self.interval, self.recent, self.backfill = browser, interval, recent, backfill
        self.settle, self.rng, self.sleep = settle, rng or random.Random(), sleep
        self.state = "starting"
        self.last_sweep: float | None = None
        self.counters = {"sweeps": 0, "visited": 0, "errors": 0}
        self.visited: set[str] = set()  # in memory only; a restart re-backfills, the store dedupes
        self.task: asyncio.Task | None = None

    def status(self) -> dict:
        return {"state": self.state, "last_sweep": self.last_sweep, "counters": dict(self.counters)}

    async def _pause(self, base: float) -> None:
        await self.sleep(base * self.rng.uniform(0.5, 1.5) if base else 0)

    async def sweep(self) -> None:
        """One pass. Sets needs_sign_in and returns when the session is signed out."""
        self.state = "sweeping"
        await self.browser.goto(START_URL)
        await self._pause(self.settle)
        if await self.browser.signed_out():
            self.state = "needs_sign_in"
            return
        links = list(dict.fromkeys(await self.browser.sidebar_links()))
        for _ in range(3):  # lazy-loaded sidebar pages
            await self.browser.scroll_sidebar()
            await self._pause(self.settle / 2)
            more = await self.browser.sidebar_links()
            if len(set(more) | set(links)) == len(links):
                break
            links = list(dict.fromkeys(links + more))
        targets = links[:self.recent]
        targets += [p for p in links[self.recent:] if p not in self.visited][:self.backfill]
        for path in targets:
            await self.browser.goto("https://chatgpt.com" + path)
            await self._pause(self.settle)
            if await self.browser.signed_out():
                self.state = "needs_sign_in"
                return
            self.visited.add(path)
            self.counters["visited"] += 1
        self.counters["sweeps"] += 1
        self.last_sweep = time.time()
        self.state = "idle"

    async def _guarded_sweep(self) -> None:
        try:
            await self.browser.run_job(lambda _page: self.sweep())
        except asyncio.CancelledError:
            raise
        except Exception:  # never log the message: it may embed page content
            self.counters["errors"] += 1
            self.state = "error"

    async def run(self) -> None:
        while True:
            await self._guarded_sweep()
            await self.sleep(self.interval * self.rng.uniform(0.8, 1.2))

    def start(self) -> None:
        self.task = asyncio.create_task(self.run())

    async def stop(self) -> None:
        if self.task:
            self.task.cancel()
            try:
                await self.task
            except asyncio.CancelledError:
                pass
