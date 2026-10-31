#!/usr/bin/env python3
"""Stream the selected deployed plugin's consistent cache snapshot as a tar.

Run inside the existing Paseo container. No network requests or credential
refreshes are made; plugin Archive selects the same account as its history UI.
Only the cache and its opaque owner hash leave the container, never credentials.
"""
import io
import re
import sqlite3
import sys
import tarfile
import tempfile
from pathlib import Path

sys.path.insert(0, "/opt/ginnungagap/paseo/plugins/chatgpt-history/server")
from history import Archive


def main():
    archive = Archive()
    try:
        if not re.fullmatch(r"[0-9a-f]{24}", archive.owner):
            raise RuntimeError("invalid cache owner")
        with tempfile.TemporaryDirectory(prefix="chat-hub-cache-snapshot-") as temporary:
            snapshot = Path(temporary) / "history.sqlite3"
            destination = sqlite3.connect(snapshot)
            try:
                archive.db.backup(destination)
            finally:
                destination.close()
            with tarfile.open(fileobj=sys.stdout.buffer, mode="w|") as output:
                info = output.gettarinfo(str(snapshot), arcname="history.sqlite3")
                info.mode = 0o600
                with snapshot.open("rb") as source:
                    output.addfile(info, source)
                owner = archive.owner.encode("ascii")
                info = tarfile.TarInfo("owner.txt")
                info.size = len(owner)
                info.mode = 0o600
                output.addfile(info, io.BytesIO(owner))
    finally:
        archive.db.close()


if __name__ == "__main__":
    main()
