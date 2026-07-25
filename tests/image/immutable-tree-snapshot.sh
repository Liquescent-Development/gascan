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

(
    cd "$root"
    {
        find . -xdev -type f -exec sha256sum {} +
        find . -xdev -type f -exec stat -c '%n	%U:%G	%a	%s' {} +
    } | LC_ALL=C sort | sha256sum | awk '{print $1}'
)
