#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

if ! command -v bun >/dev/null 2>&1; then
  echo "Bun is required for the desktop JavaScript tests." >&2
  exit 127
fi

shopt -s nullglob
tests=(./tests/*.test.cjs)
if ((${#tests[@]} == 0)); then
  echo "No desktop JavaScript test files found." >&2
  exit 1
fi

bun test "${tests[@]}"
