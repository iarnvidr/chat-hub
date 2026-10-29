#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/macos-smoke-diagnostics.sh"
SMOKE_EVIDENCE_DIR="${SMOKE_EVIDENCE_DIR:-${RUNNER_TEMP:-/tmp}/macos-smoke-evidence}"
mkdir -p "$SMOKE_EVIDENCE_DIR"
smoke_diag_init
app="${1:?usage: smoke-native-macos.sh APP}"
smoke_diag_command "native smoke setup for $app"
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
"$app/Contents/MacOS/$exe" >"$evidence/app.stdout.log" 2>"$evidence/app.stderr.log" & app_pid=$!
echo "$app_pid" >"$evidence/app.pid"
smoke_diag_command "launch/authenticated-health/activation smoke (pid $app_pid)"
printf '%s\n' "$app_pid" >"$evidence/app.pid"
service_json_path="$app_data/service.json"

on_exit() {
  local harness_exitcode=$? app_alive=false app_exitcode=null cleanup_started=false
  trap - EXIT
  # Snapshot crash reports and process state BEFORE harness cleanup. A later
  # wait status after terminate/kill is not evidence of an application crash.
  find "$HOME" "$HOME/Library/Logs/DiagnosticReports" "$HOME/Library/Logs/DiagnosticReports"/../.. -type f \( -name 'chat-hub-desktop*.ips' -o -name 'chat-hub-desktop*.crash' \) -print 2>/dev/null | sort -u >"$evidence/crash-reports.txt" || true
  if kill -0 "$app_pid" 2>/dev/null; then app_alive=true
  else
    set +e; wait "$app_pid"; app_exitcode=$?; set -e
  fi
  cleanup_started=true
  if [[ "$app_alive" == true ]]; then
    kill "$app_pid" 2>/dev/null || true
    set +e; wait "$app_pid"; local cleanup_exitcode=$?; set -e
  else cleanup_exitcode=null; fi
  # Failure-only secondary launch after primary diagnostics and cleanup wait.
  if (( harness_exitcode != 0 )); then
    python3 "$(dirname "$0")/macos-process-probe.py" "$app/Contents/MacOS/$exe" "$evidence" --duration 10 || echo 'secondary process probe failed; primary smoke status preserved' >&2
  fi
  python3 - "$evidence/diagnostics.json" "$service_json_path" "$app_alive" "$app_exitcode" "$cleanup_started" "$cleanup_exitcode" <<'PY'
import json, pathlib, sys, time
path, service, alive, code, cleanup, cleanupcode = sys.argv[1:]
pathlib.Path(path).write_text(json.dumps({"service_json_path":service,"app_alive_before_cleanup":alive=="true","app_exitcode_before_cleanup":None if code=="null" else int(code),"cleanup_started":cleanup=="true","cleanup_exitcode":None if cleanupcode=="null" else int(cleanupcode)},indent=2)+"\\n")
PY
  if (( harness_exitcode != 0 )); then smoke_diag_exit "$harness_exitcode"; fi
  exit "$harness_exitcode"
}
trap on_exit EXIT
python3 - "$evidence" "$app_data" "$app_pid" "$app" <<'PY'
import json, os, sys, time, urllib.request, urllib.error, subprocess, plistlib
from pathlib import Path

def alive(pid):
 try: os.kill(pid,0); return True
 except ProcessLookupError: return False
evidence,data,pid,app=Path(sys.argv[1]),Path(sys.argv[2]),int(sys.argv[3]),sys.argv[4]; info_path=data/'service.json'; token_path=data/'service.token'
bundle_id=plistlib.loads((Path(app)/'Contents/Info.plist').read_bytes())['CFBundleIdentifier']
for _ in range(120):
 try:
  info=json.loads(info_path.read_text()); token=token_path.read_text().strip()
  if len(token)>=64 and info.get('api','').startswith('http://127.0.0.1:'):
   health_path='/api/health'
   req=urllib.request.Request(info['api']+health_path,headers={'Authorization':'Bearer '+token})
   with urllib.request.urlopen(req,timeout=3) as r: health=json.load(r)
   if health.get('status')=='ok': break
 except (OSError,ValueError,KeyError,urllib.error.URLError): time.sleep(.5)
else: raise SystemExit('packaged app authenticated health endpoint not ready')
(evidence/'health.json').write_text(json.dumps({'status':health['status'],'version':info.get('version'),'process_id':info.get('process_id')},indent=2)+'\n')
# Exercise LaunchServices activation and an explicit Apple event while retaining
# the original direct-child PID; do not quit until EXIT cleanup.
subprocess.run(['open','-a',str(app)],check=True,timeout=15)
with (evidence/'osascript.stderr.log').open('a') as err:
 subprocess.run(['osascript','-e',f'tell application id "{bundle_id}" to activate'],check=True,timeout=15,stdout=subprocess.DEVNULL,stderr=err)
current=json.loads(info_path.read_text())
if not alive(pid): raise SystemExit('app exited during startup poke')
if current.get('process_id') != int(pid): raise SystemExit('service PID changed after activation')
for _ in range(15):
 if not alive(pid): raise SystemExit('app exited after startup poke')
 with urllib.request.urlopen(req,timeout=3) as r: assert json.load(r).get('status')=='ok'
 time.sleep(1)
if not alive(pid): raise SystemExit('app did not survive 15 seconds after poke')
current=json.loads(info_path.read_text())
if current.get('process_id') != int(pid): raise SystemExit('service PID changed during survival')
print('Packaged app authenticated health, LaunchServices/Apple-event activation, same PID and 15-second post-poke survival passed')
PY
