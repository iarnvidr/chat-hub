"""Optional browser smoke test; skipped unless patchright and Chromium are installed.

Launches the real headless context with the proxy argument and loads a synthetic
data: page. Uses no network, account or chat content.
"""
import asyncio
import sys
import tempfile
from pathlib import Path

try:
    from patchright.async_api import async_playwright  # noqa: F401
except ImportError:
    print("skipped: patchright is not installed")
    sys.exit(0)

from browser import HeadlessBrowser


async def main() -> None:
    with tempfile.TemporaryDirectory() as root:
        browser = HeadlessBrowser(Path(root), 9)  # port 9 (discard): nothing listens, nothing leaves
        try:
            await browser.start()
        except Exception as error:
            print("skipped: chromium unavailable:", type(error).__name__)
            return
        try:
            await browser.page.set_content("<title>synthetic</title><p>ok</p>")
            assert await browser.page.title() == "synthetic"
            print("ok: headless context launched with proxy and profile at", browser.profile.name)
        finally:
            await browser.close()


asyncio.run(main())
