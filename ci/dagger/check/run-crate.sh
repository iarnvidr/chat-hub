#!/usr/bin/env bash
# Invoked inside run-rust.sh's stable worktree and locked Cargo cache.
set -euo pipefail
if (( $# != 1 )) || [[ ! "$1" =~ ^chat-hub(-[a-z]+)*$ ]]; then
  printf '%s\n' 'usage: run-crate.sh CARGO_PACKAGE' >&2
  exit 2
fi
cargo clippy --locked -p "$1" --all-targets --all-features -- -D warnings
cargo test --locked -p "$1" --all-targets --timings -vv
