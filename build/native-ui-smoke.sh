#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/native-smoke-wrapper.sh"
# Preserve the runner's Rust installation despite isolated application HOME.
REAL_HOME="${HOME:-$(python3 -c 'import os,pwd; print(pwd.getpwuid(os.getuid()).pw_dir)')}"
export RUSTUP_HOME="${RUSTUP_HOME:-$REAL_HOME/.rustup}"
export CARGO_HOME="${CARGO_HOME:-$REAL_HOME/.cargo}"
app="${1:?usage: native-ui-smoke.sh APP [evidence-dir]}"
evidence="${2:-${NATIVE_UI_EVIDENCE_DIR:-${RUNNER_TEMP:-/tmp}/native-ui-evidence}}"
mkdir -p "$evidence"
if [[ "$(uname -s)" != Darwin || "$(uname -m)" != arm64 ]]; then echo 'requires macos arm64' >&2; exit 2; fi
[[ -d "$app/Contents/MacOS" ]] || { echo "app missing: $app" >&2; exit 2; }
e2e="$(mktemp -d "${RUNNER_TEMP:-/tmp}/native-ui-e2e.XXXXXX")"
chmod 700 "$e2e"
trap 'rm -rf "$e2e"' EXIT
export MUNINN_E2E_DIR="$e2e" MUNINN_E2E_DATA_DIR="$e2e/data" MUNINN_NATIVE_UI_SMOKE=1
# This UI smoke is independently runnable: provide its own deterministic TLS
# provider fixture rather than relying on native-launch-smoke having run first.
fixture="$e2e/upstream.pem"
python3 "$(dirname "$0")/native-tls-cert.py" "$fixture"
addrfile="$e2e/upstream.addr"
fixture_script="$(dirname "$0")/native-tls-fixture.py"
python3 "$fixture_script" "$fixture" "$evidence/provider-fixture.json" "$addrfile" >"$evidence/provider-fixture.log" 2>&1 &
fixture_pid=$!
cleanup_fixture() { kill "$fixture_pid" 2>/dev/null || true; wait "$fixture_pid" 2>/dev/null || true; }
trap 'cleanup_fixture; rm -rf "$e2e"' EXIT
ready=0
for _ in $(seq 1 100); do
  if [[ -s "$addrfile" ]]; then
    hostport="$(cat "$addrfile")"
    if python3 - "$hostport" "$fixture" <<'PY'
import pathlib, socket, ssl, sys
host, port = sys.argv[1].split(':')
ctx=ssl.create_default_context(cafile=sys.argv[2]); ctx.verify_flags |= ssl.VERIFY_X509_STRICT
with socket.create_connection((host,int(port)),timeout=1) as raw:
    with ctx.wrap_socket(raw,server_hostname='chatgpt.com') as tls:
        tls.sendall(b'GET / HTTP/1.1\r\nHost: chatgpt.com\r\nConnection: close\r\n\r\n')
        response=tls.recv(4096)
        if b'HTTP/1.0 200' not in response: raise SystemExit('unexpected fixture response')
PY
    then ready=1; break; fi
  fi
  if ! kill -0 "$fixture_pid" 2>/dev/null; then cat "$evidence/provider-fixture.log" >&2; exit 1; fi
  sleep .1
done
[[ "$ready" == 1 ]] || { cat "$evidence/provider-fixture.log" >&2; echo 'TLS fixture readiness timed out' >&2; exit 1; }
export MUNINN_PROVIDER_SMOKE_UPSTREAM="$(cat "$addrfile")" MUNINN_PROVIDER_SMOKE_ROOTS="$fixture"
run_smoke 17 "$evidence" "$evidence/app.stdout.log:$evidence/provider-fixture.log" -- python3 "$(dirname "$0")/native-ui-driver.py" "$app" "$evidence"
