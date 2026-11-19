#!/usr/bin/env bash
# Invoked inside run-rust.sh's stable worktree and locked Cargo cache.
set -euo pipefail
if (( $# != 1 )) || [[ ! "$1" =~ ^chat-hub(-[a-z]+)*$ ]]; then
  printf '%s\n' 'usage: run-crate.sh CARGO_PACKAGE' >&2
  exit 2
fi
if [[ "$1" == "chat-hub-desktop" ]]; then
  # Tauri command handlers and platform adapters carry pre-existing style
  # warnings. Keep correctness checks mandatory without turning this workspace
  # extraction into an unrelated desktop-wide Clippy cleanup.
  cargo clippy --locked -p "$1" --all-targets --all-features -- -D clippy::correctness
else
  cargo clippy --locked -p "$1" --all-targets --all-features -- -D warnings
fi
cargo test --locked -p "$1" --all-targets --timings -vv
if [[ "$1" == "chat-hub" ]]; then
  # Cargo test builds test harnesses, not necessarily a runnable CLI. Produce
  # an actual command binary using the same locked dependency/artifact cache.
  cargo build --locked -p chat-hub --bin chat-hub
fi
