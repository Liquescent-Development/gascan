#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/../.." && pwd -P)
source "$repo_root/packaging/macos/release-common.sh"
mode=${1:-live}

case "$mode" in
  --package-only|live) ;;
  *)
    printf 'usage: %s [--package-only]\n' "$0" >&2
    exit 64
    ;;
esac

expected_revision=$(git -C "$repo_root" rev-parse --verify HEAD)
expected_version=$(cargo metadata --locked --no-deps --format-version 1 --manifest-path "$repo_root/Cargo.toml" | jq -er '.packages[] | select(.name == "gascan") | .version')
package=$("$repo_root/packaging/macos/package.sh")
test -f "$package"
"$repo_root/packaging/macos/verify-package.sh" "$package" "$expected_revision" "$expected_version"

manifest=$(mktemp -d "${TMPDIR:-/tmp}/gascan-release-ledger.XXXXXX")
live_started=false
runtime_root=${XDG_RUNTIME_DIR:-/tmp/gascan-$(id -u)}/gascan

cleanup() {
  local original=$? cleanup_status=0
  trap - EXIT INT TERM
  if [[ $live_started == true ]]; then
    "$repo_root/packaging/macos/uninstall.sh" --remove-data >/dev/null 2>&1 || cleanup_status=1
    gascan_audit_clean_host cleanup "$runtime_root" / || cleanup_status=1
  fi
  rm -rf "$manifest"
  if [[ $original -ne 0 ]]; then exit "$original"; fi
  exit "$cleanup_status"
}
on_signal() {
  trap - EXIT INT TERM
  cleanup
  exit 130
}
trap cleanup EXIT
trap on_signal INT TERM

if [[ $mode == --package-only ]]; then
  printf 'PASS: Gas Can macOS package contract\n'
  exit 0
fi

if [[ ${GASCAN_RELEASE_CLEAN_HOST_CONFIRM:-} != YES ]]; then
  printf 'refusing live clean-host mutation without GASCAN_RELEASE_CLEAN_HOST_CONFIRM=YES\n' >&2
  exit 64
fi

gascan_exact_apple_prerequisites
gascan_audit_clean_host baseline "$runtime_root" /
live_started=true

GASCAN_EXPECTED_SOURCE_REVISION=$expected_revision GASCAN_EXPECTED_VERSION=$expected_version \
  "$repo_root/packaging/macos/install.sh" "$package"

status=0
/usr/local/bin/gascan doctor --json |
  jq -e '([.checks[] | select(.status != "pass")] | length) == 0' >/dev/null || status=$?
if [[ $status -eq 0 ]]; then
  "$repo_root/packaging/macos/release-smoke.sh" || status=$?
fi
if "$repo_root/packaging/macos/uninstall.sh" --remove-data; then
  :
else
  status=$?
fi

gascan_audit_clean_host final "$runtime_root" / || status=1
live_started=false

if [[ $status -ne 0 ]]; then
  printf 'FAIL: Gas Can macOS MVP release gate (status %s)\n' "$status" >&2
  exit "$status"
fi

printf 'PASS: Gas Can macOS MVP release gate\n'
