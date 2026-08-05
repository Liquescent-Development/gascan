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

# An upstream repository standing in for Arca. It carries a Package.swift with a
# target named ContainerBridge so the build step has something real to compile.
upstream=$fixture/upstream
mkdir -p "$upstream/Sources/ContainerBridge"
cat >"$upstream/Package.swift" <<'PACKAGE'
// swift-tools-version: 6.2
import PackageDescription
let package = Package(
    name: "Arca",
    targets: [.target(name: "ContainerBridge")]
)
PACKAGE
printf 'public let engineFixture = 1\n' >"$upstream/Sources/ContainerBridge/Fixture.swift"
git -C "$upstream" init -q
git -C "$upstream" config user.name fixture
git -C "$upstream" config user.email engine@example.invalid
git -C "$upstream" config gpg.format ssh
git -C "$upstream" config user.signingKey "$fixture/key"
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
  GASCAN_ARCA_PIN_FILE=$pin \
  GASCAN_ARCA_ENGINE_CACHE=$fixture/cache-$label \
  GASCAN_ARCA_ALLOWED_SIGNERS=$signers \
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
warm=$fixture/cache-good/arca
printf 'public let planted = 3\n' >"$warm/Sources/ContainerBridge/Planted.swift"
printf 'public let tampered = 4\n' >>"$warm/Sources/ContainerBridge/Fixture.swift"
mkdir -p "$warm/.build"
printf 'poisoned\n' >"$warm/.build/poison"
run_case good "$fixture/pin-good.json" 0
for stale in Sources/ContainerBridge/Planted.swift .build/poison; do
  [[ ! -e $warm/$stale ]] || {
    printf 'warm cache carried an unverified file into the build: %s\n' "$stale" >&2
    exit 1
  }
done
git -C "$warm" diff --quiet || {
  printf 'warm cache carried a tracked modification into the build\n' >&2
  exit 1
}

printf 'PASS: Gas Can engine pin contract\n'
