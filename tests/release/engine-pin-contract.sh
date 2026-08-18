#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/../.." && pwd -P)
script=$repo_root/scripts/build-arca-engine.sh
fixture=$(mktemp -d "${TMPDIR:-/tmp}/gascan-engine-pin-contract.XXXXXX")
# A second and deliberately short directory, for the one thing that cannot live
# under $fixture: the socket the engine case below binds. An AF_UNIX address is
# capped at sun_path's 104 bytes, and $fixture under a macOS TMPDIR spends ~81
# of those before the filename. MEASURED: the engine refused a 111-byte path
# with `--socket-path is 111 bytes and sun_path holds 104`. /tmp costs 4.
socket_dir=$(mktemp -d /tmp/gascan-engine-socket.XXXXXX)
trap 'rm -rf "$fixture" "$socket_dir"' EXIT

# A local signing identity, so the positive case needs no network and no real key.
ssh-keygen -q -t ed25519 -N '' -C engine@example.invalid -f "$fixture/key"
printf 'engine@example.invalid %s\n' "$(cat "$fixture/key.pub")" >"$fixture/allowed-signers"

# A second identity that is never written to allowed-signers. It makes "validly
# signed by a key outside the trust anchor" expressible, which is the property
# the anchor exists to enforce and which the unsigned-tag case does not reach.
ssh-keygen -q -t ed25519 -N '' -C intruder@example.invalid -f "$fixture/intruder"

# A nested repository standing in for Arca's containerization submodule. Arca
# consumes that submodule as a SwiftPM path dependency, so this one is wired up
# the same way: anything left in its sources reaches the compiler, which is what
# makes contamination inside a submodule matter rather than merely be untidy.
subupstream=$fixture/subupstream
mkdir -p "$subupstream/Sources/EngineSupport"
cat >"$subupstream/Package.swift" <<'PACKAGE'
// swift-tools-version: 6.2
import PackageDescription
let package = Package(
    name: "EngineSupport",
    products: [.library(name: "EngineSupport", targets: ["EngineSupport"])],
    targets: [.target(name: "EngineSupport")]
)
PACKAGE
printf 'public let engineSupportFixture = 1\n' >"$subupstream/Sources/EngineSupport/Support.swift"
git -C "$subupstream" init -q
git -C "$subupstream" config user.name fixture
git -C "$subupstream" config user.email engine@example.invalid
git -C "$subupstream" add -A
git -C "$subupstream" -c commit.gpgsign=false commit -qm seed

# An upstream repository standing in for Arca. It carries a Package.swift naming
# everything the build script names: the arca-engine executable product it builds,
# the SandboxEngineProto target it builds beside it, and the two test targets its
# test gate filters on. A fixture that declares fewer targets than the script
# builds does not exercise the script; it just fails differently.
#
# No `products:` array, matching Arca (Package.swift:11-34): `swift build --product
# arca-engine` has to resolve through SwiftPM's implicit executable product here for
# the same reason it does there, or the contract would exercise a shape production
# does not have.
#
# Package.swift and the prune suite are written through functions, and the suite
# names are arguments, because the negative cases below need a tree in which
# exactly one of the two gated suites is absent. Substituting into a quoted
# heredoc rather than interpolating one: the comments carry backticks, which an
# unquoted heredoc would run as commands.
upstream=$fixture/upstream
mkdir -p "$upstream/Sources/ContainerBridge" "$upstream/Sources/SandboxEngineProto" \
  "$upstream/Sources/arca-engine" "$upstream/Tests/ArcaEngineTests" \
  "$upstream/Tests/ArcaTests"
write_package() {
  sed -e "s/@ENGINE_TESTS@/$1/" -e "s/@ARCA_TESTS@/$2/" >"$upstream/Package.swift" <<'PACKAGE'
// swift-tools-version: 6.2
import PackageDescription
let package = Package(
    name: "Arca",
    dependencies: [.package(path: "containerization")],
    targets: [
        .target(
            name: "ContainerBridge",
            dependencies: [.product(name: "EngineSupport", package: "containerization")]
        ),
        // Stands in for Arca's generated engine-contract server code. Dependency
        // free on purpose: the contract under test is that the pin builds the
        // target, not what the generated code imports.
        .target(name: "SandboxEngineProto"),
        // The artifact the script actually ships. It reaches ContainerBridge, and
        // through it the submodule, so the executable build is what pulls the
        // planted-contamination proof's sources through the compiler.
        .executableTarget(
            name: "arca-engine",
            dependencies: ["ContainerBridge", "SandboxEngineProto"]
        ),
        .testTarget(name: "@ENGINE_TESTS@", dependencies: ["ContainerBridge"]),
        // Stands in for Arca's ArcaTests, which owns the DockerAPI-side prune
        // gate test. A separate target and not another class in the engine suite:
        // the whole reason the gate names two suites is that these are two
        // targets, and a fixture with one would not exercise that.
        .testTarget(name: "@ARCA_TESTS@", dependencies: ["ContainerBridge"])
    ]
)
PACKAGE
}
write_package ArcaEngineTests ArcaTests
printf 'public let engineFixture = 1\n' >"$upstream/Sources/ContainerBridge/Fixture.swift"
printf 'public let sandboxEngineProtoFixture = 1\n' >"$upstream/Sources/SandboxEngineProto/Fixture.swift"

# Arca's gen-buildinfo target, in the shape the build script now runs. The
# revision expression is an argument so that "the generator lies about which
# tree it built" is expressible below -- which is the only way to show that the
# script's assertion is load-bearing rather than a tautology. In the honest
# form, `git rev-parse HEAD` inside a detached checkout of the pinned revision
# can only ever yield the pinned revision, so an honest fixture proves nothing
# about the assertion.
#
# A tab-indented recipe, written through a quoted heredoc: make requires the
# tab, and the printf carries a literal one.
write_buildinfo_makefile() {
  printf 'REVISION = %s\n' "$1" >"$upstream/Makefile"
  cat >>"$upstream/Makefile" <<'MAKEFILE'
.PHONY: gen-buildinfo
gen-buildinfo:
	@echo "Generating build info..."
	@printf '// AUTO-GENERATED FILE - DO NOT EDIT\n' > Sources/ContainerBridge/BuildInfo.generated.swift
	@printf 'public struct ArcaBuildInfo {\n' >> Sources/ContainerBridge/BuildInfo.generated.swift
	@printf '    public static let buildRevision = "%s"\n' '$(REVISION)' >> Sources/ContainerBridge/BuildInfo.generated.swift
	@printf '}\n' >> Sources/ContainerBridge/BuildInfo.generated.swift
MAKEFILE
}
write_buildinfo_makefile '$(shell git rev-parse HEAD)'

# The committed BuildInfo.generated.swift is DELIBERATELY STALE, and it is
# tracked, because that is the state the real one was found in: Arca's recorded
# 5e1170495400b25f6334c6d8ddda5d3521b7cfd8 while the tag being pinned was
# c545612b056e028d5885968a7b9f586d694f994c, and before that it drifted through a
# whole milestone unnoticed. A fixture that committed the correct revision would
# pass whether or not the script regenerates anything.
#
# Forty f's, so it is a well-formed revision that no tree can ever have: a stale
# value that happened to match some other fixture commit would make a passing
# case ambiguous.
cat >"$upstream/Sources/ContainerBridge/BuildInfo.generated.swift" <<'BUILDINFO'
// AUTO-GENERATED FILE - DO NOT EDIT
public struct ArcaBuildInfo {
    public static let buildRevision = "ffffffffffffffffffffffffffffffffffffffff"
}
BUILDINFO
# The entitlements the build script signs with, under the name Arca gives them
# (Arca's Makefile: `ENTITLEMENTS = Arca.entitlements`), because the script reads
# them out of the checkout by that name. One key and not the five Arca carries:
# this is the one the engine cannot start a container without, and a fixture
# copy of the other four would be four more things to keep in step for nothing.
cat >"$upstream/Arca.entitlements" <<'ENTITLEMENTS'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>com.apple.security.virtualization</key>
    <true/>
</dict>
</plist>
ENTITLEMENTS
# The fixture engine refuses to serve without the entitlement the build script
# signs it with, and refuses BEFORE it creates its socket -- which is the shape
# of the real failure it stands in for. ContainerManager.initialize() constructs
# a Containerization.VmnetNetwork; unentitled, the real engine exits 1 on
# `vmnet_return_t(rawValue: 1002)` with no socket ever created.
#
# vmnet is NOT called here, deliberately. MEASURED on a standalone probe:
# vmnet_start_interface answers 1001 from a `swift build` binary and 1000 from
# the same binary ad-hoc signed with these entitlements -- so it would work as a
# probe on this machine. But a hosted runner that cannot create a vmnet
# interface at all would then fail this case for a reason with nothing to do
# with the signature, and a release gate that goes red for the environment is a
# gate people learn to ignore. What is asked instead is the kernel's view of
# THIS process's entitlements, which is environment-independent and which
# nothing but a real signature can satisfy -- not a `codesign -d` reading of the
# file on disk, which would prove the command ran rather than that it worked.
#
# It binds and exits rather than serving: the property is that the engine got
# far enough to create its socket, and a fixture that stayed up would need the
# case below to background it and reap it for no gain.
cat >"$upstream/Sources/arca-engine/main.swift" <<'MAIN'
import ContainerBridge
import Darwin
import Foundation
import SandboxEngineProto
import Security

func refuse(_ message: String, _ status: Int32) -> Never {
    FileHandle.standardError.write(Data("arca-engine: \(message)\n".utf8))
    exit(status)
}

let entitlement = "com.apple.security.virtualization"
guard let task = SecTaskCreateFromSelf(nil),
      SecTaskCopyValueForEntitlement(task, entitlement as CFString, nil) != nil
else {
    refuse("this process holds no \(entitlement); refusing to serve", 1)
}

var arguments = CommandLine.arguments.dropFirst().makeIterator()
var requested: String?
while let argument = arguments.next() {
    if argument == "--socket-path" { requested = arguments.next() }
}
guard let socketPath = requested else {
    refuse("no --socket-path", 64)
}

var address = sockaddr_un()
address.sun_family = sa_family_t(AF_UNIX)
let capacity = MemoryLayout.size(ofValue: address.sun_path)
let pathBytes = Array(socketPath.utf8)
guard pathBytes.count < capacity else {
    refuse("--socket-path is \(pathBytes.count) bytes and sun_path holds \(capacity)", 64)
}
withUnsafeMutableBytes(of: &address.sun_path) { destination in
    destination.copyBytes(from: pathBytes)
    destination[pathBytes.count] = 0
}

let descriptor = socket(AF_UNIX, SOCK_STREAM, 0)
guard descriptor >= 0 else { refuse("socket() failed with errno \(errno)", 70) }
let bound = withUnsafePointer(to: &address) { pointer in
    pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) {
        bind(descriptor, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
    }
}
guard bound == 0 else { refuse("bind() failed with errno \(errno)", 70) }
guard listen(descriptor, 1) == 0 else { refuse("listen() failed with errno \(errno)", 70) }

print(engineFixture + sandboxEngineProtoFixture)
MAIN
# A real assertion and not an empty test body: the script's gate proves the pinned
# engine passes its own suite, so a fixture whose suite cannot fail would leave the
# gate's failing direction unexercised.
#
# Each suite asserts against a value the caller supplies, and each carries a
# message naming ITSELF. Both halves are needed to pin execution rather than
# listing: `expected` is what lets a tree be built in which exactly one of the two
# suites fails, and the distinct message is what proves the failure came from that
# suite and not from the guard or the other one. Identical assertions in both --
# which is what these were -- would make the two indistinguishable in the output,
# and a `--filter` that had stopped selecting the second suite would look exactly
# like one that still did.
write_engine_suite() {
  sed -e "s/@EXPECTED@/$1/" >"$upstream/Tests/ArcaEngineTests/EngineFixtureTests.swift" <<'TEST'
import ContainerBridge
import XCTest

final class EngineFixtureTests: XCTestCase {
    func testTheEngineSuiteRan() {
        XCTAssertEqual(engineFixture, @EXPECTED@, "the ArcaEngineTests half of the gate ran")
    }
}
TEST
}
write_engine_suite 1
# XCTest and not swift-testing, matching the suite it stands in for: the script
# passes --disable-swift-testing, so a `@Test` here would be listed by nothing and
# run by nothing, and the fixture would pass while proving the opposite.
write_prune_suite() {
  sed -e "s/@PRUNE_SUITE@/$1/" -e "s/@EXPECTED@/$2/" \
    >"$upstream/Tests/ArcaTests/NetworkPruneGateTests.swift" <<'TEST'
import ContainerBridge
import XCTest

final class @PRUNE_SUITE@: XCTestCase {
    func testThePruneSuiteRan() {
        XCTAssertEqual(
            engineFixture, @EXPECTED@,
            "the ArcaTests.NetworkPruneGateTests half of the gate ran"
        )
    }
}
TEST
}
write_prune_suite NetworkPruneGateTests 1
git -C "$upstream" init -q
git -C "$upstream" config user.name fixture
git -C "$upstream" config user.email engine@example.invalid
git -C "$upstream" config gpg.format ssh
git -C "$upstream" config user.signingKey "$fixture/key"
# protocol.file.allow is scoped to this one command and belongs to the fixture,
# not the contract: git refuses file-transport submodules by default since
# CVE-2022-39253, and the fixture has nowhere but the filesystem to live.
git -C "$upstream" -c protocol.file.allow=always \
  submodule add -q "$subupstream" containerization
git -C "$upstream" add -A
git -C "$upstream" -c commit.gpgsign=false commit -qm seed
pinned=$(git -C "$upstream" rev-parse --verify HEAD)
git -C "$upstream" tag -s -m 'engine baseline' engine-baseline "$pinned"

# A second commit, so "tag points somewhere else" is expressible.
printf 'public let drift = 2\n' >"$upstream/Sources/ContainerBridge/Drift.swift"
git -C "$upstream" add -A
git -C "$upstream" -c commit.gpgsign=false commit -qm drift
drifted=$(git -C "$upstream" rev-parse --verify HEAD)
git -C "$upstream" tag -s -m 'moved' moved-tag "$drifted"
git -C "$upstream" tag unsigned-tag "$pinned"
git -C "$upstream" -c "user.signingKey=$fixture/intruder" \
  tag -s -m 'intruder' wrong-signer-tag "$pinned"

# The two halves of the resolution-ambiguity attack, described at the verify-tag
# call in scripts/build-arca-engine.sh. Git resolves a bare name by trying
# $GIT_DIR/<name>, then refs/<name>, and only then refs/tags/<name>, so an
# unqualified name in the signature gate and a refs/tags/ name in the identity
# gate can land on two different objects.
#
# Slash half: a signed refs/tags/ambiguous on the good commit, and an unsigned
# lightweight refs/tags/tags/ambiguous on the drifted one. A pin naming
# "tags/ambiguous" with the drifted revision satisfies both gates while the
# signature that was checked belongs to an object with no relation to the tree.
# `git fetch --tags` brings both refs down, so this needs no local write access.
git -C "$upstream" tag -s -m 'ambiguity bait' ambiguous "$pinned"
git -C "$upstream" tag tags/ambiguous "$drifted"

# Shadow half: an unsigned lightweight tag on the drifted commit, whose bare name
# a planted refs/<name> in the cache can shadow. Needs no slash at all.
git -C "$upstream" tag shadowed "$drifted"

# Two commits in which exactly one of the two gated suites is absent, so both
# halves of the listing guard can be shown to fire, and to fire naming the suite
# that went. Everything else about each tree is well-formed -- the pin, the
# signature, the products, the other suite -- so the only thing left to fail on
# is the guard.
#
# A guard that fired only for the suite it was first written for would be the
# defect it exists to prevent, moved one suite over: `swift test --filter` exits
# 0 when it matches nothing, so the gate would report success having run half of
# what it names.
#
# The renames are past the anchor in each pattern, not before it: the engine half
# keeps the `ArcaEngineTests` prefix and loses the `.` after it, and the prune
# half keeps its target and renames only the class. A guard grepping for bare
# substrings would pass both of these.
git -C "$upstream" mv Tests/ArcaEngineTests Tests/ArcaEngineTestsRenamed
write_package ArcaEngineTestsRenamed ArcaTests
git -C "$upstream" add -A
git -C "$upstream" -c commit.gpgsign=false commit -qm 'engine suite renamed'
engine_suite_gone=$(git -C "$upstream" rev-parse --verify HEAD)
git -C "$upstream" tag -s -m 'engine suite renamed' engine-suite-gone "$engine_suite_gone"

git -C "$upstream" mv Tests/ArcaEngineTestsRenamed Tests/ArcaEngineTests
write_package ArcaEngineTests ArcaTests
write_prune_suite NetworkPruneGateTestsRenamed 1
git -C "$upstream" add -A
git -C "$upstream" -c commit.gpgsign=false commit -qm 'prune suite renamed'
prune_suite_gone=$(git -C "$upstream" rev-parse --verify HEAD)
git -C "$upstream" tag -s -m 'prune suite renamed' prune-suite-gone "$prune_suite_gone"

# Two more trees, in which both suites are present and listed and exactly one of
# them FAILS. These pin that the script EXECUTES each suite, which the listing
# guard cannot: the guard proves a suite was declared, and a filter that selected
# only one of the two would satisfy it exactly as well as one that selected both.
#
# The concrete decay this closes: `swift test` unions repeated --filter today,
# MEASURED on Swift 6.3.3 at 46 tests against the real Arca tree. CI runs macos-26
# with an unpinned toolchain (.github/workflows/ci.yml:80), so that is not the
# Swift that gates releases. A SwiftPM in which the last --filter wins instead
# would leave every other case here green while the gate ran 3 of 46 tests.
#
# Both directions, not just the second suite: a case that only ever fired for the
# prune half would leave the engine half's execution resting on the same argument
# it just refused to accept for the other one.
# The prune class is restored here: the commit above left it renamed, and a tree
# missing a suite would trip the listing guard long before anything ran.
write_prune_suite NetworkPruneGateTests 1
write_engine_suite 2
git -C "$upstream" add -A
git -C "$upstream" -c commit.gpgsign=false commit -qm 'engine suite fails'
engine_suite_fails=$(git -C "$upstream" rev-parse --verify HEAD)
git -C "$upstream" tag -s -m 'engine suite fails' engine-suite-fails "$engine_suite_fails"

write_engine_suite 1
write_prune_suite NetworkPruneGateTests 2
git -C "$upstream" add -A
git -C "$upstream" -c commit.gpgsign=false commit -qm 'prune suite fails'
prune_suite_fails=$(git -C "$upstream" rev-parse --verify HEAD)
git -C "$upstream" tag -s -m 'prune suite fails' prune-suite-fails "$prune_suite_fails"

# A tree that is well-formed in every way but one: it declares no
# Arca.entitlements. The pin verifies, both suites are present and pass, the
# products build -- so the only thing left to refuse it is the signing step's
# guard, and a script that had quietly stopped signing would print a path here
# instead.
#
# Arca owns that file and Gas Can does not review its renames, so this is the
# same class of decay as the listing guard above, one repository over. The
# prune suite is restored first because the commit above left it failing, and a
# failing suite would end the run a long way before the guard.
write_prune_suite NetworkPruneGateTests 1
git -C "$upstream" rm -q Arca.entitlements
git -C "$upstream" add -A
git -C "$upstream" -c commit.gpgsign=false commit -qm 'entitlements gone'
entitlements_gone=$(git -C "$upstream" rev-parse --verify HEAD)
git -C "$upstream" tag -s -m 'entitlements gone' entitlements-gone "$entitlements_gone"

# Two trees for the build-revision assertion, which is the load-bearing half of
# regenerating BuildInfo.generated.swift: regeneration alone only changes WHICH
# unverified value gets compiled.
#
# Arca.entitlements is restored from the pinned commit first, because the tree
# above deleted it and the signing gate would otherwise end these runs before
# the assertion they exist to reach.
git -C "$upstream" checkout -q "$pinned" -- Arca.entitlements

# A generator that lies about which tree it built. This is the ONLY way to
# exercise the assertion: in an honest tree `git rev-parse HEAD` inside a
# detached checkout of the pinned revision can only ever return the pinned
# revision, so an honest fixture would pass with the assertion deleted.
#
# A well-formed 40-character revision that is not the pinned one, and not any
# other commit in this fixture, so a pass cannot be an accident of collision.
write_buildinfo_makefile 0123456789abcdef0123456789abcdef01234567
git -C "$upstream" add -A
git -C "$upstream" -c commit.gpgsign=false commit -qm 'buildinfo lies'
buildinfo_lies=$(git -C "$upstream" rev-parse --verify HEAD)
git -C "$upstream" tag -s -m 'buildinfo lies' buildinfo-lies "$buildinfo_lies"

# No generator at all. `make gen-buildinfo` fails outright, which is a different
# failure from a generator that runs and produces the wrong answer -- and it is
# the one that happens the day Arca renames the target. It has to be loud: a
# script that shrugged here would silently fall back to compiling whatever stale
# constant is committed, which is precisely the state this task found.
git -C "$upstream" rm -q Makefile
git -C "$upstream" add -A
git -C "$upstream" -c commit.gpgsign=false commit -qm 'buildinfo generator gone'
buildinfo_gone=$(git -C "$upstream" rev-parse --verify HEAD)
git -C "$upstream" tag -s -m 'buildinfo generator gone' buildinfo-gone "$buildinfo_gone"

# The artifact block is constant across every case here. This script gates
# scripts/build-arca-engine.sh, which compiles a tree and never downloads an
# asset, so no case below turns on an artifact's digest -- but schema 2 requires
# the block, and a fixture that omitted it would fail every case for the wrong
# reason. The digests are the published gascan-engine-m4 ones so that a reader
# comparing this file to engine/arca-pin.json sees the same values; nothing here
# depends on that, and write_pin_artifacts exists so a case CAN vary them.
pin_artifacts() {
  jq -n --arg asset_url "file://$upstream" '{
    kernel: {
      asset: "vmlinux-arm64.gz",
      url: ($asset_url + "/vmlinux-arm64.gz"),
      bytes: 9092349,
      sha256: "8a30e10d9e40dcc44396049753a3a26be74cbc77a78afca819cf8f1c13f8597a",
      content: {
        kind: "gzip-member",
        bytes: 28248576,
        sha256: "49e0f08165409769e5ae2abbe3414198c2907a15e7e20a5f3971aa7a0de33394"
      }
    },
    vminit: {
      asset: "vminit-oci-arm64.tar.gz",
      url: ($asset_url + "/vminit-oci-arm64.tar.gz"),
      bytes: 73739738,
      sha256: "51602e72883e49e4be1e27a690bf8c13b0a66cba381725cf8ea4888ec4e369be",
      content: {
        kind: "oci-manifest",
        bytes: 478,
        sha256: "cf74cd41bd430d9d8935d36c1749d9c05f19a43842f4a4cff0d01de3832222c2"
      }
    }
  }'
}

# file:// and not a bare path: the script constrains .url to schemes git cannot
# turn into a command, so the fixture must speak one of them. git clone accepts
# file:// against a local path unchanged.
write_pin() {
  jq -n --arg url "file://$upstream" --arg tag "$2" --arg rev "$3" \
    --argjson artifacts "$(pin_artifacts)" \
    '{schema: 2, name: "arca", url: $url, tag: $tag, revision: $rev, artifacts: $artifacts}' >"$1"
}

run_case() {
  # `actual=0; ... || actual=$?` and not a bare `$?` on the next line: this file
  # runs under `set -e`, so a non-zero exit would abort the test before the
  # status could be read, and every negative case would vanish silently.
  local label=$1 pin=$2 expected=$3 signers=${4:-$fixture/allowed-signers} actual=0
  # GIT_CONFIG_* carries protocol.file.allow into the script's git calls, which
  # need it to fetch the fixture's file-transport submodule. It is set here and
  # not in the script on purpose: the relaxation exists because the fixture is on
  # the filesystem, and the production pin fetches everything over https.
  GASCAN_ARCA_PIN_FILE=$pin \
  GASCAN_ARCA_ENGINE_CACHE=$fixture/cache-$label \
  GASCAN_ARCA_ALLOWED_SIGNERS=$signers \
  GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=protocol.file.allow GIT_CONFIG_VALUE_0=always \
    bash "$script" >"$fixture/$label.out" 2>&1 || actual=$?
  # `nonzero` and not a pinned code for the failing-suite cases below: the exit
  # there is `swift test`'s own, which this repository does not define and a
  # toolchain is free to change. Those cases assert WHICH suite reported the
  # failure from the captured output, which is the property, rather than pinning
  # a number that would make the contract brittle for no gain.
  if [[ $expected == nonzero ]]; then
    [[ $actual != 0 ]] || {
      printf 'case %s: expected a non-zero exit, got 0\n' "$label" >&2
      cat "$fixture/$label.out" >&2
      exit 1
    }
  else
    [[ $actual == "$expected" ]] || {
      printf 'case %s: expected exit %s, got %s\n' "$label" "$expected" "$actual" >&2
      cat "$fixture/$label.out" >&2
      exit 1
    }
  fi
}

# The well-formed pin, written up front because the missing-file cases below
# need a pin that is beyond reproach in order to isolate what they are testing.
write_pin "$fixture/pin-good.json" engine-baseline "$pinned"

# 64 — malformed pin
write_pin "$fixture/pin-short.json" engine-baseline deadbeef
run_case short-revision "$fixture/pin-short.json" 64

jq -n '{schema: 2, name: "arca", url: "x", tag: "y"}' >"$fixture/pin-nokey.json"
run_case missing-revision "$fixture/pin-nokey.json" 64

# 64 — a .url git would execute rather than fetch. ext:: runs its argument as a
# command, so an unconstrained URL is arbitrary execution at clone time.
jq -n --arg rev "$pinned" --argjson artifacts "$(pin_artifacts)" \
  '{schema: 2, name: "arca", url: "ext::sh -c touch% /dev/null", tag: "engine-baseline", revision: $rev, artifacts: $artifacts}' \
  >"$fixture/pin-exec-url.json"
run_case exec-url "$fixture/pin-exec-url.json" 64

# 64 — the schema-2 artifact block, one refusal per clause that can go wrong
# without being obviously wrong. Every case starts from pin-good.json and edits
# ONE field, so a case that stops failing names the clause that stopped holding
# rather than "something about artifacts".
#
# These gate a block this script never reads: build-arca-engine.sh compiles a
# tree and downloads no asset. They are here anyway because the schema file is
# shared with scripts/sync-arca-proto.sh and with the fetch, and this is the
# contract that runs it. A digest the fetch would reject must be refused at pin
# validation, where the message names the pin file, and not after ~83MB.
write_artifact_pin() {
  local label=$1 filter=$2
  jq "$filter" "$fixture/pin-good.json" >"$fixture/pin-$label.json"
  run_case "$label" "$fixture/pin-$label.json" 64
}
write_artifact_pin artifacts-absent 'del(.artifacts)'
write_artifact_pin artifacts-array '.artifacts = []'
# One artifact present and the other missing. A schema that checked
# `.artifacts | type == "object"` and stopped would pass this, and the fetch
# would then fail at the point of use with the pin already accepted.
write_artifact_pin vminit-absent 'del(.artifacts.vminit)'
write_artifact_pin kernel-absent 'del(.artifacts.kernel)'
# A truncated download is refused by byte length before its digest is computed,
# which is the cheaper and clearer of the two refusals -- so the length has to
# be a length. 0, negative and fractional each pass `type == "number"`.
write_artifact_pin bytes-zero '.artifacts.kernel.bytes = 0'
write_artifact_pin bytes-negative '.artifacts.kernel.bytes = -1'
write_artifact_pin bytes-fractional '.artifacts.kernel.bytes = 9092349.5'
write_artifact_pin bytes-string '.artifacts.kernel.bytes = "9092349"'
# Uppercase hex is the realistic malformed digest: shasum prints lowercase and
# every comparison downstream is a string compare, so a pasted uppercase digest
# would match nothing while looking correct to a reader.
write_artifact_pin sha256-uppercase '.artifacts.kernel.sha256 |= ascii_upcase'
write_artifact_pin sha256-short '.artifacts.kernel.sha256 = "8a30e10d"'
# .content is the identity that survives repackaging. `tar czf` is not
# reproducible, so the vminit asset's own sha256 dies the moment anyone
# repackages the same bytes; a pin that lost .content would still verify a fresh
# download and would silently stop being able to say WHAT it downloaded.
write_artifact_pin content-absent 'del(.artifacts.vminit.content)'
write_artifact_pin content-bytes-zero '.artifacts.vminit.content.bytes = 0'
write_artifact_pin content-sha256-short '.artifacts.vminit.content.sha256 = "cf74cd41"'
# An unrecognised kind is a content check no consumer can perform. Refusing it
# here fails closed; accepting it would defer the failure to a fetch that has
# already spent the download.
write_artifact_pin content-kind-unknown '.artifacts.vminit.content.kind = "zip"'
write_artifact_pin content-kind-absent 'del(.artifacts.vminit.content.kind)'
# .asset names the file on disk, so a path in it escapes the artifact directory.
write_artifact_pin asset-traversal '.artifacts.kernel.asset = "../vmlinux-arm64.gz"'
write_artifact_pin asset-slash '.artifacts.kernel.asset = "sub/vmlinux-arm64.gz"'
# The asset URL is validated separately from the git URL -- a release asset is
# not served by the git endpoint -- so it needs its own refusal of a scheme that
# is a command rather than a transport.
write_artifact_pin asset-url-exec '.artifacts.kernel.url = "ext::sh -c touch% /dev/null"'

# 64 — schema 1 itself. The bump is not cosmetic: a schema-1 pin carries no
# digests at all, so accepting one would mean a build gated on a pin the fetch
# cannot use. This is also the case that fails if only ONE of the two scripts is
# moved to schema 2, which is the mistake the shared schema file exists to make
# impossible.
write_artifact_pin schema-1 '.schema = 1'

# 64 — neither file exists; both are one mistyped environment variable away.
run_case missing-pin-file "$fixture/pin-does-not-exist.json" 64
run_case missing-allowed-signers "$fixture/pin-good.json" 64 "$fixture/no-such-signers"

# 65 — tag resolves to a different commit than the pin
write_pin "$fixture/pin-moved.json" moved-tag "$pinned"
run_case moved-tag "$fixture/pin-moved.json" 65

# 65 — tag carries no signature
write_pin "$fixture/pin-unsigned.json" unsigned-tag "$pinned"
run_case unsigned-tag "$fixture/pin-unsigned.json" 65

# 65 — tag carries a good signature from a key that is not in allowed-signers.
# unsigned-tag only reaches "no signature at all", which a script that skipped
# the trust anchor entirely would still reject. This case is the one that fails
# when the anchor is widened rather than removed, which is the likelier mistake.
write_pin "$fixture/pin-wrong-signer.json" wrong-signer-tag "$pinned"
run_case wrong-signer "$fixture/pin-wrong-signer.json" 65

# 65 — pinned revision absent from the repository
write_pin "$fixture/pin-absent.json" engine-baseline 0000000000000000000000000000000000000000
run_case absent-revision "$fixture/pin-absent.json" 65

# 65 — the compiled build revision is not the pinned revision. Capabilities
# field 20 carries this constant and gascan-arca decides Proven versus
# Unverified by comparing it against a certified one, so an engine that
# self-reports a revision unrelated to the tree it was built from makes that
# gate worth nothing: it matches nothing in the safe case and the wrong tree in
# the unsafe one.
#
# 65 and not 70: the tree is intact and the build could proceed. What is wrong
# is the pinned input's own claim about itself, which is the same class as a tag
# resolving to the wrong commit.
write_pin "$fixture/pin-buildinfo-lies.json" buildinfo-lies "$buildinfo_lies"
run_case buildinfo-lies "$fixture/pin-buildinfo-lies.json" 65
grep -q '0123456789abcdef0123456789abcdef01234567' "$fixture/buildinfo-lies.out" || {
  printf 'case buildinfo-lies: the refusal did not name the revision that was compiled\n' >&2
  cat "$fixture/buildinfo-lies.out" >&2
  exit 1
}

# 70 — the pinned tree has no build-info generator. The failure mode this
# refuses is silence: a script that tolerated a missing generator would compile
# whatever stale constant is committed, which is the exact state Arca's tree was
# found in and the reason this step exists.
write_pin "$fixture/pin-buildinfo-gone.json" buildinfo-gone "$buildinfo_gone"
run_case buildinfo-gone "$fixture/pin-buildinfo-gone.json" 70

# 64 — a .tag that is a path. Upstream really does carry the tag pair this names,
# so the refusal is the pin schema's and not an accident of the fixture: every
# other gate would pass. Rejected before any git command runs, which is why the
# code is 64 and not 65.
write_pin "$fixture/pin-slash.json" tags/ambiguous "$drifted"
run_case slash-tag "$fixture/pin-slash.json" 64

# 65 — a ref planted at refs/<tag> in the warm cache must not be what gets its
# signature checked. This is the half the .tag constraint above cannot reach: the
# name carries no slash, so the pin is well-formed, and only the refs/tags/
# qualification in the signature gate refuses it.
#
# Two runs against one cache. The first is a plain rejection whose only job is to
# leave a clone behind -- the script clones before it verifies -- and the second
# runs against that clone with the shadowing ref in place. Without the
# qualification the second run exits 0 having compiled the drifted commit, which
# is the whole defect stated as an exit code.
write_pin "$fixture/pin-shadow.json" shadowed "$drifted"
run_case shadowed-ref "$fixture/pin-shadow.json" 65
git -C "$fixture/cache-shadowed-ref/arca" update-ref refs/shadowed \
  "$(git -C "$upstream" rev-parse --verify refs/tags/ambiguous)"
run_case shadowed-ref "$fixture/pin-shadow.json" 65

# 70 — the listing guard, each half proven on its own. Nothing else about either
# tree is wrong: the pin is well-formed, the tag verifies and resolves, the
# products build, and the other suite is present and passing. The only thing
# left to fail on is the suite the gate names and the tree does not have.
#
# The message is asserted and not just the exit code, anchored at end of line so
# each half is distinguishable from the other. `swift test --filter` exits 0 when
# it matches nothing, so a gate naming two suites and checking one would report a
# success it had not earned -- and an operator reading "declares no
# ArcaEngineTests" while ArcaEngineTests is right there would be sent to the
# wrong repository.
write_pin "$fixture/pin-engine-gone.json" engine-suite-gone "$engine_suite_gone"
run_case engine-suite-gone "$fixture/pin-engine-gone.json" 70
grep -q 'declares no ArcaEngineTests$' "$fixture/engine-suite-gone.out" || {
  printf 'the listing guard did not name the missing ArcaEngineTests\n' >&2
  cat "$fixture/engine-suite-gone.out" >&2
  exit 1
}

write_pin "$fixture/pin-prune-gone.json" prune-suite-gone "$prune_suite_gone"
run_case prune-suite-gone "$fixture/pin-prune-gone.json" 70
grep -q 'declares no ArcaTests\.NetworkPruneGateTests$' "$fixture/prune-suite-gone.out" || {
  printf 'the listing guard did not name the missing ArcaTests.NetworkPruneGateTests\n' >&2
  cat "$fixture/prune-suite-gone.out" >&2
  exit 1
}

# non-zero — each filtered suite is EXECUTED, proven one half at a time. Both
# suites are present and listed in these two trees, so the listing guard passes
# and the only thing left to fail on is the test run itself.
#
# The listing guard cannot reach this. It proves a suite was declared; a --filter
# that had stopped selecting one of the two would satisfy it exactly as well as
# one that still selected both, and every other case in this file would stay
# green while the release gate ran a fraction of what it names.
#
# Three assertions per case, and all three are needed: a non-zero exit, the
# failing suite's own message present, and the listing guard's message ABSENT.
# Without the third, a case could pass on exit 70 -- the guard firing for an
# unrelated reason -- having never run a test at all.
assert_failing_suite_reported() {
  local label=$1 message=$2
  grep -q "$message" "$fixture/$label.out" || {
    printf 'case %s: the failing suite did not report itself: %s\n' "$label" "$message" >&2
    cat "$fixture/$label.out" >&2
    exit 1
  }
  ! grep -q 'the test gate matched no tests' "$fixture/$label.out" || {
    printf 'case %s: failed at the listing guard, so no suite was ever run\n' "$label" >&2
    cat "$fixture/$label.out" >&2
    exit 1
  }
}

write_pin "$fixture/pin-engine-fails.json" engine-suite-fails "$engine_suite_fails"
run_case engine-suite-fails "$fixture/pin-engine-fails.json" nonzero
assert_failing_suite_reported engine-suite-fails 'the ArcaEngineTests half of the gate ran'

write_pin "$fixture/pin-prune-fails.json" prune-suite-fails "$prune_suite_fails"
run_case prune-suite-fails "$fixture/pin-prune-fails.json" nonzero
assert_failing_suite_reported prune-suite-fails \
  'the ArcaTests.NetworkPruneGateTests half of the gate ran'

# 70 — the pinned engine declares no entitlements file, so the script cannot
# sign what it built. It must refuse rather than print an unsigned path: an
# engine signed without com.apple.security.virtualization exits on the first
# vmnet call and never serves, and the caller would meet that a long way from
# here.
write_pin "$fixture/pin-entitlements-gone.json" entitlements-gone "$entitlements_gone"
run_case entitlements-gone "$fixture/pin-entitlements-gone.json" 70
grep -q 'carries no entitlements file' "$fixture/entitlements-gone.out" || {
  printf 'the entitlements guard did not name the file it could not find\n' >&2
  cat "$fixture/entitlements-gone.out" >&2
  exit 1
}

# 0 — well-formed pin, signed tag, tag resolves to the pinned revision
run_case good "$fixture/pin-good.json" 0
grep -q 'cache-good' "$fixture/good.out" || {
  printf 'success case did not print the checkout path\n' >&2
  exit 1
}

# The binary the script printed must be able to serve, and unsigned it cannot.
# The engine reaches Containerization.VmnetNetwork before it binds anything, and
# vmnet refuses without com.apple.security.virtualization -- so an unsigned
# engine exits non-zero having created no socket, which is what this asserts
# against. The fixture engine refuses on the same terms and in the same order
# (Sources/arca-engine/main.swift above), so deleting the codesign step from
# scripts/build-arca-engine.sh turns this case red.
#
# Behavioural on purpose. A `codesign -d --entitlements -` reading of the file
# proves the command ran, not that the process got the capability, so it would
# stay green on a binary that cannot start.
#
# `tail -1` because the script prints the checkout first and the binary second.
# The socket goes in $socket_dir -- short enough for sun_path, and outside the
# cache the runs below clean.
engine_binary=$(tail -1 "$fixture/good.out")
engine_socket=$socket_dir/engine.sock
engine_status=0
"$engine_binary" --socket-path "$engine_socket" >"$fixture/engine-run.out" 2>&1 ||
  engine_status=$?
[[ $engine_status == 0 ]] || {
  printf 'the engine the script printed refused to serve: exit %s\n' "$engine_status" >&2
  cat "$fixture/engine-run.out" >&2
  exit 1
}
[[ -S $engine_socket ]] || {
  printf 'the engine the script printed created no socket at %s\n' "$engine_socket" >&2
  cat "$fixture/engine-run.out" >&2
  exit 1
}

# The cache is warm now, which is the state a release machine is always in. The
# script verifies a tag but compiles a worktree, so the worktree must be proven
# to be that tag's tree. Plant every kind of contamination a plain --detach onto
# an already-current revision would preserve, then reuse the same cache label so
# the second run sees the warm cache, and require all of it to be gone.
#
# The submodule is planted separately and deliberately. Nothing at the top level
# reaches inside it: `clean` skips gitlink directories, and `submodule update
# --force` restores tracked content but leaves untracked files. A submodule is
# also the larger half of the real source tree, so a guard that stops at the top
# level leaves most of the compiled bytes unproven.
warm=$fixture/cache-good/arca
printf 'public let planted = 3\n' >"$warm/Sources/ContainerBridge/Planted.swift"
printf 'public let tampered = 4\n' >>"$warm/Sources/ContainerBridge/Fixture.swift"
mkdir -p "$warm/.build"
printf 'poisoned\n' >"$warm/.build/poison"
printf 'public let submodulePlanted = 5\n' >"$warm/containerization/Sources/EngineSupport/Planted.swift"
printf 'public let submoduleTampered = 6\n' >>"$warm/containerization/Sources/EngineSupport/Support.swift"
mkdir -p "$warm/containerization/.build"
printf 'poisoned\n' >"$warm/containerization/.build/poison"
run_case good "$fixture/pin-good.json" 0
for stale in Sources/ContainerBridge/Planted.swift .build/poison \
  containerization/Sources/EngineSupport/Planted.swift containerization/.build/poison; do
  [[ ! -e $warm/$stale ]] || {
    printf 'warm cache carried an unverified file into the build: %s\n' "$stale" >&2
    exit 1
  }
done
# The build-info regeneration writes a TRACKED file inside the verified
# checkout, so "no tracked file differs after the run" is no longer the
# property. Excluding that path from `git diff --quiet` would be silencing a
# check to make a step pass; what is asserted instead is strictly stronger than
# what stood here before: EXACTLY ONE tracked file differs, it is the generated
# build info and nothing else, and it holds the PINNED revision rather than the
# deliberately stale one the fixture commits.
#
# Leaving the checkout dirty is deliberate and self-healing: the next run's
# `git checkout --detach --force` resets tracked files before anything reads
# them. The alternative -- restoring the file after the build -- would leave the
# cache claiming a revision the binary beside it does not have, which is the
# exact disagreement this whole step exists to remove.
modified=$(git -C "$warm" diff --name-only)
[[ $modified == Sources/ContainerBridge/BuildInfo.generated.swift ]] || {
  printf 'warm cache tracked modifications were not exactly the build info: %s\n' \
    "${modified:-<none>}" >&2
  exit 1
}
grep -q "buildRevision = \"$pinned\"" \
  "$warm/Sources/ContainerBridge/BuildInfo.generated.swift" || {
  printf 'the warm cache build info does not name the pinned revision %s\n' "$pinned" >&2
  cat "$warm/Sources/ContainerBridge/BuildInfo.generated.swift" >&2
  exit 1
}
git -C "$warm" submodule foreach --quiet --recursive git diff --quiet || {
  printf 'warm cache carried a tracked submodule modification into the build\n' >&2
  exit 1
}

printf 'PASS: Gas Can engine pin contract\n'
