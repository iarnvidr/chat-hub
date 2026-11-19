#!/usr/bin/env python3
"""Create or secure a Chat Hub web token without following or replacing files."""
from __future__ import annotations

import errno
import os
from pathlib import Path
import secrets
import stat
import sys


DIR_FLAGS = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC
FILE_FLAGS = os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC
CREATE_FLAGS = os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC


def open_private_parent(path: Path) -> int:
    absolute = Path(os.path.abspath(path))
    current = os.open("/", DIR_FLAGS)
    try:
        for part in absolute.parts[1:]:
            try:
                following = os.open(part, DIR_FLAGS, dir_fd=current)
            except FileNotFoundError:
                os.mkdir(part, 0o700, dir_fd=current)
                following = os.open(part, DIR_FLAGS, dir_fd=current)
            os.close(current)
            current = following

        info = os.fstat(current)
        if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.geteuid():
            raise ValueError("token parent must be a directory owned by this user")
        if stat.S_IMODE(info.st_mode) != 0o700:
            os.fchmod(current, 0o700)
        return current
    except BaseException:
        os.close(current)
        raise


def read_existing(parent_fd: int, name: str) -> None:
    fd = os.open(name, FILE_FLAGS, dir_fd=parent_fd)
    try:
        info = os.fstat(fd)
        if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.geteuid()
                or info.st_nlink != 1 or info.st_size == 0):
            raise ValueError("token must be a nonempty, single-link regular file owned by this user")
        if info.st_size > 4096 or not os.read(fd, 4097).strip():
            raise ValueError("token file must contain a nonempty token no larger than 4096 bytes")
        if stat.S_IMODE(info.st_mode) != 0o600:
            os.fchmod(fd, 0o600)
    finally:
        os.close(fd)


def ensure_token(path: Path) -> None:
    absolute = Path(os.path.abspath(path))
    name = absolute.name
    if not name or name in {".", ".."}:
        raise ValueError("token path must name a file")

    parent_fd = open_private_parent(absolute.parent)
    temporary = f".{name}.{os.getpid()}.{secrets.token_hex(12)}.tmp"
    try:
        try:
            read_existing(parent_fd, name)
            return
        except FileNotFoundError:
            pass

        try:
            token_fd = os.open(temporary, CREATE_FLAGS, 0o600, dir_fd=parent_fd)
            try:
                payload = (secrets.token_urlsafe(32) + "\n").encode("ascii")
                offset = 0
                while offset < len(payload):
                    offset += os.write(token_fd, payload[offset:])
                os.fsync(token_fd)
            finally:
                os.close(token_fd)

            try:
                os.link(temporary, name, src_dir_fd=parent_fd, dst_dir_fd=parent_fd,
                        follow_symlinks=False)
            except FileExistsError:
                # A concurrent starter won the publication race. Keep its token.
                pass
        finally:
            try:
                os.unlink(temporary, dir_fd=parent_fd)
            except FileNotFoundError:
                pass
        os.fsync(parent_fd)
        read_existing(parent_fd, name)
    finally:
        os.close(parent_fd)


def main() -> int:
    try:
        if len(sys.argv) != 2:
            raise ValueError("usage: ensure-private-token.py TOKEN_FILE")
        ensure_token(Path(sys.argv[1]))
    except (OSError, ValueError) as exc:
        # Never include token contents in diagnostics.
        message = str(exc)
        if isinstance(exc, OSError) and exc.errno in {errno.ELOOP, errno.ENOTDIR}:
            message = "token path contains a symlink or a non-directory component"
        print(f"web token setup: {message}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
