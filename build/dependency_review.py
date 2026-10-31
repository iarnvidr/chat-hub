#!/usr/bin/env python3
"""Fail closed on newly introduced vulnerable dependencies in a pull request."""
import json
import os
import subprocess
import sys


def main():
    repository = os.environ["GH_REPO"]
    base = os.environ["BASE_SHA"]
    head = os.environ["HEAD_SHA"]
    if not repository or not base or not head:
        raise ValueError("repository and pull request revisions are required")

    endpoint = f"repos/{repository}/dependency-graph/compare/{base}...{head}"
    result = subprocess.run(
        ["gh", "api", "--paginate", "--slurp", endpoint],
        check=True, capture_output=True, text=True,
    )
    pages = json.loads(result.stdout)
    if not isinstance(pages, list):
        raise ValueError("dependency comparison response must be a page list")

    allowed = {"GHSA-wrw7-89jp-8q8g"}
    failures = []
    for page in pages:
        if not isinstance(page, list):
            raise ValueError("dependency comparison page must be a list")
        for change in page:
            if not isinstance(change, dict):
                raise ValueError("dependency comparison entry must be an object")
            if change.get("change_type") not in {"added", "changed"}:
                continue
            dependency = change.get("name", "unknown dependency")
            vulnerabilities = change.get("vulnerabilities", [])
            if not isinstance(vulnerabilities, list):
                raise ValueError("dependency vulnerabilities must be a list")
            for vulnerability in vulnerabilities:
                if not isinstance(vulnerability, dict):
                    raise ValueError("vulnerability entry must be an object")
                advisory = vulnerability.get("advisory_ghsa_id", "unknown advisory")
                if advisory not in allowed:
                    severity = vulnerability.get("severity", "unknown severity")
                    failures.append(f"{dependency}: {advisory} ({severity})")

    if failures:
        print("New or changed vulnerable dependencies were found:", file=sys.stderr)
        for failure in failures:
            print(f"- {failure}", file=sys.stderr)
        summary = os.environ.get("GITHUB_STEP_SUMMARY")
        if summary:
            with open(summary, "a", encoding="utf-8") as stream:
                stream.write("## Dependency review failed\n\n")
                stream.writelines(f"- {failure}\n" for failure in failures)
        return 1
    print("Dependency review passed; no new vulnerable dependencies were introduced.")
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a", encoding="utf-8") as stream:
            stream.write("Dependency review passed; no new vulnerable dependencies were introduced.\n")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (KeyError, ValueError, json.JSONDecodeError, subprocess.CalledProcessError) as error:
        print(f"Dependency review failed closed: {error}", file=sys.stderr)
        raise SystemExit(1)
