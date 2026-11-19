#!/usr/bin/env python3
"""Hash the *actual* Dagger input closure of each independently tested crate.

Read the declarative Directory expressions in main.dang, including synthetic
Cargo skeleton files. A new/unrecognised expression fails closed instead of
silently omitting a compiler input from a reusable object's identity.
"""
from __future__ import annotations

import functools
import hashlib
import json
import os
from pathlib import Path
import re


JSON_STRING = r'"(?:\\.|[^"\\])*"'
DIRECTORY_CALL = re.compile(
    rf"ws\.directory\(({JSON_STRING})"
    r"(?:,\s*include:\s*(\[.*?\]))?"
    r"(?:,\s*exclude:\s*(\[.*?\]))?\)", re.S,
)
FILE_CALL = re.compile(rf"\.withFile\(({JSON_STRING}),\s*ws\.file\(({JSON_STRING})\)\)")
NEW_FILE = re.compile(rf"\.withNewFile\(({JSON_STRING}),\s*({JSON_STRING})\)")


def declarations(module: str) -> tuple[dict[str, str], dict[str, str]]:
    directories = dict(re.findall(r"\blet (\w+): Directory! \{\n(.*?)\n  \}", module, re.S))
    checks = dict(re.findall(r"\bpub (\w+): Void @check \{\n(.*?)\n  \}", module, re.S))
    if not directories or not checks:
        raise ValueError("Dagger crate source declarations could not be parsed")
    return directories, checks


def parse_list(value: str | None) -> list[str]:
    return [] if value is None else [json.loads(item) for item in re.findall(JSON_STRING, value)]


@functools.lru_cache(maxsize=128)
def glob_regex(pattern: str) -> re.Pattern:
    result = ""
    index = 0
    while index < len(pattern):
        if pattern[index:index + 3] == "**/":
            result += "(?:[^/]+/)*"
            index += 3
        elif pattern[index:index + 2] == "**":
            result += ".*"
            index += 2
        elif pattern[index] == "*":
            result += "[^/]*"
            index += 1
        elif pattern[index] == "?":
            result += "[^/]"
            index += 1
        else:
            result += re.escape(pattern[index])
            index += 1
    return re.compile("^" + result + "$")


def matches(relative: str, patterns: list[str]) -> bool:
    return any(glob_regex(pattern).match(relative) for pattern in patterns)


def files_under(root: Path, relative_dir: str) -> tuple[str, ...]:
    directory = root / relative_dir.lstrip("/")
    if not directory.is_dir() or directory.is_symlink():
        raise ValueError(f"Dagger source directory is missing or unsafe: {relative_dir}")
    found: list[str] = []
    for parent, directories, names in os.walk(directory, followlinks=False):
        for name in directories + names:
            if (Path(parent) / name).is_symlink():
                raise ValueError("Dagger source closure contains a symlink")
        directories[:] = [name for name in directories if name not in (".git", "target", "__pycache__", "node_modules")]
        for name in names:
            path = Path(parent, name)
            if path.is_file():
                found.append(path.relative_to(directory).as_posix())
    return tuple(found)


def scope(name: str, root: Path, module: str | None = None) -> dict[str, Path | str]:
    module = module if module is not None else (root / "ci/dagger/check/main.dang").read_text()
    directories, _ = declarations(module)
    cache: dict[str, dict[str, Path | str]] = {}
    pending: set[str] = set()

    def evaluate(label: str) -> dict[str, Path | str]:
        if label in cache:
            return cache[label]
        if label not in directories or label in pending:
            raise ValueError(f"Missing or cyclic Dagger source closure: {label}")
        pending.add(label)
        body = directories[label]
        parent = re.match(r"\s*(\w+)\b", body)
        if parent is None:
            raise ValueError("Unsupported Dagger source scope expression")
        result = dict(evaluate(parent[1])) if parent[1] != "ws" else {}
        directory_calls = list(DIRECTORY_CALL.finditer(body))
        file_calls = list(FILE_CALL.finditer(body))
        synthetic_calls = list(NEW_FILE.finditer(body))
        if (len(directory_calls) != body.count("ws.directory(")
                or len(file_calls) != body.count(".withFile(")
                or len(synthetic_calls) != body.count(".withNewFile(")):
            raise ValueError("Unsupported Dagger source input expression; refusing a cache fingerprint")
        # Only these declarations may modify a scope. Unknown methods must
        # become a test/code review event, not an accidental cache hit.
        stripped = body
        for match in sorted(directory_calls + file_calls + synthetic_calls,
                            key=lambda item: item.start(), reverse=True):
            stripped = stripped[:match.start()] + stripped[match.end():]
        stripped = re.sub(r"\b" + re.escape(parent[1]) + r"\b", "", stripped, count=1)
        # .withDirectory wrappers remain after their nested ws.directory is
        # removed: allow those, including multi-line whitespace.
        stripped = re.sub(r"\.withDirectory\(\s*" + JSON_STRING + r"\s*,\s*\)", "", stripped)
        if stripped.strip():
            raise ValueError(f"Unsupported Dagger Directory operation in {label}")
        for call in directory_calls:
            source = json.loads(call[1]).strip("/")
            include, exclude = parse_list(call[2]), parse_list(call[3])
            for relative in files_under(root, source):
                if include and not matches(relative, include):
                    continue
                if matches(relative, exclude) or any(
                    relative.startswith(pattern.rstrip("/") + "/") for pattern in exclude if "*" not in pattern
                ):
                    continue
                full = f"{source}/{relative}".lstrip("/")
                result[full] = root / full
        for call in file_calls:
            result[json.loads(call[1]).lstrip("/")] = root / json.loads(call[2]).lstrip("/")
        for call in synthetic_calls:
            result[json.loads(call[1]).lstrip("/")] = json.loads(call[2])
        cache[label] = result
        pending.remove(label)
        return result

    return dict(evaluate(name))


def crate_scope(root: Path, crate: str) -> str:
    _, checks = declarations((root / "ci/dagger/check/main.dang").read_text())
    matches_crate = []
    for body in checks.values():
        package = re.search(r'"/run-crate.sh",\s*"([^"]+)"', body)
        if package and package[1] == crate:
            source = re.search(r'withMountedDirectory\("/input",\s*(\w+)\)', body)
            if source is None:
                raise ValueError(f"Dagger crate check is missing its source scope: {crate}")
            matches_crate.append(source[1])
    if len(matches_crate) != 1:
        raise ValueError(f"Expected exactly one Dagger check for {crate}")
    return matches_crate[0]


def fingerprint(root: Path, crate: str) -> dict[str, str | int]:
    label = crate_scope(root, crate)
    inputs = scope(label, root)
    digest = hashlib.sha256()
    for relative, value in sorted(inputs.items()):
        if (not relative or relative.startswith("/") or "\\" in relative or "\x00" in relative or
                any(part in ("", ".", "..") for part in relative.split("/"))):
            raise ValueError("Unsafe Dagger input path")
        data = value.encode() if isinstance(value, str) else value.read_bytes()
        digest.update(relative.encode() + b"\x00" + hashlib.sha256(data).digest() + b"\n")
    return {"scope": label, "sha256": digest.hexdigest(), "files": len(inputs)}
