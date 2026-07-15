#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

cleanup_root=${TMPDIR:-/tmp}/gascan-gate4-cleanup
mkdir -p "$cleanup_root"
chmod 700 "$cleanup_root"
manifest=
cleanup() {
  status=$?
  trap - EXIT INT TERM HUP
  if test -n "$manifest" && test -f "$manifest"; then
    "$root/scripts/apple-e2e-cleanup.sh" "$manifest" || status=1
  fi
  exit "$status"
}
trap cleanup EXIT INT TERM HUP

for stale in "$cleanup_root"/*.json; do
  test -e "$stale" || continue
  "$root/scripts/apple-e2e-cleanup.sh" "$stale"
done

./scripts/apple-test-preflight.sh

case ${1-} in
  "")
    tests="apple_lifecycle apple_recovery"
    ;;
  apple_lifecycle|apple_recovery)
    tests=$1
    ;;
  *)
    printf 'usage: %s [apple_lifecycle|apple_recovery]\n' "$0" >&2
    exit 64
    ;;
esac

for test_name in $tests; do
  manifest="$cleanup_root/$test_name-$$.json"
  export GASCAN_E2E_CLEANUP_MANIFEST=$manifest
  cargo test -p gascan-e2e --test "$test_name" -- --ignored --test-threads=1 --nocapture
  if test -f "$manifest"; then
    "$root/scripts/apple-e2e-cleanup.sh" "$manifest"
  fi
  manifest=
done
