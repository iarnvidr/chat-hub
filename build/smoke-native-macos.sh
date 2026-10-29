#!/usr/bin/env bash
set -euo pipefail
app="${1:?usage: smoke-native-macos.sh APP}"
[[ "$(uname -s)" == Darwin && "$(uname -m)" == arm64 ]] || { echo 'requires macos arm64' >&2; exit 2; }
[[ -d "$app/Contents/MacOS" ]] || { echo "app missing: $app" >&2; exit 2; }
evidence="${SMOKE_EVIDENCE_DIR:-${RUNNER_TEMP:-/tmp}/macos-smoke-evidence}"; mkdir -p "$evidence"
codesign --verify --deep --strict "$app"
open -n "$app"
trap 'osascript -e '\''tell application "Chat Hub" to quit'\'' >/dev/null 2>&1 || true' EXIT
python3 - "$evidence" <<'PY'
import json,sys,time,urllib.request
base='http://127.0.0.1:47871'
for _ in range(40):
 try:
  with urllib.request.urlopen(base+'/health',timeout=3) as r: data=json.load(r)
  if data.get('ok') is True: break
 except Exception: time.sleep(.5)
else: raise SystemExit('packaged app health endpoint did not become ready')
open(sys.argv[1]+'/health.json','w').write(json.dumps(data,indent=2)+'\n')
PY
