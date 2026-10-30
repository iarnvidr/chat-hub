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

# This driver writes one compact record with identifiers, booleans and counts only.
# Do not reuse clean(): its generic schema is deliberately inappropriate here.
NATIVE_NAME = 'native-library-result.json'
NATIVE_MAX = 64 * 1024
NATIVE_TOP = {'passed','app','receipt_counts','ui','canonical_id','fts_anchor','library_revision',
              'same_pid_reopen','old_process_closed','old_listeners_closed','restart',
              'primary_error','cleanup_error','final_native_pid_closed','final_listeners_closed'}
NATIVE_BOOL = {'passed','fts_anchor','old_process_closed','old_listeners_closed',
               'final_listeners_closed'}
NATIVE_NUM = {'library_revision','final_native_pid_closed'}
NATIVE_IDS = {'canonical_id'}
NATIVE_TEXT = {'app','primary_error','cleanup_error'}
NATIVE_COUNTS = {'imported','updated','unchanged','failed'}
NATIVE_UI_BOOL = {'ready','row','selected','alternate','anchor'}
NATIVE_UI_NUM = {'revision'}
NATIVE_UI_TEXT = {'node_id','leaf_id','canonical_id','selected_leaf_id','alternate_node_id'}
NATIVE_RESTART_BOOL = {'version_matches_bundle','executable_matches_bundle','default_data_matches_platform','persisted_reader','old_process_closed','old_listeners_closed'}
NATIVE_RESTART_NUM = {'old_pid','new_pid','service_pid'}
NATIVE_RESTART_TEXT = set()
NATIVE_RESTART_NESTED = {'ui'}
NATIVE_SAME_PID = {'passed','process_id'}

def _safe_text(value, limit=256):
 if not isinstance(value,str) or len(value)>limit or not value.isprintable(): return False
 return not re.search(r'(?i)(bearer|token|cookie|password|secret|private.?key|credential|authorization|https?://|/Users/|/home/|\\\\)',value)

def _native_record(value):
 if type(value) is not dict or set(value)-NATIVE_TOP or type(value.get('passed')) is not bool:
  raise ValueError('invalid top-level schema or passed field')
 out={'passed':value['passed']}
 for k in NATIVE_TOP-NATIVE_BOOL-{'receipt_counts','ui','restart'}:
  if k not in value: continue
  x=value[k]
  if k in NATIVE_NUM:
   if type(x) is not int or x<0: raise ValueError('invalid numeric field')
   out[k]=x
  elif k in NATIVE_IDS:
   # Canonical IDs are structured JSON identities, not filesystem-safe slugs.
   if (not isinstance(x,str) or len(x)>512 or not x.startswith('conversation:')
       or not _safe_text(x,512)):
    raise ValueError('invalid canonical identifier')
   try: parts=json.loads(x[len('conversation:'):])
   except (ValueError,TypeError): raise ValueError('invalid canonical identifier') from None
   if (type(parts) is not list or len(parts)!=3 or
       parts != ['chatgpt','provisional:default','synthetic-export-one']):
    raise ValueError('invalid canonical identifier')
   out[k]=x
  elif k in NATIVE_TEXT:
   if k=='app':
    if not isinstance(x,str) or not x or len(x)>4096 or '\\x00' in x: raise ValueError('invalid app identity')
    # Driver supplies the trusted bundle path. Retain only its basename.
    x=pathlib.PurePath(x).name
    if not _safe_text(x): raise ValueError('invalid app identity')
   elif not _safe_text(x): raise ValueError('unsafe diagnostic/identity text')
   out[k]=x
 for k in NATIVE_BOOL:
  if k in value:
   if type(value[k]) is not bool: raise ValueError('invalid boolean field')
   out[k]=value[k]
 for k, allowed, groups in [('restart',NATIVE_RESTART_BOOL|NATIVE_RESTART_NUM|NATIVE_RESTART_TEXT|NATIVE_RESTART_NESTED,[(NATIVE_RESTART_BOOL,bool),(NATIVE_RESTART_NUM,int),(NATIVE_RESTART_TEXT,str)])]:
  if k not in value: continue
  d=value[k]
  if k=='restart' and isinstance(d,dict) and 'ui' in d:
   d=dict(d)
   nested=d.pop('ui')
   if not isinstance(nested,dict): raise ValueError('invalid restart.ui object')
   # Validate the driver's repeated post-restart UI proof with the same schema.
   safe_ui={}
   for field,x in nested.items():
    if field in NATIVE_UI_BOOL: ok=type(x) is bool
    elif field in NATIVE_UI_NUM: ok=type(x) is int and 0<=x<=2**53
    elif field in NATIVE_UI_TEXT: ok=(isinstance(x,str) and len(x)<=512 and _safe_text(x,512) and (field=='canonical_id' and x.startswith('conversation:') or field!='canonical_id' and bool(re.fullmatch(r'[A-Za-z0-9_-]+',x))))
    else: ok=False
    if not ok: raise ValueError('invalid restart.ui field')
    safe_ui[field]=x
  if type(d) is not dict or set(d)-allowed: raise ValueError('invalid '+k+' object')
  safe={}
  for field,x in d.items():
   group=next(g for g,t in groups if field in g)
   if group in (NATIVE_BOOL,NATIVE_UI_BOOL,NATIVE_RESTART_BOOL): ok=type(x) is bool
   elif k=='receipt_counts': ok=type(x) is int and 0<=x<=2**53
   elif group in (NATIVE_NUM,NATIVE_UI_NUM,NATIVE_RESTART_NUM): ok=type(x) is int and 0<=x<=2**53
   elif field in NATIVE_UI_TEXT: ok=(isinstance(x,str) and len(x)<=512 and _safe_text(x,512) and (field=='canonical_id' and x.startswith('conversation:') or field!='canonical_id' and bool(re.fullmatch(r'[A-Za-z0-9_-]+',x))))
   else: ok=False
   if not ok: raise ValueError('invalid '+k+' field')
   safe[field]=x
  if k=='restart' and 'safe_ui' in locals(): safe['ui']=safe_ui; del safe_ui
  out[k]=safe

 if 'same_pid_reopen' in value:
  d=value['same_pid_reopen']
  if type(d) is not dict or set(d)!=NATIVE_SAME_PID or type(d['passed']) is not bool or type(d['process_id']) is not int or d['process_id']<0: raise ValueError('invalid same_pid_reopen')
  out['same_pid_reopen']=d
 if 'receipt_counts' in value:
  d=value['receipt_counts']
  if type(d) is not dict or set(d)!=NATIVE_COUNTS or any(type(x) is not int or x<0 or x>2**53 for x in d.values()): raise ValueError('invalid receipt_counts')
  out['receipt_counts']=dict(d)
 if 'ui' in value:
  d=value['ui']
  if type(d) is not dict or set(d)- (NATIVE_UI_BOOL|NATIVE_UI_NUM|NATIVE_UI_TEXT): raise ValueError('invalid ui object')
  out['ui']={}
  for field,x in d.items():
   ok=(type(x) is bool if field in NATIVE_UI_BOOL else type(x) is int and 0<=x<=2**53 if field in NATIVE_UI_NUM else isinstance(x,str) and len(x)<=512 and _safe_text(x,512) and (field=='canonical_id' and x.startswith('conversation:') or field!='canonical_id' and bool(re.fullmatch(r'[A-Za-z0-9_-]+',x))))
   if not ok: raise ValueError('invalid ui field')
   out['ui'][field]=x
 if value['passed']:
  required={'app','receipt_counts','ui','canonical_id','fts_anchor','library_revision','same_pid_reopen','old_process_closed','old_listeners_closed','restart','final_native_pid_closed','final_listeners_closed'}
  if not required.issubset(value): raise ValueError('success record missing required fields')
  if type(value['same_pid_reopen']) is not dict or set(value['same_pid_reopen'])!={'passed','process_id'} or type(value['same_pid_reopen']['passed']) is not bool or type(value['same_pid_reopen']['process_id']) is not int: raise ValueError('invalid same_pid_reopen')
  if type(value['restart']) is not dict or 'ui' not in value['restart']: raise ValueError('restart UI proof missing')
 return out

def _retain_native(src,dest):
 try:
  if src.is_symlink(): raise ValueError('symlink result')
  if src.stat().st_size>NATIVE_MAX: raise ValueError('result exceeds 64 KiB')
  result=_native_record(json.loads(src.read_text(encoding='utf-8')))
  payload=json.dumps(result,sort_keys=True,indent=2)+'\n'
  if len(payload.encode('utf-8'))>NATIVE_MAX: raise ValueError('sanitized result exceeds 64 KiB')
  dest.parent.mkdir(parents=True,exist_ok=True)
  dest.write_text(payload,encoding='utf-8')
 except (OSError,UnicodeError,ValueError,TypeError,RecursionError) as exc:
  raise RuntimeError(f'{src.name}: invalid native-library result ({str(exc)[:120]})') from None

def main():
 args=sys.argv[1:]
 native_sources=[]
 if '--native-library-source' in args:
  i=args.index('--native-library-source')
  if i+2!=len(args): raise SystemExit('--native-library-source must be the final option with one path')
  native_sources.append(args[i+1]); del args[i:i+2]
 if len(args)<2: raise SystemExit('usage: prepare-native-evidence.py OUTPUT SOURCE... [--native-library-source PATH]')
 out=pathlib.Path(args[0]).resolve(); out.mkdir(parents=True,exist_ok=True)
 if native_sources and not any(pathlib.Path(raw).resolve()==pathlib.Path(native_sources[0]).resolve() for raw in args[1:]):
  native_root=pathlib.Path(native_sources[0]).resolve()
  if not native_root.is_dir(): raise RuntimeError(f'{native_root.name}: expected native-library result missing')
  native_files=list(native_root.rglob(NATIVE_NAME))
  if not native_files: raise RuntimeError(f'{native_root.name}: expected native-library result missing')
  for src in native_files:
   if src.is_symlink(): raise RuntimeError(f'{src.name}: invalid native-library result (symlink result)')
   _retain_native(src,out/'native-library'/src.name)
 for n,raw in enumerate(args[1:]):
  root=pathlib.Path(raw).resolve()
  if not root.is_dir(): continue
  is_native = any(pathlib.Path(p).resolve()==root for p in native_sources)
  native_files=list(root.rglob(NATIVE_NAME))
  if is_native and not native_files: raise RuntimeError(f'{root.name}: expected native-library result missing')
  if is_native:
   for src in native_files:
    if src.is_symlink(): raise RuntimeError(f'{src.name}: invalid native-library result (symlink result)')
  if native_files:
   # Native-library runs are isolated: no app logs, command channel, exports or databases.
   for src in native_files:
    rel=src.relative_to(root)
    _retain_native(src,out/f'run-{n}'/rel)
   continue
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
