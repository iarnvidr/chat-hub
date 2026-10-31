#!/usr/bin/env bash
# Build and mount-verify an ad-hoc signed Chat Hub DMG with source and NOTICE.
set -euo pipefail
app_input="$1"
sources_input="$2"
if [[ ! -d "$app_input" ]]; then
  echo "DMG packaging app bundle does not exist: $app_input" >&2
  exit 1
fi
if [[ ! -d "$sources_input" ]]; then
  echo "DMG packaging source bundle does not exist: $sources_input" >&2
  exit 1
fi
app="$(cd "$app_input" && pwd)"
sources="$(cd "$sources_input" && pwd)"
image_dir="$(dirname "$3")"
mkdir -p "$image_dir"
image="$(cd "$image_dir" && pwd)/$(basename "$3")"
[[ "$image" == *.dmg ]]
test -f "$app/Contents/Info.plist"
test -f "$app/Contents/Resources/NOTICE"
test -f "$sources/manifest.json"
codesign --verify --deep --strict "$app"
version=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$app/Contents/Info.plist")
stage="$(mktemp -d)"
mount="$stage/mount"
attached=0
cleanup() { if [[ "$attached" == 1 ]]; then hdiutil detach "$mount" >/dev/null || return; fi; rm -rf "$stage"; }
trap cleanup EXIT
mkdir -p "$stage/payload" "$mount"
ditto "$app" "$stage/payload/Chat Hub.app"
ditto "$sources" "$stage/payload/Corresponding Source"
ln -s /Applications "$stage/payload/Applications"
bash "$(dirname "$0")/create-dmg-image.sh" -volname 'Chat Hub' -srcfolder "$stage/payload" -fs HFS+ -format UDZO -ov "$image"
hdiutil verify "$image"
hdiutil attach "$image" -readonly -nobrowse -mountpoint "$mount"
attached=1
codesign --verify --deep --strict "$mount/Chat Hub.app"
test "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$mount/Chat Hub.app/Contents/Info.plist")" = "$version"
test -f "$mount/Chat Hub.app/Contents/Resources/NOTICE"
test -L "$mount/Applications" && test "$(readlink "$mount/Applications")" = /Applications
python3 - "$mount/Corresponding Source" <<'PY'
import hashlib, json, pathlib, sys
root = pathlib.Path(sys.argv[1]); manifest = json.loads((root/'manifest.json').read_text())
assert manifest['format'] == 'chat-hub-corresponding-source-v1'
for entry in manifest['files']:
    assert hashlib.sha256((root/entry['path']).read_bytes()).hexdigest() == entry['sha256']
assert (root/'NOTICE').is_file() and (root/'LICENSE').is_file()
PY
hdiutil detach "$mount"
attached=0
echo "$image"
