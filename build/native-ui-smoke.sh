#!/usr/bin/env bash
set -euo pipefail
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
export MUNINN_E2E_DIR="$e2e" MUNINN_NATIVE_UI_SMOKE=1
python3 "$(dirname "$0")/native-ui-driver.py" "$app" "$evidence"
