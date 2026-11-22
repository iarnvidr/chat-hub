#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"

# The infra-owned restored library is supplied to the worker. Never guess a
# path to the seed: it is not mounted into this checkout by default.
database="${CHAT_HUB_DATABASE:-${CHAT_HUB_LIBRARY_PATH:-}}"
if [[ -z "$database" ]]; then
  echo 'Set CHAT_HUB_DATABASE to the restored library database path.' >&2
  exit 2
fi
if [[ ! -f "$database" ]]; then
  echo 'CHAT_HUB_DATABASE must point to an existing restored database file.' >&2
  exit 2
fi

if [[ "${CHAT_HUB_WEB_AUTH:-token}" != none ]]; then
  state_root="${XDG_STATE_HOME:-${HOME:?HOME must be set}/.local/state}"
  token_file="${CHAT_HUB_WEB_TOKEN_FILE:-$state_root/chat-hub-web/access-token}"
  python3 "$repo_root/scripts/ensure-private-token.py" "$token_file"
  export CHAT_HUB_WEB_TOKEN_FILE="$token_file"
fi

export CHAT_HUB_DATABASE="$database"
export CHAT_HUB_DESKTOP_UI="${CHAT_HUB_DESKTOP_UI:-$repo_root/desktop-ui}"
export CHAT_HUB_WEB_ADDR="${CHAT_HUB_WEB_ADDR:-0.0.0.0:8080}"
export CHAT_HUB_CAPTURE_ENABLED=false

exec cargo run --locked --manifest-path "$repo_root/Cargo.toml" -p chat-hub-web
