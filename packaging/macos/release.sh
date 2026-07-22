#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/../.." && pwd -P)
# shellcheck source=release-common.sh
source "$repo_root/packaging/macos/release-common.sh"
# shellcheck source=release-config.sh
source "$repo_root/packaging/macos/release-config.sh"
# shellcheck source=release-gates.sh
source "$repo_root/packaging/macos/release-gates.sh"

usage() {
  cat >&2 <<'EOF_USAGE'
usage: release.sh VERSION [--check]
                  [--codesign-identity NAME] [--installer-identity NAME]
                  [--notary-profile NAME] [--tap PATH] [--config FILE]

Drives an already-tagged release: verifies every gate, then builds, signs,
notarizes, publishes, and updates the Homebrew cask.

  --check   run every gate and exit without building or publishing

Configuration resolves by flag, then environment, then the config file
(default: ${XDG_CONFIG_HOME:-$HOME/.config}/gascan/release.env). Nothing is
defaulted.

This never creates, moves, or deletes a tag. Create and push the signed tag
first:
    git tag -s vVERSION -m 'Gas Can VERSION' && git push origin vVERSION
EOF_USAGE
}

version=
check_only=false
flag_application=
flag_installer=
flag_profile=
flag_tap=
config_file="${XDG_CONFIG_HOME:-$HOME/.config}/gascan/release.env"

while [[ $# -gt 0 ]]; do
  case $1 in
    --check) check_only=true; shift;;
    --codesign-identity) flag_application=${2-}; shift 2;;
    --installer-identity) flag_installer=${2-}; shift 2;;
    --notary-profile) flag_profile=${2-}; shift 2;;
    --tap) flag_tap=${2-}; shift 2;;
    --config) config_file=${2-}; shift 2;;
    -h|--help) usage; exit 0;;
    -*) printf 'unknown flag: %s\n' "$1" >&2; usage; exit 64;;
    *)
      [[ -z $version ]] || { printf 'unexpected argument: %s\n' "$1" >&2; usage; exit 64; }
      version=$1; shift;;
  esac
done

[[ -n $version ]] || { usage; exit 64; }
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || {
  printf 'version must be MAJOR.MINOR.PATCH, got: %s\n' "$version" >&2
  usage
  exit 64
}

application_identity=$(gascan_release_config GASCAN_CODESIGN_IDENTITY "$flag_application" "$config_file")
installer_identity=$(gascan_release_config GASCAN_INSTALLER_SIGNING_IDENTITY "$flag_installer" "$config_file")
notary_profile=$(gascan_release_config GASCAN_NOTARYTOOL_PROFILE "$flag_profile" "$config_file")
tap_path=$(gascan_release_config GASCAN_TAP_PATH "$flag_tap" "$config_file")

cd "$repo_root"
printf 'checking release preconditions for %s\n' "$version" >&2
gascan_gate_tools
gascan_gate_version "$repo_root" "$version"
gascan_assert_release_inputs_clean "$repo_root" "release $version"
gascan_gate_tag "$repo_root" "$version"
gascan_gate_no_release "$version"
gascan_gate_identities "$application_identity" "$installer_identity"
gascan_gate_notary "$notary_profile"
gascan_gate_tap "$tap_path"
printf 'all release preconditions pass for %s\n' "$version" >&2

if [[ $check_only == true ]]; then
  printf 'check only: nothing was built, published, or changed\n' >&2
  exit 0
fi
