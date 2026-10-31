#!/usr/bin/env bash
# Explicit public-site load check. Optional isolated proxy/capture opt-in;
# never signs in or submits a message.
set -euo pipefail
[[ "${MUNINN_PROVIDER_DIRECT_SMOKE:-}" == 1 ]] || { echo 'Set MUNINN_PROVIDER_DIRECT_SMOKE=1 to test the public ChatGPT page.' >&2; exit 2; }
app="${1:?usage: native-provider-direct-smoke.sh APP EVIDENCE_DIR}"
evidence="${2:?evidence directory required}"
e2e="$(mktemp -d "${TMPDIR:-/tmp}/chat-hub-direct-e2e.XXXXXX")"
chmod 700 "$e2e"
trap 'rm -rf "$e2e"' EXIT
export MUNINN_E2E_DIR="$e2e" MUNINN_E2E_DATA_DIR="$e2e/data" MUNINN_NATIVE_UI_SMOKE=1
[[ "${MUNINN_PROVIDER_CAPTURE_SMOKE:-0}" == 0 || "${MUNINN_PROVIDER_CAPTURE_SMOKE:-0}" == 1 ]] || { echo 'Capture smoke opt-in must be 0 or 1.' >&2; exit 2; }
export CHAT_HUB_CAPTURE_ENABLED="${MUNINN_PROVIDER_CAPTURE_SMOKE:-0}"
unset MUNINN_PROVIDER_SMOKE_UPSTREAM MUNINN_PROVIDER_SMOKE_ROOTS
python3 "$(dirname "$0")/native-ui-driver.py" "$app" "$evidence"
