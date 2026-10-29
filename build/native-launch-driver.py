#!/usr/bin/env python3
"""Required real-bundle WKWebView, API, relaunch, and shutdown smoke."""
import json, os, pathlib, plistlib, signal, socket, subprocess, sys, time, urllib.error, urllib.request
app, evidence = map(pathlib.Path, sys.argv[1:3]); evidence.mkdir(parents=True, exist_ok=True)
data = pathlib.Path(os.environ['HOME'])/'Library/Application Support/io.github.iarnvidr.chat-hub'
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
    if op=='eval' and 'value' not in v: time.sleep(.1); continue
    return v.get('value',v)
   time.sleep(.1)
  raise RuntimeError(f'{op} IPC timed out')
 finally: cmd.unlink(missing_ok=True); result.unlink(missing_ok=True)

def alive(pid):
 try: os.kill(pid,0); return True
 except ProcessLookupError: return False

def health(info,token):
 url=info['api']+'/api/health'; req=urllib.request.Request(url,headers={'Authorization':'Bearer '+token})
 with urllib.request.urlopen(req,timeout=3) as r:
  body=json.load(r)
 if body.get('status')!='ok': raise RuntimeError('authenticated API health failed')
 return body

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
 os.chmod(data.parent,0o700) if data.parent.exists() else None
 exe=bundle['CFBundleExecutable']; binary=app/'Contents/MacOS'/exe
 proc=subprocess.Popen([str(binary),'--native-ui-smoke','--updater-smoke'],stdout=(evidence/'app.log').open('w'),stderr=subprocess.STDOUT)
 info,token=wait_service(); state=bridge('state');
 if state.get('process_id')!=proc.pid or not state.get('window_visible'): raise RuntimeError('native main window not visible or PID mismatch')
 service=bridge('eval',js="return await window.ChatHub.invoke('service_info');")
 record['service_info']={'api':service.get('api'),'proxy':service.get('proxy'),'version':service.get('version'),'process_id':service.get('process_id')}
 if not isinstance(service,dict) or service.get('process_id')!=proc.pid or not service.get('api') or not service.get('proxy'): raise RuntimeError(f'service_info assertion failed: {service!r}')
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
 subprocess.run(['osascript','-e',f'tell application id "{bid}" to quit'],check=True,timeout=15)
 try: proc.wait(timeout=20)
 except subprocess.TimeoutExpired: raise RuntimeError('AppleScript graceful quit timed out')
 if proc.returncode not in (0,-signal.SIGTERM): raise RuntimeError(f'quit exit code {proc.returncode}')
 ports_closed(info); record['graceful_quit']=True
 # Launch Services does not inherit this Python process environment. Set only the
 # smoke bridge variables for the user session so the reopened app can expose IPC.
 subprocess.run(['launchctl','setenv','MUNINN_NATIVE_UI_SMOKE','1'],check=True,timeout=5)
 subprocess.run(['launchctl','setenv','MUNINN_E2E_DIR',str(e2e)],check=True,timeout=5)
 opener=subprocess.Popen(['open','-n','-a',str(app),'--args','--native-ui-smoke','--updater-smoke'],stdout=(evidence/'reopen.log').open('w'),stderr=subprocess.STDOUT)
 opener.wait(timeout=20)
 # An explicit -n launch is an actual full-bundle relaunch, not an executable-only restart.
 proc=None
 end=time.monotonic()+90; newpid=None
 while time.monotonic()<end:
  # Native bridge IPC readiness from the launched app confirms its own PID.
  try:
   info2,token2=wait_service(); state2=bridge('state'); newpid=state2.get('process_id')
   if newpid != pid and health(info2,token2).get('status')=='ok': break
  except (RuntimeError,OSError,ValueError,urllib.error.URLError): time.sleep(.3)
 else: raise RuntimeError('full bundle reopen did not expose a healthy new process')
 pid=newpid
 health(info2,token2); record['reopen_pid']=newpid; record['reopen_health']='ok'
 subprocess.run(['osascript','-e',f'tell application id "{bid}" to quit'],check=True,timeout=15)
 end=time.monotonic()+20
 while time.monotonic()<end and alive(newpid): time.sleep(.2)
 if alive(newpid): raise RuntimeError('reopened app graceful quit did not terminate PID')
 ports_closed(info2)
 subprocess.run(['launchctl','unsetenv','MUNINN_NATIVE_UI_SMOKE'],check=False,timeout=5)
 subprocess.run(['launchctl','unsetenv','MUNINN_E2E_DIR'],check=False,timeout=5)
 record['passed']=True
except Exception as exc:
 record.update(passed=False,error=f'{type(exc).__name__}: {exc}'); print(record['error'],file=sys.stderr)
finally:
 (evidence/'native-launch-result.json').write_text(json.dumps(record,sort_keys=True,indent=2)+'\n')
 if proc is not None and proc.poll() is None:
  proc.terminate()
  try: proc.wait(timeout=8)
  except subprocess.TimeoutExpired: proc.kill(); proc.wait()
if not record.get('passed'): raise SystemExit(1)
