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
# .tag is constrained to characters that cannot form a path. A tag name
# containing a slash is legal to git, and "tags/foo" as a pin would name two
# different objects to two different resolvers -- see the note on the
# verify-tag call below. The refs/tags/ qualification there is the real fix;
# this is the second lock on the same door, and it is the one that fails early
# and names the pin file.
jq -e '
  (.schema == 1) and
  (.name | type == "string" and length > 0) and
  (.url | type == "string" and length > 0 and test("^(https|file)://")) and
  (.tag | type == "string" and test("^[A-Za-z0-9._-]+$")) and
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
# The status is captured and re-raised because a bare `trap 'rmdir ...' EXIT`
# makes the trap's own status the script's. MEASURED: `bash -c 'set -euo
# pipefail; d=$(mktemp -d); trap "rmdir \"$d\"" EXIT; touch "$d/x"; exit 65'`
# exits 1, not 65, and leaves the directory. Any stray entry under the lock -- a
# .DS_Store, an NFS sillyrename -- would therefore collapse every documented exit
# code (64, 65, 69, 70, 75) to 1 AND strand the lock, which fails closed for
# every later run. `|| true` on the rmdir because a lock we cannot remove must
# not rewrite the exit code of the thing that actually failed.
status=0
trap 'status=$?; rmdir "$lock" 2>/dev/null || true; exit "$status"' EXIT
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
# refs/tags/ANYWHERE A TAG NAME APPEARS. Naming the tag unqualified here while
# qualifying it below let the signature gate and the identity gate resolve two
# different objects: git tries $GIT_DIR/<name>, then refs/<name>, and only then
# refs/tags/<name>. REPRODUCED against a fixture repository -- with an annotated,
# properly signed refs/tags/foo and an unsigned lightweight refs/tags/tags/foo on
# an attacker's commit, a pin naming "tags/foo" passed all three gates: the
# unqualified name resolved to the good tag for verification while
# refs/tags/tags/foo^{} resolved to the attacker's commit, which is what got
# compiled. `git fetch --tags` brings both refs down, so no local write access is
# needed. A ref planted at refs/<tag> in the warm cache shadows refs/tags/<tag>
# the same way and needs no slash at all.
#
# The tree handed to the compiler was already proven to be the tag's. Which tag
# was signed was not.
git -C "$checkout" -c "gpg.ssh.allowedSignersFile=$allowed_signers" \
  verify-tag "refs/tags/${tag}" >/dev/null || {
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
# Two suites, not one. ArcaEngineTests cannot reach the `docker network prune`
# attachment gate: NetworkHandlers is in DockerAPI, ArcaEngineTests depends only
# on ArcaEngine, and tests/release/engine-targets-check.sh exists to keep that
# edge from ever appearing. So the test that proves prune declines to delete an
# in-use network lives in ArcaTests, and until it was named here nothing ran it.
#
# The named suite and NOT all of ArcaTests: that target also holds integration
# tests that want a live daemon and VMs, which have no business in a release
# gate. Widening this to `ArcaTests` would trade a gate that runs too little for
# one that cannot run at all.
#
# --configuration release, matching the build above: leaving this unconfigured
# would make SwiftPM build the whole package a second time in debug, and this
# package vendors containerization, so that would be a very expensive mistake.
#
# --disable-swift-testing because both filtered suites are pure XCTest, so the flag
# skips nothing this script intends to run -- while in release configuration SwiftPM
# launches the swift-testing runner by invoking an executable target with
# --test-bundle-path. Arca's `Arca` executable is an ArgumentParser command, so it
# rejects the unknown option and the run exits non-zero with every XCTest passing --
# a green suite reported as a failed build. The rest of the package DOES carry
# swift-testing tests (15 files across ArcaIPTests and ArcaTests import Testing), so
# "the package has no swift-testing tests" would be a false reason for this flag.
#
# The listing guard exists because `swift test --filter` exits 0 when the filter
# matches nothing. VERIFIED in the pinned checkout: `--filter ZZZNoSuchSuiteName`
# exits 0 reporting `Executed 0 tests, with 0 failures` and only `warning: No
# matching test cases were run` on stderr. Without the guard the gate below reports
# success having verified nothing the day ArcaEngineTests is renamed or moved --
# silently, in the one script standing between a signed tag and a shipped binary.
#
# `swift test list` and not the "Executed N tests" output: the listing is a
# machine-readable contract -- one `Module.Suite/test` identifier per line on stdout,
# build progress on stderr -- and `--filter` matches against those same identifiers,
# so a listing with no ArcaEngineTests entry is exactly the case where the filter
# would match nothing. Parsing the human-readable run output was considered and
# rejected as a more brittle contract that can drift silently across Swift versions,
# which is the decay class this guard exists to close.
#
# The whole listing is grepped, and NO --filter is passed to it, because the obvious
# form -- filter the listing and fail when it comes back empty -- CANNOT WORK.
# MEASURED on Swift 6.3.3: `swift test list --disable-swift-testing --filter
# ZZZNoSuchSuiteName` exits 0 and prints the full UNFILTERED listing; `swift test
# list` accepts --filter and silently ignores it. So a filtered-listing guard would
# never fire. ("Unknown option '--filter'" belongs to the other form: `swift test
# --list-tests --filter`, which exits 64 and warns that --list-tests is deprecated in
# favour of `swift test list`. Neither form takes a filter usefully.)
#
# The listing builds the test targets, so the run below is incremental against it.
listed=$(swift test list --package-path "$checkout" --configuration release \
  --disable-swift-testing) || {
  printf 'could not list the pinned engine tests in %s\n' "$checkout" >&2
  exit 70
}

# Each filtered suite is asserted separately, and each names itself when it is
# the one that went missing. A single grep matching either, or one message
# naming both, would let the DockerAPI-side suite vanish behind a still-present
# ArcaEngineTests -- the same hole this guard was written to close, reopened one
# suite over. The patterns are anchored past the suite name (`\.` for the target,
# `/` for the class) so a renamed-but-prefixed leftover cannot satisfy them.
require_listed() {
  grep -q "$2" <<<"$listed" || {
    printf 'the test gate matched no tests: %s declares no %s\n' "$checkout" "$1" >&2
    exit 70
  }
}
require_listed ArcaEngineTests '^ArcaEngineTests\.'
require_listed ArcaTests.NetworkPruneGateTests '^ArcaTests\.NetworkPruneGateTests/'
# Two --filter flags and not one alternation: SwiftPM unions repeated --filter,
# and keeping them separate means each pattern is the same string as the guard's,
# minus the anchors, rather than a regex that has to be read twice.
swift test --package-path "$checkout" --configuration release \
  --disable-swift-testing \
  --filter ArcaEngineTests \
  --filter 'ArcaTests\.NetworkPruneGateTests' >&2

binary=$checkout/.build/release/arca-engine
[[ -x $binary ]] || {
  printf 'engine build produced no executable at %s\n' "$binary" >&2
  exit 70
}

printf '%s\n%s\n' "$checkout" "$binary"
