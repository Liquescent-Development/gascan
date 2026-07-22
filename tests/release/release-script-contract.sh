#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/../.." && pwd -P)
fixture=$(mktemp -d "${TMPDIR:-/tmp}/gascan-release-contract.XXXXXX")
trap 'rm -rf "$fixture"' EXIT

config=$repo_root/packaging/macos/release-config.sh
[[ -r $config ]] || { printf 'release-config.sh is not readable\n' >&2; exit 1; }
# shellcheck source=/dev/null
source "$config"

cat >"$fixture/release.env" <<'EOF_CONFIG'
GASCAN_NOTARYTOOL_PROFILE=from-file
GASCAN_TAP_PATH=/tmp/tap-from-file
EOF_CONFIG

# shellcheck disable=SC1007 # VAR= command clears the ambient env var for this call
observed=$(GASCAN_NOTARYTOOL_PROFILE= gascan_release_config \
  GASCAN_NOTARYTOOL_PROFILE '' "$fixture/release.env")
[[ $observed == from-file ]] || {
  printf 'config file value not used: %s\n' "$observed" >&2; exit 1; }

observed=$(GASCAN_NOTARYTOOL_PROFILE=from-env gascan_release_config \
  GASCAN_NOTARYTOOL_PROFILE '' "$fixture/release.env")
[[ $observed == from-env ]] || {
  printf 'environment did not beat config file: %s\n' "$observed" >&2; exit 1; }

observed=$(GASCAN_NOTARYTOOL_PROFILE=from-env gascan_release_config \
  GASCAN_NOTARYTOOL_PROFILE from-flag "$fixture/release.env")
[[ $observed == from-flag ]] || {
  printf 'flag did not beat environment: %s\n' "$observed" >&2; exit 1; }

set +e
# shellcheck disable=SC1007 # VAR= command clears the ambient env var for this call
missing=$(GASCAN_CODESIGN_IDENTITY= gascan_release_config \
  GASCAN_CODESIGN_IDENTITY '' "$fixture/release.env" 2>&1 >/dev/null)
missing_code=$?
set -e
[[ $missing_code -ne 0 ]] || {
  printf 'a missing required value was accepted\n' >&2; exit 1; }
for needle in GASCAN_CODESIGN_IDENTITY --codesign-identity release.env; do
  grep -Fq -- "$needle" <<<"$missing" || {
    printf 'missing-value message omits %s: %s\n' "$needle" "$missing" >&2
    exit 1; }
done

cat >"$fixture/spaces.env" <<'EOF_SPACES'
GASCAN_CODESIGN_IDENTITY=Developer ID Application: Example LLC (TEAMID1234)
EOF_SPACES
# shellcheck disable=SC1007 # VAR= command clears the ambient env var for this call
observed=$(GASCAN_CODESIGN_IDENTITY= gascan_release_config \
  GASCAN_CODESIGN_IDENTITY '' "$fixture/spaces.env")
[[ $observed == 'Developer ID Application: Example LLC (TEAMID1234)' ]] || {
  printf 'value with spaces was mangled: %s\n' "$observed" >&2; exit 1; }

cat >"$fixture/hostile.env" <<'EOF_HOSTILE'
GASCAN_TAP_PATH=$(touch /tmp/gascan-config-executed)
EOF_HOSTILE
rm -f /tmp/gascan-config-executed
# shellcheck disable=SC1007 # VAR= command clears the ambient env var for this call
observed=$(GASCAN_TAP_PATH= gascan_release_config \
  GASCAN_TAP_PATH '' "$fixture/hostile.env")
if [[ -e /tmp/gascan-config-executed ]]; then
  rm -f /tmp/gascan-config-executed
  printf 'config file contents were executed\n' >&2
  exit 1
fi
# shellcheck disable=SC2016 # single quotes are deliberate: comparing a literal, not expanding it
[[ $observed == '$(touch /tmp/gascan-config-executed)' ]] || {
  printf 'hostile value not preserved literally: %s\n' "$observed" >&2; exit 1; }

observed=$(GASCAN_TAP_PATH=from-env gascan_release_config \
  GASCAN_TAP_PATH '' "$fixture/absent.env")
[[ $observed == from-env ]] || {
  printf 'absent config file broke resolution: %s\n' "$observed" >&2; exit 1; }

printf 'PASS: Gas Can release script contract\n'
