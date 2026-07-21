#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/../.." && pwd -P)
source "$repo_root/packaging/macos/release-common.sh"

usage() {
  printf 'usage: %s PACKAGE.pkg\n' "$0" >&2
  exit 64
}

[[ $# -eq 1 ]] || usage
package=$1
[[ -f $package ]] || { printf 'package does not exist: %s\n' "$package" >&2; exit 66; }

expected_revision=${GASCAN_EXPECTED_SOURCE_REVISION:-}
expected_version=${GASCAN_EXPECTED_VERSION:-}
[[ $expected_revision =~ ^[0-9a-f]{40}$ && $expected_version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || {
  printf 'GASCAN_EXPECTED_SOURCE_REVISION and GASCAN_EXPECTED_VERSION are required trust inputs\n' >&2
  exit 64
}
"$repo_root/packaging/macos/verify-package.sh" "$package" "$expected_revision" "$expected_version"
[[ $(uname -s) == Darwin && $(uname -m) == arm64 ]] || {
  printf 'Gas Can requires Apple silicon and macOS 26 or newer\n' >&2
  exit 69
}
major=$(sw_vers -productVersion | awk -F. '{print $1}')
[[ $major =~ ^[0-9]+$ && $major -ge 26 ]] || {
  printf 'Gas Can requires macOS 26 or newer\n' >&2
  exit 69
}
command -v container >/dev/null || {
  printf 'Apple container 1.1.0 is required; install it before Gas Can\n' >&2
  exit 69
}
gascan_exact_apple_prerequisites || {
  printf 'Apple container client/service must be exact supported 1.1.0 release and running\n' >&2
  exit 69
}

sudo installer -pkg "$package" -target /
printf 'Gas Can installed. The per-user daemon starts on demand.\n'
