#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")" && pwd)"
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/evidence"; : > "$tmp/summary"
SMOKE_EVIDENCE_DIR="$tmp/evidence" GITHUB_STEP_SUMMARY="$tmp/summary"
export SMOKE_EVIDENCE_DIR GITHUB_STEP_SUMMARY
source "$root/macos-smoke-diagnostics.sh"
smoke_diag_init
smoke_diag_command 'mock command'
set +e
bash -c 'source "$1"; SMOKE_EVIDENCE_DIR="$2"; GITHUB_STEP_SUMMARY="$3"; source "$1"; smoke_diag_init; smoke_diag_command test; false' _ "$root/macos-smoke-diagnostics.sh" "$tmp/evidence" "$tmp/summary" >"$tmp/out" 2>&1
rc=$?
set -e
test "$rc" -eq 1
grep -q '::error title=release-launch-smoke::test (exit code 1)' "$tmp/out"
grep -q '::group::macOS native smoke failure diagnostics' "$tmp/out"
grep -q 'Failure: test (exit code 1)' "$tmp/summary"
test "$(grep -c '::error title=' "$tmp/out")" -eq 1
echo 'macOS smoke diagnostics failure status, grouped output, summary and nonrecursive error annotation passed'
