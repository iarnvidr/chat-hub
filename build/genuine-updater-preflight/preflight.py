#!/usr/bin/env python3
"""Read-only, PID-scoped AX readiness check for the released v0.1.1 app."""
import argparse
import hashlib
import http.client
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
from urllib.parse import urlsplit

APP_DATA_SUFFIX = Path("Library/Application Support/io.github.iarnvidr.chat-hub")
BUNDLE_ID = "io.github.iarnvidr.chat-hub"
SERVICE_FIELDS = {"service", "version", "api", "process_id"}
HEX_SHA256 = re.compile(r"[0-9a-fA-F]{64}\Z")


class PreflightBlocked(Exception):
    def __init__(self, reason):
        super().__init__(reason)
        self.reason = reason


def file_sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def owned_regular_file(path):
    if path.is_symlink() or not path.is_file():
        raise PreflightBlocked("unsafe_input_file")
    stat = path.stat()
    if stat.st_uid != os.getuid():
        raise PreflightBlocked("input_owner_mismatch")


def read_service(home):
    data_dir = Path(home) / APP_DATA_SUFFIX
    if data_dir.is_symlink() or not data_dir.is_dir() or data_dir.stat().st_uid != os.getuid():
        raise PreflightBlocked("default_data_directory_unverified")
    service_path = data_dir / "service.json"
    token_path = data_dir / "service.token"
    owned_regular_file(service_path)
    owned_regular_file(token_path)
    try:
        record = json.loads(service_path.read_text(encoding="utf-8"))
        token = token_path.read_text(encoding="ascii").strip()
    except (OSError, UnicodeError, ValueError):
        raise PreflightBlocked("service_record_unreadable") from None
    if type(record) is not dict or set(record) != SERVICE_FIELDS:
        raise PreflightBlocked("service_record_schema_mismatch")
    if record["service"] != "chat-hub" or record["version"] != "0.1.1":
        raise PreflightBlocked("baseline_service_version_mismatch")
    if type(record["process_id"]) is not int or record["process_id"] <= 0:
        raise PreflightBlocked("baseline_pid_invalid")
    if not re.fullmatch(r"[0-9a-fA-F]{64,}", token):
        raise PreflightBlocked("service_token_invalid")
    try:
        parsed = urlsplit(record["api"] if isinstance(record["api"], str) else "")
        port = parsed.port
    except ValueError:
        raise PreflightBlocked("baseline_api_endpoint_invalid") from None
    if (parsed.scheme != "http" or parsed.hostname != "127.0.0.1"
            or port is None or parsed.path not in ("", "/")
            or parsed.username is not None or parsed.password is not None
            or parsed.query or parsed.fragment):
        raise PreflightBlocked("baseline_api_endpoint_invalid")
    return data_dir, record, token, port


def check_owned_listener(pid, port, runner=subprocess.run):
    try:
        result = runner(["/usr/sbin/lsof", "-nP", "-a", "-p", str(pid), "-iTCP", "-sTCP:LISTEN"],
                        capture_output=True, text=True, timeout=5, check=False)
    except (OSError, subprocess.SubprocessError):
        raise PreflightBlocked("baseline_listener_discovery_failed") from None
    if result.returncode != 0:
        raise PreflightBlocked("baseline_listener_discovery_failed")
    expected = re.compile(rf"\bTCP\s+127\.0\.0\.1:{port}\s+\(LISTEN\)\s*\Z")
    matches = [line for line in result.stdout.splitlines() if expected.search(line)]
    if len(matches) != 1:
        raise PreflightBlocked("baseline_listener_identity_mismatch")


def check_health(port, token, connection_factory=http.client.HTTPConnection):
    try:
        connection = connection_factory("127.0.0.1", port, timeout=2)
        connection.request("GET", "/api/health", headers={"Authorization": f"Bearer {token}"})
        response = connection.getresponse()
        body = response.read(4096)
    except (OSError, http.client.HTTPException):
        raise PreflightBlocked("baseline_health_unavailable") from None
    finally:
        if "connection" in locals():
            connection.close()
    try:
        payload = json.loads(body)
    except (ValueError, UnicodeError):
        raise PreflightBlocked("baseline_health_contract_mismatch") from None
    if response.status != 200 or payload != {"status": "ok"}:
        raise PreflightBlocked("baseline_health_contract_mismatch")


def validate_ax_result(raw, expected_pid):
    allowed = {"status", "reason", "pid", "identity_matches", "trusted",
               "check_button_count", "check_button_enabled", "check_button_press_supported"}
    if type(raw) is not dict or set(raw) != allowed:
        raise PreflightBlocked("ax_helper_schema_mismatch")
    if type(raw["pid"]) is not int or raw["pid"] != expected_pid:
        raise PreflightBlocked("ax_helper_pid_mismatch")
    for field in ("identity_matches", "trusted", "check_button_enabled", "check_button_press_supported"):
        if type(raw[field]) is not bool:
            raise PreflightBlocked("ax_helper_schema_mismatch")
    if type(raw["check_button_count"]) is not int or raw["check_button_count"] < 0:
        raise PreflightBlocked("ax_helper_schema_mismatch")
    reason = raw["reason"]
    if reason is not None and reason not in {
        "invalid_arguments", "process_identity_unavailable", "process_identity_mismatch",
        "accessibility_trust_missing", "check_button_not_unique", "check_button_not_actionable",
        "ax_tree_limit",
    }:
        raise PreflightBlocked("ax_helper_schema_mismatch")
    if raw["status"] != "ready" or reason is not None:
        raise PreflightBlocked(reason or "ax_preflight_not_ready")
    if not (raw["identity_matches"] and raw["trusted"] and raw["check_button_count"] == 1
            and raw["check_button_enabled"] and raw["check_button_press_supported"]):
        raise PreflightBlocked("ax_preflight_not_ready")
    return raw


def preflight(args, *, platform=None, home=None, runner=subprocess.run,
              connection_factory=http.client.HTTPConnection, temp_factory=tempfile.TemporaryDirectory):
    if (platform or sys.platform) != "darwin":
        raise PreflightBlocked("macos_required")
    dmg = Path(args.dmg)
    app = Path(args.app)
    if not dmg.is_absolute() or not app.is_absolute():
        raise PreflightBlocked("absolute_paths_required")
    for path in (dmg,):
        owned_regular_file(path)
    if dmg.resolve() != dmg.absolute() or app.is_symlink() or not app.is_dir() or app.resolve() != app.absolute():
        raise PreflightBlocked("unsafe_app_bundle")
    if (app.stat().st_uid != os.getuid() or app.parent.stat().st_uid != os.getuid()
            or not os.access(app.parent, os.W_OK) or str(app).startswith("/Volumes/")):
        raise PreflightBlocked("app_copy_not_owned_writable")
    if not HEX_SHA256.fullmatch(args.expected_dmg_sha256) or not HEX_SHA256.fullmatch(args.expected_executable_sha256):
        raise PreflightBlocked("expected_digest_invalid")
    dmg_digest = file_sha256(dmg)
    if dmg_digest.lower() != args.expected_dmg_sha256.lower():
        raise PreflightBlocked("published_dmg_digest_mismatch")

    try:
        codesign = runner(["/usr/bin/codesign", "--verify", "--deep", "--strict", str(app)],
                          capture_output=True, text=True, timeout=30, check=False)
    except (OSError, subprocess.SubprocessError):
        raise PreflightBlocked("baseline_code_signature_check_failed") from None
    if codesign.returncode != 0:
        raise PreflightBlocked("baseline_code_signature_invalid")

    _, record, token, port = read_service(home or Path.home())
    check_owned_listener(record["process_id"], port, runner)
    check_health(port, token, connection_factory)

    helper_source = Path(__file__).with_name("ax-preflight.swift")
    with temp_factory(prefix="chat-hub-ax-preflight-") as temp_dir:
        helper = Path(temp_dir) / "ax-preflight"
        try:
            compile_result = runner(["/usr/bin/swiftc", "-O", str(helper_source), "-o", str(helper)],
                                    capture_output=True, text=True, timeout=60, check=False)
        except (OSError, subprocess.SubprocessError):
            raise PreflightBlocked("ax_helper_compile_failed") from None
        if compile_result.returncode != 0:
            raise PreflightBlocked("ax_helper_compile_failed")
        try:
            result = runner([str(helper), str(record["process_id"]), BUNDLE_ID,
                             "0.1.1", args.expected_executable_sha256.lower(), str(app)],
                            capture_output=True, text=True, timeout=20, check=False)
        except (OSError, subprocess.SubprocessError):
            raise PreflightBlocked("ax_helper_execution_failed") from None
        if result.returncode not in (0, 2):
            raise PreflightBlocked("ax_helper_execution_failed")
        try:
            ax = validate_ax_result(json.loads(result.stdout), record["process_id"])
        except (ValueError, TypeError):
            raise PreflightBlocked("ax_helper_output_invalid") from None

    return {
        "status": "ready",
        "baseline_version": "0.1.1",
        "bundle_id": BUNDLE_ID,
        "process_id": record["process_id"],
        "dmg_sha256": dmg_digest,
        "executable_sha256": args.expected_executable_sha256.lower(),
        "codesign_integrity_verified": True,
        "health_status_ok": True,
        "accessibility_trusted": ax["trusted"],
        "check_button_count": ax["check_button_count"],
        "check_button_enabled": ax["check_button_enabled"],
        "check_button_press_supported": ax["check_button_press_supported"],
        "checked_feed": False,
    }


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dmg", required=True, help="unchanged published v0.1.1 DMG already downloaded")
    parser.add_argument("--expected-dmg-sha256", required=True, help="digest from reviewed release provenance")
    parser.add_argument("--app", required=True, help="writable owned copy of the unchanged app")
    parser.add_argument("--expected-executable-sha256", required=True, help="pinned baseline executable digest")
    args = parser.parse_args(argv)
    try:
        result = preflight(args)
    except PreflightBlocked as exc:
        print(json.dumps({"status": "blocked", "reason": exc.reason}, sort_keys=True))
        return 2
    except (OSError, subprocess.SubprocessError):
        print(json.dumps({"status": "blocked", "reason": "preflight_io_or_process_failure"}, sort_keys=True))
        return 2
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
