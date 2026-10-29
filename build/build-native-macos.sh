#!/usr/bin/env bash
# Build a native Apple Silicon Chat Hub app bundle; ad-hoc signed, not notarized.
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
cargo test --locked --workspace
cargo build --release --locked --target aarch64-apple-darwin
app="$out/Chat Hub.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
binary="${CARGO_TARGET_DIR:-target}/aarch64-apple-darwin/release/chat-hub"
test -x "$binary"
cp "$binary" "$app/Contents/MacOS/chat-hub"
cp LICENSE NOTICE "$app/Contents/Resources/"
version="$(python3 -c 'import json; print(json.load(open("src-tauri/tauri.conf.json"))["version"])')"
cat > "$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict><key>CFBundleExecutable</key><string>chat-hub</string><key>CFBundleIdentifier</key><string>io.github.iarnvidr.chat-hub</string><key>CFBundleName</key><string>Chat Hub</string><key>CFBundlePackageType</key><string>APPL</string><key>CFBundleShortVersionString</key><string>$version</string><key>CFBundleVersion</key><string>$version</string></dict></plist>
EOF
codesign --force --deep --sign - "$app"
codesign --verify --deep --strict --verbose=2 "$app" 2>&1 | tee "$out/verify-macos.txt"
lipo "$app/Contents/MacOS/chat-hub" -verify_arch arm64
