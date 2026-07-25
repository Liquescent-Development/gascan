#!/bin/sh
set -eu

test $# -eq 1 || {
    printf 'usage: immutable-tree-snapshot.sh ROOT\n' >&2
    exit 64
}
root=$1
test -d "$root" || {
    printf 'immutable tree root is not a directory: %s\n' "$root" >&2
    exit 1
}

python3 - "$root" <<'PY'
import hashlib
import os
import stat
import sys

root = os.fsencode(sys.argv[1])


def fail(message):
    print(message, file=sys.stderr)
    raise SystemExit(1)


def field(digest, value):
    if not isinstance(value, bytes):
        value = str(value).encode("ascii")
    digest.update(len(value).to_bytes(8, "big"))
    digest.update(value)


def metadata_fields(info):
    return (info.st_uid, info.st_gid, stat.S_IMODE(info.st_mode))


try:
    root_info = os.lstat(root)
except OSError as error:
    fail(f"cannot inspect immutable tree root: {error}")

if not stat.S_ISDIR(root_info.st_mode):
    fail(f"immutable tree root is not a directory: {os.fsdecode(root)}")

root_device = root_info.st_dev
entries = [(b".", root, root_info)]


def collect(directory, relative):
    try:
        children = sorted(os.scandir(directory), key=lambda entry: entry.name)
    except OSError as error:
        fail(f"cannot scan immutable tree: {error}")

    for child in children:
        child_relative = child.name if relative == b"." else relative + b"/" + child.name
        try:
            info = child.stat(follow_symlinks=False)
        except OSError as error:
            fail(f"cannot inspect immutable tree entry {os.fsdecode(child_relative)}: {error}")
        if info.st_dev != root_device:
            fail(
                "unsupported immutable tree entry on another device: "
                + os.fsdecode(child_relative)
            )
        entries.append((child_relative, child.path, info))
        if stat.S_ISDIR(info.st_mode):
            collect(child.path, child_relative)


collect(root, b".")
digest = hashlib.sha256()

for relative, path, info in entries:
    if stat.S_ISDIR(info.st_mode):
        values = (b"directory", relative, *metadata_fields(info))
    elif stat.S_ISLNK(info.st_mode):
        try:
            target = os.readlink(path)
        except OSError as error:
            fail(f"cannot read immutable tree symlink {os.fsdecode(relative)}: {error}")
        values = (b"symlink", relative, *metadata_fields(info), target)
    elif stat.S_ISREG(info.st_mode):
        flags = os.O_RDONLY
        if hasattr(os, "O_NOFOLLOW"):
            flags |= os.O_NOFOLLOW
        try:
            descriptor = os.open(path, flags)
            with os.fdopen(descriptor, "rb") as source:
                before = os.fstat(source.fileno())
                content = hashlib.sha256()
                byte_count = 0
                while True:
                    block = source.read(1024 * 1024)
                    if not block:
                        break
                    byte_count += len(block)
                    content.update(block)
                after = os.fstat(source.fileno())
            current = os.lstat(path)
        except OSError as error:
            fail(f"cannot read immutable tree file {os.fsdecode(relative)}: {error}")
        stable = (
            before.st_dev,
            before.st_ino,
            before.st_mode,
            before.st_uid,
            before.st_gid,
            before.st_size,
            before.st_mtime_ns,
            before.st_ctime_ns,
        )
        if (
            stable
            != (
                after.st_dev,
                after.st_ino,
                after.st_mode,
                after.st_uid,
                after.st_gid,
                after.st_size,
                after.st_mtime_ns,
                after.st_ctime_ns,
            )
            or stable
            != (
                current.st_dev,
                current.st_ino,
                current.st_mode,
                current.st_uid,
                current.st_gid,
                current.st_size,
                current.st_mtime_ns,
                current.st_ctime_ns,
            )
            or byte_count != before.st_size
        ):
            fail(f"immutable tree changed while reading: {os.fsdecode(relative)}")
        values = (
            b"file",
            relative,
            *metadata_fields(before),
            before.st_size,
            content.digest(),
        )
    else:
        fail(f"unsupported immutable tree entry: {os.fsdecode(relative)}")

    field(digest, b"record")
    for value in values:
        field(digest, value)

print(digest.hexdigest())
PY
