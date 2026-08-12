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

expect 'Cargo.lock runs rust only' \
  'Cargo.lock' \
  'rust=true contracts=false engine=false'

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
