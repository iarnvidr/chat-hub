#!/usr/bin/env bash
set -euo pipefail
app="${1:?usage: smoke-native-macos.sh APP}"
[[ "$(uname -s)" == Darwin && "$(uname -m)" == arm64 ]] || { echo 'requires macos arm64' >&2; exit 2; }
[[ -d "$app/Contents/MacOS" ]] || { echo "app missing: $app" >&2; exit 2; }
evidence="${SMOKE_EVIDENCE_DIR:-${RUNNER_TEMP:-/tmp}/macos-smoke-evidence}"
mkdir -p "$evidence"
export RUST_BACKTRACE=full OBJC_DEBUG_MISSING_POOLS=NO NSUnbufferedIO=YES CHAT_HUB_CRASH_EVIDENCE="$evidence/objc-exceptions.log"
export HOME="$evidence/home"
app_data="$HOME/Library/Application Support/io.github.iarnvidr.chat-hub"
mkdir -p "$app_data"
chmod 700 "$HOME" "$HOME/Library" "$HOME/Library/Application Support" "$app_data"
codesign --verify --deep --strict "$app"
exe=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$app/Contents/Info.plist")
"$app/Contents/MacOS/$exe" >"$evidence/app.log" 2>&1 & app_pid=$!
printf '%s\n' "$app_pid" >"$evidence/app.pid"
on_exit() {
  local app_exitcode=$? app_alive=false
  trap - EXIT
  if kill -0 "$app_pid" 2>/dev/null; then
    app_alive=true
    kill "$app_pid" 2>/dev/null || true
  fi
  wait "$app_pid" 2>/dev/null || true
  service_json_path="$app_data/service.json"
  find "$HOME" "$HOME/Library/Logs/DiagnosticReports" "$HOME/Library/Logs/DiagnosticReports"/../.. -type f \( -name 'chat-hub-desktop*.ips' -o -name 'chat-hub-desktop*.crash' \) -print 2>/dev/null | sort -u >"$evidence/crash-reports.txt" || true
  python3 - "$evidence/diagnostics.json" "$service_json_path" "$app_alive" "$app_exitcode" <<'PY'
import json, pathlib, sys
path, service, alive, code = sys.argv[1:]
pathlib.Path(path).write_text(json.dumps({"service_json_path": service, "app_alive": alive == "true", "app_exitcode": int(code)}, indent=2) + "\n")
PY
  exit "$app_exitcode"
}
trap on_exit EXIT
python3 - "$evidence" "$app_data" "$app_pid" <<'PY'
import json, os, sys, time, urllib.request, urllib.error
from pathlib import Path
evidence,data,pid=Path(sys.argv[1]),Path(sys.argv[2]),int(sys.argv[3]); info_path=data/'service.json'; token_path=data/'service.token'
for _ in range(120):
 try:
  info=json.loads(info_path.read_text()); token=token_path.read_text().strip()
  if len(token)>=64 and info.get('api','').startswith('http://127.0.0.1:'):
   req=urllib.request.Request(info['api']+'/api/health',headers={'Authorization':'Bearer '+token})
   with urllib.request.urlopen(req,timeout=3) as r: health=json.load(r)
   if health.get('status')=='ok': break
 except (OSError,ValueError,KeyError,urllib.error.URLError): time.sleep(.5)
else: raise SystemExit('packaged app authenticated health endpoint not ready')
(evidence/'health.json').write_text(json.dumps({'status':health['status'],'version':info.get('version'),'process_id':info.get('process_id')},indent=2)+'\n')
for _ in range(15):
 os.kill(pid,0); time.sleep(1)
with urllib.request.urlopen(req,timeout=3) as r: assert json.load(r).get('status')=='ok'
print('Packaged app authenticated health, startup poke, 15-second survival passed')
PY
