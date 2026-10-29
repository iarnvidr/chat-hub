#!/usr/bin/env python3
"""Exercise update controls in the actual app window using its gated Tauri IPC."""
import json, os, pathlib, plistlib, subprocess, sys, time
app, evidence = map(pathlib.Path, sys.argv[1:3])
evidence.mkdir(parents=True, exist_ok=True)
e2e = pathlib.Path(os.environ['MUNINN_E2E_DIR'])
bundle = plistlib.loads((app/'Contents/Info.plist').read_bytes())
binary = app/'Contents/MacOS'/bundle['CFBundleExecutable']
proc = None

def bridge(op, **fields):
    ident = f'ui-{time.time_ns()}'
    cmd, result = e2e/f'{ident}.cmd.json', e2e/f'{ident}.res.json'
    tmp = cmd.with_suffix('.tmp')
    tmp.write_text(json.dumps({'id': ident, 'op': op, 'target': 'chrome', **fields}))
    os.replace(tmp, cmd)
    try:
        end = time.monotonic()+40
        while time.monotonic()<end:
            if result.exists():
                value=json.loads(result.read_text())
                if value.get('error') or value.get('ok') is False: raise RuntimeError(str(value))
                if op == 'eval' and 'value' not in value:
                    time.sleep(.1)
                    continue
                return value.get('value', value)
            time.sleep(.1)
        raise RuntimeError(f'native E2E {op} IPC timed out')
    finally:
        cmd.unlink(missing_ok=True); result.unlink(missing_ok=True)

record={'app': str(app), 'binary': str(binary)}
try:
    if not binary.is_file(): raise RuntimeError(f'actual bundle executable missing: {binary}')
    env=os.environ.copy()
    proc=subprocess.Popen([str(binary), '--native-ui-smoke', '--updater-smoke'], env=env,
                          stdout=open(evidence/'app.stdout.log','w'), stderr=subprocess.STDOUT)
    deadline=time.monotonic()+90
    while time.monotonic()<deadline:
        if proc.poll() is not None: raise RuntimeError(f'app exited prematurely ({proc.returncode})')
        try:
            state=bridge('state')
            if state.get('window_visible'): break
        except (RuntimeError, OSError, ValueError): time.sleep(.2)
    else: raise RuntimeError('actual Tauri main window did not become visible')
    record['window_state']=state
    # Screenshot the foreground app's real window, not a generated/mock image.
    subprocess.run(['osascript','-e','tell application "System Events" to set frontmost of first process whose unix id is '+str(proc.pid)+' to true'],check=True,timeout=10)
    time.sleep(.5)
    shot=evidence/'native-ui.png'
    subprocess.run(['screencapture','-x',str(shot)],check=True,timeout=20)
    if not shot.is_file() or shot.stat().st_size<1000: raise RuntimeError('native screenshot missing or empty')
    record['screenshot']=str(shot)
    js=pathlib.Path(__file__).with_name('native-ui-interactions.js').read_text()
    # The automation wrapper runs an async function body; the interaction file
    # itself is an IIFE expression, so explicitly return its value to IPC.
    result=bridge('eval', js='return await ('+js.strip().rstrip(';')+');')
    if not isinstance(result,dict) or result.get('clicked') is not True or not result.get('version') or not result.get('message'):
        raise RuntimeError(f'update UI assertion failed: {result!r}')
    record['interaction']=result
    # Ask native automation to execute a real JS Tauri invoke in the external provider webview.
    denial=bridge('provider_denial')
    deadline=time.monotonic()+15
    report=e2e/'provider-denial.res.json'
    while time.monotonic()<deadline and not report.exists(): time.sleep(.1)
    if not report.exists(): raise RuntimeError('provider denial result was not reported')
    denial=json.loads(report.read_text())
    if denial.get('denied') is not True: raise RuntimeError(f'provider service_info was not denied: {denial!r}')
    record['provider_denial']=denial
    record['passed']=True
except Exception as exc:
    record.update(passed=False,error=f'{type(exc).__name__}: {exc}')
    print(record['error'],file=sys.stderr)
    sys.exit(1)
finally:
    (evidence/'native-ui-result.json').write_text(json.dumps(record,sort_keys=True,indent=2)+'\n')
    if proc is not None and proc.poll() is None:
        proc.terminate()
        try: proc.wait(timeout=10)
        except subprocess.TimeoutExpired: proc.kill(); proc.wait()
if not record.get('passed'): raise SystemExit(1)
