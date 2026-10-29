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
open -n "$app"
trap 'osascript -e '\''tell application "Chat Hub" to quit'\'' >/dev/null 2>&1 || true' EXIT
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
