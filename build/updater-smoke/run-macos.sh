#!/usr/bin/env bash
# Real debug Tauri updater integration. Requires hosted Apple Silicon macOS.
set -euo pipefail
# HOME is replaced below for updater isolation; resolve Rust's real homes first.
REAL_HOME="${HOME:-$(python3 -c 'import os,pwd; print(pwd.getpwuid(os.getuid()).pw_dir)')}"
export RUSTUP_HOME="${RUSTUP_HOME:-$REAL_HOME/.rustup}"
export CARGO_HOME="${CARGO_HOME:-$REAL_HOME/.cargo}"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
MUNINN_UPDATER_SMOKE_APP="${CHAT_HUB_UPDATER_SMOKE_APP:-${MUNINN_UPDATER_SMOKE_APP:-}}"
: "${MUNINN_UPDATER_SMOKE_APP:?set CHAT_HUB_UPDATER_SMOKE_APP (or legacy MUNINN_UPDATER_SMOKE_APP) to built debug Chat Hub.app}"
[[ "$(uname -s)" == Darwin && "$(uname -m)" == arm64 ]] || { echo 'FAIL: Apple Silicon macOS required' >&2; exit 2; }
[[ -d "$MUNINN_UPDATER_SMOKE_APP/Contents/MacOS" ]] || { echo 'FAIL: expected actual .app bundle' >&2; exit 2; }
TMP="$(mktemp -d "${TMPDIR:-/tmp}/muninn-updater-smoke.XXXXXX")"; chmod 700 "$TMP"
# Persist only the final boolean evidence document when requested; fixture/key/tmp data stay private and ephemeral.
EVIDENCE_DIR="${MUNINN_UPDATER_SMOKE_EVIDENCE_DIR:-}"
cleanup() {
  local rc=$?
  if [[ -n "$EVIDENCE_DIR" ]]; then
    mkdir -p "$EVIDENCE_DIR"; chmod 700 "$EVIDENCE_DIR"
    # Results contain assertion/status evidence only; never copy logs, keys, or runtime state.
    for name in healthy-result tampered-result evidence; do
      if [[ -s "$TMP/$name.json" ]]; then install -m 600 "$TMP/$name.json" "$EVIDENCE_DIR/$name.json"; fi
    done
  fi
  [[ "${MUNINN_UPDATER_SMOKE_KEEP:-0}" == 1 ]] || rm -rf "$TMP"
  return "$rc"
}
trap cleanup EXIT
mkdir -p "$TMP/data" "$TMP/feed" "$TMP/e2e" "$TMP/home" "$TMP/healthy" "$TMP/tampered"
# Preserve real bundle roots and isolate the source app from installer mutation.
export HOME="$TMP/home" MUNINN_DATA_DIR="$TMP/data" MUNINN_E2E_DIR="$TMP/e2e"
export MUNINN_NATIVE_UI_SMOKE=1
PORT="$(python3 -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1]);s.close()')"
export MUNINN_UPDATER_SMOKE_ENDPOINT="http://127.0.0.1:$PORT/latest.json"
# Build each version from an independent temporary source archive. Never
# version-bump, regenerate locks, or compile into the operator's checkout.
build_fixture() {
  local version="$1" target="$TMP/target-$1" source="$TMP/source-$1"
  mkdir -p "$source"
  git -C "$ROOT" archive HEAD | tar -x -C "$source"
  # Include the assigned smoke harness changes, which may not yet be committed.
  rm -rf "$source/build/updater-smoke"
  mkdir -p "$source/build"
  cp -R "$ROOT/build/updater-smoke" "$source/build/updater-smoke"
  python3 "$ROOT/build/updater-smoke/prepare_fixture.py" "$source" "$version"
  CARGO_TARGET_DIR="$target" cargo build --locked --offline --manifest-path "$source/src-tauri/Cargo.toml" --target aarch64-apple-darwin --features native-smoke
  local app="$TMP/build-$version/Chat Hub-$version.app" exe
  mkdir -p "$(dirname "$app")"
  ditto "$MUNINN_UPDATER_SMOKE_APP" "$app"
  exe="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$app/Contents/Info.plist")"
  cp "$target/aarch64-apple-darwin/debug/chat-hub-desktop" "$app/Contents/MacOS/$exe"
  /usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $version" "$app/Contents/Info.plist"
  python3 - "$app" "$version" <<'PY'
import pathlib,plistlib,sys
p=pathlib.Path(sys.argv[1]); info=p/'Contents/Info.plist'
with info.open('rb') as f: data=plistlib.load(f)
assert data.get('CFBundleShortVersionString') == sys.argv[2], (data.get('CFBundleShortVersionString'),sys.argv[2])
PY
  codesign --force --deep --sign - "$app"
}
OLD="0.1.0"
NEW="0.1.1"
# Each fixture's desktop manifest/config/package and locked package record match.
build_fixture "$OLD"
build_fixture "$NEW"
# Use the genuinely compiled old-version app as both isolated starting copies.
APP="$TMP/build-$OLD/Chat Hub-$OLD.app"
for bundle in "$TMP/healthy/Chat Hub.app" "$TMP/tampered/Chat Hub.app"; do
  rm -rf "$bundle"
  ditto "$APP" "$bundle"
  codesign --force --deep --sign - "$bundle"
done

# Isolated ephemeral minisign key; no key material is printed.
npx --yes @tauri-apps/cli@2.10.1 signer generate --ci --write-keys "$TMP/test.key" >"$TMP/keygen.log" 2>&1
PUB="$(find "$TMP" -maxdepth 1 -type f -name '*.key.pub' -print -quit)"
[[ -s "$PUB" ]] || { echo 'FAIL: signer did not produce public key' >&2; exit 2; }
export MUNINN_UPDATER_SMOKE_PUBKEY="$(cat "$PUB")"
# Produce genuine gzip/tar updater payloads rooted at Chat Hub.app.
make_archive() {
  local version="$1" out="$2" copy="$TMP/Chat Hub.app"
  rm -rf "$copy"; ditto "$TMP/build-$version/Chat Hub-$version.app" "$copy"
  codesign --force --deep --sign - "$copy"
  mkdir -p "$out"
  tar -czf "$out/Chat-Hub.app.tar.gz" -C "$TMP" "Chat Hub.app"
  # CLI emits signature in the requested output file; never scrape stdout.
  npx --yes @tauri-apps/cli@2.10.1 signer sign --private-key-path "$TMP/test.key" --password '' "$out/Chat-Hub.app.tar.gz" >"$TMP/sign.log" 2>&1
  [[ -s "$out/Chat-Hub.app.tar.gz.sig" ]] || { echo 'FAIL: signer did not emit detached .sig file' >&2; exit 2; }
  cp "$out/Chat-Hub.app.tar.gz.sig" "$out/signature"
}
mkdir -p "$TMP/old" "$TMP/new"
make_archive "$OLD" "$TMP/old"
make_archive "$NEW" "$TMP/new"
SIG="$(cat "$TMP/new/Chat-Hub.app.tar.gz.sig")"
OLD_SIG="$(cat "$TMP/old/Chat-Hub.app.tar.gz.sig")"

python3 - "$TMP" "$OLD" "$OLD_SIG" "$NEW" "$SIG" "$PORT" <<'PY'
import json,pathlib,sys
p=pathlib.Path(sys.argv[1]); old,old_sig,version,sig,port=sys.argv[2:]
(p/'old.json').write_text(json.dumps({'version':old,'signature':old_sig})+'\n')
(p/'new.json').write_text(json.dumps({'version':version,'signature':sig})+'\n')
(p/'feed/latest.json').write_text(json.dumps({'version':version,'notes':'synthetic test','pub_date':None,'platforms':{'darwin-aarch64':{'url':f'http://127.0.0.1:{port}/Chat-Hub.app.tar.gz','signature':sig}}})+'\n')
PY
cp "$TMP/new/Chat-Hub.app.tar.gz" "$TMP/feed/Chat-Hub.app.tar.gz"
export MUNINN_UPDATER_SMOKE_CURRENT_VERSION="$OLD"
python3 "$ROOT/build/updater-smoke/verify_artifacts.py" "$TMP"
# loopback-only server, shared by actual plugin fetches
python3 -m http.server "$PORT" --bind 127.0.0.1 --directory "$TMP/feed" >"$TMP/http.log" 2>&1 & SERVER=$!
trap 'kill "$SERVER" 2>/dev/null || true; cleanup' EXIT
# Wait for the actual feed endpoint, not an assumed startup delay. Keep the server
# alive throughout polling and fail with a bounded, credential-free diagnostic.
ready=0
for attempt in {1..50}; do
  if ! kill -0 "$SERVER" 2>/dev/null; then
    echo 'FAIL: updater feed server exited before readiness' >&2
    tail -n 20 "$TMP/http.log" >&2 || true
    exit 2
  fi
  if curl --silent --show-error --fail --max-time 1 \
      "http://127.0.0.1:$PORT/latest.json" -o /dev/null 2>/dev/null; then
    ready=1
    break
  fi
  sleep 0.1
done
if [[ "$ready" != 1 ]]; then
  echo 'FAIL: updater feed did not become ready at loopback endpoint' >&2
  tail -n 20 "$TMP/http.log" >&2 || true
  exit 2
fi
# Run actual installed bundle; launcher itself accesses bridge via E2E command files.
python3 "$ROOT/build/updater-smoke/driver.py" healthy "$TMP" "$TMP/healthy/Chat Hub.app"
# Tamper distinct copy, keep healthy candidate/evidence untouched, and retain genuine NEW signature/version.
python3 - "$TMP" "$NEW" <<'PY'
import json,pathlib,sys
p=pathlib.Path(sys.argv[1]); version=sys.argv[2]
archive=p/'feed/Chat-Hub.app.tar.gz'
source=p/'new/Chat-Hub.app.tar.gz'; b=bytearray(source.read_bytes()); b[len(b)//2]^=1; archive.write_bytes(b)
# Keep the original signed metadata and same in-feed URL: only payload bytes are tampered.
PY
python3 "$ROOT/build/updater-smoke/driver.py" tampered "$TMP" "$TMP/tampered/Chat Hub.app"
python3 "$ROOT/build/updater-smoke/verify_evidence.py" "$TMP"
echo 'PASS: native updater smoke; boolean-only evidence generated'
