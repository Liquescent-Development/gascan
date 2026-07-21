#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd -P)
cd "$root"

printf 'TAP version 13\n'
if "$root/scripts/run-apple-e2e.sh" apple_security; then
  printf 'ok 1 - real macOS security acceptance\n'
  printf '1..1\n'
else
  status=$?
  printf 'not ok 1 - real macOS security acceptance\n'
  printf '1..1\n'
  exit "$status"
fi
