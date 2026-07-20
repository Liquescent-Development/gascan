#!/usr/bin/env bash
set -euo pipefail

usage() {
  printf 'usage: %s PACKAGE.pkg\n' "$0" >&2
  exit 64
}

[[ $# -eq 1 ]] || usage
package=$1
[[ -f $package ]] || { printf 'package does not exist: %s\n' "$package" >&2; exit 66; }

expanded=$(mktemp -d "${TMPDIR:-/tmp}/gascan-install-package.XXXXXX")
trap 'rm -rf "$expanded"' EXIT
pkgutil --expand "$package" "$expanded/pkg"
identifier=$(sed -n 's/.*identifier="\([^"]*\)".*/\1/p' "$expanded/pkg/PackageInfo")
if [[ $identifier != dev.gascan.pkg ]]; then
  printf 'unexpected package identifier: %s\n' "${identifier:-missing}" >&2
  exit 65
fi
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
container system version --format json >/dev/null || {
  printf 'Apple container 1.1.0 is unavailable\n' >&2
  exit 69
}
container system status --format json >/dev/null || {
  printf 'Apple container service is not ready; run `container system start` first\n' >&2
  exit 69
}

payload=$(pkgutil --payload-files "$package")
for required in \
  ./usr/local/bin/gascan \
  ./usr/local/bin/gascand \
  ./usr/local/bin/gascan-apple-attach \
  ./usr/local/share/gascan/LICENSE \
  ./usr/local/share/gascan/default-gascan.toml \
  ./usr/local/share/gascan/build-manifest.json; do
  grep -qx "$required" <<<"$payload" || {
    printf 'package is missing required payload: %s\n' "$required" >&2
    exit 65
  }
done
if grep -Eq '/(container|container-apiserver)$' <<<"$payload"; then
  printf 'refusing package that embeds the Apple runtime\n' >&2
  exit 65
fi

sudo installer -pkg "$package" -target /
printf 'Gas Can installed. The per-user daemon starts on demand.\n'
