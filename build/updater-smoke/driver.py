#!/usr/bin/env python3
"""Exercise the real ChatHub updater IPC through the gated native eval bridge."""
import json, os, pathlib, plistlib, subprocess, sys, time, traceback
mode, root_arg, app_arg = sys.argv[1:]
root, app = pathlib.Path(root_arg), pathlib.Path(app_arg)
e2e = pathlib.Path(os.environ['MUNINN_E2E_DIR'])
def executable(bundle):
    info=plistlib.loads((bundle/'Contents/Info.plist').read_bytes())
    return (bundle/'Contents/MacOS'/info['CFBundleExecutable']).resolve()
def bridge(expression, timeout=40):
    ident='updater-'+str(time.time_ns()); command=e2e/(ident+'.cmd.json'); result=e2e/(ident+'.res.json')
    tmp=command.with_suffix('.tmp')
    tmp.write_text(json.dumps({'id':ident,'op':'eval','target':'chrome','js':'return await ('+expression+');'})); os.replace(tmp,command)
    try:
        end=time.monotonic()+timeout
        while time.monotonic()<end:
            if result.exists():
                value=json.loads(result.read_text())
                if value.get('error') or value.get('ok') is False: raise RuntimeError(str(value))
                if 'value' not in value: time.sleep(.1); continue
                result_value=value['value']
                if isinstance(result_value,dict) and result_value.get('error'): raise RuntimeError(result_value['error'])
                return result_value
            time.sleep(.1)
        raise RuntimeError('native E2E eval bridge timed out')
    finally: command.unlink(missing_ok=True); result.unlink(missing_ok=True)
def invoke(name,args=None): return bridge('window.ChatHub.invoke('+json.dumps(name)+','+json.dumps(args or {})+')')
def restart_transport_interrupted(exc):
    """Only bridge/transport failures can be explained by app.restart() closing IPC."""
    text=str(exc).lower()
    return any(token in text for token in ('timed out','timeout','bridge','connection','transport','native e2e'))
def native_state():
    ident='updater-state-'+str(time.time_ns()); command=e2e/(ident+'.cmd.json'); result=e2e/(ident+'.res.json')
    tmp=command.with_suffix('.tmp'); tmp.write_text(json.dumps({'id':ident,'op':'state','target':'chrome','args':{}})); os.replace(tmp,command)
    try:
        end=time.monotonic()+40
        while time.monotonic()<end:
            if result.exists():
                value=json.loads(result.read_text())
                if value.get('error') or value.get('ok') is False: raise RuntimeError(str(value))
                return value.get('value',value)
            time.sleep(.1)
        raise RuntimeError('native state bridge timed out')
    finally: command.unlink(missing_ok=True); result.unlink(missing_ok=True)
def launch():
    out=open(root/(mode+'.stdout.log'),'w'); err=open(root/(mode+'.stderr.log'),'w')
    return subprocess.Popen([str(executable(app)),'--native-ui-smoke','--updater-smoke'],env=os.environ.copy(),stdout=out,stderr=err)
def wait_ready(proc, timeout=20):
    # A visible native window precedes frontend module initialization. Wait for
    # the actual application bridge, without replacing it with internal APIs.
    end=time.monotonic()+timeout
    last=None
    while time.monotonic()<end:
        if proc.poll() is not None: raise RuntimeError('native app exited prematurely')
        try:
            last=native_state()
            if last.get('window_visible'):
                try:
                    evidence=bridge("({ready:typeof window.ChatHub?.invoke==='function',href:location.href,readyState:document.readyState,chatHub:typeof window.ChatHub})",timeout=2)
                    if isinstance(evidence,dict) and evidence.get('ready'):
                        return {'native_state':last,'frontend':evidence}
                except (RuntimeError,OSError,ValueError): pass
        except (RuntimeError,OSError,ValueError): pass
        time.sleep(.2)
    raise RuntimeError(f'actual ChatHub frontend bridge did not become ready within {timeout}s; last native state={last!r}')
def version(bundle): return plistlib.loads((bundle/'Contents/Info.plist').read_bytes()).get('CFBundleShortVersionString')
old=json.loads((root/'old.json').read_text())['version']; new=json.loads((root/'new.json').read_text())['version']
if mode not in ('healthy','tampered'): raise RuntimeError('mode must be healthy or tampered')
proc=launch(); after_pid=after_exe=None
try:
    wait_ready(proc)
    before=native_state(); before_pid=before.get('process_id'); before_exe=before.get('executable_path') or before.get('executable')
    if not isinstance(before_pid,int) or before_pid<=0 or not before_exe: raise RuntimeError(f'pre-install identity missing: {before}')
    status=invoke('update_status')
    if not isinstance(status,dict) or not status.get('version'): raise RuntimeError(f'actual app version unavailable: {status}')
    if mode=='healthy':
        if status['version'] != old: raise RuntimeError(f'fixture executable version mismatch: expected {old}, got {status["version"]}')
    checked=invoke('check_for_updates',{'includePreviews':False})
    if not isinstance(checked,dict) or checked.get('status')!='available' or checked.get('version')!=new: raise RuntimeError(f'signed update was not offered: {checked}')
    if mode=='healthy':
        # app.restart() may terminate the IPC bridge before its successful response arrives.
        # Only classify explicit transport/eval/bridge loss as an interrupted call; success
        # is established exclusively by process, bundle and plugin postconditions below.
        install_call='returned'
        try:
            invoke('install_update')
        except RuntimeError as exc:
            if not restart_transport_interrupted(exc):
                raise RuntimeError(f'healthy signed install failed: {exc}')
            install_call='interrupted_during_restart: '+str(exc)[:200]
        try: proc.wait(timeout=60)
        except subprocess.TimeoutExpired: raise RuntimeError('updater install did not exit within 60s')
        end=time.monotonic()+120; after=None
        while time.monotonic()<end:
            try:
                candidate=native_state()
                if candidate.get('process_id') != before_pid: after=candidate; break
            except (RuntimeError,OSError,ValueError): pass
            time.sleep(.25)
        if not after: raise RuntimeError('no fresh post-install native process observed')
        after_pid=after.get('process_id'); after_exe=after.get('executable_path') or after.get('executable')
        expected=str(executable(app))
        if not isinstance(after_pid,int) or after_pid<=0 or not after_exe or str(pathlib.Path(after_exe).resolve())!=expected: raise RuntimeError(f'post-install executable identity mismatch: {after}')
        post=invoke('update_status')
        if not isinstance(post,dict) or post.get('version')!=new: raise RuntimeError(f'relaunched application version is not bundled target version: {post}')
        evidence={'signature_accepted':True,'replacement_installed':version(app)==new,'restart_requested':True,'restart_call':install_call,'restart_observed':True,'restart_process_exited':proc.poll() is not None,'restart_identity':{'before_pid':before_pid,'after_pid':after_pid,'expected_executable':expected,'actual_executable':str(pathlib.Path(after_exe).resolve())},'before_version':old,'after_version':post['version']}
        (root/'healthy-result.json').write_text(json.dumps(evidence,sort_keys=True)+'\n')
        if not all((evidence['replacement_installed'],evidence['restart_process_exited'],after_pid!=before_pid)): raise RuntimeError('post-install assertions failed')
    else:
        failure=None
        try: invoke('install_update')
        except RuntimeError as exc: failure=str(exc)
        observed_status=invoke('update_status')
        # Trust the backend's classification from the concrete updater Error variant,
        # never substring-match arbitrary error text in this harness.
        rejected=(bool(failure) and isinstance(observed_status,dict)
                  and observed_status.get('status')=='signature_rejected')
        still=version(app)==old
        live=proc.poll() is None
        evidence={'tamper_rejected':rejected,'tamper_status':{'state':'failed' if rejected else 'unknown','reason':'signature_verification_failed' if rejected else 'unclassified','detail':failure,'backend_status':observed_status.get('status') if isinstance(observed_status,dict) else None},'old_version_retained':still,'app_usable':live}
        (root/'tampered-result.json').write_text(json.dumps(evidence,sort_keys=True)+'\n')
        if not all((rejected,still,live)): raise RuntimeError(f'tamper rejection assertions failed: {evidence}')
except Exception:
    detail=traceback.format_exc()
    print(detail,file=sys.stderr,flush=True)
    (root/(mode+'-failure-traceback.txt')).write_text(detail)
    raise
finally:
    if proc.poll() is None: proc.terminate()
    try: proc.wait(timeout=10)
    except subprocess.TimeoutExpired: proc.kill()
    if isinstance(after_pid,int) and after_exe:
        try:
            observed=subprocess.run(['ps','-p',str(after_pid),'-o','comm='],capture_output=True,text=True,check=True).stdout.strip()
            if pathlib.Path(observed).resolve()==executable(app).resolve(): os.kill(after_pid,15)
        except (OSError,ValueError,subprocess.CalledProcessError): pass
