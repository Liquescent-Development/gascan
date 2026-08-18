#!/usr/bin/env bash
# Materialise the engine proto from the pinned Arca revision, with the same
# provenance guarantees as scripts/build-arca-engine.sh and none of its cost.
#
# That script exists to compile Arca. It clones the full history, initialises a
# submodule and runs `swift build`, which is right for a release gate and wrong
# for a `cargo build`: the cache it produces is 1.3 GB. This script answers a
# much smaller question -- what does the pinned revision say the contract is --
# and a depth-1, blob-filtered fetch of one tag answers it in about a second.
#
# The verification is NOT reduced to match. The tag signature is checked against
# the tracked allowed-signers file and the tag is asserted to resolve to the
# pinned revision, exactly as the build script does. A cheaper fetch is not a
# weaker claim; it is the same claim over fewer bytes.
#
# Prints the directory holding the extracted proto tree on stdout.
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/.." && pwd -P)
pin_file=${GASCAN_ARCA_PIN_FILE:-$repo_root/engine/arca-pin.json}
cache_root=${GASCAN_ARCA_PROTO_CACHE:-$repo_root/.artifacts/arca-proto}
allowed_signers=${GASCAN_ARCA_ALLOWED_SIGNERS:-$repo_root/engine/allowed-signers}
# Not overridable, for the reason build-arca-engine.sh gives: the pin file is a
# fixture surface, the schema that judges it is not.
pin_schema=$repo_root/engine/arca-pin-schema.jq

for command in git jq; do
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
# The same schema file scripts/build-arca-engine.sh validates against. The two
# scripts read the same pin file and must agree on what a valid one is, or a pin
# this script accepts is one the build refuses -- and through schema 1 that
# agreement was two copies of one jq program, which is a promise rather than a
# mechanism. Reading one file makes disagreement impossible instead of unlikely.
[[ -f $pin_schema ]] || {
  printf 'engine pin schema is missing: %s\n' "$pin_schema" >&2
  exit 64
}
jq -e --from-file "$pin_schema" "$pin_file" >/dev/null 2>&1 || {
  printf 'engine pin file is malformed: %s\n' "$pin_file" >&2
  exit 64
}

url=$(jq -er '.url' "$pin_file")
tag=$(jq -er '.tag' "$pin_file")
revision=$(jq -er '.revision' "$pin_file")

# Keyed by revision, so a pin bump cannot be served by a stale extract and no
# invalidation logic has to be written or trusted. The published path appears
# only via an atomic rename of a fully-populated staging tree, so a build that
# sees this directory sees all of it. That is also why this script needs no lock
# where build-arca-engine.sh does: that one mutates a single shared checkout in
# place, while two concurrent runs of this one produce identical bytes and race
# only to rename them.
extract=$cache_root/$revision
if [[ -d $extract ]]; then
  printf '%s\n' "$extract"
  exit 0
fi

mkdir -p "$cache_root"
staging=$(mktemp -d "$cache_root/.staging.XXXXXX")
trap 'rm -rf "$staging"' EXIT

work=$staging/repo
mkdir -p "$work"
git init --quiet "$work"
git -C "$work" remote add origin "$url"
# --depth 1 with a blob filter fetches the annotated tag object, the commit it
# points at and the trees -- enough to verify the signature and resolve the tag,
# and about 108 KB rather than the build cache's 1.3 GB. Blobs are fetched
# lazily from the promisor remote when the checkout below reads them.
git -C "$work" fetch --quiet --depth 1 --filter=blob:none origin "tag" "$tag"

# Fully qualified, for the reason spelled out at the same call in
# scripts/build-arca-engine.sh: an unqualified name resolves by a different rule
# than the refs/tags/ form used two lines down, so the object whose signature is
# checked need not be the object whose identity is checked. This call was immune
# only by accident -- `git init` plus a single `fetch origin tag <tag>` leaves
# exactly one ref that could match -- and immune by accident is not a property
# anything downstream should rest on.
git -C "$work" -c "gpg.ssh.allowedSignersFile=$allowed_signers" \
  verify-tag "refs/tags/${tag}" >/dev/null || {
  printf 'engine pin tag signature does not verify against %s: %s\n' \
    "$allowed_signers" "$tag" >&2
  exit 65
}
tag_target=$(git -C "$work" rev-parse --verify "refs/tags/${tag}^{}") || {
  printf 'engine pin tag is absent: %s\n' "$tag" >&2
  exit 65
}
[[ $tag_target == "$revision" ]] || {
  printf 'engine pin tag %s resolves to %s, not the pinned revision %s\n' \
    "$tag" "$tag_target" "$revision" >&2
  exit 65
}

# Asked before extracting, so the failure names the actual problem. `git archive`
# on an absent path dies with "pathspec did not match any files", which is true
# and tells a reader nothing about which pin is wrong or why.
git -C "$work" cat-file -e "${revision}:proto/arca/engine/v1/engine.proto" 2>/dev/null || {
  printf 'engine proto is absent at the pinned revision: %s\n' "$revision" >&2
  printf 'expected proto/arca/engine/v1/engine.proto; the pin must name a revision carrying the contract\n' >&2
  exit 65
}

# Only the proto tree. Nothing else from Arca is materialised, so this cannot
# quietly become a second, weaker copy of the engine build.
mkdir -p "$staging/tree"
git -C "$work" archive --format=tar "$revision" proto | tar -x -C "$staging/tree"

[[ -f $staging/tree/proto/arca/engine/v1/engine.proto ]] || {
  printf 'extraction produced no engine proto from revision %s\n' "$revision" >&2
  exit 70
}

rm -rf "$work"

# Publishing needs a claim, not a bare `mv`. `mv dir existing-dir` does not
# fail -- it moves the source *inside* the target -- so a lost race would
# silently produce $extract/tree and every later build would read a path that
# does not exist. mkdir is atomic on POSIX, so the claim decides one winner.
claim=$cache_root/.claim.$revision
if mkdir "$claim" 2>/dev/null; then
  trap 'rm -rf "$staging"; rmdir "$claim" 2>/dev/null || true' EXIT
  mv "$staging/tree" "$extract"
else
  # Another run holds the claim for this exact revision. Wait for it rather than
  # racing it, but never wait indefinitely: a claim whose holder died must
  # surface as an error a person can act on, not as a build that hangs.
  waited=0
  until [[ -d $extract ]]; do
    if ((waited >= 60)); then
      printf 'another run claimed %s but never published it\n' "$revision" >&2
      printf 'if no build is running, remove the stale claim: %s\n' "$claim" >&2
      exit 75
    fi
    sleep 1
    waited=$((waited + 1))
  done
fi

printf '%s\n' "$extract"
