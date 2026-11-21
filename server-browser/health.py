"""Container HEALTHCHECK: the status endpoint answers on loopback."""
import os
import sys
import urllib.request

addr = os.environ.get("CHAT_HUB_BROWSER_STATUS_ADDR", "127.0.0.1:8790")
try:
    urllib.request.urlopen(f"http://{addr}/status", timeout=3).read()
except Exception:
    sys.exit(1)
