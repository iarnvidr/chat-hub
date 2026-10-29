#!/usr/bin/env python3
"""Loopback-only deterministic HTTPS fixture; synthetic content, not ChatGPT login."""
import http.server, pathlib, ssl, sys, threading
cert, evidence, addrfile = map(pathlib.Path, sys.argv[1:4])
class Handler(http.server.BaseHTTPRequestHandler):
 def do_GET(self):
  body=b'{"conversation_id":"native-smoke-synthetic","title":"Synthetic fixture only","fixture":"synthetic-native-smoke","mapping":{"synthetic-message":{"parent":null,"message":{"author":{"role":"assistant"},"content":{"parts":["fixture response; no provider login"]}}}}}'
  self.send_response(200); self.send_header('Content-Type','application/json'); self.send_header('Content-Length',str(len(body))); self.end_headers(); self.wfile.write(body)
 def log_message(self,*args): pass
server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler)
ctx=ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER); ctx.load_cert_chain(str(cert),str(cert.with_suffix('.key')))
server.socket=ctx.wrap_socket(server.socket,server_side=True)
addrfile.write_text(f'127.0.0.1:{server.server_port}')
threading.Thread(target=server.serve_forever,daemon=True).start()
# Fixture remains alive for the app's native smoke run.
threading.Event().wait()
