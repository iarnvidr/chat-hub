#!/usr/bin/env bash
set -euo pipefail
app="${1:?usage: native-ui-smoke.sh APP [evidence-dir]}"
evidence="${2:-${NATIVE_UI_EVIDENCE_DIR:-${RUNNER_TEMP:-/tmp}/native-ui-evidence}}"
mkdir -p "$evidence"
if [[ "$(uname -s)" != Darwin || "$(uname -m)" != arm64 ]]; then echo 'requires macos arm64' >&2; exit 2; fi
[[ -d "$app/Contents/MacOS" ]] || { echo "app missing: $app" >&2; exit 2; }
printf '%s\n' 'BLOCKED: native UI automation bridge/screenshots not implemented in this checkout.' | tee "$evidence/blocker.txt" >&2
exit 2
