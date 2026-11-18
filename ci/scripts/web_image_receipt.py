#!/usr/bin/env python3
"""Create a source/run-bound receipt for a published Chat Hub web image."""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import re
import sys


SHA = re.compile(r"[a-f0-9]{40}\Z")
IMAGE = re.compile(r"ghcr\.io/iarnvidr/chat-hub-web(?::[A-Za-z0-9_.-]+)?@sha256:[a-f0-9]{64}\Z")


def create(args: argparse.Namespace) -> dict:
    if not SHA.fullmatch(args.source_sha):
        raise ValueError("source SHA must be a full lowercase Git commit")
    if not IMAGE.fullmatch(args.image_ref):
        raise ValueError("image must be the Chat Hub web GHCR image addressed by digest")
    if args.run_id < 1 or args.run_attempt < 1 or args.publisher_run_id < 1:
        raise ValueError("workflow run identifiers must be positive")
    if args.publisher_run_attempt < 1:
        raise ValueError("publisher workflow run attempt must be positive")
    return {
        "schema": "chat-hub-web-image-receipt/v1",
        "source": {
            "repository": "iarnvidr/chat-hub",
            "commit": args.source_sha,
            "workflow": ".github/workflows/check.yml",
            "run_id": args.run_id,
            "run_attempt": args.run_attempt,
        },
        "publication": {
            "workflow": ".github/workflows/publish-web-image.yml",
            "run_id": args.publisher_run_id,
            "run_attempt": args.publisher_run_attempt,
            "image_ref": args.image_ref,
            "health_endpoint": "/healthz",
        },
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--source-sha", required=True)
    parser.add_argument("--run-id", type=int, required=True)
    parser.add_argument("--run-attempt", type=int, required=True)
    parser.add_argument("--publisher-run-id", type=int, required=True)
    parser.add_argument("--publisher-run-attempt", type=int, required=True)
    parser.add_argument("--image-ref", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    try:
        receipt = create(args)
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(receipt, sort_keys=True, indent=2) + "\n", encoding="utf-8")
    except (OSError, ValueError) as exc:
        print(f"web image receipt: {exc}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
