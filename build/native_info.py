"""Assemble native bundle metadata while preserving existing installation identity."""
import json
import plistlib
import sys
from pathlib import Path


def metadata(config):
    info = {
        "CFBundleDevelopmentRegion": "en",
        "CFBundleExecutable": "chat-hub",
        "CFBundleIconFile": "icon",
        "CFBundleIdentifier": config["identifier"],
        "CFBundleInfoDictionaryVersion": "6.0",
        "CFBundleName": config["productName"],
        "CFBundleDisplayName": config["productName"],
        "CFBundlePackageType": "APPL",
        "CFBundleShortVersionString": config["version"],
        "CFBundleVersion": config["version"],
        "LSApplicationCategoryType": "public.app-category.productivity",
        "LSMinimumSystemVersion": config["bundle"]["macOS"]["minimumSystemVersion"],
        "NSHighResolutionCapable": True,
        "NSAppTransportSecurity": {"NSAllowsLocalNetworking": True},
    }
    # Both packaging paths and Tauri consume the same URL registration.
    extra = Path(__file__).resolve().parents[1] / "src-tauri" / config["bundle"]["macOS"]["infoPlist"]
    info.update(plistlib.loads(extra.read_bytes()))
    return info


if __name__ == "__main__":
    root = Path(__file__).resolve().parents[1]
    config = json.loads((root / "src-tauri/tauri.conf.json").read_text())
    contents = Path(sys.argv[1]) / "Contents"
    (contents / "Info.plist").write_bytes(plistlib.dumps(metadata(config)))
    (contents / "PkgInfo").write_bytes(b"APPL????")
