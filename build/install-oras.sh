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

rm -f "$destination/$archive"
gh release download v1.2.3 --repo oras-project/oras --pattern "$archive" --dir "$destination"
(cd "$destination" && printf '%s  %s\n' "$expected" "$archive" | shasum -a 256 -c >&2)
tar -xzf "$destination/$archive" -C "$destination" oras
chmod 0755 "$destination/oras"
echo "$destination/oras"
