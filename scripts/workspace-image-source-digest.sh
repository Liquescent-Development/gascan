#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
    printf 'usage: %s ROOT\n' "${0##*/}" >&2
    exit 2
fi

root=$(cd -- "$1" && pwd -P)

umask 077
paths=$(mktemp "${TMPDIR:-/tmp}/workspace-image-source-paths.XXXXXX")
records=$(mktemp "${TMPDIR:-/tmp}/workspace-image-source-records.XXXXXX")
trap 'rm -f -- "$paths" "$records"' EXIT

git -C "$root" ls-files -z -- images/workspace >"$paths"

selected=0
while IFS= read -r -d '' path; do
    case "$path" in
        images/workspace/approved-image.txt|images/workspace/approved-source.sha256)
            continue
            ;;
    esac

    if [[ "$path" == *$'\t'* || "$path" == *$'\n'* ]]; then
        printf 'unsafe workspace image source path: %q\n' "$path" >&2
        exit 1
    fi

    file="$root/$path"
    if [[ -L "$file" || ! -f "$file" ]]; then
        printf 'workspace image source must be a regular file: %s\n' "$path" >&2
        exit 1
    fi

    file_digest=$(shasum -a 256 <"$file")
    file_digest=${file_digest%%[[:space:]]*}
    if [[ ! "$file_digest" =~ ^[0-9a-f]{64}$ ]]; then
        printf 'failed to hash workspace image source: %s\n' "$path" >&2
        exit 1
    fi

    printf '%s\t%s\n' "$path" "$file_digest" >>"$records"
    ((selected += 1))
done <"$paths"

if ((selected == 0)); then
    printf 'workspace image source tree is empty\n' >&2
    exit 1
fi

digest=$(shasum -a 256 "$records")
digest=${digest%%[[:space:]]*}
if [[ ! "$digest" =~ ^[0-9a-f]{64}$ ]]; then
    printf 'failed to hash workspace image source records\n' >&2
    exit 1
fi

printf '%s\n' "$digest"
