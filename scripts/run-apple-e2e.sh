#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

cleanup_root=$("$root/scripts/apple-e2e-session-root.sh")
cargo build -p gascan-e2e --bin gascan-e2e-cli
trusted_cli=$(realpath "$root/target/debug/gascan-e2e-cli")
session_root=$(mktemp -d "$cleanup_root/session-XXXXXXXXXXXX")
chmod 700 "$session_root"
export GASCAN_E2E_SESSION_ROOT=$session_root
manifest=
cleanup() {
  status=$?
  trap - EXIT INT TERM HUP
  if test -n "$manifest" && test -f "$manifest"; then
    "$root/scripts/apple-e2e-cleanup.sh" "$manifest" "$trusted_cli" "$cleanup_root" || status=1
  fi
  rmdir "$session_root" 2>/dev/null || true
  exit "$status"
}
trap cleanup EXIT INT TERM HUP

for stale in "$cleanup_root"/*.json; do
  test -e "$stale" || continue
  "$root/scripts/apple-e2e-cleanup.sh" "$stale" "$trusted_cli" "$cleanup_root"
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
    "$root/scripts/apple-e2e-cleanup.sh" "$manifest" "$trusted_cli" "$cleanup_root"
  fi
  manifest=
done
