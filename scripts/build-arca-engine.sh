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

# The engine product, plus SandboxEngineProto so the generated server half is
# proven to build rather than merely proven to have been emitted —
# crates/gascan-engine-proto generates a client from the same revision, so
# without this the pinned server end would be the only one nothing compiled.
#
# ContainerBridge is no longer named: arca-engine reaches it transitively, and
# naming it separately would hide the day that edge disappears.
#
# Two invocations, not one: `swift build` rejects --product and --target in
# the same call ("mutually exclusive"), and arca-engine is selected by
# product while SandboxEngineProto has no product of its own to select by.
# Both share the same .build directory, so the second call is incremental.
swift build --package-path "$checkout" --configuration release \
  --product arca-engine >&2
swift build --package-path "$checkout" --configuration release \
  --target SandboxEngineProto >&2

# Arca has no CI, so nothing else ever runs the engine's own tests and they
# would rot unnoticed. This is a clean checkout of the signed tag, which makes
# it the right place: it proves the pinned engine passes its own suite rather
# than proving a developer's working tree did.
#
# --configuration release, matching the build above: leaving this unconfigured
# would make SwiftPM build the whole package a second time in debug, and this
# package vendors containerization, so that would be a very expensive mistake.
#
# --disable-swift-testing because the package has no swift-testing tests, and
# in release SwiftPM launches that runner by invoking an executable target with
# --test-bundle-path. Arca's `Arca` executable is an ArgumentParser command, so
# it rejects the unknown option and the run exits non-zero with every XCTest
# passing -- a green suite reported as a failed build.
swift test --package-path "$checkout" --configuration release \
  --disable-swift-testing --filter ArcaEngineTests >&2

binary=$checkout/.build/release/arca-engine
[[ -x $binary ]] || {
  printf 'engine build produced no executable at %s\n' "$binary" >&2
  exit 70
}

printf '%s\n%s\n' "$checkout" "$binary"
