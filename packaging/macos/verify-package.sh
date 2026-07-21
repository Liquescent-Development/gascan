#!/usr/bin/env bash
set -euo pipefail

source "$(cd "$(dirname "$0")" && pwd -P)/release-common.sh"

[[ $# -eq 3 ]] || { printf 'usage: %s PACKAGE REVISION VERSION\n' "$0" >&2; exit 64; }
package=$1 expected_revision=$2 expected_version=$3
[[ $expected_revision =~ ^[0-9a-f]{40}$ ]] || exit 64
[[ $expected_version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || exit 64

work=$(mktemp -d "${TMPDIR:-/tmp}/gascan-verify-package.XXXXXX")
trap 'rm -rf "$work"' EXIT
pkgutil --expand "$package" "$work/pkg"
[[ ! -e $work/pkg/Scripts ]] || { printf 'package scripts are forbidden\n' >&2; exit 65; }
package_info=$work/pkg/PackageInfo
[[ -f $package_info ]] || { printf 'PackageInfo is missing\n' >&2; exit 65; }
attribute() { xmllint --xpath "string(/pkg-info/@$1)" "$package_info"; }
[[ $(attribute identifier) == dev.gascan.pkg ]] || { printf 'unexpected package identifier\n' >&2; exit 65; }
[[ $(attribute version) == "$expected_version" ]] || { printf 'unexpected package version\n' >&2; exit 65; }
[[ $(attribute install-location) == / ]] || { printf 'unexpected install location\n' >&2; exit 65; }

gascan_assert_exact_payload "$package" || exit $?

mkdir "$work/root"
(cd "$work/root" && gzip -dc "$work/pkg/Payload" | cpio -idm --quiet)
# macOS 26's pkgbuild serializes its protected com.apple.provenance xattr as
# paired AppleDouble payload records. They are not installed as `._*` files.
# Require that exact representation and reject every other xattr.
while IFS= read -r path; do
  [[ $(xattr "$work/root/$path") == com.apple.provenance ]] || {
    printf 'payload xattr set is not the canonical macOS provenance record: %s\n' "$path" >&2
    exit 65
  }
done <<'EOF_XATTR_PATHS'
.
usr
usr/local
usr/local/bin
usr/local/bin/gascan
usr/local/bin/gascan-apple-attach
usr/local/bin/gascand
usr/local/share
usr/local/share/gascan
usr/local/share/gascan/LICENSE
usr/local/share/gascan/build-manifest.json
usr/local/share/gascan/default-gascan.toml
EOF_XATTR_PATHS
manifest=$work/root/usr/local/share/gascan/build-manifest.json
jq -e --arg revision "$expected_revision" --arg version "$expected_version" '
  . == {
    architecture: "arm64",
    files: .files,
    product: "Gas Can",
    schema: 1,
    source_revision: $revision,
    version: $version
  } and
  (.files | map(.path) == ["usr/local/bin/gascan", "usr/local/bin/gascan-apple-attach", "usr/local/bin/gascand"]) and
  all(.files[]; (.sha256 | test("^[0-9a-f]{64}$")))
' "$manifest" >/dev/null || { printf 'build manifest is invalid\n' >&2; exit 65; }

while IFS=$'\t' read -r relative expected; do
  actual=$(shasum -a 256 "$work/root/$relative" | awk '{print $1}')
  [[ $actual == "$expected" ]] || { printf 'checksum mismatch: %s\n' "$relative" >&2; exit 65; }
  [[ $(lipo -archs "$work/root/$relative") == arm64 ]] || {
    printf 'executable is not thin arm64: %s\n' "$relative" >&2
    exit 65
  }
done < <(jq -r '.files[] | [.path, .sha256] | @tsv' "$manifest")
