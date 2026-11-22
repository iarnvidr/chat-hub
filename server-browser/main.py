"""Container entrypoint: headless ChatGPT browser + collector + loopback status/prompt.

Environment:
  CHAT_HUB_BROWSER_ROOT      persistent volume root (default /var/lib/chat-hub-browser)
  CHAT_HUB_PROXY_ADDR        capture proxy, loopback only (default 127.0.0.1:8788)
  CHAT_HUB_CA_DIR            directory holding the proxy's public ca.pem (required)
  CHAT_HUB_BROWSER_STATUS_ADDR  loopback listener for GET /status and POST /prompt (default 127.0.0.1:8790)
  CHAT_HUB_BROWSER_SECRET_FILE  file holding the bearer secret for POST /prompt (unset: prompts disabled)
  CHAT_HUB_SWEEP_INTERVAL_SECONDS  base sweep interval (default 1800)
"""
from __future__ import annotations
import asyncio
import hmac
import ipaddress
import json
import os
import signal
import sys
from pathlib import Path
from browser import HeadlessBrowser
from collector import Collector
from policy import prompt_target_url, prompt_text_ok
from prompts import PromptJobs


def loopback_addr(value: str) -> tuple[str, int]:
    host, _, port = value.rpartition(":")
    if not ipaddress.ip_address(host.strip("[]")).is_loopback or not 0 < int(port) < 65536:
        raise ValueError("address must be a loopback host:port")
    return host.strip("[]"), int(port)


MAX_BODY = 48 * 1024  # prompt text is capped at 32 KiB; JSON escaping can inflate it


def load_secret(path: str | None) -> str | None:
    """Shared bearer secret for POST /prompt, read from a file. None disables prompts."""
    if not path:
        return None
    secret = Path(path).read_text().strip()
    if len(secret) < 16 or not secret.isascii() or not secret.isprintable() or " " in secret:
        raise ValueError("CHAT_HUB_BROWSER_SECRET_FILE must hold a 16+ character printable secret")
    return secret


def authorized(header: str, secret: str) -> bool:
    scheme, _, token = header.partition(" ")
    return scheme.lower() == "bearer" and hmac.compare_digest(token.encode("utf-8"), secret.encode("utf-8"))


async def status_server(collector: Collector, host: str, port: int,
                        prompts: PromptJobs | None = None, secret: str | None = None):
    """Loopback control surface.

    GET /status: metadata only (state, last sweep, counters, live state), never chat content.
    POST /prompt: bearer secret required; {conversation_id|null, text}. The text is
    handed to the browser job and never logged, echoed or reported by /status.
    """
    def status() -> dict:
        body = collector.status()
        if prompts:
            body["live"] = prompts.live_state(collector.state)
        return body

    async def handle(reader, writer):
        def reply(code: str, body: dict) -> None:
            data = json.dumps(body).encode()
            writer.write(f"HTTP/1.1 {code}\r\nContent-Type: application/json\r\nCache-Control: no-store\r\n"
                         f"Content-Length: {len(data)}\r\nConnection: close\r\n\r\n".encode() + data)

        try:
            line = (await asyncio.wait_for(reader.readline(), 5)).decode("latin-1").split()
            headers: dict[str, str] = {}
            for _ in range(64):
                raw = (await asyncio.wait_for(reader.readline(), 5)).decode("latin-1")
                if raw in ("\r\n", "\n", ""):
                    break
                name, _, value = raw.partition(":")
                headers[name.strip().lower()] = value.strip()
            if line[:2] == ["GET", "/status"]:
                reply("200 OK", status())
            elif line[:2] == ["POST", "/prompt"] and prompts and secret:
                await prompt(reader, reply, headers)
            else:
                method, path = (line + ["", ""])[:2]
                wrong_method = method not in ("GET", "POST") or (path, method) in (("/status", "POST"), ("/prompt", "GET"))
                reply("405 Method Not Allowed" if wrong_method else "404 Not Found", {})
            await writer.drain()
        except (asyncio.TimeoutError, ConnectionError, IndexError, asyncio.IncompleteReadError, ValueError):
            pass
        finally:
            writer.close()

    async def prompt(reader, reply, headers) -> None:
        if not authorized(headers.get("authorization", ""), secret):
            return reply("401 Unauthorized", {"code": "unauthorized"})
        length = headers.get("content-length", "")
        if not length.isascii() or not length.isdigit():
            return reply("411 Length Required", {"code": "length_required"})
        if int(length) > MAX_BODY:
            return reply("413 Payload Too Large", {"code": "too_large"})
        try:
            request = json.loads(await asyncio.wait_for(reader.readexactly(int(length)), 10))
            conversation_id = request["conversation_id"]
            text = request["text"]
            prompt_target_url(conversation_id)
        except (ValueError, KeyError, TypeError):
            return reply("400 Bad Request", {"code": "invalid_request"})
        if not prompt_text_ok(text):
            return reply("400 Bad Request", {"code": "invalid_request"})
        job_id = prompts.submit(conversation_id, text)
        if job_id is None:
            return reply("409 Conflict", {"code": "busy"})
        reply("202 Accepted", {"accepted": True, "job_id": job_id})

    return await asyncio.start_server(handle, host, port)


async def main() -> int:
    ca_cert = Path(os.environ["CHAT_HUB_CA_DIR"]) / "ca.pem"
    _, proxy_port = loopback_addr(os.environ.get("CHAT_HUB_PROXY_ADDR", "127.0.0.1:8788"))
    status_host, status_port = loopback_addr(os.environ.get("CHAT_HUB_BROWSER_STATUS_ADDR", "127.0.0.1:8790"))
    for _ in range(60):  # chat-hub-web creates the CA on first capture start
        if ca_cert.is_file():
            break
        await asyncio.sleep(1)
    browser = HeadlessBrowser(Path(os.environ.get("CHAT_HUB_BROWSER_ROOT", "/var/lib/chat-hub-browser")),
                              proxy_port, ca_cert)
    collector = Collector(browser, float(os.environ.get("CHAT_HUB_SWEEP_INTERVAL_SECONDS", "1800")))
    prompts = PromptJobs(browser)
    server = await status_server(collector, status_host, status_port, prompts,
                                 load_secret(os.environ.get("CHAT_HUB_BROWSER_SECRET_FILE")))
    stop = asyncio.Event()
    for name in (signal.SIGINT, signal.SIGTERM):
        asyncio.get_running_loop().add_signal_handler(name, stop.set)
    try:
        await browser.start()
        collector.start()
        await stop.wait()
    finally:
        await collector.stop()
        await prompts.stop()
        await browser.close()
        server.close()
        await server.wait_closed()
    return 0


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
