#!/bin/sh
set -eu

repo_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
install_root=${CHAT_HUB_CLI_INSTALL_ROOT:-"$HOME/.local"}

if ! command -v cargo >/dev/null 2>&1; then
  printf '%s\n' 'Rust/Cargo is required to build the CLI.' >&2
  exit 1
fi

cargo install --path "$repo_dir" --locked --root "$install_root" --bin chat-hub --force
printf '\nInstalled %s\n' "$install_root/bin/chat-hub"
printf '%s\n' 'Run: chat-hub --help'
case ":$PATH:" in
  *":$install_root/bin:"*) ;;
  *) printf 'Add %s to your shell PATH.\n' "$install_root/bin" ;;
esac
