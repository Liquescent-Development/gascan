#!/bin/sh
# Run every contract script, reporting each exit code separately so a failure
# names the script rather than an aggregate.
set -eu

root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

status=0
count=0

for script in tests/release/*-contract.sh tests/ci/*-contract.sh; do
  test -f "$script" || continue
  count=$((count + 1))
  if "$script" >/dev/null 2>&1; then
    printf 'ok   %s\n' "$script"
  else
    rc=$?
    printf 'FAIL %s rc=%d\n' "$script" "$rc"
    printf -- '--- output of %s ---\n' "$script"
    "$script" 2>&1 || true
    printf -- '--- end %s ---\n' "$script"
    status=1
  fi
done

test "$count" -gt 0 || {
  printf 'ci-run-release-contracts: no contract scripts matched\n' >&2
  exit 1
}

printf 'ci-run-release-contracts: %d contract(s), status=%d\n' "$count" "$status"
exit "$status"
