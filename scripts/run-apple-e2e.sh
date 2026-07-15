#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

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
  cargo test -p gascan-e2e --test "$test_name" -- --ignored --test-threads=1 --nocapture
done
