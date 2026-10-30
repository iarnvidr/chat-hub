#!/usr/bin/env python3
"""Bounded direct packaged-binary launch probe for failed macOS smoke runs."""
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time


def probe(binary, evidence_dir, duration=10.0, binary_args=()):
    evidence = Path(evidence_dir)
    evidence.mkdir(parents=True, exist_ok=True)
    stdout_path, stderr_path = evidence / "probe.stdout.log", evidence / "probe.stderr.log"
    env = os.environ.copy()
    env["RUST_BACKTRACE"] = "full"
    started = time.monotonic()
    with stdout_path.open("wb") as stdout_file, stderr_path.open("wb") as stderr_file:
      proc = subprocess.Popen([str(binary), *map(str, binary_args)], stdout=stdout_file, stderr=stderr_file, env=env)
    try:
        try:
            code = proc.wait(timeout=duration)
            elapsed_ms = round((time.monotonic() - started) * 1000)
            result = {"status": "natural_exit", "exit_code": code if code >= 0 else None,
                      "signal": -code if code < 0 else None, "elapsed_ms": elapsed_ms,
                      "cleanup_requested": False}
        except subprocess.TimeoutExpired:
            elapsed_ms = round((time.monotonic() - started) * 1000)
            result = {"status": "survived_duration", "elapsed_ms": elapsed_ms,
                      "survived_duration": True, "cleanup_requested": True,
                      "cleanup_signal": "SIGTERM"}
            proc.terminate()
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                result["cleanup_signal"] = "SIGKILL"
                proc.kill()
                proc.wait()
            result["cleanup_exit_code"] = proc.returncode
    finally:
        if proc.poll() is None:
            proc.kill()
            proc.wait()
    result["stdout_log"] = str(stdout_path)
    result["stderr_log"] = str(stderr_path)
    print(json.dumps(result, sort_keys=True))
    for path in (stdout_path, stderr_path):
        print(f"--- {path} (last 80 lines) ---", file=sys.stderr)
        try:
            lines = path.read_text(errors="replace").splitlines()
            print("\n".join(lines[-80:]), file=sys.stderr)
        except OSError as exc:
            print(f"could not read log: {exc}", file=sys.stderr)
    return 0


def main():
    argv = sys.argv[1:]
    duration = 10.0
    if "--duration" in argv:
        index = argv.index("--duration")
        duration = float(argv[index + 1])
        del argv[index:index + 2]
    binary, evidence_dir, *binary_args = argv
    if binary_args and binary_args[0] == "--":
        binary_args = binary_args[1:]
    try:
        return probe(binary, evidence_dir, duration, binary_args)
    except Exception as exc:
        print(f"secondary process probe error: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
