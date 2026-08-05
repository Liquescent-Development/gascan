#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/.." && pwd -P)
pin_file=${GASCAN_ARCA_PIN_FILE:-$repo_root/engine/arca-pin.json}
cache_root=${GASCAN_ARCA_ENGINE_CACHE:-$repo_root/.artifacts/arca-engine}
allowed_signers=${GASCAN_ARCA_ALLOWED_SIGNERS:-$repo_root/engine/allowed-signers}

for command in git jq swift; do
  command -v "$command" >/dev/null || {
    printf 'required command is unavailable: %s\n' "$command" >&2
    exit 69
  }
done

[[ -f $pin_file ]] || {
  printf 'engine pin file is missing: %s\n' "$pin_file" >&2
  exit 64
}
[[ -f $allowed_signers ]] || {
  printf 'engine allowed-signers file is missing: %s\n' "$allowed_signers" >&2
  exit 64
}
jq -e '
  (.schema == 1) and
  (.name | type == "string" and length > 0) and
  (.url | type == "string" and length > 0 and test("^(https|file)://")) and
  (.tag | type == "string" and length > 0) and
  (.revision | type == "string" and test("^[0-9a-f]{40}$"))
' "$pin_file" >/dev/null 2>&1 || {
  printf 'engine pin file is malformed: %s\n' "$pin_file" >&2
  exit 64
}

url=$(jq -er '.url' "$pin_file")
tag=$(jq -er '.tag' "$pin_file")
revision=$(jq -er '.revision' "$pin_file")

checkout=$cache_root/arca
mkdir -p "$cache_root"
# Everything below mutates the cache destructively, so two concurrent runs
# against the same cache would compile a torn tree. mkdir is atomic on POSIX and
# needs no tool this script does not already require. A held lock is an error and
# never a wait: a run that hangs on a lock is a release that hangs.
lock=$cache_root/.lock
mkdir "$lock" || {
  printf 'engine cache is in use or its lock is stale: %s\n' "$lock" >&2
  exit 75
}
trap 'rmdir "$lock"' EXIT
[[ -d $checkout/.git ]] || git clone --quiet "$url" "$checkout"
git -C "$checkout" remote set-url origin "$url"
# --force accepts a moved tag deliberately. A moved tag is not silently trusted:
# it fails below on the tag-target assertion, which is the real gate and reports
# the actual mismatch instead of an opaque fetch rejection. --prune-tags is the
# other half: deleting the tag upstream is this design's only revocation channel,
# and without it a warm cache keeps verifying a tag that no longer exists.
git -C "$checkout" fetch --quiet --prune --prune-tags --tags --force origin

git -C "$checkout" cat-file -e "${revision}^{commit}" 2>/dev/null || {
  printf 'pinned revision is absent from %s after fetch: %s\n' "$url" "$revision" >&2
  exit 65
}
git -C "$checkout" -c "gpg.ssh.allowedSignersFile=$allowed_signers" \
  verify-tag "$tag" >/dev/null || {
  printf 'engine pin tag signature does not verify against %s: %s\n' \
    "$allowed_signers" "$tag" >&2
  exit 65
}
tag_target=$(git -C "$checkout" rev-parse --verify "refs/tags/${tag}^{}") || {
  printf 'engine pin tag is absent: %s\n' "$tag" >&2
  exit 65
}
[[ $tag_target == "$revision" ]] || {
  printf 'engine pin tag %s resolves to %s, not the pinned revision %s\n' \
    "$tag" "$tag_target" "$revision" >&2
  exit 65
}

# The assertions above verify the tag; these three make the bytes handed to the
# compiler provably that tag's tree. A plain detach onto the revision a warm
# cache already holds is a no-op, so tracked edits and untracked plants would
# both survive into the build. -x is deliberate: it discards .build, and a
# poisoned build artifact serves an attacker as well as a poisoned source.
git -C "$checkout" checkout --quiet --detach --force "$revision"
git -C "$checkout" clean -qffdx
# Arca pins its containerization submodule to an SSH remote, which no hosted CI
# runner can reach. Rewriting the transport costs no provenance: the submodule
# content is fixed by the gitlink object ID recorded in the signed tag's tree,
# and git rejects any fetched object that does not hash to it.
git -C "$checkout" -c 'url.https://github.com/.insteadOf=git@github.com:' \
  submodule update --init --recursive --force --quiet
# Neither line above reaches inside a submodule: the top-level clean skips gitlink
# directories, and `submodule update --force` forces the checkout but leaves
# untracked files where they are. containerization is a SwiftPM path dependency,
# so a .swift left in its sources is compiled. Do not delete this as redundant.
# --quiet suppresses foreach's "Entering ..." line, which would otherwise land on
# stdout and corrupt the checkout path this script contracts to print there.
git -C "$checkout" submodule foreach --quiet --recursive git clean -qffdx

swift build --package-path "$checkout" --configuration release --target ContainerBridge >&2

printf '%s\n' "$checkout"
