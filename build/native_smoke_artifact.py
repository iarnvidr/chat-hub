#!/usr/bin/env python3
"""Resolve native-smoke binary from Cargo metadata, never assumed workspace layout."""
import json
import subprocess
import sys


def artifact_path(metadata):
    packages = [p for p in metadata["packages"] if p["name"] == "chat-hub-desktop"]
    if len(packages) != 1:
        raise ValueError("expected exactly one chat-hub-desktop package")
    binaries = [t for t in packages[0]["targets"] if "bin" in t["kind"]]
    if len(binaries) != 1:
        raise ValueError("expected exactly one chat-hub-desktop binary target")
    return f'{metadata["target_directory"]}/aarch64-apple-darwin/debug/{binaries[0]["name"]}'


def main():
    result = subprocess.run(
        ["cargo", "metadata", "--locked", "--manifest-path", sys.argv[1], "--no-deps", "--format-version", "1"],
        check=True, capture_output=True, text=True,
    )
    print(artifact_path(json.loads(result.stdout)))


if __name__ == "__main__":
    main()
