#!/usr/bin/env python3
"""Smoke-test the packaged Chat Hub server against an empty synthetic library."""
from __future__ import annotations

import json
import os
from pathlib import Path
import sqlite3
import subprocess
import tempfile
import time
from urllib.error import URLError
from urllib.request import urlopen


def main() -> None:
    with tempfile.TemporaryDirectory(prefix="chat-hub-web-smoke-", dir="/tmp") as temporary:
        root = Path(temporary)
        os.chmod(root, 0o700)
        database = root / "library.sqlite3"
        with sqlite3.connect(database):
            pass
        token_file = root / "access-token"
        token_file.write_text("synthetic-smoke-token\n", encoding="utf-8")
        os.chmod(token_file, 0o600)

        environment = os.environ.copy()
        environment.update({
            "CHAT_HUB_DATABASE": str(database),
            "CHAT_HUB_WEB_TOKEN_FILE": str(token_file),
            "CHAT_HUB_DESKTOP_UI": "/app/desktop-ui",
            "CHAT_HUB_WEB_ADDR": "127.0.0.1:18080",
            "CHAT_HUB_CAPTURE_ENABLED": "false",
        })
        process = subprocess.Popen(
            ["/usr/local/bin/chat-hub-web"],
            env=environment,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        try:
            deadline = time.monotonic() + 15
            while time.monotonic() < deadline:
                if process.poll() is not None:
                    raise RuntimeError("server exited before health became ready")
                try:
                    with urlopen("http://127.0.0.1:18080/healthz", timeout=0.5) as response:
                        payload = json.loads(response.read(4096))
                    if payload != {"status": "ok", "capture": "disabled"}:
                        raise RuntimeError("health response did not match the safe smoke contract")
                    return
                except (OSError, URLError, TimeoutError, json.JSONDecodeError):
                    time.sleep(0.1)
            raise RuntimeError("server health endpoint did not become ready")
        finally:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=3)


if __name__ == "__main__":
    main()
