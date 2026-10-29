#!/usr/bin/env python3
"""Required real-bundle WKWebView, API, relaunch, and shutdown smoke."""
import json, os, pathlib, plistlib, signal, socket, subprocess, sys, time, traceback, urllib.error, urllib.request
app, evidence = map(pathlib.Path, sys.argv[1:3]); evidence.mkdir(parents=True, exist_ok=True)
data = pathlib.Path(os.environ['MUNINN_E2E_DATA_DIR']).resolve()
e2e=pathlib.Path(os.environ['MUNINN_E2E_DIR']); bundle=plistlib.loads((app/'Contents/Info.plist').read_bytes()); bid=bundle['CFBundleIdentifier']
record={'app':str(app),'bundle_id':bid}; proc=None

def bridge(op, **fields):
 ident=f'launch-{time.time_ns()}'; cmd=e2e/f'{ident}.cmd.json'; result=e2e/f'{ident}.res.json'
 tmp=cmd.with_suffix('.tmp'); tmp.write_text(json.dumps({'id':ident,'op':op,'target':'chrome',**fields})); os.replace(tmp,cmd)
 try:
  end=time.monotonic()+40
  while time.monotonic()<end:
   if result.exists():
    v=json.loads(result.read_text())
    if v.get('error') or v.get('ok') is False: raise RuntimeError(str(v))
    if op=='eval' and 'value' in v:
     value=v['value']
     if isinstance(value,dict) and value.get('error'): raise RuntimeError(f'JavaScript evaluation failed: {value["error"]}')
     return value
    if op=='eval': time.sleep(.1); continue
    return v
   time.sleep(.1)
  raise RuntimeError(f'{op} IPC timed out')
 finally: cmd.unlink(missing_ok=True); result.unlink(missing_ok=True)

def wait_ui_bridge(timeout=20):
 # Tauri's frontend is a module script; native window creation can precede
 # module evaluation. Poll the actual packaged DOM rather than assuming startup.
 end=time.monotonic()+timeout
 last=None
 while time.monotonic()<end:
  try:
   last=bridge('eval',js="return {ready:typeof window.ChatHub?.invoke==='function',href:location.href,readyState:document.readyState,title:document.title,keys:Object.keys(window).filter(k=>/chat|tauri|invoke|ipc/i.test(k)).slice(0,30),tauri:typeof window.__TAURI__,internals:typeof window.__TAURI_INTERNALS__,invoke:typeof window.__TAURI_INTERNALS__?.invoke};")
   if isinstance(last,dict) and last.get('ready'):
    return last
  except (RuntimeError,OSError,ValueError):
   pass
  time.sleep(.2)
 raise RuntimeError(f'UI bridge readiness timeout after {timeout}s; last DOM evidence={last!r}')

def alive(pid):
 # For our direct child, poll() performs waitpid and reaps an exited process;
 # kill(pid, 0) alone reports unreaped zombies as alive.
 if proc is not None and proc.pid == pid:
  return proc.poll() is None
 try: os.kill(pid,0); return True
 except ProcessLookupError: return False

def health(info,token):
 url=info['api']+'/api/health'; req=urllib.request.Request(url,headers={'Authorization':'Bearer '+token})
 with urllib.request.urlopen(req,timeout=3) as r:
  body=json.load(r)
 if body.get('status')!='ok': raise RuntimeError('authenticated API health failed')
 return body

def synthetic_tls_fixture(info, token):
 # Exercise the app's advertised CONNECT proxy against loopback HTTPS override;
 # trust only the newly created per-install CA, never system roots.
 import ssl
 proxy_host, proxy_port = info['proxy'].rsplit(':',1)
 ca=data/'ca'/'ca.pem'
 if not ca.is_file(): raise RuntimeError('per-install CA certificate missing')
 context=ssl.create_default_context(cafile=str(ca))
 with socket.create_connection((proxy_host.strip('[]'),int(proxy_port)),timeout=5) as raw:
  raw.sendall(b'CONNECT chatgpt.com:443 HTTP/1.1\r\nHost: chatgpt.com:443\r\n\r\n')
  response=b''
  while b'\r\n\r\n' not in response:
   chunk=raw.recv(1)
   if not chunk: raise RuntimeError('proxy closed before CONNECT response')
   response+=chunk
  if not response.startswith(b'HTTP/1.1 200'): raise RuntimeError('CONNECT rejected: '+response.decode(errors='replace'))
  with context.wrap_socket(raw,server_hostname='chatgpt.com') as tls:
   tls.sendall(b'GET /backend-api/conversation/native-smoke-synthetic HTTP/1.1\r\nHost: chatgpt.com\r\nConnection: close\r\n\r\n')
   data_bytes=b''
   while True:
    chunk=tls.recv(65536)
    if not chunk: break
    data_bytes+=chunk
 if b'fixture response; no provider login' not in data_bytes: raise RuntimeError('synthetic fixture body did not traverse proxy')
 end=time.monotonic()+10
 while time.monotonic()<end:
  req=urllib.request.Request(info['api']+'/api/conversations/native-smoke-synthetic',headers={'Authorization':'Bearer '+token})
  try:
   with urllib.request.urlopen(req,timeout=2) as r: record=json.load(r)
   if record.get('provider')=='chatgpt' and 'fixture response; no provider login' in record['messages'][0]['content']:
    content=record['messages'][0]['content'].encode()
    import hashlib
    return {'host':'chatgpt.com','fixture':'synthetic; not ChatGPT login','persisted_conversation_id':record['id'],'message_content_bytes':len(content),'message_content_sha256':hashlib.sha256(content).hexdigest(),'message_content_matches_fixture':True}
  except urllib.error.HTTPError as exc:
   if exc.code!=404: raise
  time.sleep(.2)
 raise RuntimeError('synthetic capture missing from persisted conversation API')

def wait_service():
 end=time.monotonic()+90
 while time.monotonic()<end:
  if proc and proc.poll() is not None: raise RuntimeError(f'app exited: {proc.returncode}')
  try:
   info=json.loads((data/'service.json').read_text()); token=(data/'service.token').read_text().strip()
   if len(token)<64: raise RuntimeError('app token invalid')
   health(info,token); return info,token
  except (OSError,ValueError,KeyError,urllib.error.URLError): time.sleep(.3)
 raise RuntimeError('authenticated API did not become healthy')

def ports_closed(info):
 for field in ('api','proxy'):
  hostport=info[field].removeprefix('http://').removeprefix('https://').split('/')[0]; host,port=hostport.rsplit(':',1)
  s=socket.socket(); s.settimeout(.5); result=s.connect_ex((host.strip('[]'),int(port))); s.close()
  if result==0: raise RuntimeError(f'{field} listener remained open')

def launch():
 return subprocess.Popen(['open','-a',str(app)],stdout=(evidence/'open.log').open('a'),stderr=subprocess.STDOUT)
try:
 data.mkdir(parents=True, exist_ok=True); os.chmod(data,0o700)
 exe=bundle['CFBundleExecutable']; binary=app/'Contents/MacOS'/exe
 proc=subprocess.Popen([str(binary),'--native-ui-smoke','--updater-smoke'],stdout=(evidence/'app.log').open('w'),stderr=subprocess.STDOUT)
 info,token=wait_service(); state=bridge('state');
 if state.get('process_id')!=proc.pid or not state.get('window_visible'): raise RuntimeError('native main window not visible or PID mismatch')
 record['ui_bridge_readiness']=wait_ui_bridge()
 service=bridge('eval',js="return await window.ChatHub.invoke('service_info');")
 record['service_info']={'api':service.get('api'),'proxy':service.get('proxy'),'version':service.get('version'),'process_id':service.get('process_id')}
 if not isinstance(service,dict) or service.get('process_id')!=proc.pid or not service.get('api') or not service.get('proxy'): raise RuntimeError(f'service_info assertion failed: {service!r}')
 record['synthetic_tls_capture']=synthetic_tls_fixture(service,token)
 js="const text=document.body.innerText; if(!text.includes('Chat Hub')||!document.querySelector('[data-testid=app-version]')?.textContent.includes('Version')||!document.querySelector('[data-testid=check-updates]')) throw Error('Chat Hub UI/version/control not rendered'); const s=await window.ChatHub.invoke('service_info'); if(!s.api||!s.proxy) throw Error('service addresses missing'); return {rendered:true,title:document.title,version:document.querySelector('[data-testid=app-version]').textContent,service:s};"
 rendered=bridge('eval',js='return await (async()=>{'+js+'})();')
 if not isinstance(rendered,dict) or rendered.get('rendered') is not True: raise RuntimeError(f'DOM assertion failed: {rendered!r}')
 record['dom']=rendered; record['api_health']=health(info,token)
 subprocess.run(['osascript','-e',f'tell application "System Events" to set frontmost of first process whose unix id is {proc.pid} to true'],check=True,timeout=10)
 time.sleep(.5); shot=evidence/'native-launch.png'; subprocess.run(['screencapture','-x',str(shot)],check=True,timeout=20)
 if not shot.is_file() or shot.stat().st_size<1000: raise RuntimeError('actual screenshot missing/empty')
 record['screenshot']=str(shot)
 pid=proc.pid
 for _ in range(15):
  if not alive(pid): raise RuntimeError('app did not survive 15 seconds')
  health(info,token); time.sleep(1)
 record['survival_seconds']=15
 # Reopen/activate while still running. Do not send quit here: quit is the
 # final shutdown assertion, not a substitute for testing the app reopen event.
 subprocess.run(['open','-a',str(app)],check=True,timeout=15)
 end=time.monotonic()+30
 while time.monotonic()<end:
  try:
   state2=bridge('state')
   if state2.get('process_id')==pid and state2.get('window_visible'): break
  except (RuntimeError,OSError,ValueError): pass
  time.sleep(.2)
 else: raise RuntimeError('same-PID app/window did not reopen')
 info2,token2=wait_service()
 # Native callback returns only after the actual provider invoke rejects.
 denial_result=bridge('provider_denial')
 if denial_result.get('denied') is not True: raise RuntimeError(f'provider service_info was not denied: {denial_result!r}')
 record['provider_denial']=denial_result
 if state2.get('process_id')!=pid: raise RuntimeError('reopen changed process PID')
 if info2.get('api')!=info.get('api') or info2.get('proxy')!=info.get('proxy'): raise RuntimeError('reopen changed same-PID service endpoints')
 health(info2,token2); record['reopen_pid']=pid; record['reopen_health']='ok'
 subprocess.run(['osascript','-e',f'tell application id "{bid}" to quit'],check=True,timeout=15)
 end=time.monotonic()+20
 while time.monotonic()<end:
  if not alive(pid): break
  time.sleep(.2)
 if alive(pid): raise RuntimeError('reopened app graceful quit did not terminate PID')
 ports_closed(info2); record['ports_closed_after_quit']=True
 record['passed']=True
except Exception as exc:
 record.update(passed=False,error=f'{type(exc).__name__}: {exc}',traceback=traceback.format_exc()); print(record['traceback'],file=sys.stderr,flush=True)
finally:
 (evidence/'native-launch-result.json').write_text(json.dumps(record,sort_keys=True,indent=2)+'\n')
 if proc is not None and proc.poll() is None:
  proc.terminate()
  try: proc.wait(timeout=8)
  except subprocess.TimeoutExpired: proc.kill(); proc.wait()
if not record.get('passed'): raise SystemExit(1)
