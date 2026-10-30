#!/usr/bin/env python3
"""Release-only actual-bundle persisted library smoke (never a production build gate)."""
import argparse
import json
import os
import pathlib
import plistlib
import socket
import subprocess
import sys
import time
import uuid
import urllib.request
import pwd


def request(e2e, op, *, timeout=45, **fields):
    ident = f'library-{uuid.uuid4().hex}'
    command = e2e / f'{ident}.cmd.json'
    result = e2e / f'{ident}.res.json'
    command.write_text(json.dumps({'id': ident, 'op': op, 'target': 'chrome', **fields}))
    end = time.monotonic() + timeout
    ack_seen = False
    try:
        while time.monotonic() < end:
            if result.exists():
                try:
                    value = json.loads(result.read_text())
                except (OSError, ValueError):
                    time.sleep(.1)
                    continue
                if value.get('error') or value.get('ok') is False:
                    raise RuntimeError(f'{op} failed (native command returned an error)')
                if op != 'eval' or 'value' in value:
                    return value.get('value', value)
                # eval first publishes an ACK to this same file. Poll for the
                # terminal value, but acknowledge that the native protocol can
                # race and overwrite it with the ACK; do not convert that to pass.
                ack_seen = True
            time.sleep(.1)
        suffix = ' (ACK observed; terminal value may have been overwritten by the known same-file race)' if ack_seen else ''
        raise RuntimeError(f'{op} timed out waiting for terminal response{suffix}')
    finally:
        command.unlink(missing_ok=True)
        result.unlink(missing_ok=True)


def _health_ok(value):
    # service_health IPC is the native semantic validator; its public shape is only {healthy:true}.
    return isinstance(value, dict) and value.get('healthy') is True


def _v1_health_ok(value):
    return isinstance(value, dict) and value.get('status') == 'healthy' and value.get('service', {}).get('status') == 'ready' and value.get('database', {}).get('status') == 'healthy'


def _receipt_ok(receipt):
    return isinstance(receipt, dict) and all(type(receipt.get(k)) is int and receipt[k] == v for k, v in {'imported': 1, 'updated': 0, 'unchanged': 0, 'failed': 0}.items()) and receipt.get('errors_truncated') is False


def _ui_ok(value):
    return isinstance(value, dict) and value.get('ready') is True and value.get('row') is True and value.get('revision') and value.get('selected') and value.get('alternate') and value.get('anchor') is True


def _native_pid(value):
    return type(value) is int and value > 0


def _coherent_native_pid(state, service):
    if not isinstance(state, dict) or not isinstance(service, dict):
        return False
    pid = state.get('process_id')
    return _native_pid(pid) and service.get('pid') == pid and _native_pid(service.get('pid'))


def _new_native_pid(state, service, old_pid):
    return _coherent_native_pid(state, service) and state['process_id'] != old_pid


def _native_identity_ok(state, service, expected_version, expected_executable):
    return (isinstance(state, dict) and isinstance(service, dict)
            and state.get('version') == expected_version
            and service.get('version') == expected_version
            and state.get('executable_path') == expected_executable
            and service.get('executable') == expected_executable)


def _default_data_ok(value, expected_path):
    return (isinstance(value, dict) and value.get('ok') is True
            and value.get('data_dir_source') == 'platform_default'
            and value.get('configured_matches_platform_default') is True
            and value.get('configured_data_dir') == expected_path
            and value.get('platform_default_data_dir') == expected_path)


def _pid_running(pid):
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False
    except PermissionError:
        return True


def _connection_refused(address):
    try:
        with socket.create_connection(address, timeout=1):
            return False
    except ConnectionRefusedError:
        return True
    except OSError:
        return False


def run(app, evidence, allow_default_app_data=False, disposable_home=None):
    app, evidence = app.resolve(), evidence.resolve()
    evidence.mkdir(parents=True, exist_ok=True)
    if not allow_default_app_data:
        raise RuntimeError('refusing default app data without --allow-default-app-data')
    os_home = pathlib.Path(pwd.getpwuid(os.getuid()).pw_dir).resolve()
    if disposable_home is None or pathlib.Path(disposable_home).resolve() != os_home or pathlib.Path.home().resolve() != os_home:
        raise RuntimeError('require --disposable-home equal to the actual OS user home (HOME overrides are rejected)')
    if not os.environ.get('MUNINN_E2E_DIR'):
        raise RuntimeError('MUNINN_E2E_DIR is required')
    if os.environ.get('MUNINN_E2E_DATA_DIR'):
        raise RuntimeError('MUNINN_E2E_DATA_DIR override is forbidden: this smoke must use actual default app data')
    e2e = pathlib.Path(os.environ['MUNINN_E2E_DIR']).resolve()
    info = plistlib.loads((app / 'Contents/Info.plist').read_bytes())
    binary = app / 'Contents/MacOS' / info['CFBundleExecutable']
    if not binary.is_file():
        raise RuntimeError(f'actual bundle executable missing: {binary}')
    # Do not overwrite or clean an existing user's library. Caller approval alone
    # does not authorize destructive cleanup; refuse unless the data dir is absent.
    from pathlib import Path
    data = os_home / 'Library' / 'Application Support' / info.get('CFBundleIdentifier', '')
    if data.exists():
        raise RuntimeError(f'default app data already exists at {data}; refusing to modify user data (use a disposable macOS account)')
    env = os.environ.copy()
    env['MUNINN_NATIVE_UI_SMOKE'] = '1'
    env['MUNINN_E2E_ALLOW_DEFAULT_DATA_DIR'] = '1'
    env.pop('MUNINN_E2E_DATA_DIR', None)
    env.pop('MUNINN_DATA_DIR', None)
    log = (evidence / 'app.log').open('w')
    proc = subprocess.Popen(['open', '-W', str(app)], env=env, stdout=log, stderr=subprocess.STDOUT)
    record = {'app': str(app), 'passed': False, 'ack_race_fixed': False}
    expected_executable = str(binary.resolve())
    expected_version = str(info['CFBundleShortVersionString'])
    expected_data = str(data.resolve())
    try:
        end = time.monotonic() + 90
        while time.monotonic() < end:
            if proc.poll() is not None:
                raise RuntimeError(f'app exited prematurely ({proc.returncode})')
            try:
                state = request(e2e, 'state', timeout=3)
                if state.get('window_visible'):
                    health = request(e2e, 'eval', js="return await window.ChatHub.invoke('service_health');")
                    if not _health_ok(health): raise RuntimeError('native service unhealthy')
                    break
            except (RuntimeError, OSError, ValueError):
                time.sleep(.2)
        else:
            raise RuntimeError('actual packaged main window did not become visible')
        data_info = request(e2e, 'default_data_info')
        if not _default_data_ok(data_info, expected_data): raise RuntimeError('native default data resolver proof failed')
        service = request(e2e, 'eval', js="const s=await window.__TAURI_INTERNALS__.invoke('service_info'); return {api:s.api,proxy:s.proxy,pid:s.process_id,version:s.version,executable:s.executable};")
        if not isinstance(service, dict): raise RuntimeError('native service info unavailable')
        if not _coherent_native_pid(state, service): raise RuntimeError('native state/service_info process_id mismatch or invalid PID')
        if not _native_identity_ok(state, service, expected_version, expected_executable): raise RuntimeError('native version/executable identity does not match bundle')
        token_path = data / 'service.token'
        token = token_path.read_text().strip()
        req = urllib.request.Request(service['api'].rstrip('/') + '/api/v1/status', headers={'Authorization': 'Bearer ' + token})
        with urllib.request.urlopen(req, timeout=5) as response: raw_health = json.loads(response.read())
        if not _v1_health_ok(raw_health): raise RuntimeError('authenticated native API status failed required health fields')
        before = request(e2e, 'eval', js="return await window.ChatHub.invoke('list_conversations',{query:'',cursor:null});")
        before_revision = before.get('library_revision')
        if type(before_revision) is not int: raise RuntimeError('canonical API library_revision is not an integer')
        receipt = request(e2e, 'import_library_fixture')
        if not isinstance(receipt, dict) or not _receipt_ok(receipt): raise RuntimeError('fixture import receipt counts do not match the one-conversation official fixture')
        record['receipt'] = receipt
        def verify_reader():
            script = "const f=document.querySelector('#search'); f.value='Synthetic branching export'; f.dispatchEvent(new Event('input',{bubbles:true})); await new Promise(r=>setTimeout(r,800)); const row=[...document.querySelectorAll('.conversation-item')].find(x=>x.innerText.includes('Synthetic branching export')); if(!row) throw Error('imported title absent from actual search'); row.click(); await new Promise(r=>setTimeout(r,500)); const selected=document.querySelector('#messages')?.innerText.includes('Selected synthetic answer'); const branch=document.querySelector('#branch'); const alt=[...branch.options].find(o=>o.textContent.includes('left')); if(!alt) throw Error('alternate branch absent'); branch.value=alt.value; branch.dispatchEvent(new Event('change')); await new Promise(r=>setTimeout(r,100)); const revision=(await window.ChatHub.invoke('list_conversations',{query:'',cursor:null})).library_revision; return {ready:document.querySelector('#health')?.textContent.includes('READY'),row:true,revision:String(revision),selected,alternate:document.querySelector('#messages')?.innerText.includes('Alternate synthetic answer'),anchor:!!document.querySelector('#messages [data-node-id]')};"
            result = request(e2e, 'eval', js=script)
            if not _ui_ok(result): raise RuntimeError('actual search/detail/health criteria failed')
            return result
        result = verify_reader()
        listed = request(e2e, 'eval', js="return await window.ChatHub.invoke('list_conversations',{query:'',cursor:null});")
        fixture_rows = [x for x in listed.get('items', []) if x.get('title') == 'Synthetic branching export']
        if len(fixture_rows) != 1 or fixture_rows[0].get('id') != 'synthetic-export-one': raise RuntimeError('canonical API list lacks exact imported fixture identity')
        hit = request(e2e, 'eval', js="return await window.ChatHub.invoke('list_conversations',{query:'Selected synthetic answer',cursor:null});")
        if not any(x.get('message_id') and x.get('conversation',{}).get('id') == 'synthetic-export-one' for x in hit.get('items', [])): raise RuntimeError('canonical FTS search lacks selected message anchor')
        record['ui'] = result
        record['canonical_id'] = fixture_rows[0]['id']
        record['fts_anchor'] = True
        record['library_revision'] = before_revision
        old_pid = state.get('process_id')
        if not _native_pid(old_pid): raise RuntimeError('native state lacks actual positive app PID')
        if not _coherent_native_pid(state, service):
            raise RuntimeError('cannot establish native-owned listener addresses and old PID')
        addresses = []
        for key in ('api', 'proxy'):
            raw = service.get(key)
            if not isinstance(raw, str) or raw.count(':') != 1:
                raise RuntimeError(f'invalid native {key} listener address')
            host, port = raw.rsplit(':', 1)
            if host not in ('127.0.0.1', 'localhost') or not port.isdigit() or not 0 < int(port) < 65536:
                raise RuntimeError(f'unsafe native {key} listener address')
            addresses.append((host, int(port)))
        subprocess.run(['osascript', '-e', f'tell application id "{info["CFBundleIdentifier"]}" to quit'], check=True, timeout=15)
        deadline=time.monotonic()+20
        while time.monotonic()<deadline and _pid_running(old_pid): time.sleep(.2)
        if _pid_running(old_pid): raise RuntimeError('old native process remains after graceful quit')
        for address in addresses:
            if not _connection_refused(address):
                raise RuntimeError('old native API/proxy listener remains open after quit')
        record['old_process_closed'] = True
        record['old_listeners_closed'] = True
        proc = subprocess.Popen(['open', '-W', str(app)], env=env, stdout=log, stderr=subprocess.STDOUT)
        end = time.monotonic() + 90
        while time.monotonic() < end:
            if proc.poll() is not None:
                raise RuntimeError('app exited during restart')
            try:
                state = request(e2e, 'state', timeout=3)
                if state.get('window_visible'):
                    data_info = request(e2e, 'default_data_info')
                    if not _default_data_ok(data_info, expected_data):
                        raise RuntimeError('restarted native default data resolver proof failed')
                    restarted_service = request(e2e, 'eval', js="const s=await window.__TAURI_INTERNALS__.invoke('service_info'); return {api:s.api,proxy:s.proxy,pid:s.process_id,version:s.version,executable:s.executable};")
                    if not _new_native_pid(state, restarted_service, old_pid):
                        raise RuntimeError('restart native PID is invalid, incoherent, or unchanged')
                    if not _native_identity_ok(state, restarted_service, expected_version, expected_executable):
                        raise RuntimeError('restarted native version/executable identity does not match bundle')
                    health=request(e2e, 'eval', js="return await window.ChatHub.invoke('service_health');")
                    if not _health_ok(health): raise RuntimeError('restarted service unhealthy')
                    break
            except (RuntimeError, OSError, ValueError):
                pass
            time.sleep(.2)
        else:
            raise RuntimeError('restarted packaged app did not become visible')
        restarted_health = request(e2e, 'eval', js="return await window.ChatHub.invoke('service_health');")
        req = urllib.request.Request(service['api'].rstrip('/') + '/api/v1/status', headers={'Authorization': 'Bearer ' + token})
        with urllib.request.urlopen(req, timeout=5) as response: restarted_raw = json.loads(response.read())
        if not _health_ok(restarted_health) or not _v1_health_ok(restarted_raw): raise RuntimeError('restart native IPC/API health proof failed')
        after = request(e2e, 'eval', js="return await window.ChatHub.invoke('list_conversations',{query:'',cursor:null});")
        if after.get('library_revision') != before_revision: raise RuntimeError('canonical library revision changed across restart')
        result = verify_reader()
        if not result.get('ready'): raise RuntimeError('restart DOM health is not ready')
        record['restart'] = {'old_pid': old_pid, 'new_pid': state['process_id'], 'service_pid': restarted_service['pid'], 'version_matches_bundle': True, 'executable_matches_bundle': True, 'default_data_matches_platform': True, 'persisted_reader': True, 'ui': result}
        record['passed'] = True
        return record
    finally:
        if proc.poll() is None:
            proc.terminate()
            try: proc.wait(timeout=15)
            except subprocess.TimeoutExpired:
                proc.kill(); proc.wait()
        log.close()
        (evidence / 'native-library-result.json').write_text(json.dumps(record, sort_keys=True, indent=2) + '\n')


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('app', type=pathlib.Path)
    p.add_argument('evidence', type=pathlib.Path)
    p.add_argument('--allow-default-app-data', action='store_true')
    p.add_argument('--disposable-home', type=pathlib.Path, help='explicitly identify the actual OS user home as disposable')
    a = p.parse_args()
    try:
        result = run(a.app, a.evidence, a.allow_default_app_data, a.disposable_home)
        print(json.dumps(result, sort_keys=True))
    except Exception as exc:
        print(f'native library smoke failed: {type(exc).__name__}: {exc}', file=sys.stderr)
        raise SystemExit(1)

if __name__ == '__main__':
    main()
