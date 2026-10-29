#!/usr/bin/env python3
"""Independently validate native updater evidence, including structured identity/reason."""
import json, pathlib, sys
root=pathlib.Path(sys.argv[1])
try: healthy=json.loads((root/'healthy-result.json').read_text()); tampered=json.loads((root/'tampered-result.json').read_text())
except (OSError,json.JSONDecodeError) as error: raise SystemExit(f'FAIL: missing or invalid native evidence: {error}')
identity=healthy.get('restart_identity')
tstatus=tampered.get('tamper_status')
checks=(healthy.get('signature_accepted') is True,healthy.get('replacement_installed') is True,
 healthy.get('restart_requested') is True,healthy.get('restart_observed') is True,
 healthy.get('restart_process_exited') is True,healthy.get('settings_persisted') is True,
 healthy.get('tabs_persisted') is True,isinstance(identity,dict),
 isinstance(identity.get('before_pid') if isinstance(identity,dict) else None,int),
 isinstance(identity.get('after_pid') if isinstance(identity,dict) else None,int),
 identity.get('before_pid',0)>0 if isinstance(identity,dict) else False,
 identity.get('after_pid',0)>0 if isinstance(identity,dict) else False,
 identity.get('before_pid')!=identity.get('after_pid') if isinstance(identity,dict) else False,
 bool(identity.get('expected_executable')) if isinstance(identity,dict) else False,
 identity.get('actual_executable')==identity.get('expected_executable') if isinstance(identity,dict) else False,
 tampered.get('tamper_rejected') is True,isinstance(tstatus,dict),
 tstatus.get('state')=='failed' if isinstance(tstatus,dict) else False,
 tstatus.get('reason')=='signature_verification_failed' if isinstance(tstatus,dict) else False,
 tampered.get('old_version_retained') is True,tampered.get('app_usable') is True)
if not all(checks): raise SystemExit('FAIL: required restart identity/persistence/structured tamper assertions absent')
(root/'evidence.json').write_text(json.dumps({'healthy':healthy,'tampered':tampered},sort_keys=True)+'\n')
