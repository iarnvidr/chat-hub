#!/usr/bin/env python3
"""Bounded failure evidence for native smoke scripts; never reads file contents."""
import html, os, pathlib, sys
step, status, command, evidence, logfiles, namespace = sys.argv[1:]
summary=os.environ.get('GITHUB_STEP_SUMMARY'); reason='Native smoke failed (see complete traceback in driver stderr).'
first=''
for name in logfiles.split(':'):
 p=pathlib.Path(name)
 if p.is_file():
  try:
   for line in p.read_text(errors='replace').splitlines():
    if 'Traceback (most recent call last):' in line: first=line; break
   if first: break
  except OSError: pass
safe=html.escape(first or reason)
print(f'::error title={step}::{safe}', flush=True)
if step in ('15','17'): print(f'::warning title={step}::Smoke test failed internally with exit code {status}; continue-on-error does not indicate success.',flush=True)
MAX_SUMMARY=400*1024
def bounded(lines):
 out=[]
 for line in lines:
  line=str(line)
  out.append(line[:200] + ('…[line truncated]' if len(line)>200 else ''))
 return out
lines=[f'## Native smoke step {step}: FAILED',f'- Exit code: `{status}`',f'- Command: `{command}`',f'- Failure: {first or reason}', '### Captured output (last 80 lines per stream)']
for name in logfiles.split(':'):
 p=pathlib.Path(name)
 if p.is_file(): lines += [f'#### `{p.name}`','```text',*bounded(p.read_text(errors='replace').splitlines()[-80:]),'```']
lines += ['### Evidence directory (names and metadata only)']
for p in sorted(pathlib.Path(evidence).glob('**/*'))[:80] if pathlib.Path(evidence).exists() else []:
  if p.is_dir(): continue
  name=p.name.lower()
  if any(secret in name for secret in ('key','token','secret','credential')): continue
  lines.append(f'- `{p.name}` (file)')
if namespace:
 diagfiles=[p for p in pathlib.Path(evidence).glob('*.diag.json')]
 if diagfiles:
  for p in sorted(diagfiles):
   try:
    import json
    d=json.loads(p.read_text())
    # Side-channel contains only bounded URL/readiness/stage metadata, never page content.
    lines += ['### Native bridge progress',f'- Request: `{d.get("id", "unknown")}`',f'- Stage: `{d.get("stage", "unknown")}`',f'- Metadata: `{json.dumps(d.get("detail", {}), sort_keys=True)}`']
   except (OSError, ValueError): lines += [f'- Diagnostic metadata unreadable: `{p.name}`']
 else: lines += ['### Webview namespace evidence (unavailable: native bridge did not publish progress metadata)','`typeof ChatHub`/`invoke`, `href`, `readyState`, title, window-key matches and Tauri globals: unavailable.']
text='\n'.join(bounded(lines))+'\n'
if len(text.encode())>MAX_SUMMARY: text=text.encode()[:MAX_SUMMARY-40].decode(errors='ignore')+'\n…[summary truncated to 400 KiB]\n'
if summary:
 import tempfile
 target=pathlib.Path(summary); target.parent.mkdir(parents=True,exist_ok=True)
 previous=target.read_bytes() if target.exists() else b''
 combined=previous+text.encode()
 if len(combined)>MAX_SUMMARY: combined=combined[-MAX_SUMMARY:]
 fd,tmp=tempfile.mkstemp(prefix=target.name+'.',dir=target.parent)
 with os.fdopen(fd,'wb') as f: f.write(combined); f.flush(); os.fsync(f.fileno())
 os.replace(tmp,target)
print(text,flush=True)
