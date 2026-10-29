#!/usr/bin/env python3
"""Validate and assemble Tauri v2 updater release metadata/artifacts."""
import argparse, base64, binascii, json, re, sys
from pathlib import Path
from urllib.parse import urlparse

MINISIGN_COMMENT = b"untrusted comment: minisign public key: "
MINISIGN_ALGORITHM = b"\x45\x64"


def parse_public_key(value):
    """Parse a plaintext minisign key or Tauri's base64-encoded key file."""
    if isinstance(value, str):
        value = value.encode("ascii")
    if not isinstance(value, bytes):
        raise ValueError("public key must be text or bytes")
    value = value.strip()
    if not value:
        raise ValueError("public key is empty")
    if not value.startswith(b"untrusted comment:"):
        try:
            value = base64.b64decode(value, validate=True)
        except (binascii.Error, ValueError) as exc:
            raise ValueError("public key must be plaintext minisign or strict base64") from exc
    lines = value.splitlines()
    if len(lines) != 2 or not lines[0].startswith(MINISIGN_COMMENT):
        raise ValueError("malformed minisign public key")
    try:
        comment_key_id = bytes.fromhex(lines[0][len(MINISIGN_COMMENT):].decode("ascii"))
        payload = base64.b64decode(lines[1], validate=True)
    except (UnicodeDecodeError, ValueError, binascii.Error) as exc:
        raise ValueError("malformed minisign public key encoding") from exc
    if len(comment_key_id) != 8 or len(payload) != 42:
        raise ValueError("minisign public key must contain an 8-byte key ID and 42-byte key")
    if comment_key_id != payload[2:10][::-1]:
        raise ValueError("minisign public-key comment ID does not match payload key ID")
    if payload[:2] != MINISIGN_ALGORITHM:
        raise ValueError("unsupported minisign public-key algorithm")
    return payload


def minisign_public_key(value):
    payload = parse_public_key(value)
    return MINISIGN_COMMENT + payload[2:10][::-1].hex().upper().encode("ascii") + b"\n" + base64.b64encode(payload) + b"\n"

SEMVER = re.compile(r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$")

def stable(version):
    return bool(SEMVER.fullmatch(version))

UPDATER_ARCHIVE = "Chat-Hub.app.tar.gz"


def updater_url(repo, tag, asset=UPDATER_ARCHIVE):
    if not isinstance(repo, str) or not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repo):
        raise ValueError("repository must be owner/name")
    if not isinstance(tag, str) or not re.fullmatch(r"v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)", tag):
        raise ValueError("tag must be v-prefixed stable SemVer")
    if asset != UPDATER_ARCHIVE or " " in asset:
        raise ValueError("updater asset name must be URL-safe")
    return f"https://github.com/{repo}/releases/download/{tag}/{asset}"


def validate_feed(feed):
    if not isinstance(feed, dict) or not stable(str(feed.get("version", ""))):
        raise ValueError("feed version must be stable SemVer")
    if not isinstance(feed.get("notes"), str) or not feed["notes"]:
        raise ValueError("feed notes must be non-empty text")
    if feed.get("pub_date") is not None and not isinstance(feed["pub_date"], str):
        raise ValueError("pub_date must be null or text")
    if not isinstance(feed.get("platforms"), dict) or set(feed["platforms"]) != {"darwin-aarch64"}:
        raise ValueError("feed must contain only darwin-aarch64")
    for platform, item in feed["platforms"].items():
        if not isinstance(platform, str) or not isinstance(item, dict): raise ValueError("invalid platform entry")
        url, sig = item.get("url"), item.get("signature")
        parsed = urlparse(url) if isinstance(url, str) else None
        if not parsed or parsed.scheme != "https" or not parsed.netloc or parsed.username or parsed.password:
            raise ValueError("platform requires an absolute HTTPS URL")
        version = str(feed["version"])
        repo_and_path = parsed.path.split("/releases/download/")
        if parsed.netloc != "github.com" or len(repo_and_path) != 2:
            raise ValueError("platform URL must be a GitHub release download URL")
        repo = repo_and_path[0].lstrip("/")
        if "%" in url:
            raise ValueError("platform URL must not percent-encode release asset names")
        expected = updater_url(repo, f"v{version}")
        if url != expected:
            raise ValueError(f"platform URL must exactly match {expected}")
        if not isinstance(sig, str) or not sig.strip() or sig.strip().lower() in {"placeholder", "example", "signature"}:
            raise ValueError("platform requires a real detached signature")

def verify_source_version(bundle_version, requested_version):
    if not stable(requested_version) or bundle_version != requested_version:
        raise ValueError("actual app bundle version must equal requested stable version")

def make_feed(version, url, signature, repo=None, tag=None):
    if not stable(version): raise ValueError("release version must be stable SemVer without preview/prerelease/build metadata")
    if not isinstance(url, str) or not isinstance(signature, str): raise ValueError("HTTPS URL and signature required")
    if repo is not None or tag is not None:
        expected = updater_url(repo, tag)
        if url != expected:
            raise ValueError(f"updater URL must exactly match {expected}")
    feed={"version":version,"notes":f"Chat Hub {version}","pub_date":None,"platforms":{"darwin-aarch64":{"url":url,"signature":signature}}}
    validate_feed(feed); return feed

def main():
    p=argparse.ArgumentParser(); sub=p.add_subparsers(dest="cmd",required=True)
    v=sub.add_parser("validate"); v.add_argument("feed",type=Path)
    c=sub.add_parser("check-version"); c.add_argument("bundle_version"); c.add_argument("requested_version")
    g=sub.add_parser("generate"); g.add_argument("version"); g.add_argument("url"); g.add_argument("signature"); g.add_argument("output",type=Path); g.add_argument("--repo",required=True); g.add_argument("--tag",required=True)
    a=p.parse_args()
    if a.cmd=="validate": validate_feed(json.loads(a.feed.read_text()))
    elif a.cmd=="check-version": verify_source_version(a.bundle_version, a.requested_version)
    else:
        feed=make_feed(a.version,a.url,a.signature,a.repo,a.tag)
        a.output.parent.mkdir(parents=True, exist_ok=True)
        a.output.write_text(json.dumps(feed,indent=2)+"\n")
if __name__=="__main__":
    try: main()
    except (ValueError, OSError, json.JSONDecodeError) as e: print(e,file=sys.stderr); sys.exit(1)
