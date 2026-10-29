#!/usr/bin/env bash
# Shared failure wrapper: invoke with `run_smoke STEP EVIDENCE LOGS... -- command args...`.
run_smoke() {
  local step="$1" evidence="$2"; shift 2
  local logs="$1"; shift
  [[ "$1" == -- ]] && shift
  local cmd; printf -v cmd '%q ' "$@"
  set +e
  "$@"
  local rc=$?
  set -e
  if (( rc != 0 )); then
    python3 "$(dirname "${BASH_SOURCE[0]}")/native-smoke-report.py" "$step" "$rc" "$cmd" "$evidence" "$logs" "$([[ "$step" == 16 || "$step" == 17 ]] && echo namespace || echo '')"
    return "$rc"
  fi
}
