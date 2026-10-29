#!/usr/bin/env bash
set -euo pipefail
app="${1:?usage: native-launch-smoke.sh APP [evidence-dir]}"
evidence="${2:-${NATIVE_LAUNCH_EVIDENCE_DIR:-${RUNNER_TEMP:-/tmp}/native-launch-evidence}}"
[[ "$(uname -s)" == Darwin && "$(uname -m)" == arm64 ]] || { echo 'requires macos arm64' >&2; exit 2; }
[[ -d "$app/Contents/MacOS" ]] || { echo "app missing: $app" >&2; exit 2; }
mkdir -p "$evidence"
e2e="$(mktemp -d "${RUNNER_TEMP:-/tmp}/native-launch-e2e.XXXXXX")"
chmod 700 "$e2e"
export MUNINN_E2E_DIR="$e2e" MUNINN_NATIVE_UI_SMOKE=1
fixture="$e2e/upstream.pem"
python3 - "$fixture" <<'PY'
import pathlib, subprocess, sys
cert, key = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[1]).with_suffix('.key')
subprocess.run(['openssl','req','-x509','-newkey','rsa:2048','-nodes','-days','2','-keyout',str(key),'-out',str(cert),'-subj','/CN=chatgpt.com','-addext','subjectAltName=DNS:chatgpt.com,DNS:localhost,IP:127.0.0.1'],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
PY
addrfile="$e2e/upstream.addr"
python3 "$(dirname "$0")/native-tls-fixture.py" "$fixture" "$evidence/provider-fixture.json" "$addrfile" &
fixture_pid=$!
cleanup_fixture() { kill "$fixture_pid" 2>/dev/null || true; wait "$fixture_pid" 2>/dev/null || true; }
trap cleanup_fixture EXIT
for _ in $(seq 1 100); do [[ -s "$addrfile" ]] && break; sleep .1; done
[[ -s "$addrfile" ]] || { echo 'TLS fixture failed to start' >&2; exit 1; }
export MUNINN_PROVIDER_SMOKE_UPSTREAM="$(cat "$addrfile")" MUNINN_PROVIDER_SMOKE_ROOTS="$fixture"
python3 "$(dirname "$0")/native-launch-driver.py" "$app" "$evidence"
