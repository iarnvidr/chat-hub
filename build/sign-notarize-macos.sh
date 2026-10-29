#!/usr/bin/env bash
set -euo pipefail
umask 077
fail(){ printf 'ERROR: %s\n' "$1" >&2; exit 1; }
for n in APPLE_DEVELOPER_ID_APPLICATION_P12_BASE64 APPLE_DEVELOPER_ID_APPLICATION_P12_PASSWORD APPLE_TEAM_ID APPLE_NOTARY_API_KEY_ID APPLE_NOTARY_API_ISSUER_ID APPLE_NOTARY_API_PRIVATE_KEY; do [[ -n "${!n:-}" ]] || fail "required signing environment variable is unset: $n"; done
[[ $# == 1 ]] || fail 'usage: sign-notarize-macos.sh /absolute/path/Muninn.app'
app=$1; [[ $app = /* && -d $app && ${app##*/} = Muninn.app ]] || fail 'expected existing absolute path ending in Muninn.app'
[[ $(uname -s) == Darwin ]] || fail 'signing/notarization must run on macOS'
for t in security codesign xcrun spctl ditto; do command -v "$t" >/dev/null || fail "missing required tool: $t"; done
[[ -x $app/Contents/Resources/bin/deno ]] || fail 'required Deno helper missing or not executable'
entitlements="$(dirname "$0")/deno-jit-entitlements.plist"; [[ -f $entitlements ]] || fail 'Deno entitlements missing'
work=$(mktemp -d "${TMPDIR:-/tmp}/muninn-sign.XXXXXXXX"); keychain=$work/signing.keychain-db; profile=muninn-notary-$$
orig_keys=(); created=''
restore(){ ((${#orig_keys[@]})) && security list-keychains -d user -s "${orig_keys[@]}" >/dev/null 2>&1 || true; }
cleanup(){ rc=$?; trap - EXIT HUP INT TERM; [[ -n $created ]] && { restore; security delete-keychain "$keychain" >/dev/null 2>&1 || true; }; rm -rf "$work"; return $rc; }
trap cleanup EXIT; trap 'exit 129' HUP; trap 'exit 130' INT; trap 'exit 143' TERM
keys_output=$(security list-keychains -d user 2>/dev/null) || fail 'could not read keychain search list'
while IFS= read -r line; do line=${line#\"}; line=${line%\"}; [[ -n $line ]] && orig_keys+=("$line"); done <<<"$keys_output"
[[ ${#orig_keys[@]} -gt 0 ]] || fail 'could not read keychain search list'
printf %s "$APPLE_DEVELOPER_ID_APPLICATION_P12_BASE64" | /usr/bin/base64 -D >"$work/cert.p12" || fail 'certificate decode failed'
printf %s "$APPLE_NOTARY_API_PRIVATE_KEY" >"$work/AuthKey.p8"; chmod 600 "$work/cert.p12" "$work/AuthKey.p8"
kp=$(/usr/bin/openssl rand -hex 24) || fail 'keychain password generation failed'
security create-keychain -p "$kp" "$keychain" >/dev/null 2>&1 || fail 'keychain creation failed'; created=1
security list-keychains -d user -s "$keychain" >/dev/null 2>&1 || fail 'keychain search-list setup failed'
security set-keychain-settings -lut 21600 "$keychain" >/dev/null 2>&1 || fail 'keychain settings failed'
security unlock-keychain -p "$kp" "$keychain" >/dev/null 2>&1 || fail 'keychain unlock failed'
security import "$work/cert.p12" -k "$keychain" -P "$APPLE_DEVELOPER_ID_APPLICATION_P12_PASSWORD" -T /usr/bin/codesign >/dev/null 2>&1 || fail 'certificate import failed'
security set-key-partition-list -S apple-tool:,apple: -s -k "$kp" "$keychain" >/dev/null 2>&1 || fail 'keychain access configuration failed'
ids=$(security find-identity -v -p codesigning "$keychain" 2>/dev/null) || fail 'identity enumeration failed'
identity=$(printf '%s\n' "$ids" | /usr/bin/python3 -c 'import re,sys; t=sys.argv[1]; a=[m.group(1) for l in sys.stdin if (m:=re.search(r"\b([0-9A-F]{40})\b.*Developer ID Application:.*\(([A-Z0-9]{10})\)",l)) and m.group(2)==t]; print(a[0] if len(a)==1 else "")' "$APPLE_TEAM_ID") || fail 'identity parsing failed'
[[ -n $identity ]] || fail 'expected exactly one Developer ID Application identity with matching Team ID'
xcrun notarytool store-credentials "$profile" --key "$work/AuthKey.p8" --key-id "$APPLE_NOTARY_API_KEY_ID" --issuer "$APPLE_NOTARY_API_ISSUER_ID" --keychain "$keychain" >/dev/null 2>&1 || fail 'notary credential setup failed'
while IFS= read -r -d '' f; do if /usr/bin/file -b "$f" | grep -q Mach-O; then if [[ $f == "$app/Contents/Resources/bin/deno" ]]; then codesign --force --options runtime --timestamp --entitlements "$entitlements" --sign "$identity" "$f" >/dev/null 2>&1 || fail 'Deno signing failed'; else codesign --force --options runtime --timestamp --sign "$identity" "$f" >/dev/null 2>&1 || fail "nested signing failed: ${f#"$app"/}"; fi; fi; done < <(python3 -c 'import os,sys; r=sys.argv[1]; p=[os.path.join(b,n) for b,_,ns in os.walk(r) for n in ns]; [sys.stdout.buffer.write(os.fsencode(x)+b"\0") for x in sorted(p,key=lambda x:(-x.count(os.sep),x))]' "$app/Contents")
codesign --force --options runtime --timestamp --sign "$identity" "$app" >/dev/null 2>&1 || fail 'app signing failed'
codesign --verify --strict --verbose=2 "$app" >/dev/null 2>&1 || fail 'signature verification failed'
d=$(codesign --display --verbose=4 "$app" 2>&1) || fail 'signature details unavailable'
t=$(printf '%s\n' "$d" | awk -F= '$1=="TeamIdentifier"{print $2;n++} END{if(n!=1)exit 1}') || fail 'signature has no unique TeamIdentifier'
[[ $t == "$APPLE_TEAM_ID" ]] || fail 'signature TeamIdentifier mismatch or ad hoc signature'
a=$(printf '%s\n' "$d" | awk -F= '$1=="Authority"{print $2;exit}')
[[ $a == "Developer ID Application: "* ]] || fail 'signature authority is not Developer ID Application'
ditto -c -k --sequesterRsrc --keepParent "$app" "$work/Muninn.zip" >/dev/null 2>&1 || fail 'archive creation failed'
s=$(xcrun notarytool submit "$work/Muninn.zip" --keychain-profile "$profile" --wait --output-format json 2>/dev/null) || fail 'notary submission failed'
status=$(printf %s "$s" | /usr/bin/python3 -c 'import json,sys;print(json.load(sys.stdin).get("status", ""))') || fail 'notary response parse failed'
[[ $status == Accepted ]] || fail "notarization rejected (status: ${status:-unknown})"
codesign --verify --strict "$app" >/dev/null 2>&1 || fail 'post-notary signature verification failed'
xcrun stapler staple "$app" >/dev/null 2>&1 || fail 'ticket stapling failed'; xcrun stapler validate "$app" >/dev/null 2>&1 || fail 'staple validation failed'
assessment=$(spctl --assess --type execute --verbose=2 "$app" 2>&1) || fail 'Gatekeeper assessment failed'
printf '%s\n' "$assessment" | sed -E 's/(origin|source)=.*/\1=[redacted]/' >&2
printf 'PASS: signature verified for TeamIdentifier %s; notarization Accepted; ticket stapled and Gatekeeper assessment passed.\n' "$APPLE_TEAM_ID"
