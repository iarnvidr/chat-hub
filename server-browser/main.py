"""Container entrypoint: headless ChatGPT browser + collector + loopback status.

Environment:
  CHAT_HUB_BROWSER_ROOT      persistent volume root (default /var/lib/chat-hub-browser)
  CHAT_HUB_PROXY_ADDR        capture proxy, loopback only (default 127.0.0.1:8788)
  CHAT_HUB_CA_DIR            directory holding the proxy's public ca.pem (required)
  CHAT_HUB_BROWSER_STATUS_ADDR  loopback status listener (default 127.0.0.1:8790)
  CHAT_HUB_SWEEP_INTERVAL_SECONDS  base sweep interval (default 1800)
"""
from __future__ import annotations
import asyncio
import ipaddress
import json
import os
import signal
import sys
from pathlib import Path
from browser import HeadlessBrowser
from collector import Collector


def loopback_addr(value: str) -> tuple[str, int]:
    host, _, port = value.rpartition(":")
    if not ipaddress.ip_address(host.strip("[]")).is_loopback or not 0 < int(port) < 65536:
        raise ValueError("address must be a loopback host:port")
    return host.strip("[]"), int(port)


async def status_server(collector: Collector, host: str, port: int):
    """GET /status only; metadata (state, last sweep, counters), never chat content."""
    async def handle(reader, writer):
        try:
            line = (await asyncio.wait_for(reader.readline(), 5)).decode("latin-1").split()
            while (await asyncio.wait_for(reader.readline(), 5)) not in (b"\r\n", b"\n", b""):
                pass
            if line[:2] == ["GET", "/status"]:
                code, body = "200 OK", json.dumps(collector.status()).encode()
            else:
                code, body = ("405 Method Not Allowed" if line[:1] != ["GET"] else "404 Not Found"), b"{}"
            writer.write(f"HTTP/1.1 {code}\r\nContent-Type: application/json\r\nCache-Control: no-store\r\n"
                         f"Content-Length: {len(body)}\r\nConnection: close\r\n\r\n".encode() + body)
            await writer.drain()
        except (asyncio.TimeoutError, ConnectionError, IndexError):
            pass
        finally:
            writer.close()
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
    server = await status_server(collector, status_host, status_port)
    stop = asyncio.Event()
    for name in (signal.SIGINT, signal.SIGTERM):
        asyncio.get_running_loop().add_signal_handler(name, stop.set)
    try:
        await browser.start()
        collector.start()
        await stop.wait()
    finally:
        await collector.stop()
        await browser.close()
        server.close()
        await server.wait_closed()
    return 0


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
