#!/usr/bin/env python3
"""Validate and assemble Tauri v2 updater release metadata/artifacts."""
import argparse, json, re, sys
from pathlib import Path
from urllib.parse import urlparse

SEMVER = re.compile(r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$")

def stable(version):
    return bool(SEMVER.fullmatch(version))

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
        if not parsed.path.endswith("/Chat Hub.app.tar.gz"):
            raise ValueError("platform URL must identify Chat Hub.app.tar.gz")
        if not isinstance(sig, str) or not sig.strip() or sig.strip().lower() in {"placeholder", "example", "signature"}:
            raise ValueError("platform requires a real detached signature")

def verify_source_version(bundle_version, requested_version):
    if not stable(requested_version) or bundle_version != requested_version:
        raise ValueError("actual app bundle version must equal requested stable version")

def make_feed(version, url, signature):
    if not stable(version): raise ValueError("release version must be stable SemVer without preview/prerelease/build metadata")
    if not isinstance(url, str) or not isinstance(signature, str): raise ValueError("HTTPS URL and signature required")
    feed={"version":version,"notes":f"Chat Hub {version}","pub_date":None,"platforms":{"darwin-aarch64":{"url":url,"signature":signature}}}
    validate_feed(feed); return feed

def main():
    p=argparse.ArgumentParser(); sub=p.add_subparsers(dest="cmd",required=True)
    v=sub.add_parser("validate"); v.add_argument("feed",type=Path)
    c=sub.add_parser("check-version"); c.add_argument("bundle_version"); c.add_argument("requested_version")
    g=sub.add_parser("generate"); g.add_argument("version"); g.add_argument("url"); g.add_argument("signature"); g.add_argument("output",type=Path)
    a=p.parse_args()
    if a.cmd=="validate": validate_feed(json.loads(a.feed.read_text()))
    elif a.cmd=="check-version": verify_source_version(a.bundle_version, a.requested_version)
    else:
        feed=make_feed(a.version,a.url,a.signature)
        a.output.parent.mkdir(parents=True, exist_ok=True)
        a.output.write_text(json.dumps(feed,indent=2)+"\n")
if __name__=="__main__":
    try: main()
    except (ValueError, OSError, json.JSONDecodeError) as e: print(e,file=sys.stderr); sys.exit(1)
