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

gates=$repo_root/packaging/macos/release-gates.sh
[[ -r $gates ]] || { printf 'release-gates.sh is not readable\n' >&2; exit 1; }
# shellcheck source=/dev/null
source "$repo_root/packaging/macos/release-common.sh"
# shellcheck source=/dev/null
source "$gates"

# Version disagreement is rejected and the message names the workspace version.
workspace_version=$(cd "$repo_root" && cargo metadata --locked --no-deps \
  --format-version 1 | jq -er '.packages[] | select(.name == "gascan") | .version')
set +e
mismatch=$(gascan_gate_version "$repo_root" 99.99.99 2>&1 >/dev/null)
mismatch_code=$?
set -e
[[ $mismatch_code -ne 0 ]] || {
  printf 'version disagreement accepted\n' >&2; exit 1; }
grep -Fq "$workspace_version" <<<"$mismatch" || {
  printf 'mismatch message omits workspace version: %s\n' "$mismatch" >&2; exit 1; }
gascan_gate_version "$repo_root" "$workspace_version" >/dev/null || {
  printf 'the workspace version was rejected\n' >&2; exit 1; }

# Tag gates, exercised behaviorally in a disposable clone with an ephemeral
# signing key -- the technique publish-contract.sh uses, so the property holds
# for whatever version the workspace carries.
clone=$fixture/clone
git clone --quiet "$repo_root" "$clone"
ssh-keygen -q -t ed25519 -N '' -C release@example.invalid -f "$fixture/key"
printf 'release@example.invalid %s\n' "$(cat "$fixture/key.pub")" \
  >"$fixture/allowed-signers"
git -C "$clone" config user.name release
git -C "$clone" config user.email release@example.invalid
git -C "$clone" config gpg.format ssh
git -C "$clone" config user.signingKey "$fixture/key"
git -C "$clone" config gpg.ssh.allowedSignersFile "$fixture/allowed-signers"
# Use a version no real tag carries. The clone's origin IS this repository, so
# a real released version would already be present on the remote and the
# unpushed case could never be exercised.
probe_version=99.99.99
tag=v$probe_version

# absent tag
if gascan_gate_tag "$clone" "$probe_version" >/dev/null 2>&1; then
  printf 'an absent tag was accepted\n' >&2; exit 1
fi

# lightweight tag
git -C "$clone" tag "$tag"
if gascan_gate_tag "$clone" "$probe_version" >/dev/null 2>&1; then
  printf 'a lightweight tag was accepted\n' >&2; exit 1
fi
git -C "$clone" tag -d "$tag" >/dev/null

# annotated but unsigned
git -C "$clone" tag -a "$tag" -m unsigned
if gascan_gate_tag "$clone" "$probe_version" >/dev/null 2>&1; then
  printf 'an unsigned annotated tag was accepted\n' >&2; exit 1
fi
git -C "$clone" tag -d "$tag" >/dev/null

# signed but not pointing at HEAD
git -C "$clone" tag -s "$tag" -m 'not head' "$(git -C "$clone" rev-parse HEAD~1)"
if gascan_gate_tag "$clone" "$probe_version" >/dev/null 2>&1; then
  printf 'a tag that does not peel to HEAD was accepted\n' >&2; exit 1
fi
git -C "$clone" tag -d "$tag" >/dev/null

# signed, at HEAD, but absent from the remote
git -C "$clone" tag -s "$tag" -m 'at head'
set +e
unpushed=$(gascan_gate_tag "$clone" "$probe_version" 2>&1 >/dev/null)
unpushed_code=$?
set -e
[[ $unpushed_code -ne 0 ]] || {
  printf 'an unpushed tag was accepted\n' >&2; exit 1; }
grep -Fq 'git push origin' <<<"$unpushed" || {
  printf 'unpushed-tag message omits the push command: %s\n' "$unpushed" >&2
  exit 1; }

# Tap gate rejects a path that is not a git work tree.
if gascan_gate_tap "$fixture" >/dev/null 2>&1; then
  printf 'a non-repository tap path was accepted\n' >&2; exit 1
fi

printf 'PASS: Gas Can release script contract\n'
