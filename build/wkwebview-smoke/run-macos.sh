#!/usr/bin/env bash
set -euo pipefail
# WKWebView proxy/trust/capture smoke requires the app-owned native bridge.
evidence="${WKWEBVIEW_SMOKE_EVIDENCE_DIR:-${RUNNER_TEMP:-/tmp}/wkwebview-smoke-evidence}"
mkdir -p "$evidence"
printf '%s\n' 'BLOCKED: Tauri shell does not expose app-scoped CONNECT proxy, per-install CA trust, or capture/ingest bridge.' | tee "$evidence/blocker.txt" >&2
exit 2
