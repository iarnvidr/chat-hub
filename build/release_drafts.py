#!/usr/bin/env python3
"""Fail closed before deleting an explicitly selected, obsolete empty release draft."""
import argparse
import json
import re


def stable_version(tag):
    match = re.fullmatch(r"v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", str(tag))
    return tuple(map(int, match.groups())) if match else None


def validate_cleanup(draft, latest, release_id, tag):
    current, old = stable_version(latest.get("tag_name")), stable_version(tag)
    if (not current or not old or old >= current or latest.get("draft") is not False
            or latest.get("prerelease") is not False):
        raise ValueError("Draft must be older than a published stable release")
    if (draft.get("id") != release_id or draft.get("tag_name") != tag
            or draft.get("draft") is not True or draft.get("assets") != []):
        raise ValueError("Refusing to delete a published, mismatched or asset-bearing release")
    return release_id


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--draft", required=True)
    parser.add_argument("--latest", required=True)
    parser.add_argument("--release-id", required=True, type=int)
    parser.add_argument("--tag", required=True)
    args = parser.parse_args()
    with open(args.draft) as stream:
        draft = json.load(stream)
    with open(args.latest) as stream:
        latest = json.load(stream)
    print(validate_cleanup(draft, latest, args.release_id, args.tag))


if __name__ == "__main__":
    main()
