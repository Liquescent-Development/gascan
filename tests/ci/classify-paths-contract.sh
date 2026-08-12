#!/bin/sh
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/../.." && pwd)
classify="$root/scripts/ci-classify-paths.sh"

failures=0

expect() {
  description=$1
  paths=$2
  want=$3
  got=$(printf '%s\n' "$paths" | "$classify" | grep -v '^::notice::' | tr '\n' ' ')
  got=$(printf '%s' "$got" | sed 's/ *$//')
  if test "$got" = "$want"; then
    printf 'ok   %s\n' "$description"
  else
    printf 'FAIL %s\n  want: %s\n  got:  %s\n' "$description" "$want" "$got"
    failures=$((failures + 1))
  fi
}

expect 'a crate change runs rust only' \
  'crates/gascan/src/main.rs' \
  'rust=true contracts=false engine=false'

# Deliberately expensive: the engine job is a cold Swift build and nothing caches
# it, so every dependency bump pays for one. A tonic or hyper bump is the
# likeliest way to break a transport property that compiles everywhere and only
# fails against a real socket, which is the one thing this tier proves.
expect 'Cargo.lock runs rust and engine, because a transport bump is what the live tier catches' \
  'Cargo.lock' \
  'rust=true contracts=false engine=true'

expect 'Cargo.toml does too' \
  'Cargo.toml' \
  'rust=true contracts=false engine=true'

# The workspace lock only. `scripts/Cargo.lock` belongs to an unrelated dependency
# -- docs/release/releasing.md calls it out as a version bump must leave alone --
# and a case pattern matches the whole path, so it falls through to scripts/*.
# Pinned because "Cargo.lock" reads like it would match both.
expect 'the unrelated scripts/Cargo.lock does not run rust or engine' \
  'scripts/Cargo.lock' \
  'rust=false contracts=true engine=false'

expect 'the proto runs rust, because gascan-proto compiles it' \
  'proto/gascan/v1/gascan.proto' \
  'rust=true contracts=false engine=false'

expect 'a docs change runs contracts only' \
  'docs/status/arca-integration-handoff.md' \
  'rust=false contracts=true engine=false'

expect 'README runs contracts only' \
  'README.md' \
  'rust=false contracts=true engine=false'

expect 'the pin runs engine and rust, because the pin decides the client codegen input' \
  'engine/arca-pin.json' \
  'rust=true contracts=false engine=true'

expect 'the allowed-signers file runs engine and rust for the same reason' \
  'engine/allowed-signers' \
  'rust=true contracts=false engine=true'

expect 'the engine build script runs engine and contracts' \
  'scripts/build-arca-engine.sh' \
  'rust=false contracts=true engine=true'

expect 'the engine targets check runs engine and contracts, not contracts alone' \
  'tests/release/engine-targets-check.sh' \
  'rust=false contracts=true engine=true'

expect 'another tests/release path still runs contracts only' \
  'tests/release/engine-pin-contract.sh' \
  'rust=false contracts=true engine=false'

expect 'the live tier runs engine and rust, because only the engine job runs its ignored tests' \
  'crates/gascan-arca/tests/live/read_rpcs.rs' \
  'rust=true contracts=false engine=true'

expect 'the live tier target root does too' \
  'crates/gascan-arca/tests/live.rs' \
  'rust=true contracts=false engine=true'

expect 'the rest of gascan-arca'"'"'s tests do too, being the same subject' \
  'crates/gascan-arca/tests/backend_unary.rs' \
  'rust=true contracts=false engine=true'

# The finding this case exists for: channel.rs owns the placeholder authority,
# the Unix connector and source_chain(), and every property proving those works
# lives in the live tier. Classified rust-only, editing it skipped the engine job
# and merged with the tier never run.
expect 'the live tier'"'"'s SUBJECT runs engine, not just its tests' \
  'crates/gascan-arca/src/channel.rs' \
  'rust=true contracts=false engine=true'

expect 'the generated client the tier drives runs engine too' \
  'crates/gascan-engine-proto/build.rs' \
  'rust=true contracts=false engine=true'

expect 'a crate the live tier does not exercise still runs rust only' \
  'crates/gascan-apple/src/inspect.rs' \
  'rust=true contracts=false engine=false'

expect 'the proto sync script runs rust and contracts' \
  'scripts/sync-arca-proto.sh' \
  'rust=true contracts=true engine=false'

expect 'another script runs contracts only' \
  'scripts/produce-gascamp-bundle.sh' \
  'rust=false contracts=true engine=false'

expect 'the workflow itself runs everything' \
  '.github/workflows/ci.yml' \
  'rust=true contracts=true engine=true'

expect 'the classifier itself runs everything' \
  'scripts/ci-classify-paths.sh' \
  'rust=true contracts=true engine=true'

expect 'agent config runs nothing' \
  '.claude/settings.json' \
  'rust=false contracts=false engine=false'

expect 'an unmapped path runs everything' \
  'brand-new-directory/thing.txt' \
  'rust=true contracts=true engine=true'

expect 'areas union across several paths' \
  'crates/gascan/src/main.rs
engine/arca-pin.json' \
  'rust=true contracts=false engine=true'

expect 'empty input runs nothing' \
  '' \
  'rust=false contracts=false engine=false'

# A path with a space must not be word-split into two paths.
expect 'a path containing a space is one path' \
  'docs/a file.md' \
  'rust=false contracts=true engine=false'

notice=$(printf 'brand-new-directory/thing.txt\n' | "$classify" | grep -c '^::notice::')
if test "$notice" -ge 1; then
  printf 'ok   an unmapped path emits a notice\n'
else
  printf 'FAIL an unmapped path emits a notice\n'
  failures=$((failures + 1))
fi

quiet=$(printf 'crates/gascan/src/main.rs\n' | "$classify" | grep -c '^::notice::' || true)
if test "$quiet" -eq 0; then
  printf 'ok   a mapped path emits no notice\n'
else
  printf 'FAIL a mapped path emits no notice\n'
  failures=$((failures + 1))
fi

if test "$failures" -eq 0; then
  printf 'classify-paths: all checks passed\n'
else
  printf 'classify-paths: %d check(s) failed\n' "$failures" >&2
  exit 1
fi
