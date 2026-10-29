#!/usr/bin/env bash
set -euo pipefail
app="${1:?usage: smoke-native-macos.sh APP}"
[[ "$(uname -s)" == Darwin && "$(uname -m)" == arm64 ]] || { echo 'requires macos arm64' >&2; exit 2; }
[[ -d "$app/Contents/MacOS" ]] || { echo "app missing: $app" >&2; exit 2; }
evidence="${SMOKE_EVIDENCE_DIR:-${RUNNER_TEMP:-/tmp}/macos-smoke-evidence}"
mkdir -p "$evidence"
# Isolate the packaged release from any developer state/token.
export HOME="$evidence/home"
app_data="$HOME/Library/Application Support/io.github.iarnvidr.chat-hub"
mkdir -p "$app_data"
chmod 700 "$HOME" "$HOME/Library" "$HOME/Library/Application Support" "$app_data"
codesign --verify --deep --strict "$app"
exe=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$app/Contents/Info.plist")
app_binary="$app/Contents/MacOS/$exe"
"$app_binary" >"$evidence/app.log" 2>&1 &
app_pid=$!
printf '%s\n' "$app_pid" >"$evidence/app.pid"
write_diagnostics() {
  python3 - "$evidence" "$app_data" "$app_pid" <<'PYDIAG'
import json, os, sys
from pathlib import Path
evidence, data, pid = Path(sys.argv[1]), Path(sys.argv[2]), int(sys.argv[3])
info = data / 'service.json'
try:
    parsed = json.loads(info.read_text())
    safe = {k: parsed[k] for k in ('version','process_id') if isinstance(parsed.get(k), (str,int,float,bool))}
    api = parsed.get('api')
    if isinstance(api, str) and api.startswith('http://127.0.0.1:'):
        safe['api'] = api
except Exception:
    safe = {}
try:
    os.kill(pid, 0); alive = True
except OSError:
    alive = False
try:
    exitcode = Path(evidence/'app.exitcode').read_text().strip()
except OSError:
    exitcode = None
(evidence/'diagnostics.json').write_text(json.dumps({'app_pid':pid,'app_alive':alive,'app_exitcode':exitcode,'service_json_path':str(info),'service_token_path':str(data/'service.token'),'service':safe},indent=2)+'\\n')
PYDIAG
}
on_exit() {
  rc=$?
  trap - EXIT
  app_rc=0
  if kill -0 "$app_pid" 2>/dev/null; then kill "$app_pid" 2>/dev/null || true; fi
  wait "$app_pid" 2>/dev/null || app_rc=$?
  printf '%s\\n' "$app_rc" >"$evidence/app.exitcode"
  write_diagnostics || true
  if (( rc != 0 )); then
    echo "Native app startup failed; sanitized diagnostic log follows (evidence: $evidence)" >&2
    python3 - "$evidence/app.log" <<'PYSAN'
import re,sys
from pathlib import Path
text=Path(sys.argv[1]).read_text(errors='replace')
text=re.sub(r'(?im)^.*(?:authorization|bearer|service\\.token|access[_ -]?token|password|secret).*$', '[redacted sensitive log line]', text)
print(text[-12000:])
PYSAN
  fi
  exit "$rc"
}
trap on_exit EXIT
python3 - "$evidence" "$app_data" <<'PY'
import json,sys,time,urllib.request,urllib.error
from pathlib import Path
evidence, app_data = Path(sys.argv[1]), Path(sys.argv[2])
info_path, token_path = app_data/'service.json', app_data/'service.token'
for _ in range(120):
 try:
  info=json.loads(info_path.read_text())
  token=token_path.read_text().strip()
  if len(token) >= 64 and info.get('api','').startswith('http://127.0.0.1:'):
   request=urllib.request.Request(info['api']+'/api/health',headers={'Authorization':'Bearer '+token})
   with urllib.request.urlopen(request,timeout=3) as response: health=json.load(response)
   if health.get('status') == 'ok': break
 except (OSError, ValueError, KeyError, urllib.error.URLError):
  time.sleep(.5)
else: raise SystemExit('packaged app authenticated health endpoint did not become ready')
if info.get('version') != '0.1.0': raise SystemExit('packaged app runtime version mismatch')
if not info.get('process_id'): raise SystemExit('packaged app service process id missing')
# Evidence intentionally excludes the bearer token and service metadata.
(evidence/'health.json').write_text(json.dumps({'status':health['status'],'version':info['version'],'process_id':info['process_id']},indent=2)+'\n')
PY
