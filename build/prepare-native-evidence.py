#!/usr/bin/env python3
"""Build a strict, non-secret allowlisted mirror for CI artifact uploads."""
import json, os, pathlib, re, shutil, sys

# Never enumerate/copy source trees wholesale. Only these known evidence names
# are opened; credentials, env dumps, certificates, keys and tokens are excluded.
JSON_NAMES = {'native-launch-result.json','native-ui-result.json','health.json',
              'healthy-result.json','tampered-result.json','evidence.json',
              'provider-fixture.json','diagnostics.json'}
LOG_NAMES = {'app.log','open.log','app.stdout.log','app.stderr.log','healthy.stdout.log',
             'healthy.stderr.log','tampered.stdout.log','tampered.stderr.log',
             'http.log','provider-fixture.log','osascript.stderr.log','objc-exceptions.log'}
PNG_NAMES = {'native-launch.png','native-ui.png'}
# Explicit schema fields. Unrecognized fields (including nested token/cookie data)
# are dropped; updater identity fields are retained for independent verification.
KEYS = set('passed error traceback eval_attempts op attempt request_id elapsed_ms outcome wake '
 'ui_bridge_readiness update_status service_info api proxy version process_id '
 'synthetic_tls_capture host fixture persisted_conversation_id message_content_bytes '
 'message_content_sha256 message_content_matches_fixture dom rendered title api_health '
 'status reopen_pid reopen_health provider_denial denied survival_seconds '
 'ports_closed_after_quit screenshot app bundle_id healthy tampered signature_accepted '
 'replacement_installed restart_requested restart_observed restart_process_exited after_version '
 'before_version restart_identity before_pid after_pid expected_executable actual_executable '
 'tamper_rejected tamper_status state reason old_version_retained app_usable '
 'old_running_identity_retained before_identity running_service_info executable '
 'restart_identity before_pid after_pid process_id'.split())
SECRET = re.compile(r'(?i)(authorization\s*:\s*bearer\s+|bearer\s+|(?:access[_-]?token|refresh[_-]?token|service[_-]?token|cookie|password|private[_-]?key)\s*[=:]\s*)([^\s,;"\']+)')

def clean(v):
 if isinstance(v, dict):
  return {k: clean(x) for k,x in v.items() if k in KEYS and not re.search(r'(?i)(token|cookie|password|secret|private.?key|credential|env)',k)}
 if isinstance(v, list): return [clean(x) for x in v]
 if isinstance(v, str): return SECRET.sub(r'\1[REDACTED]',v)[:2000]
 if v is None or type(v) in (bool,int,float): return v
 return None

def main():
 if len(sys.argv)<3: raise SystemExit('usage: prepare-native-evidence.py OUTPUT SOURCE...')
 out=pathlib.Path(sys.argv[1]).resolve(); out.mkdir(parents=True,exist_ok=True)
 for n,raw in enumerate(sys.argv[2:]):
  root=pathlib.Path(raw).resolve()
  if not root.is_dir(): continue
  for base,dirs,files in os.walk(root,followlinks=False):
   dirs[:]=[d for d in dirs if not (pathlib.Path(base)/d).is_symlink() and not d.startswith('.')]
   rel=pathlib.Path(base).relative_to(root)
   for name in files:
    src=pathlib.Path(base)/name
    if src.is_symlink(): continue
    if name not in JSON_NAMES|LOG_NAMES|PNG_NAMES: continue
    dest=out/f'run-{n}'/rel/name; dest.parent.mkdir(parents=True,exist_ok=True)
    try:
     if name in JSON_NAMES:
      value=json.loads(src.read_text())
      dest.write_text(json.dumps(clean(value),sort_keys=True,indent=2)+'\n')
     elif name in LOG_NAMES:
      if src.stat().st_size>8*1024*1024: continue
      text=src.read_text(errors='replace')
      dest.write_text(SECRET.sub(r'\1[REDACTED]',text))
     else:
      if src.stat().st_size>10*1024*1024: continue
      with src.open('rb') as stream:
       if stream.read(8)!=b'\x89PNG\r\n\x1a\n': continue
      shutil.copyfile(src,dest)
    except (OSError,ValueError,json.JSONDecodeError): continue
 print(f'Prepared allowlisted evidence tree: {out}')
if __name__=='__main__': main()
