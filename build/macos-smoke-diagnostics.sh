# Shared failure reporter for native macOS smoke entry points. Source from bash.
smoke_diag_init() {
  SMOKE_DIAG_COMMAND='(not started)'; SMOKE_DIAG_CODE=0
  trap 'smoke_diag_exit $?' EXIT
}
smoke_diag_exit() {
  local rc="$1" reason escaped f
  trap - EXIT
  (( rc == 0 )) && return 0
  reason="${SMOKE_DIAG_COMMAND:-unknown command} (exit code $rc)"
  escaped=${reason//'%'/'%25'}; escaped=${escaped//$'\r'/'%0D'}; escaped=${escaped//$'\n'/'%0A'}
  echo "::error title=release-launch-smoke::$escaped"
  local summary="${GITHUB_STEP_SUMMARY:-}"
  {
    echo '::group::macOS native smoke failure diagnostics'
    echo "Failure: $reason"
    echo "App PID: ${app_pid:-unassigned}; alive: $(if [[ -n ${app_pid:-} ]] && kill -0 "$app_pid" 2>/dev/null; then echo yes; else echo no; fi)"
    for f in "${SMOKE_EVIDENCE_DIR:-${RUNNER_TEMP:-/tmp}/macos-smoke-evidence}/app.log" "${SMOKE_EVIDENCE_DIR:-${RUNNER_TEMP:-/tmp}/macos-smoke-evidence}/app.stdout.log" "${SMOKE_EVIDENCE_DIR:-${RUNNER_TEMP:-/tmp}/macos-smoke-evidence}/app.stderr.log" "${SMOKE_EVIDENCE_DIR:-${RUNNER_TEMP:-/tmp}/macos-smoke-evidence}/osascript.stderr.log"; do
      [[ -f "$f" ]] || continue; echo "--- $f (last 80 lines) ---"; tail -80 "$f" || true
    done
    if [[ -f "${SMOKE_EVIDENCE_DIR:-${RUNNER_TEMP:-/tmp}/macos-smoke-evidence}/crash-reports.txt" ]]; then cat "${SMOKE_EVIDENCE_DIR:-${RUNNER_TEMP:-/tmp}/macos-smoke-evidence}/crash-reports.txt"; fi
    if command -v log >/dev/null 2>&1; then log show --last 1m --predicate 'process == "chat-hub-desktop"' --style compact 2>&1 | tail -60 || true; fi
    echo '::endgroup::'
  } || true
  if [[ -n "$summary" ]]; then
    { echo '### macOS native smoke failure'; echo; echo "Failure: $reason"; echo; echo 'See grouped workflow log and uploaded smoke evidence for bounded diagnostics.'; } >> "$summary" || true
  fi
  return "$rc"
}
smoke_diag_command() { SMOKE_DIAG_COMMAND="$*"; }
