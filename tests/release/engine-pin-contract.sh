#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/../.." && pwd -P)
script=$repo_root/scripts/build-arca-engine.sh
fixture=$(mktemp -d "${TMPDIR:-/tmp}/gascan-engine-pin-contract.XXXXXX")
trap 'rm -rf "$fixture"' EXIT

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

# An upstream repository standing in for Arca. It carries a Package.swift with
# targets named ContainerBridge and SandboxEngineProto, because those are the two
# the build script names. A fixture that declares fewer targets than the script
# builds does not exercise the script; it just fails differently.
upstream=$fixture/upstream
mkdir -p "$upstream/Sources/ContainerBridge" "$upstream/Sources/SandboxEngineProto"
cat >"$upstream/Package.swift" <<'PACKAGE'
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
        .target(name: "SandboxEngineProto")
    ]
)
PACKAGE
printf 'public let engineFixture = 1\n' >"$upstream/Sources/ContainerBridge/Fixture.swift"
printf 'public let sandboxEngineProtoFixture = 1\n' >"$upstream/Sources/SandboxEngineProto/Fixture.swift"
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

# file:// and not a bare path: the script constrains .url to schemes git cannot
# turn into a command, so the fixture must speak one of them. git clone accepts
# file:// against a local path unchanged.
write_pin() {
  jq -n --arg url "file://$upstream" --arg tag "$2" --arg rev "$3" \
    '{schema: 1, name: "arca", url: $url, tag: $tag, revision: $rev}' >"$1"
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
  [[ $actual == "$expected" ]] || {
    printf 'case %s: expected exit %s, got %s\n' "$label" "$expected" "$actual" >&2
    cat "$fixture/$label.out" >&2
    exit 1
  }
}

# The well-formed pin, written up front because the missing-file cases below
# need a pin that is beyond reproach in order to isolate what they are testing.
write_pin "$fixture/pin-good.json" engine-baseline "$pinned"

# 64 — malformed pin
write_pin "$fixture/pin-short.json" engine-baseline deadbeef
run_case short-revision "$fixture/pin-short.json" 64

jq -n '{schema: 1, name: "arca", url: "x", tag: "y"}' >"$fixture/pin-nokey.json"
run_case missing-revision "$fixture/pin-nokey.json" 64

# 64 — a .url git would execute rather than fetch. ext:: runs its argument as a
# command, so an unconstrained URL is arbitrary execution at clone time.
jq -n --arg rev "$pinned" \
  '{schema: 1, name: "arca", url: "ext::sh -c touch% /dev/null", tag: "engine-baseline", revision: $rev}' \
  >"$fixture/pin-exec-url.json"
run_case exec-url "$fixture/pin-exec-url.json" 64

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

# 0 — well-formed pin, signed tag, tag resolves to the pinned revision
run_case good "$fixture/pin-good.json" 0
grep -q 'cache-good' "$fixture/good.out" || {
  printf 'success case did not print the checkout path\n' >&2
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
git -C "$warm" diff --quiet || {
  printf 'warm cache carried a tracked modification into the build\n' >&2
  exit 1
}
git -C "$warm" submodule foreach --quiet --recursive git diff --quiet || {
  printf 'warm cache carried a tracked submodule modification into the build\n' >&2
  exit 1
}

printf 'PASS: Gas Can engine pin contract\n'
