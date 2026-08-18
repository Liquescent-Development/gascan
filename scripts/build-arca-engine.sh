#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/.." && pwd -P)
pin_file=${GASCAN_ARCA_PIN_FILE:-$repo_root/engine/arca-pin.json}
cache_root=${GASCAN_ARCA_ENGINE_CACHE:-$repo_root/.artifacts/arca-engine}
allowed_signers=${GASCAN_ARCA_ALLOWED_SIGNERS:-$repo_root/engine/allowed-signers}
# Not overridable. The pin file is, because the release contract drives this
# script with fixture pins, but the schema is what decides whether a pin is
# well-formed -- an override would let a caller weaken the gate rather than
# exercise it.
pin_schema=$repo_root/engine/arca-pin-schema.jq

# make, for gen-buildinfo below. It is Arca's Makefile that owns the shape of
# BuildInfo.generated.swift, and regenerating it here by hand would be a second
# copy of that shape, silently emitting the old one the day Arca changes it.
for command in codesign git jq make swift; do
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
# The schema is a file, and both this script and sync-arca-proto.sh validate
# against that same file. The two read the same pin and must agree on what a
# valid one is, or a pin this script accepts is one the proto sync refuses.
# Through schema 1 that agreement was two copies of one jq program and a comment
# asking a maintainer to keep them in step, which is not a mechanism.
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

# Sources/ContainerBridge/BuildInfo.generated.swift is TRACKED, and what is
# committed is whatever tree someone last ran `make` against. MEASURED at the
# time this was written: it recorded 5e1170495400b25f6334c6d8ddda5d3521b7cfd8
# while the tag being pinned was c545612b056e028d5885968a7b9f586d694f994c -- and
# it had drifted through the whole of milestone 3 before that, because nothing
# read it that mattered. This script compiled that constant and never ran make.
#
# Field 20 of Capabilities now carries it (SandboxEngineService.swift:182 ->
# ArcaVersion.buildRevision -> ArcaBuildInfo.buildRevision), and gascan-arca
# decides Proven versus Unverified by comparing it against a certified constant.
# A gate reading a stale constant matches nothing in the safe case and the wrong
# tree in the unsafe one, so regeneration is what makes the self-report worth
# comparing at all.
#
# Here and not earlier: `git clean -qffdx` above does not touch it (it is
# tracked), but `git checkout --detach --force` on the NEXT run resets it, so
# the dirt this leaves in the cache is self-healing and no cleanliness check is
# being silenced to accommodate it. Nothing between here and `swift build` reads
# the working tree's status.
#
# make and not an inline heredoc: the Makefile owns the shape of that file, and
# a copy of it here would keep emitting the old shape the day Arca changes it.
# >&2 because gen-buildinfo echoes progress, and stdout is this script's
# contract with its caller.
make -C "$checkout" gen-buildinfo >&2 || {
  printf 'could not regenerate the pinned engine build info in %s\n' "$checkout" >&2
  exit 70
}

# THIS ASSERTION IS THE POINT OF THE REGENERATION ABOVE. Without it the engine
# still self-reports its revision and the capability gate is worth nothing --
# regenerating merely changes which unverified value gets compiled.
#
# The generated source and not the built binary: this runs before `swift build`,
# so a mismatch costs no compile, and the constant the compiler will read is
# exactly the one asserted here. The engine is separately observed to report
# this value at run time, which is the acceptance for this task and not
# something a build gate can stand in for.
#
# Anchored on the full 40-character form. `buildRevision` is deliberately
# distinct from `gitCommit`, which is a 7-character display value Docker's
# /version returns; a prefix is not an identity and a pattern that admitted one
# would let a seven-character match pass for a forty-character claim.
build_info=$checkout/Sources/ContainerBridge/BuildInfo.generated.swift
[[ -f $build_info ]] || {
  printf 'the pinned engine generated no build info at %s\n' "$build_info" >&2
  exit 70
}
# The `|| true` keeps a no-match from ending the script under `set -e` before
# the comparison below can report WHICH value was found. A grep that matches
# nothing yields an empty string, which fails the comparison and prints it.
built_revision=$(sed -n 's/.*buildRevision = "\([0-9a-f]\{40\}\)".*/\1/p' "$build_info" | head -1 || true)
[[ $built_revision == "$revision" ]] || {
  printf 'the pinned engine compiles build revision %s, not the pinned revision %s (%s)\n' \
    "${built_revision:-<none found>}" "$revision" "$build_info" >&2
  exit 65
}

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

# One pattern per suite, used BOTH as the listing assertion and as the filter, so
# the two cannot drift. A guard asserting a suite the filter does not select --
# or a filter selecting a suite the guard does not assert -- reads as covered and
# is not, and a comment claiming the two agree is not a mechanism.
#
# Anchored, and the anchor is doing work in both roles. `--filter` is an
# unanchored substring regex, so a bare `ArcaEngineTests` would silently pull a
# future `ArcaEngineTestsIntegration` target into the release gate -- a
# daemon-requiring suite, run on a runner with no daemon, failing a long way from
# its cause -- and the guard, which only checks presence, could not see it. `\.`
# anchors the engine half on its target; `/` anchors the prune half on its class,
# because that suite's target keeps its name when only the class moves.
# The property that matters is that anchoring changed nothing about WHICH tests
# run -- the anchored pair selects the same set as the unanchored pair did.
# MEASURED on Swift 6.3.3 at Arca `fede19c`, when this guard was written: both
# forms selected 46 tests, 43 + 3. The count is not the claim and has moved since
# (Landing 3 and 4 add tests to ArcaEngineTests); re-derive it with
# `swift test list --disable-swift-testing --filter '^ArcaEngineTests\.'` rather
# than trusting a number written here.
engine_suite='^ArcaEngineTests\.'
prune_suite='^ArcaTests\.NetworkPruneGateTests/'

# Each suite is asserted separately, and each names itself when it is the one
# that went missing. A single grep matching either, or one message naming both,
# would let the DockerAPI-side suite vanish behind a still-present
# ArcaEngineTests -- the same hole this guard was written to close, reopened one
# suite over.
require_listed() {
  grep -q "$2" <<<"$listed" || {
    printf 'the test gate matched no tests: %s declares no %s\n' "$checkout" "$1" >&2
    exit 70
  }
}
require_listed ArcaEngineTests "$engine_suite"
require_listed ArcaTests.NetworkPruneGateTests "$prune_suite"
# Two --filter flags and not one alternation: SwiftPM unions repeated --filter,
# which keeps each pattern identical to the guard's rather than a regex that has
# to be read twice. That union is a property of the toolchain and not of this
# script, and CI's toolchain is unpinned, so it is pinned from the outside:
# tests/release/engine-pin-contract.sh runs a tree in which only the SECOND
# suite fails and requires this script to report it. The day a SwiftPM makes the
# last --filter win instead, that case goes red rather than this gate quietly
# running 3 of 46 tests.
swift test --package-path "$checkout" --configuration release \
  --disable-swift-testing \
  --filter "$engine_suite" \
  --filter "$prune_suite" >&2

binary=$checkout/.build/release/arca-engine
[[ -x $binary ]] || {
  printf 'engine build produced no executable at %s\n' "$binary" >&2
  exit 70
}

# An unsigned engine cannot start a container, so an unsigned binary is not a
# build product this script may hand to a caller. ContainerManager.initialize()
# constructs a Containerization.VmnetNetwork, and vmnet refuses that without
# com.apple.security.virtualization. MEASURED on the real engine with the
# signature as the only variable: unentitled it exits 1 on `failed to create
# vmnet network with status vmnet_return_t(rawValue: 1002)` and never creates
# its socket -- and 1002 is VMNET_MEM_FAILURE in vmnet.h, so the diagnostic
# sends whoever meets it looking for a memory fault. Signed, the same binary
# initialises all three managers and serves.
#
# Here and not at build time: this is the last step before the path is printed,
# so no caller can be handed an unsigned path, and it is after `swift test`,
# which shares the .build directory and would discard a signature applied
# earlier by relinking the executable.
#
# The entitlements come from the checkout this script has already verified --
# the same signed tag whose tree was compiled -- and from nowhere else. A copy
# living in Gas Can would be a second thing to keep in step with Arca's, and it
# is Arca's engine that has to hold these entitlements.
#
# Ad-hoc (`--sign -`) because it needs no certificate and no keychain, which is
# what makes it work on a hosted CI runner. AD-HOC IS SUFFICIENT FOR THE RELEASE
# GATE AND FOR THE LIVE TIER, WHICH RUN THE ENGINE ON THE MACHINE THAT BUILT IT.
# IT IS NOT SUFFICIENT FOR A SHIPPED .pkg: a distributed binary needs a real
# Developer ID identity and notarisation, which is milestone 4's work with the
# rest of packaging. Do not read this line as that being done.
#
# `--options runtime --timestamp` matches the invocation Arca's own Makefile
# codesign target uses, so switching `-` for a Developer ID here is the only
# edit that migration needs. Both flags are inert for an ad-hoc signature, which
# carries no CMS blob to timestamp: MEASURED, `codesign -dvvv` reports the same
# `flags=0x10002(adhoc,runtime)` and no Timestamp field with and without.
entitlements=$checkout/Arca.entitlements
[[ -f $entitlements ]] || {
  printf 'the pinned engine carries no entitlements file: %s\n' "$entitlements" >&2
  exit 70
}
# >&2 is defence in depth and not a fix for anything observed. Stdout is this
# script's contract with its caller -- two lines, the checkout and the binary --
# and every other command here is redirected for that reason. MEASURED: codesign
# writes `replacing existing signature` to stderr and 0 bytes to stdout, on a
# cold cache and a warm one. The redirect stays so that a codesign that starts
# saying something on stdout cannot corrupt the contract.
codesign --force --sign - --options runtime --timestamp \
  --entitlements "$entitlements" "$binary" >&2

printf '%s\n%s\n' "$checkout" "$binary"
