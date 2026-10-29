#!/usr/bin/env bash
# Build a real Tauri Apple Silicon app bundle, ad-hoc signed and not notarized.
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ "$(uname -s)" != Darwin || "$(uname -m)" != arm64 ]]; then
  echo "Run on an Apple Silicon Mac with Xcode Command Line Tools." >&2
  exit 2
fi
out="${1:-$PWD/dist}"
mkdir -p "$out"
out="$(cd "$out" && pwd)"
export MACOSX_DEPLOYMENT_TARGET=14.0
npm ci --ignore-scripts
cargo test --locked --manifest-path src-tauri/Cargo.toml --workspace
npm run tauri -- build --locked --config src-tauri/tauri.conf.json --target aarch64-apple-darwin --bundles app
app="src-tauri/target/aarch64-apple-darwin/release/bundle/macos/Chat Hub.app"
if [[ ! -d "$app" ]]; then app="target/aarch64-apple-darwin/release/bundle/macos/Chat Hub.app"; fi
test -d "$app"
codesign --force --deep --sign - "$app"
codesign --verify --deep --strict --verbose=2 "$app" > "$out/verify-macos.txt" 2>&1
lipo "$app/Contents/MacOS/$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$app/Contents/Info.plist")" -verify_arch arm64
ditto "$app" "$out/Chat Hub.app"
printf 'PASS: ad-hoc signature verified; not notarized.\n' >> "$out/verify-macos.txt"
