#!/usr/bin/env bash
set -euo pipefail

destination="${1:-${RUNNER_TEMP:-/tmp}/oras}"
mkdir -p "$destination"
case "$(uname -s)/$(uname -m)" in
  Darwin/arm64)
    archive=oras_1.2.3_darwin_arm64.tar.gz
    expected=ca4188a0c690536d34baf2a4c3507b3fcc11a06979aeec6da3560a0c4d944a6a
    ;;
  Linux/x86_64)
    archive=oras_1.2.3_linux_amd64.tar.gz
    expected=b4efc97a91f471f323f193ea4b4d63d8ff443ca3aab514151a30751330852827
    ;;
  *)
    echo "Unsupported ORAS host: $(uname -s)/$(uname -m)" >&2
    exit 2
    ;;
esac

binary="$destination/oras"
binary_digest_file="$destination/.oras.sha256"
if [[ -x "$binary" && -f "$binary_digest_file" ]]; then
  IFS= read -r recorded_digest < "$binary_digest_file" || recorded_digest=
  if [[ "$recorded_digest" =~ ^[a-f0-9]{64}$ ]]; then
    actual_digest="$(shasum -a 256 "$binary" | awk '{print $1}')"
    if [[ "$actual_digest" == "$recorded_digest" ]] && \
       "$binary" version 2>/dev/null | grep -Eq '^Version:[[:space:]]+1\.2\.3([[:space:]]|$)'; then
      echo "$binary"
      exit 0
    fi
  fi
fi

rm -f "$destination/$archive"
gh release download v1.2.3 --repo oras-project/oras --pattern "$archive" --dir "$destination"
(cd "$destination" && printf '%s  %s\n' "$expected" "$archive" | shasum -a 256 -c >&2)
tar -xzf "$destination/$archive" -C "$destination" oras
chmod 0755 "$binary"
binary_digest="$(shasum -a 256 "$binary" | awk '{print $1}')"
[[ "$binary_digest" =~ ^[a-f0-9]{64}$ ]]
printf '%s\n' "$binary_digest" > "$binary_digest_file"
chmod 0600 "$binary_digest_file"
"$binary" version 2>/dev/null | grep -Eq '^Version:[[:space:]]+1\.2\.3([[:space:]]|$)'
echo "$binary"
