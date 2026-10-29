#!/usr/bin/env python3
"""Loopback-only deterministic HTTPS fixture; synthetic content, not ChatGPT login."""
import faulthandler, http.server, pathlib, socketserver, ssl, sys, threading, time
ca, evidence, addrfile = map(pathlib.Path, sys.argv[1:4])
cert = ca.with_name(ca.stem + "-leaf.pem")
def stage(message):
 print(f"fixture_stage={message} monotonic={time.monotonic():.6f}", file=sys.stderr, flush=True)
faulthandler.enable(file=sys.stderr)
faulthandler.dump_traceback_later(5, repeat=False, file=sys.stderr)
stage('before_bind')
class LoopbackHTTPServer(http.server.ThreadingHTTPServer):
 def server_bind(self):
  socketserver.TCPServer.server_bind(self)
  self.server_name = 'localhost'
  self.server_port = self.socket.getsockname()[1]
stage('binding')
class Handler(http.server.BaseHTTPRequestHandler):
 def do_GET(self):
  body=b'{"conversation_id":"native-smoke-synthetic","title":"Synthetic fixture only","fixture":"synthetic-native-smoke","mapping":{"synthetic-message":{"parent":null,"message":{"author":{"role":"assistant"},"content":{"parts":["fixture response; no provider login"]}}}}}'
  self.send_response(200); self.send_header('Content-Type','application/json'); self.send_header('Content-Length',str(len(body))); self.end_headers(); self.wfile.write(body)
 def log_message(self,*args): pass
server=LoopbackHTTPServer(('127.0.0.1',0),Handler)
stage('bound')
stage('before_tls_context')
ctx=ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER); stage('before_load_cert_chain'); ctx.load_cert_chain(str(cert),str(ca.with_name(ca.stem + "-leaf.key")))
stage('before_wrap_socket')
server.socket=ctx.wrap_socket(server.socket,server_side=True)
stage('before_write_addr')
addrfile.write_text(f'127.0.0.1:{server.server_port}')
stage('ready')
threading.Thread(target=server.serve_forever,daemon=True).start()
faulthandler.cancel_dump_traceback_later()
# Fixture remains alive for the app's native smoke run.
threading.Event().wait()
