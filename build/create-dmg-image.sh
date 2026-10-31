#!/usr/bin/env bash
# Disk Arbitration can briefly hold a new image busy on hosted macOS runners.
# Retry only that diagnostic; all other hdiutil failures remain immediate.
set -euo pipefail
log="$(mktemp)"
trap 'rm -f "$log"' EXIT
for attempt in 1 2 3; do
  if hdiutil create "$@" >"$log" 2>&1; then
    cat "$log"
    exit 0
  else
    result=$?
  fi
  cat "$log" >&2
  case "$(cat "$log")" in
    *"Resource busy"*)
      if [[ "$attempt" == 3 ]]; then exit "$result"; fi
      echo "Retrying busy DMG creation (attempt $((attempt + 1))/3)." >&2
      sleep "$((attempt * 2))"
      ;;
    *) exit "$result" ;;
  esac
done
