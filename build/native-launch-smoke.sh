#!/usr/bin/env bash
set -euo pipefail
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
python3 "$(dirname "$0")/native-tls-fixture.py" "$fixture" "$evidence/provider-fixture.json" "$addrfile" >"$evidence/provider-fixture.log" 2>&1 &
fixture_pid=$!
cleanup_fixture() { kill "$fixture_pid" 2>/dev/null || true; wait "$fixture_pid" 2>/dev/null || true; }
trap cleanup_fixture EXIT
for _ in $(seq 1 100); do [[ -s "$addrfile" ]] && break; sleep .1; done
if [[ ! -s "$addrfile" ]]; then
  echo 'TLS fixture failed to start; bounded fixture diagnostic follows:' >&2
  cat "$evidence/provider-fixture.log" >&2 || true
  exit 1
fi
export MUNINN_PROVIDER_SMOKE_UPSTREAM="$(cat "$addrfile")" MUNINN_PROVIDER_SMOKE_ROOTS="$fixture"
python3 "$(dirname "$0")/native-launch-driver.py" "$app" "$evidence"
