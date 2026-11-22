"""Single in-flight prompt job on top of `HeadlessBrowser.send_prompt`.

The queue is the browser's job lock; this object only refuses a second prompt
while one is running and remembers the last outcome as metadata. It never keeps
the prompt text beyond the running task.
"""
from __future__ import annotations
import asyncio
import secrets

OUTCOMES = {"done": "idle", "needs_sign_in": "needs_sign_in", "error": "error"}


class PromptJobs:
    def __init__(self, browser):
        self.browser = browser
        self.task: asyncio.Task | None = None
        self.last = "idle"  # idle | needs_sign_in | error

    def busy(self) -> bool:
        return self.task is not None and not self.task.done()

    def submit(self, conversation_id: str | None, text: str) -> str | None:
        """Start a job and return its id, or None when one is already running."""
        if self.busy():
            return None
        job_id = secrets.token_hex(8)
        self.task = asyncio.create_task(self._run(conversation_id, text))
        return job_id

    async def _run(self, conversation_id, text) -> None:
        try:
            self.last = OUTCOMES.get(await self.browser.send_prompt(conversation_id, text), "error")
        except asyncio.CancelledError:
            raise
        except Exception:  # never log the message: it may embed the prompt
            self.last = "error"

    def live_state(self, collector_state: str) -> str:
        """idle | busy | needs_sign_in | error, as reported to the web server."""
        if self.busy():
            return "busy"
        return "needs_sign_in" if collector_state == "needs_sign_in" else self.last

    async def stop(self) -> None:
        if self.task and not self.task.done():
            self.task.cancel()
            try:
                await self.task
            except asyncio.CancelledError:
                pass
