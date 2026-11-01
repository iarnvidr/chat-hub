#!/usr/bin/env python3
"""Safely diagnose repeated installs of the pinned ORAS release binary."""

from __future__ import annotations

import os
import re
import subprocess
import sys
from pathlib import Path


def safe_stderr(value: str, secret: str) -> str:
    value = value.replace(secret, "[redacted]") if secret else value
    value = re.sub(r"(?i)bearer\s+\S+", "Bearer [redacted]", value)
    value = re.sub(r"(?i)(token|access_token|authorization)=([^\s&]+)", r"\1=[redacted]", value)
    lines = [line.strip() for line in value.splitlines() if line.strip()]
    return " | ".join(lines[-4:])[:1200] or "(no stderr)"


def install(script: Path, destination: Path, label: str, secret: str) -> bool:
    result = subprocess.run(
        ["bash", str(script), str(destination)],
        check=False,
        capture_output=True,
        text=True,
        env=os.environ.copy(),
    )
    binary = destination / "oras"
    valid_path = result.returncode == 0 and result.stdout.strip() == str(binary)
    if valid_path and binary.is_file():
        version = subprocess.run(
            [str(binary), "version"], check=False, capture_output=True, text=True
        )
        valid_path = version.returncode == 0 and "1.2.3" in version.stdout
        version_summary = safe_stderr(version.stdout, secret) if version.returncode else version.stdout.strip()[:120]
    else:
        version_summary = "not run"
    print(
        f"{label}: installer_exit={result.returncode}; "
        f"path_verified={str(valid_path).lower()}; version={version_summary}"
    )
    if result.returncode:
        print(f"{label} stderr (sanitized): {safe_stderr(result.stderr, secret)}")
    elif not valid_path:
        print(f"{label}: installer output or pinned version did not match")
    return valid_path


def main() -> int:
    if len(sys.argv) != 3:
        print("usage: diagnose-oras-install.py INSTALL_SCRIPT UNIQUE_DESTINATION", file=sys.stderr)
        return 2
    script = Path(sys.argv[1])
    unique = Path(sys.argv[2])
    if not script.is_file() or unique == Path("/"):
        print("diagnostic inputs are invalid", file=sys.stderr)
        return 2
    secret = os.environ.get("GH_TOKEN", "")
    # The normal package flow installs at RUNNER_TEMP/oras. Repeat the exact
    # pinned installer there, then at the cache action's distinct destination.
    shared = unique.parent / "oras"
    first_ok = install(script, shared, "shared-destination", secret)
    repeated_ok = install(script, shared, "shared-repeat", secret)
    unique_ok = install(script, unique, "cache-destination", secret)
    print(
        "summary: "
        f"shared_first={str(first_ok).lower()} "
        f"shared_repeat={str(repeated_ok).lower()} "
        f"cache_destination={str(unique_ok).lower()}"
    )
    return 0 if first_ok and repeated_ok and unique_ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
