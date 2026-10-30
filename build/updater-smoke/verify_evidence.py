#!/usr/bin/env python3
"""Independently validate native updater evidence, including structured identity/reason."""
import json, pathlib, sys
root=pathlib.Path(sys.argv[1])
try: healthy=json.loads((root/'healthy-result.json').read_text()); tampered=json.loads((root/'tampered-result.json').read_text())
except (OSError,json.JSONDecodeError) as error: raise SystemExit(f'FAIL: missing or invalid native evidence: {error}')
healthy_identity=healthy.get('restart_identity')
tamper_before=tampered.get('before_identity')
tamper_retained=tampered.get('running_service_info')
tstatus=tampered.get('tamper_status')
checks={
 'healthy.signature_accepted': healthy.get('signature_accepted') is True,
 'healthy.replacement_installed': healthy.get('replacement_installed') is True,
 'healthy.restart_requested': healthy.get('restart_requested') is True,
 'healthy.restart_observed': healthy.get('restart_observed') is True,
 'healthy.restart_process_exited': healthy.get('restart_process_exited') is True,
 'healthy.after_version': healthy.get('after_version') == '0.1.1',
 'healthy.before_version': healthy.get('before_version') == '0.1.0',
 'healthy.restart_identity.object': isinstance(healthy_identity, dict),
 'healthy.restart_identity.before_pid.integer': type(healthy_identity.get('before_pid')) is int if isinstance(healthy_identity, dict) else False,
 'healthy.restart_identity.after_pid.integer': type(healthy_identity.get('after_pid')) is int if isinstance(healthy_identity, dict) else False,
 'healthy.restart_identity.before_pid.positive': healthy_identity.get('before_pid', 0) > 0 if isinstance(healthy_identity, dict) else False,
 'healthy.restart_identity.after_pid.positive': healthy_identity.get('after_pid', 0) > 0 if isinstance(healthy_identity, dict) else False,
 'healthy.restart_identity.pids_differ': healthy_identity.get('before_pid') != healthy_identity.get('after_pid') if isinstance(healthy_identity, dict) else False,
 'healthy.restart_identity.expected_executable': bool(healthy_identity.get('expected_executable')) if isinstance(healthy_identity, dict) else False,
 'healthy.restart_identity.actual_matches_expected': healthy_identity.get('actual_executable') == healthy_identity.get('expected_executable') if isinstance(healthy_identity, dict) else False,
 'tampered.tamper_rejected': tampered.get('tamper_rejected') is True,
 'tampered.tamper_status.object': isinstance(tstatus, dict),
 'tampered.tamper_status.state': tstatus.get('state') == 'failed' if isinstance(tstatus, dict) else False,
 'tampered.tamper_status.reason': tstatus.get('reason') == 'signature_verification_failed' if isinstance(tstatus, dict) else False,
 'tampered.old_version_retained': tampered.get('old_version_retained') is True,
 'tampered.app_usable': tampered.get('app_usable') is True,
 'tampered.old_running_identity_retained': tampered.get('old_running_identity_retained') is True,
 'tampered.before_identity.object': isinstance(tamper_before, dict),
 'tampered.before_identity.process_id.integer': type(tamper_before.get('process_id')) is int if isinstance(tamper_before, dict) else False,
 'tampered.before_identity.process_id.positive': tamper_before.get('process_id') > 0 if isinstance(tamper_before, dict) and type(tamper_before.get('process_id')) is int else False,
 'tampered.before_identity.version': tamper_before.get('version') == '0.1.0' if isinstance(tamper_before, dict) else False,
 'tampered.before_identity.executable': bool(tamper_before.get('executable')) if isinstance(tamper_before, dict) else False,
 'tampered.running_service_info.object': isinstance(tamper_retained, dict),
 'tampered.running_service_info.version': tamper_retained.get('version') == '0.1.0' if isinstance(tamper_retained, dict) else False,
 'tampered.running_service_info.process_id.integer': type(tamper_retained.get('process_id')) is int if isinstance(tamper_retained, dict) else False,
 'tampered.running_service_info.process_id.positive': tamper_retained.get('process_id') > 0 if isinstance(tamper_retained, dict) and type(tamper_retained.get('process_id')) is int else False,
 'tampered.running_service_info.process_id.matches_before': tamper_retained.get('process_id') == tamper_before.get('process_id') if isinstance(tamper_before, dict) and isinstance(tamper_retained, dict) else False,
 'tampered.running_service_info.executable.matches_before': tamper_retained.get('executable') == tamper_before.get('executable') if isinstance(tamper_before, dict) and isinstance(tamper_retained, dict) else False,
}
failed=[name for name,passed in checks.items() if not passed]
if failed:
 print('FAIL: required native updater assertions failed: '+', '.join(failed),file=sys.stderr)
 def keys(value):
  if not isinstance(value,dict): return type(value).__name__
  return {k: (keys(v) if isinstance(v,dict) else type(v).__name__) for k,v in value.items()}
 print('Evidence keys/types (values omitted):',file=sys.stderr)
 for label,value in (('healthy',healthy),('tampered',tampered)):
  print(f'  {label}: {keys(value)}',file=sys.stderr)
 for name in ('healthy.stdout.log','healthy.stderr.log','tampered.stdout.log','tampered.stderr.log','http.log'):
  path=root/name
  if path.is_file():
   try:
    lines=path.read_text(errors='replace').splitlines()[-40:]
    print(f'--- {name}: last {len(lines)} lines ---',file=sys.stderr)
    for line in lines: print(line[:200] if 'panic' not in line.lower() else line[:1000],file=sys.stderr)
   except OSError: pass
 raise SystemExit(1)
(root/'evidence.json').write_text(json.dumps({'healthy':healthy,'tampered':tampered},sort_keys=True)+'\n')
