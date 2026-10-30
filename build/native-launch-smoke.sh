#!/usr/bin/env bash
set -euo pipefail
export RUST_BACKTRACE=1
source "$(dirname "$0")/native-smoke-wrapper.sh"
# Preserve the runner's real Rust toolchain home before tests isolate HOME.
REAL_HOME="${HOME:-$(python3 -c 'import os,pwd; print(pwd.getpwuid(os.getuid()).pw_dir)')}"
export RUSTUP_HOME="${RUSTUP_HOME:-$REAL_HOME/.rustup}"
export CARGO_HOME="${CARGO_HOME:-$REAL_HOME/.cargo}"
app="${1:?usage: native-launch-smoke.sh APP [evidence-dir]}"
evidence="${2:-${NATIVE_LAUNCH_EVIDENCE_DIR:-${RUNNER_TEMP:-/tmp}/native-launch-evidence}}"
[[ "$(uname -s)" == Darwin && "$(uname -m)" == arm64 ]] || { echo 'requires macos arm64' >&2; exit 2; }
[[ -d "$app/Contents/MacOS" ]] || { echo "app missing: $app" >&2; exit 2; }
mkdir -p "$evidence"
e2e="$(mktemp -d "${RUNNER_TEMP:-/tmp}/native-launch-e2e.XXXXXX")"
chmod 700 "$e2e"
export MUNINN_E2E_DIR="$e2e" MUNINN_E2E_DATA_DIR="$e2e/data" MUNINN_NATIVE_UI_SMOKE=1
fixture="$e2e/upstream.pem"
# Certificate SANs are generated portably in the dedicated helper.
# subjectAltName=DNS:chatgpt.com is configured there.
python3 "$(dirname "$0")/native-tls-cert.py" "$fixture"
addrfile="$e2e/upstream.addr"
fixture_script="$(dirname "$0")/native-tls-fixture.py"
python3 "$fixture_script" "$fixture" "$evidence/provider-fixture.json" "$addrfile" >"$evidence/provider-fixture.log" 2>&1 &
fixture_pid=$!
cleanup_fixture() { kill "$fixture_pid" 2>/dev/null || true; wait "$fixture_pid" 2>/dev/null || true; }
cleanup_launch() { local rc=$?; cleanup_fixture; rm -rf "$e2e"; return "$rc"; }
trap cleanup_launch EXIT
report_fixture_failure() {
  local status=unknown reason='address file not published before timeout'
  if kill -0 "$fixture_pid" 2>/dev/null; then
    reason='fixture still alive but address file not published before timeout'
    kill "$fixture_pid" 2>/dev/null || true
  else
    if wait "$fixture_pid" 2>/dev/null; then status=0; else status=$?; fi
    reason='fixture exited before publishing address file'
  fi
  echo 'TLS fixture startup failed; complete bounded diagnostics follow:' >&2
  printf 'reason: %s\n' "$reason" >&2
  printf 'command: %q %q %q %q %q\npid: %s\nwait_exit_status: %s\n' "$(command -v python3 || echo '<python3-not-found>')" "$fixture_script" "$fixture" "$evidence/provider-fixture.json" "$addrfile" "$fixture_pid" "$status" >&2
  printf 'python: ' >&2; python3 --version >&2 || true
  for path in "$fixture_script" "$fixture" "${fixture%.pem}.key" "$(dirname "$fixture")" "$addrfile" "$evidence/provider-fixture.log"; do
    if [[ -e "$path" ]]; then ls -ld "$path" >&2; else printf 'missing: %s\n' "$path" >&2; fi
  done
  if [[ -s "$evidence/provider-fixture.log" ]]; then cat "$evidence/provider-fixture.log" >&2; else echo '(fixture stdout/stderr log empty)' >&2; fi
  exit 1
}
ready=0
for _ in $(seq 1 100); do
  if [[ -s "$addrfile" ]]; then
    host="${MUNINN_PROVIDER_SMOKE_HOSTNAME:-chatgpt.com}"
    if python3 - "$addrfile" "$fixture" "$host" <<'PY'
import http.client, pathlib, socket, ssl, sys
host, port = pathlib.Path(sys.argv[1]).read_text().strip().split(':')
context = ssl.create_default_context(cafile=sys.argv[2])
context.verify_flags |= ssl.VERIFY_X509_STRICT
with socket.create_connection((host, int(port)), timeout=1) as raw:
    with context.wrap_socket(raw, server_hostname=sys.argv[3]) as tls:
        tls.sendall(b'GET / HTTP/1.1\r\nHost: chatgpt.com\r\nConnection: close\r\n\r\n')
        response = http.client.HTTPResponse(tls)
        response.begin()
        body = response.read()
        if response.status != 200 or b'synthetic-native-smoke' not in body:
            raise SystemExit(f'TLS fixture returned unexpected HTTP status {response.status}')
PY
    then ready=1; break; fi
  fi
  if ! kill -0 "$fixture_pid" 2>/dev/null; then report_fixture_failure; fi
  sleep .1
done
[[ "$ready" == 1 ]] || report_fixture_failure
export MUNINN_PROVIDER_SMOKE_UPSTREAM="$(cat "$addrfile")" MUNINN_PROVIDER_SMOKE_ROOTS="$fixture"
run_smoke 16 "$evidence" "$evidence/app.log:$evidence/open.log:$evidence/provider-fixture.log" -- python3 "$(dirname "$0")/native-launch-driver.py" "$app" "$evidence"
