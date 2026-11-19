#!/usr/bin/env bash
# The caller holds the same Dagger LOCKED cache mount for synchronization and Cargo.
set -euo pipefail

if (( $# < 3 )); then
  printf '%s\n' 'usage: run-rust.sh INPUT_DIR CACHE_ROOT COMMAND [ARG ...]' >&2
  exit 2
fi
input_dir="$1"
cache_root="$2"
shift 2

if [[ ! -d "$input_dir" || -L "$input_dir" ]]; then
  printf '%s\n' 'Rust source input must be a real directory, not a symlink.' >&2
  exit 2
fi
if [[ "$cache_root" != /* || "$cache_root" == / || -L "$cache_root" ]]; then
  printf '%s\n' 'Rust cache root must be an absolute, non-root, non-symlink directory.' >&2
  exit 2
fi
input_dir="$(realpath "$input_dir")"
cache_root="$(realpath -m "$cache_root")"
if [[ "$cache_root" == / || "$cache_root" == "$input_dir" || "$cache_root" == "$input_dir"/* ]]; then
  printf '%s\n' 'Rust cache must be a non-root directory outside the source input.' >&2
  exit 2
fi
worktree="$cache_root/worktree"
target="$(realpath -m "${CARGO_TARGET_DIR:-$cache_root/target/default}")"
case "$target" in
  "$cache_root"/target/*) ;;
  *) printf '%s\n' 'CARGO_TARGET_DIR must be a namespace below CACHE_ROOT/target/.' >&2; exit 2 ;;
esac
if [[ -L "$worktree" || "$input_dir" == "$worktree" || "$input_dir" == "$cache_root"/* ]]; then
  printf '%s\n' 'Rust source must be outside the cache; cached worktree must not be a symlink.' >&2
  exit 2
fi
if [[ -n "$(find "$input_dir" -type l -print -quit)" ]]; then
  printf '%s\n' 'Rust source symlinks are unsupported: provide ordinary files inside the source input.' >&2
  exit 2
fi
mkdir -p "$worktree" "$target"
anchor="$(mktemp "$cache_root/.clock.XXXXXX")"
trap 'rm -f -- "$anchor"' EXIT
if [[ -n "$(find "$target" -type f -newer "$anchor" -print -quit)" ]]; then
  printf '%s\n' 'Restored Cargo target has future timestamps (clock skew). Retry with a fresh Cargo cache; refusing potentially stale output.' >&2
  exit 2
fi

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
python3 "$script_dir/stable-worktree.py" "$input_dir" "$cache_root"
rm -f -- "$anchor"
trap - EXIT
export CARGO_TARGET_DIR="$target"
cd "$worktree"
if [[ -n "${CHAT_HUB_CARGO_LOG:-}" ]]; then
  log_path="$(realpath -m "$CHAT_HUB_CARGO_LOG")"
  log_name="$(basename "$log_path")"
  if [[ "$(dirname "$log_path")" != "$cache_root/check-logs" || ! "$log_name" =~ ^[A-Za-z0-9_.-]+\.log$ ]]; then
    printf '%s\n' 'CHAT_HUB_CARGO_LOG must be under CACHE_ROOT/check-logs/.' >&2
    exit 2
  fi
  if [[ -L "$cache_root/check-logs" || -L "$CHAT_HUB_CARGO_LOG" ]]; then
    printf '%s\n' 'Rust diagnostic log must not be a symlink.' >&2
    exit 2
  fi
  mkdir -p "$cache_root/check-logs"
  "$@" 2>&1 | tee -- "$CHAT_HUB_CARGO_LOG"
else
  exec "$@"
fi
