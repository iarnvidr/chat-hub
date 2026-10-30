#!/usr/bin/env python3
"""Release-only actual-bundle persisted library smoke (never a production build gate)."""
import argparse
import json
import os
import pathlib
import plistlib
import socket
import tempfile
import urllib.parse
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
    payload = json.dumps({'id': ident, 'op': op, 'target': 'chrome', **fields}).encode()
    fd, temporary = tempfile.mkstemp(prefix=f'.{ident}.', suffix='.tmp', dir=e2e)
    try:
        with os.fdopen(fd, 'wb') as stream:
            stream.write(payload)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, command)
    finally:
        pathlib.Path(temporary).unlink(missing_ok=True)
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
                    return value['value'] if op == 'eval' and 'value' in value else value
                raise RuntimeError(f'{op} returned acknowledgement without terminal value')
            time.sleep(.1)
        raise RuntimeError(f'{op} timed out waiting for terminal response')
    finally:
        command.unlink(missing_ok=True)
        result.unlink(missing_ok=True)


def _operation_value(value, op):
    if not isinstance(value, dict) or value.get('ok') is not True or 'value' not in value:
        raise RuntimeError(f'{op} returned no successful terminal value')
    return value['value']


def _listener_address(raw, *, allow_bare=False):
    try:
        if allow_bare and isinstance(raw, str) and '://' not in raw:
            host, separator, port = raw.rpartition(':')
            return host == '127.0.0.1' and bool(separator) and port.isdigit() and 0 < int(port) < 65536
        parsed = urllib.parse.urlsplit(raw)
        return (parsed.scheme == 'http' and parsed.hostname == '127.0.0.1'
                and parsed.username is None and parsed.password is None
                and parsed.path in ('', '/') and not parsed.query and not parsed.fragment
                and parsed.port is not None and 0 < parsed.port < 65536)
    except (TypeError, ValueError):
        return False


def _listener_pair(service):
    if not isinstance(service, dict):
        raise RuntimeError('native service info unavailable')
    addresses = []
    for key, bare in (('api', False), ('proxy', True)):
        raw = service.get(key)
        if not _listener_address(raw, allow_bare=bare):
            raise RuntimeError(f'unsafe native {key} listener address')
        parsed = urllib.parse.urlsplit(raw if '://' in raw else 'http://' + raw)
        addresses.append((parsed.hostname, parsed.port))
    if addresses[0] == addresses[1]:
        raise RuntimeError('native API and proxy listener addresses are not distinct')
    return addresses


def _health_ok(value):
    # service_health IPC is the native semantic validator; its public shape is only {healthy:true}.
    return isinstance(value, dict) and value.get('healthy') is True


def _v1_health_ok(value):
    return isinstance(value, dict) and value.get('status') == 'healthy' and value.get('service', {}).get('status') == 'ready' and value.get('database', {}).get('status') == 'healthy'


def _receipt_ok(receipt):
    return isinstance(receipt, dict) and all(type(receipt.get(k)) is int and receipt[k] == v for k, v in {'imported': 1, 'updated': 0, 'unchanged': 0, 'failed': 0}.items()) and receipt.get('errors_truncated') is False


def _ui_ok(value, expected_revision=None, expected_node=None, expected_leaf=None):
    return (isinstance(value, dict) and value.get('ready') is True and value.get('row') is True
            and type(value.get('revision')) is int
            and (expected_revision is None or value['revision'] == expected_revision)
            and value.get('selected') is True and value.get('alternate') is True
            and value.get('anchor') is True
            and (expected_node is None or value.get('node_id') == expected_node)
            and (expected_leaf is None or value.get('leaf_id') == expected_leaf))


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
    def _launch():
        # `open` is a short-lived LaunchServices client; its exit status is not
        # the lifetime of the native application.
        return subprocess.Popen(['open', '-W', str(app), '--args', '--native-ui-smoke', '--updater-smoke'], env=env, stdout=log, stderr=subprocess.STDOUT)

    proc = _launch()
    record = {'app': str(app), 'passed': False}
    owned_pid = None
    owned_addresses = []
    expected_executable = str(binary.resolve())
    expected_version = str(info['CFBundleShortVersionString'])
    expected_data = str(data.resolve())
    try:
        end = time.monotonic() + 90
        while time.monotonic() < end:
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
        receipt_response = request(e2e, 'import_library_fixture')
        receipt = receipt_response.get('receipt') if isinstance(receipt_response, dict) else None
        if not _receipt_ok(receipt): raise RuntimeError('fixture import receipt counts do not match the one-conversation official fixture')
        post_import = request(e2e, 'eval', js="return await window.ChatHub.invoke('list_conversations',{query:'',cursor:null});")
        post_revision = post_import.get('library_revision')
        if type(post_revision) is not int or post_revision <= before_revision:
            raise RuntimeError('post-import revision did not advance or is not canonical')
        record['receipt_counts'] = {key: receipt.get(key) for key in ('imported','updated','unchanged','failed')}
        def verify_reader(expected_revision, canonical_id, expected_node, expected_leaf):
            # All waits are deadline-bounded condition polls (never blind sleeps).
            # JSON quoting keeps opaque backend identifiers data, not executable JS.
            node_js, leaf_js = json.dumps(expected_node), json.dumps(expected_leaf)
            script = f"const expectedNode={node_js},expectedLeaf={leaf_js}; const until=async f=>{{const end=Date.now()+5000;while(Date.now()<end){{if(f())return true;await new Promise(r=>setTimeout(r,20));}}return false}}; const f=document.querySelector('#search'); if(!f)throw Error('search control absent'); f.value='Selected synthetic answer'; f.dispatchEvent(new Event('input',{{bubbles:true}})); if(!await until(()=>[...document.querySelectorAll('.conversation-item')].some(x=>x.querySelector('.search-snippet')?.textContent.includes('Selected synthetic answer')))throw Error('body FTS row absent'); const row=[...document.querySelectorAll('.conversation-item')].find(x=>x.querySelector('.search-snippet')?.textContent.includes('Selected synthetic answer')); row.click(); if(!await until(()=>!![...document.querySelectorAll('#messages article[data-node-id]')].find(a=>a.dataset.nodeId===expectedNode)&&document.querySelector('#messages')?.innerText.includes('Selected synthetic answer')))throw Error('selected detail not rendered'); const branch=document.querySelector('#branch'); if(!branch||!await until(()=>branch.options.length>1))throw Error('alternate leaf absent'); const alt=[...branch.options].find(o=>o.value===expectedLeaf); if(!alt)throw Error('expected alternate leaf absent'); branch.value=alt.value; branch.dispatchEvent(new Event('change')); if(!await until(()=>document.querySelector('#messages')?.innerText.includes('Alternate synthetic answer')))throw Error('alternate detail not rendered'); const article=[...document.querySelectorAll('#messages article[data-node-id]')].find(a=>a.dataset.nodeId===expectedNode); const rev=(await window.ChatHub.invoke('list_conversations',{{query:'',cursor:null}})).library_revision; return {{ready:document.querySelector('#health')?.textContent.includes('READY'),row:true,revision:rev,selected:true,alternate:true,anchor:!!article?.id,node_id:article?.dataset.nodeId,leaf_id:branch.value}};"
            result = request(e2e, 'eval', js=script)
            if not _ui_ok(result, expected_revision, expected_node, expected_leaf): raise RuntimeError('actual search/detail/health criteria failed')
            return result
        listed = request(e2e, 'eval', js="return await window.ChatHub.invoke('list_conversations',{query:'',cursor:null});")
        fixture_rows = [x for x in listed.get('items', []) if x.get('title') == 'Synthetic branching export']
        if len(fixture_rows) != 1 or fixture_rows[0].get('provider') != 'chatgpt' or fixture_rows[0].get('account_scope') != 'provisional:default' or fixture_rows[0].get('source_id') != 'synthetic-export-one': raise RuntimeError('canonical API list lacks exact imported fixture identity')
        hit = request(e2e, 'eval', js="return await window.ChatHub.invoke('list_conversations',{query:'Selected synthetic answer',cursor:null});")
        canonical_id = fixture_rows[0]['id']
        match_rows = [x for x in hit.get('items', []) if x.get('conversation',{}).get('id') == canonical_id and isinstance(x.get('message_id'), str) and isinstance(x.get('leaf_node_id'), str)]
        if len(match_rows) != 1: raise RuntimeError('canonical FTS search lacks unique selected message and alternate leaf anchors')
        expected_node, expected_leaf = match_rows[0]['message_id'], match_rows[0]['leaf_node_id']
        result = verify_reader(post_revision, canonical_id, expected_node, expected_leaf)
        record['ui'] = result
        record['canonical_id'] = canonical_id
        record['fts_anchor'] = True
        record['library_revision'] = post_revision
        # Import and rendering must not silently replace the native service. Re-sample
        # both native identity sources before beginning the intentional restart.
        imported_state = request(e2e, 'state', timeout=3)
        imported_service = request(e2e, 'eval', js="const s=await window.__TAURI_INTERNALS__.invoke('service_info'); return {api:s.api,proxy:s.proxy,pid:s.process_id,version:s.version,executable:s.executable};")
        if (not _coherent_native_pid(imported_state, imported_service)
                or imported_state['process_id'] != state['process_id']
                or not _native_identity_ok(imported_state, imported_service, expected_version, expected_executable)
                or not imported_state.get('window_visible')):
            raise RuntimeError('native process identity/readiness changed during import or UI verification')
        state, service = imported_state, imported_service
        old_pid = state.get('process_id')
        if not _native_pid(old_pid): raise RuntimeError('native state lacks actual positive app PID')
        if not _coherent_native_pid(state, service):
            raise RuntimeError('cannot establish native-owned listener addresses and old PID')
        addresses = _listener_pair(service)
        owned_pid, owned_addresses = old_pid, addresses

        # A second LaunchServices open with the exact smoke flags must route to
        # the already-running native app, not merely leave its old window alive.
        _launch()
        reopen_deadline = time.monotonic() + 30
        same_pid_state = same_pid_service = None
        while time.monotonic() < reopen_deadline:
            try:
                candidate_state = request(e2e, 'state', timeout=3)
                candidate_service = request(e2e, 'eval', js="const s=await window.__TAURI_INTERNALS__.invoke('service_info'); return {api:s.api,proxy:s.proxy,pid:s.process_id,version:s.version,executable:s.executable};")
                if candidate_state.get('window_visible') and _coherent_native_pid(candidate_state, candidate_service):
                    same_pid_state, same_pid_service = candidate_state, candidate_service
                    break
            except (RuntimeError, OSError, ValueError):
                pass
            time.sleep(.2)
        if same_pid_state is None:
            raise RuntimeError('same-PID LaunchServices reopen did not produce a ready native window')
        if (same_pid_state['process_id'] != old_pid
                or not _native_identity_ok(same_pid_state, same_pid_service, expected_version, expected_executable)):
            raise RuntimeError('same-PID reopen changed native PID, version, or executable identity')
        same_pid_data = request(e2e, 'default_data_info')
        same_pid_health = request(e2e, 'eval', js="return await window.ChatHub.invoke('service_health');")
        if not _default_data_ok(same_pid_data, expected_data) or not _health_ok(same_pid_health):
            raise RuntimeError('same-PID reopen default data or native IPC health failed')
        same_pid_token = token_path.read_text().strip()
        same_pid_req = urllib.request.Request(same_pid_service['api'].rstrip('/') + '/api/v1/status', headers={'Authorization': 'Bearer ' + same_pid_token})
        with urllib.request.urlopen(same_pid_req, timeout=5) as response: same_pid_raw = json.loads(response.read())
        if not _v1_health_ok(same_pid_raw):
            raise RuntimeError('same-PID reopen authenticated native API health failed')
        same_pid_list = request(e2e, 'eval', js="return await window.ChatHub.invoke('list_conversations',{query:'',cursor:null});")
        if same_pid_list.get('library_revision') != post_revision:
            raise RuntimeError('same-PID reopen canonical library revision changed')
        same_pid_ui = verify_reader(post_revision, canonical_id, expected_node, expected_leaf)
        same_pid_state_after = request(e2e, 'state', timeout=3)
        same_pid_service_after = request(e2e, 'eval', js="const s=await window.__TAURI_INTERNALS__.invoke('service_info'); return {api:s.api,proxy:s.proxy,pid:s.process_id,version:s.version,executable:s.executable};")
        if (not same_pid_state_after.get('window_visible')
                or not _coherent_native_pid(same_pid_state_after, same_pid_service_after)
                or same_pid_state_after['process_id'] != old_pid
                or not _native_identity_ok(same_pid_state_after, same_pid_service_after, expected_version, expected_executable)):
            raise RuntimeError('same-PID reopen identity/readiness changed during reader verification')
        same_pid_list_after = request(e2e, 'eval', js="return await window.ChatHub.invoke('list_conversations',{query:'',cursor:null});")
        if same_pid_list_after.get('library_revision') != post_revision:
            raise RuntimeError('same-PID reopen reader revision changed')
        record['same_pid_reopen'] = {'passed': True, 'process_id': old_pid}
        subprocess.run(['osascript', '-e', f'tell application id "{info["CFBundleIdentifier"]}" to quit'], check=True, timeout=15)
        deadline=time.monotonic()+20
        while time.monotonic()<deadline and _pid_running(old_pid): time.sleep(.2)
        if _pid_running(old_pid): raise RuntimeError('old native process remains after graceful quit')
        for address in addresses:
            if not _connection_refused(address):
                raise RuntimeError('old native API/proxy listener remains open after quit')
        record['old_process_closed'] = True
        record['old_listeners_closed'] = True
        owned_pid = None
        owned_addresses = []
        proc = _launch()
        end = time.monotonic() + 90
        while time.monotonic() < end:
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
                    owned_pid = state['process_id']
                    owned_addresses = _listener_pair(restarted_service)
                    break
            except (RuntimeError, OSError, ValueError):
                pass
            time.sleep(.2)
        else:
            raise RuntimeError('restarted packaged app did not become visible')
        restarted_health = request(e2e, 'eval', js="return await window.ChatHub.invoke('service_health');")
        restarted_token = token_path.read_text().strip()
        restarted_req = urllib.request.Request(restarted_service['api'].rstrip('/') + '/api/v1/status', headers={'Authorization': 'Bearer ' + restarted_token})
        with urllib.request.urlopen(restarted_req, timeout=5) as response: restarted_raw = json.loads(response.read())
        if not _health_ok(restarted_health) or not _v1_health_ok(restarted_raw): raise RuntimeError('restart native IPC/API health proof failed')
        after = request(e2e, 'eval', js="return await window.ChatHub.invoke('list_conversations',{query:'',cursor:null});")
        if after.get('library_revision') != post_revision: raise RuntimeError('canonical library revision changed across restart')
        result = verify_reader(post_revision, canonical_id, expected_node, expected_leaf)
        record['restart'] = {'old_pid': old_pid, 'new_pid': state['process_id'], 'service_pid': restarted_service['pid'], 'version_matches_bundle': True, 'executable_matches_bundle': True, 'default_data_matches_platform': True, 'persisted_reader': True, 'ui': result}
        record['passed'] = True
        return record
    finally:
        # The app is owned by `open`; never substitute terminating its launcher
        # wrapper for a graceful quit of the native app. Quit only our verified app.
        if owned_pid is not None:
            try:
                final_state = request(e2e, 'state', timeout=2)
                final_service = request(e2e, 'eval', js="const s=await window.__TAURI_INTERNALS__.invoke('service_info'); return {api:s.api,proxy:s.proxy,pid:s.process_id,version:s.version,executable:s.executable};")
                if (_coherent_native_pid(final_state, final_service)
                        and final_state['process_id'] == owned_pid
                        and _native_identity_ok(final_state, final_service, expected_version, expected_executable)
                        and _listener_pair(final_service) == owned_addresses):
                    subprocess.run(['osascript', '-e', f'tell application id "{info["CFBundleIdentifier"]}" to quit'], check=True, timeout=15, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                    deadline = time.monotonic() + 20
                    while time.monotonic() < deadline and _pid_running(owned_pid): time.sleep(.2)
                    closed = not _pid_running(owned_pid) and all(_connection_refused(address) for address in owned_addresses)
                    if not closed:
                        record['cleanup_error'] = 'owned native app or listener did not close after graceful quit'
                    else:
                        record['final_native_pid_closed'] = owned_pid
                        record['final_listeners_closed'] = True
                else:
                    record['cleanup_error'] = 'refusing cleanup: native PID/identity/listeners no longer match owned app'
            except Exception as exc:
                record['cleanup_error'] = f'graceful quit unavailable: {type(exc).__name__}'
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
