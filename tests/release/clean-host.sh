#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/../.." && pwd -P)
source "$repo_root/packaging/macos/release-common.sh"
mode=${1:-live}
package_builder="$repo_root/packaging/macos/package.sh"
package_verifier="$repo_root/packaging/macos/verify-package.sh"
installer="$repo_root/packaging/macos/install.sh"
uninstaller="$repo_root/packaging/macos/uninstall.sh"
release_smoke="$repo_root/packaging/macos/release-smoke.sh"
installed_gascan=/usr/local/bin/gascan
install_root=/

if [[ ${GASCAN_RELEASE_TESTING:-} == YES ]]; then
  package_builder=${GASCAN_RELEASE_TEST_PACKAGE_BUILDER:-$package_builder}
  package_verifier=${GASCAN_RELEASE_TEST_PACKAGE_VERIFIER:-$package_verifier}
  installer=${GASCAN_RELEASE_TEST_INSTALLER:-$installer}
  uninstaller=${GASCAN_RELEASE_TEST_UNINSTALLER:-$uninstaller}
  release_smoke=${GASCAN_RELEASE_TEST_SMOKE:-$release_smoke}
  installed_gascan=${GASCAN_RELEASE_TEST_INSTALLED_GASCAN:-$installed_gascan}
  install_root=${GASCAN_RELEASE_TEST_INSTALL_ROOT:-$install_root}
fi

case "$mode" in
  --package-only|live) ;;
  *)
    printf 'usage: %s [--package-only]\n' "$0" >&2
    exit 64
    ;;
esac

manifest=$(mktemp -d "${TMPDIR:-/tmp}/gascan-release-ledger.XXXXXX")
live_started=false
data_may_exist=false
runtime_root=${XDG_RUNTIME_DIR:+$XDG_RUNTIME_DIR/gascan}
runtime_root=${GASCAN_RELEASE_TEST_RUNTIME_ROOT:-${runtime_root:-$(gascan_user_runtime_root)}}

cleanup() {
  local cleanup_status=0
  if [[ $live_started == true ]]; then
    if [[ $data_may_exist == true ]]; then
      "$uninstaller" --remove-data >/dev/null 2>&1 || cleanup_status=1
    else
      "$uninstaller" >/dev/null 2>&1 || cleanup_status=1
    fi
    gascan_audit_clean_host cleanup "$runtime_root" "$install_root" || cleanup_status=1
  fi
  rm -rf "$manifest"
  if [[ $cleanup_status -ne 0 ]]; then
    printf 'clean-host cleanup left recorded resources\n' >&2
  fi
  return "$cleanup_status"
}
on_exit() {
  local original=$? cleanup_status=0
  trap - EXIT INT TERM
  cleanup || cleanup_status=$?
  if [[ $original -ne 0 ]]; then exit "$original"; fi
  exit "$cleanup_status"
}
trap on_exit EXIT
trap 'exit 130' INT TERM
gascan_release_test_signal

expected_revision=$(git -C "$repo_root" rev-parse --verify HEAD)
expected_version=$(cargo metadata --locked --no-deps --format-version 1 --manifest-path "$repo_root/Cargo.toml" | jq -er '.packages[] | select(.name == "gascan") | .version')
package=$("$package_builder")
test -f "$package"
"$package_verifier" "$package" "$expected_revision" "$expected_version"

if [[ $mode == --package-only ]]; then
  printf 'PASS: Gas Can macOS package contract\n'
  exit 0
fi

if [[ ${GASCAN_RELEASE_CLEAN_HOST_CONFIRM:-} != YES ]]; then
  printf 'refusing live clean-host mutation without GASCAN_RELEASE_CLEAN_HOST_CONFIRM=YES\n' >&2
  exit 64
fi

gascan_exact_apple_prerequisites
gascan_audit_clean_host baseline "$runtime_root" "$install_root"
live_started=true

GASCAN_EXPECTED_SOURCE_REVISION=$expected_revision GASCAN_EXPECTED_VERSION=$expected_version \
  "$installer" "$package"

status=0
doctor_json=$("$installed_gascan" doctor --json) || status=$?
if [[ $status -eq 0 ]]; then
  jq -e '([.checks[] | select(.status != "pass")] | length) == 0' <<<"$doctor_json" >/dev/null || status=$?
fi
if [[ $status -eq 0 ]]; then
  data_may_exist=true
  "$release_smoke" || status=$?
fi
uninstall_status=0
if [[ $data_may_exist == true ]]; then
  "$uninstaller" --remove-data || uninstall_status=$?
else
  "$uninstaller" || uninstall_status=$?
fi
if [[ $uninstall_status -eq 0 ]]; then
  :
else
  status=$uninstall_status
fi

if gascan_audit_clean_host final "$runtime_root" "$install_root"; then
  [[ $uninstall_status -eq 0 ]] && live_started=false
else
  status=1
fi

if [[ $status -ne 0 ]]; then
  printf 'FAIL: Gas Can macOS MVP release gate (status %s)\n' "$status" >&2
  exit "$status"
fi

if [[ ${GASCAN_RELEASE_TESTING:-} == YES ]]; then
  printf 'test-mode clean-host execution cannot produce Gate 5 PASS\n' >&2
  exit 125
fi

printf 'PASS: Gas Can macOS MVP release gate\n'
