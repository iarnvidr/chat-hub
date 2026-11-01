#!/usr/bin/env python3
"""Bound the version history of Chat Hub's dedicated Rust compiler cache package."""

from __future__ import annotations

import datetime as dt
import json
import os
import re
import urllib.error
import urllib.parse
import urllib.request


API = "https://api.github.com"
API_VERSION = "2022-11-28"
PAGE_SIZE = 100
MAX_PAGES = 100
MAX_RESPONSE_BYTES = 8 * 1024 * 1024
MAX_VERSIONS = 6
PREPUBLISH_LIMIT = MAX_VERSIONS - 1
TAG_PREFIX = "cargo-sccache-v1-"


class RetentionError(Exception):
    """A safe cache publication could not be guaranteed."""


def repository_parts(repository: str) -> tuple[str, str]:
    if not re.fullmatch(r"[A-Za-z0-9_.-]{1,100}/[A-Za-z0-9_.-]{1,100}", repository):
        raise RetentionError("invalid repository identifier")
    owner, name = repository.split("/", 1)
    return owner.lower(), name.lower()


def package_name(repository: str) -> str:
    _, name = repository_parts(repository)
    return f"chat-hub-rust-cache-{name}"


def platform_prefix(platform: str) -> str:
    if platform not in {"linux", "macos"}:
        raise RetentionError("unknown cache platform")
    return f"{TAG_PREFIX}{platform}-"


def parse_timestamp(value: object) -> dt.datetime:
    if not isinstance(value, str):
        raise RetentionError("package version timestamp is malformed")
    try:
        parsed = dt.datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError as exc:
        raise RetentionError("package version timestamp is malformed") from exc
    if parsed.tzinfo is None:
        raise RetentionError("package version timestamp lacks timezone")
    return parsed


def validate_version(value: object) -> dict[str, object]:
    if not isinstance(value, dict):
        raise RetentionError("package version record is malformed")
    version_id = value.get("id")
    if isinstance(version_id, bool) or not isinstance(version_id, int) or version_id <= 0:
        raise RetentionError("package version ID is malformed")
    created = parse_timestamp(value.get("created_at"))
    metadata = value.get("metadata")
    if not isinstance(metadata, dict):
        raise RetentionError("package version metadata is malformed")
    container = metadata.get("container")
    if not isinstance(container, dict):
        raise RetentionError("package version tags are malformed")
    tags = container.get("tags")
    if not isinstance(tags, list) or any(not isinstance(tag, str) for tag in tags):
        raise RetentionError("package version tags are malformed")
    return {"id": version_id, "created_at": created, "tags": tuple(tags)}


def select_versions_to_keep(versions: list[dict[str, object]], limit: int = PREPUBLISH_LIMIT) -> set[int]:
    """Keep newest current OS tags plus recent fallback versions, bounded by limit."""
    if limit < 1:
        raise RetentionError("invalid retention limit")
    ordered = sorted(versions, key=lambda item: (item["created_at"], item["id"]), reverse=True)
    keep: set[int] = set()
    for platform in ("linux", "macos"):
        prefix = platform_prefix(platform)
        tagged = next((item for item in ordered if any(tag.startswith(prefix) for tag in item["tags"])), None)
        if tagged is not None:
            keep.add(int(tagged["id"]))

    for item in ordered:
        version_id = int(item["id"])
        if version_id in keep:
            continue
        if len(keep) >= limit:
            break
        keep.add(version_id)
    if len(keep) > limit:
        keep = set(sorted(keep, key=lambda item: next(
            (version["created_at"], version["id"]) for version in ordered if version["id"] == item
        ), reverse=True)[:limit])
    return keep


def request_json(method: str, url: str, token: str) -> tuple[int, object | None]:
    request = urllib.request.Request(url, method=method, headers={
        "Accept": "application/vnd.github+json",
        "Authorization": f"Bearer {token}",
        "X-GitHub-Api-Version": API_VERSION,
        "User-Agent": "chat-hub-cache-retention",
    })
    try:
        with urllib.request.urlopen(request, timeout=20) as response:
            status = response.status
            body = response.read(MAX_RESPONSE_BYTES + 1)
    except urllib.error.HTTPError as exc:
        status = exc.code
        if status == 204:
            return status, None
        body = exc.read(MAX_RESPONSE_BYTES + 1)
    except (OSError, urllib.error.URLError, TimeoutError) as exc:
        raise RetentionError("GitHub package retention request failed") from exc
    if len(body) > MAX_RESPONSE_BYTES:
        raise RetentionError("GitHub package response exceeds limit")
    if status == 204 or not body:
        return status, None
    try:
        return status, json.loads(body)
    except (json.JSONDecodeError, UnicodeDecodeError) as exc:
        raise RetentionError("GitHub package response is malformed") from exc


def package_api_base(repository: str, token: str) -> tuple[str, str, str]:
    owner, repo = repository_parts(repository)
    repo_url = f"{API}/repos/{urllib.parse.quote(owner)}/{urllib.parse.quote(repo)}"
    status, payload = request_json("GET", repo_url, token)
    if status != 200 or not isinstance(payload, dict):
        raise RetentionError("repository owner could not be verified")
    owner_data = payload.get("owner")
    full_name = payload.get("full_name")
    if (not isinstance(owner_data, dict) or not isinstance(owner_data.get("login"), str) or
            owner_data["login"].lower() != owner or not isinstance(full_name, str) or
            full_name.lower() != f"{owner}/{repo}"):
        raise RetentionError("repository identity response did not match")
    owner_type = owner_data.get("type")
    if owner_type == "User":
        owner_route = "users"
    elif owner_type == "Organization":
        owner_route = "orgs"
    else:
        raise RetentionError("repository owner type is unsupported")
    package = package_name(repository)
    encoded_owner = urllib.parse.quote(owner, safe="")
    encoded_package = urllib.parse.quote(package, safe="")
    base = f"{API}/{owner_route}/{encoded_owner}/packages/container/{encoded_package}/versions"
    return base, owner, package


def list_versions(base: str, token: str) -> list[dict[str, object]]:
    records: list[dict[str, object]] = []
    for page in range(1, MAX_PAGES + 1):
        url = f"{base}?per_page={PAGE_SIZE}&page={page}"
        status, payload = request_json("GET", url, token)
        if status == 404 and page == 1:
            return []
        if status != 200 or not isinstance(payload, list):
            raise RetentionError("package versions could not be listed completely")
        records.extend(validate_version(row) for row in payload)
        if len(records) > PAGE_SIZE * MAX_PAGES:
            raise RetentionError("package version history exceeds retention scan limit")
        if len(payload) < PAGE_SIZE:
            return records
    raise RetentionError("package version pagination limit reached")


def preflight_publish(repository: str, token: str | None = None) -> dict[str, int]:
    """Prune before push, leaving room for one new version under the hard cap."""
    token = token or os.environ.get("GH_TOKEN", "")
    if not token:
        raise RetentionError("package retention credentials are unavailable")
    base, _owner, _package = package_api_base(repository, token)
    versions = list_versions(base, token)
    if len(versions) <= PREPUBLISH_LIMIT:
        return {"before": len(versions), "deleted": 0, "remaining": len(versions)}

    keep = select_versions_to_keep(versions, PREPUBLISH_LIMIT)
    deleted = delete_unkept(base, versions, keep, token)
    remaining = len(versions) - deleted
    if remaining > PREPUBLISH_LIMIT:
        raise RetentionError("cache version limit could not be established")
    return {"before": len(versions), "deleted": deleted, "remaining": remaining}


def delete_unkept(base: str, versions: list[dict[str, object]], keep: set[int], token: str) -> int:
    delete_ids = [int(item["id"]) for item in sorted(
        (version for version in versions if int(version["id"]) not in keep),
        key=lambda item: (item["created_at"], item["id"]))]
    deleted = 0
    for version_id in delete_ids:
        status, _ = request_json("DELETE", f"{base}/{version_id}", token)
        if status != 204:
            raise RetentionError("old cache version pruning was not confirmed")
        deleted += 1
    return deleted


def prune_after_publish(repository: str, token: str | None = None) -> dict[str, int]:
    """Trim concurrent publisher overage to the hard six-version target."""
    token = token or os.environ.get("GH_TOKEN", "")
    if not token:
        raise RetentionError("package retention credentials are unavailable")
    base, _owner, _package = package_api_base(repository, token)
    for _attempt in range(3):
        versions = list_versions(base, token)
        if len(versions) <= MAX_VERSIONS:
            return {"before": len(versions), "deleted": 0, "remaining": len(versions)}
        keep = select_versions_to_keep(versions, MAX_VERSIONS)
        try:
            deleted = delete_unkept(base, versions, keep, token)
        except RetentionError:
            # Another serialized or overlapping publisher may already have
            # removed the same old version. Re-list before deciding it failed.
            continue
        remaining = len(versions) - deleted
        if remaining <= MAX_VERSIONS:
            return {"before": len(versions), "deleted": deleted, "remaining": remaining}
    raise RetentionError("post-publish package cleanup could not be confirmed")
