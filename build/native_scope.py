#!/usr/bin/env python3
"""Fail-closed scope check for required macOS native validation."""
import argparse
import fnmatch
from pathlib import Path
import subprocess


NATIVE_PATTERNS = (
    'src-tauri/**', 'build/**', 'tests/native/**', 'src/**', 'frontend/**',
    'desktop-ui/**', 'crates/**', 'ci/actions/**', 'Cargo.toml',
    'Cargo.lock', 'rust-toolchain.toml', 'tauri.conf.json',
    '.github/workflows/macos.yml', '.github/workflows/check.yml',
    '.github/workflows/accept-native.yml',
)


def requires_native(paths):
    return any(fnmatch.fnmatchcase(path, pattern) for path in paths for pattern in NATIVE_PATTERNS)


def changed_paths(root, base, head):
    for revision in (base, head):
        subprocess.run(['git', '-C', str(root), 'cat-file', '-e', f'{revision}^{{commit}}'],
                       check=True, capture_output=True)
    result = subprocess.run(
        ['git', '-C', str(root), 'diff', '--no-renames', '--name-only', '-z',
         '--diff-filter=ACMRD', f'{base}...{head}'],
        check=True, capture_output=True)
    return [item.decode('utf-8', errors='strict') for item in result.stdout.split(b'\0') if item]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--root', type=Path, default=Path.cwd())
    parser.add_argument('--base', required=True)
    parser.add_argument('--head', required=True)
    args = parser.parse_args()
    paths = changed_paths(args.root, args.base, args.head)
    required = requires_native(paths)
    print(f'required={str(required).lower()}')
    print(f'changed_paths={len(paths)}')


if __name__ == '__main__':
    main()
