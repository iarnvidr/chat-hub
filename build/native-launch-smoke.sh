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
python3 "$(dirname "$0")/native-launch-driver.py" "$app" "$evidence"
