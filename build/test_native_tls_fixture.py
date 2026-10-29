"""Behavioral regression: portable certificate generation and live TLS fixture."""
import pathlib
import ssl
import subprocess
import sys
import tempfile
import time
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parents[1]
FIXTURE = ROOT / "build/native-tls-fixture.py"

with tempfile.TemporaryDirectory() as directory:
    work = pathlib.Path(directory)
    cert = work / "upstream.pem"
    subprocess.run([sys.executable, str(ROOT / "build/native-tls-cert.py"), str(cert)], check=True, stdout=subprocess.DEVNULL)
    addrfile = work / "upstream.addr"
    process = subprocess.Popen([sys.executable, str(FIXTURE), str(cert), str(work / "evidence.json"), str(addrfile)])
    try:
        for _ in range(100):
            if addrfile.exists() and addrfile.stat().st_size:
                break
            if process.poll() is not None:
                raise RuntimeError(f"TLS fixture exited at startup: {process.returncode}")
            time.sleep(0.05)
        assert addrfile.exists() and addrfile.stat().st_size, "fixture did not publish its address"
        host, port = addrfile.read_text().split(":")
        context = ssl.create_default_context(cafile=str(cert))
        with urllib.request.urlopen(f"https://{host}:{port}/", context=context, timeout=3) as response:
            assert response.status == 200
            assert b"synthetic-native-smoke" in response.read()
    finally:
        process.terminate()
        process.wait(timeout=3)
print("native TLS fixture certificate generation and HTTPS readiness passed")
