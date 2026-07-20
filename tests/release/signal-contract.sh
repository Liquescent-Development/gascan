#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/../.." && pwd -P)
fixture=$(mktemp -d "${TMPDIR:-/tmp}/gascan-signal-contract.XXXXXX")
trap 'rm -rf "$fixture"' EXIT

assert_interrupted() {
  local label=$1; shift
  local output status=0
  output=$("$@" 2>&1) || status=$?
  [[ $status -eq 130 ]] || { printf '%s returned %s, expected 130\n%s\n' "$label" "$status" "$output" >&2; exit 1; }
  [[ $output != *'PASS:'* ]] || { printf '%s printed PASS after interruption\n' "$label" >&2; exit 1; }
}

mkdir -p "$fixture/smoke-tmp" "$fixture/gate-tmp"
assert_interrupted smoke env TMPDIR="$fixture/smoke-tmp" GASCAN_RELEASE_GASCAN=/usr/bin/true \
  GASCAN_RELEASE_TESTING=YES GASCAN_RELEASE_TEST_SIGNAL_AFTER_TRAPS=TERM \
  "$repo_root/packaging/macos/release-smoke.sh"
! compgen -G "$fixture/smoke-tmp/gascan-release-root.*" >/dev/null

assert_interrupted clean-host env TMPDIR="$fixture/gate-tmp" \
  GASCAN_RELEASE_TESTING=YES GASCAN_RELEASE_TEST_SIGNAL_AFTER_TRAPS=INT \
  "$repo_root/tests/release/clean-host.sh"
! compgen -G "$fixture/gate-tmp/gascan-release-ledger.*" >/dev/null

printf 'PASS: Gas Can release signal contract\n'
