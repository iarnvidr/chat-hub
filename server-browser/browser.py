"""Headless server Chromium whose entire traffic goes through the capture proxy.

No CDP port, screencast, input surface or script evaluation is exposed. The
sign-in session lives only in `<root>/profile`, which is also Chromium's HOME so
its NSS database (holding the capture CA) never touches the system trust store.
"""
from __future__ import annotations
import asyncio
import os
import subprocess
from pathlib import Path
from policy import conversation_path, navigation_url, prompt_target_url, request_allowed, signed_out_url

NSS_NICKNAME = "chat-hub-capture-ca"
LOGIN_BUTTON = '[data-testid="login-button"]'
# ChatGPT page selectors used to send a prompt. They are guesses from the public
# page and need acceptance against the real page; keep every one of them here.
COMPOSER_SELECTORS = ("#prompt-textarea", 'div.ProseMirror[contenteditable="true"]', "textarea[name='prompt-textarea']")
GENERATING_INDICATOR = '[data-testid="stop-button"]'


def trust_ca(profile_home: Path, ca_cert: Path, certutil: str = "certutil") -> None:
    """Import the capture CA (PEM, `<CHAT_HUB_CA_DIR>/ca.pem`) into the profile's NSS db.

    Requires `certutil` (Debian package libnss3-tools). Never system-wide.
    """
    ca_cert = Path(ca_cert)
    if not ca_cert.is_file() or b"BEGIN CERTIFICATE" not in ca_cert.read_bytes():
        raise ValueError("capture CA certificate is missing or not PEM")
    nssdb = Path(profile_home) / ".pki" / "nssdb"
    nssdb.mkdir(parents=True, exist_ok=True, mode=0o700)
    os.chmod(nssdb, 0o700)
    db = f"sql:{nssdb}"
    if not (nssdb / "cert9.db").exists():
        subprocess.run([certutil, "-N", "-d", db, "--empty-password"], check=True, timeout=30)
    # Replace any older CA under the same nickname so a rotated CA is picked up.
    subprocess.run([certutil, "-D", "-d", db, "-n", NSS_NICKNAME], check=False, timeout=30,
                   stderr=subprocess.DEVNULL)
    subprocess.run([certutil, "-A", "-d", db, "-n", NSS_NICKNAME, "-t", "C,,", "-i", str(ca_cert)],
                   check=True, timeout=30)


class HeadlessBrowser:
    def __init__(self, root: Path, proxy_port: int, ca_cert: Path | None = None):
        self.root, self.proxy_port, self.ca_cert = Path(root), proxy_port, ca_cert
        self.root.mkdir(parents=True, exist_ok=True, mode=0o700)
        os.chmod(self.root, 0o700)
        self.profile = self.root / "profile"
        self.context = self.driver = None
        self.page = None
        self._jobs = asyncio.Lock()
        self.prompt_waiting = 0  # the history sweep yields while a prompt job waits

    async def start(self) -> None:
        if self.context:
            return
        self.profile.mkdir(exist_ok=True, mode=0o700)
        if self.ca_cert:
            trust_ca(self.root, self.ca_cert)
        from patchright.async_api import async_playwright
        self.driver = await async_playwright().start()
        try:
            self.context = await self.driver.chromium.launch_persistent_context(
                str(self.profile), headless=True, accept_downloads=False,
                service_workers="block", chromium_sandbox=True,
                proxy={"server": f"http://127.0.0.1:{self.proxy_port}"},
                env={**os.environ, "HOME": str(self.root)},  # Chromium's NSS db lives in the profile root
                args=["--disable-dev-shm-usage", "--mute-audio", "--disable-background-networking"])
            await self.context.route("**/*", self._route)
            self.page = self.context.pages[0] if self.context.pages else await self.context.new_page()
            self.page.on("download", lambda download: asyncio.create_task(download.cancel()))
        except BaseException:
            await self.close()
            raise

    async def _route(self, route, request) -> None:
        if request_allowed(request.url, request.resource_type, request.is_navigation_request()):
            await route.continue_()
        else:
            await route.abort("blockedbyclient")

    async def goto(self, url: str) -> None:
        await self.page.goto(navigation_url(url), wait_until="domcontentloaded", timeout=45000)

    def current_url(self) -> str:
        return self.page.url

    async def signed_out(self) -> bool:
        # Logged-out chatgpt.com can render without redirecting; its login button is the tell.
        return signed_out_url(self.page.url) or await self.page.locator(LOGIN_BUTTON).count() > 0

    async def sidebar_links(self) -> list[str]:
        hrefs = await self.page.locator('a[href^="/c/"]').evaluate_all("els => els.map(e => e.getAttribute('href'))")
        return [path for path in map(conversation_path, hrefs) if path]

    async def scroll_sidebar(self) -> None:
        links = self.page.locator('a[href^="/c/"]')
        if await links.count():
            await links.last.scroll_into_view_if_needed(timeout=5000)

    async def run_job(self, job, priority: bool = False):
        """Serialize browser work: one job at a time on the single page."""
        if priority:
            self.prompt_waiting += 1
        try:
            async with self._jobs:
                if priority:
                    self.prompt_waiting -= 1
                    priority = False
                return await job(self.page)
        finally:
            if priority:
                self.prompt_waiting -= 1

    async def send_prompt(self, conversation_id: str | None, text: str, timeout: float = 180.0) -> str:
        """Type `text` into the ChatGPT composer and wait for the reply to finish.

        Returns `done`, `needs_sign_in` or `error`. The text is only handed to the
        page: it is never logged, stored or put in an exception message.
        """
        url = prompt_target_url(conversation_id)

        async def job(page) -> str:
            composer_css = ", ".join(COMPOSER_SELECTORS)
            await self.goto(url)
            try:  # either the composer or the login button settles the page
                await page.locator(f"{composer_css}, {LOGIN_BUTTON}").first.wait_for(state="visible", timeout=30000)
            except Exception:
                return "needs_sign_in" if await self.signed_out() else "error"
            if await self.signed_out():
                return "needs_sign_in"
            try:
                composer = page.locator(composer_css).first
                await composer.click(timeout=10000)
                await composer.fill(text, timeout=15000)
                await page.keyboard.press("Enter")
                indicator = page.locator(GENERATING_INDICATOR).first
                try:  # a very fast reply can finish before the indicator is seen
                    await indicator.wait_for(state="visible", timeout=15000)
                except Exception:
                    pass
                await indicator.wait_for(state="hidden", timeout=int(timeout * 1000))
            except Exception:  # never include the message: it may embed the prompt
                return "error"
            return "done"

        return await self.run_job(job, priority=True)

    async def close(self) -> None:
        if self.context:
            await self.context.close()
        if self.driver:
            await self.driver.stop()
        self.context = self.driver = self.page = None
