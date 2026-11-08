#!/usr/bin/env python3
"""Publish-time identity for accepted native artifacts; never rebuild or modify them."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess


def preview_version(version, sha):
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?", version):
        raise ValueError("Invalid application version")
    if not re.fullmatch(r"[a-f0-9]{40}", sha):
        raise ValueError("Expected a full accepted commit SHA")
    # A display/artifact identity, not a new updater version or bundle version.
    return f"{version.split('+', 1)[0]}-preview.{sha[:12]}"


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def prepare(root, sha, run_id):
    root = Path(root)
    artifacts = root / "artifacts"
    source = json.loads((artifacts / "release-input.json").read_text())
    version = json.loads((root / "src-tauri/tauri.conf.json").read_text())["version"]
    identity = preview_version(version, sha)
    tree = subprocess.check_output(["git", "-C", str(root), "rev-parse", "HEAD^{tree}"], text=True).strip()
    if (source.get("accepted_commit") != sha or source.get("accepted_tree") != tree
            or source.get("version") != version or source.get("source_run_id") != run_id
            or source.get("acceptance") != "trusted-main-build"):
        raise ValueError("Candidate no longer identifies the accepted merge")
    stem = f"Chat_Hub_{version}_macos-arm64"
    wanted = {stem + ".zip", stem + ".zip.sha256", stem + ".dmg"}
    if set(source.get("files", {})) != wanted:
        raise ValueError("Expected accepted native ZIP, checksum and DMG")
    for name in wanted:
        path = artifacts / name
        if path.is_symlink() or not path.is_file() or path.stat().st_size > 512 * 1024 * 1024:
            raise ValueError("Invalid accepted artifact")
        if digest(path) != source["files"][name]:
            raise ValueError("Unverified native artifact: " + name)
    hashes = {}
    for extension in ("zip", "dmg"):
        original = artifacts / f"{stem}.{extension}"
        name = f"Chat_Hub_{identity}_macos-arm64.{extension}"
        destination = artifacts / name
        if destination.is_symlink():
            raise ValueError("Preview artifact must not be a symlink")
        shutil.copyfile(original, destination)
        hashes[name] = digest(destination)
        if hashes[name] != source["files"][original.name]:
            raise ValueError("Preview copy differs from accepted artifact")
    info = {
        "schema": 2, "kind": "ad-hoc-signed-merge-preview", "channel": "merge-preview",
        "repository": source["repository"], "commit": sha, "tree": tree,
        "tag": "merge-" + sha, "app_version": version, "display_version": identity,
        "native_run_id": run_id, "native_artifact_digest": source["source_artifact_digest"],
        "files": hashes,
    }
    (artifacts / "SHA256SUMS").write_text("".join(hashes[n] + "  " + n + "\n" for n in sorted(hashes)))
    (artifacts / "build-info.json").write_text(json.dumps(info, sort_keys=True) + "\n")
    (artifacts / "merge-notes.md").write_text(
        f"## Preview {identity}\n\nAccepted commit: {sha}\n"
        f"Embedded app version: {version} (base version, not a new stable update).\n"
        f"Validated native build: https://github.com/{source['repository']}/actions/runs/{run_id}\n\n"
        "The ZIP and DMG filenames include this preview's commit identity. Their bytes are "
        "exactly those from native acceptance; there is no second compilation. "
        "The app's Software section also shows the source commit/tree of the native build. "
        "That source commit can precede the merge when acceptance reuses an identical tree.\n\n"
        "Apple Silicon: ad-hoc signed and not notarized; Gatekeeper may block it. "
        "This published prerelease is not a draft. It does not replace the latest stable "
        "release or modify the signed updater feed.\n"
    )
    return info


def verify_existing(root, release, sha):
    artifacts = Path(root) / "artifacts"
    info = json.loads((artifacts / "build-info.json").read_text())
    names = set(info["files"]) | {"SHA256SUMS", "build-info.json"}
    assets = release.get("assets", [])
    if (info["commit"] != sha or release.get("tag_name") != "merge-" + sha
            or release.get("draft") is not False or release.get("prerelease") is not True
            or len(assets) != len(names) or {a.get("name") for a in assets} != names):
        raise ValueError("Existing release is not this validated preview; refusing overwrite")
    for asset in assets:
        path = artifacts / asset["name"]
        if (asset.get("size") != path.stat().st_size
                or asset.get("digest") != "sha256:" + digest(path)):
            raise ValueError("Existing asset identity cannot be verified; refusing overwrite")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path.cwd())
    parser.add_argument("--sha", required=True)
    parser.add_argument("--run-id", type=int)
    parser.add_argument("--verify-existing", type=Path)
    args = parser.parse_args()
    if args.verify_existing:
        verify_existing(args.root, json.loads(args.verify_existing.read_text()), args.sha)
    elif args.run_id and args.run_id > 0:
        prepare(args.root, args.sha, args.run_id)
    else:
        parser.error("--run-id must be positive when preparing a preview")


if __name__ == "__main__":
    main()
