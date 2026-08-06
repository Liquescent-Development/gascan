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

# The interrupted script must leave no working directory behind. Written as a
# function rather than `! compgen -G ...` because a `!`-prefixed command is
# exempt from errexit: the bare form returned 1 on leftovers and the script
# carried on to print PASS, so both cleanup assertions could never fail.
assert_no_leftovers() {
  local label=$1 pattern=$2
  if compgen -G "$pattern" >/dev/null; then
    printf '%s left working directories behind: %s\n' "$label" "$pattern" >&2
    exit 1
  fi
}

mkdir -p "$fixture/smoke-tmp" "$fixture/gate-tmp"
# release-smoke.sh existence-checks all three installed binaries (lines 51-54)
# before it reaches gascan_release_test_signal (line 404), and it executes none
# of them on this path. Overriding only GASCAN_RELEASE_GASCAN left the other two
# resolving to /usr/local/bin, so this contract silently required Gas Can to be
# installed on the host: it passed on a developer Mac and exited 69 ("installed
# gascand is unavailable") on a hosted runner. Pin all three seams the script
# already publishes, so the contract tests trap handling and nothing else.
assert_interrupted smoke env TMPDIR="$fixture/smoke-tmp" GASCAN_RELEASE_GASCAN=/usr/bin/true \
  GASCAN_RELEASE_GASCAND=/usr/bin/true \
  GASCAN_RELEASE_APPLE_ATTACH_HELPER=/usr/bin/true \
  GASCAN_RELEASE_TESTING=YES GASCAN_RELEASE_TEST_SIGNAL_AFTER_TRAPS=TERM \
  "$repo_root/packaging/macos/release-smoke.sh"
assert_no_leftovers smoke "$fixture/smoke-tmp/gascan-release-root.*"

assert_interrupted clean-host env TMPDIR="$fixture/gate-tmp" \
  GASCAN_RELEASE_TESTING=YES GASCAN_RELEASE_TEST_SIGNAL_AFTER_TRAPS=INT \
  "$repo_root/tests/release/clean-host.sh"
assert_no_leftovers clean-host "$fixture/gate-tmp/gascan-release-ledger.*"

printf 'PASS: Gas Can release signal contract\n'
