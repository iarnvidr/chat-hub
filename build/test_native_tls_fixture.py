"""Behavioral regression: portable certificate generation and live TLS fixture."""
import pathlib
import ssl
import socket
import os
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
    # Loopback binding must not call HTTPServer's reverse-DNS getfqdn path.
    probe = work / 'probe.py'
    probe.write_text('import runpy, socket, sys\nsocket.getfqdn = lambda *a, **k: (_ for _ in ()).throw(RuntimeError("reverse DNS called"))\nfixture, *args = sys.argv[1:]\nsys.argv = [fixture, *args]\nrunpy.run_path(fixture, run_name="__main__")\n')
    log = (work / "fixture.log").open("wb")
    process = subprocess.Popen([sys.executable, str(probe), str(FIXTURE), str(cert), str(work / "evidence.json"), str(addrfile)], stdout=log, stderr=subprocess.STDOUT)
    try:
        for _ in range(100):
            if addrfile.exists() and addrfile.stat().st_size:
                break
            if process.poll() is not None:
                raise RuntimeError(f"TLS fixture exited at startup: {process.returncode}; log={log.name}\n{pathlib.Path(log.name).read_text()}")
            time.sleep(0.05)
        assert addrfile.exists() and addrfile.stat().st_size, "fixture did not publish its address"
        output = log.name
        assert 'fixture_stage=ready' in pathlib.Path(output).read_text(), 'fixture stage markers missing'
        host, port = addrfile.read_text().split(":")
        context = ssl.create_default_context(cafile=str(cert))
        context.verify_flags |= ssl.VERIFY_X509_STRICT
        with socket.create_connection((host, int(port)), timeout=3) as raw:
            with context.wrap_socket(raw, server_hostname='chatgpt.com') as tls:
                tls.sendall(b'GET / HTTP/1.1\r\nHost: chatgpt.com\r\nConnection: close\r\n\r\n')
                chunks = []
                while True:
                    chunk = tls.recv(4096)
                    if not chunk: break
                    chunks.append(chunk)
                response = b''.join(chunks)
                assert b'200 OK' in response and b'synthetic-native-smoke' in response
    finally:
        process.terminate()
        process.wait(timeout=3)
        log.close()
print("native TLS fixture certificate generation and HTTPS readiness passed")
