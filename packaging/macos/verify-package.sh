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
# Which xattrs the payload carries is a property of the build host, not of Gas
# Can: macOS 26 attaches the protected com.apple.provenance to every file it
# creates and it cannot be stripped, while a host that attaches nothing produces
# the same payload carrying none. Either way they are not installed as `._*`
# files. Accept exactly those two representations -- and require the whole
# payload to agree on one of them, so a single file carrying a stray xattr is
# still a rejection. The paths come from the shared allowlist rather than a
# second copy of the list, so the two cannot drift.
canonical_xattrs=
first_path=true
while IFS= read -r path; do
  observed=$(xattr "$work/root/$path" | LC_ALL=C sort | tr '\n' ',')
  if [[ $first_path == true ]]; then
    first_path=false
    case $observed in
      '' | com.apple.provenance,) canonical_xattrs=$observed ;;
      *)
        printf 'payload xattr set is not a canonical representation: %s\n' "$path" >&2
        exit 65
        ;;
    esac
  fi
  [[ $observed == "$canonical_xattrs" ]] || {
    printf 'payload xattr set is not uniform across the payload: %s\n' "$path" >&2
    exit 65
  }
done < <(gascan_expected_payload_files)
manifest=$work/root/usr/local/share/gascan/build-manifest.json
jq -e --arg revision "$expected_revision" --arg version "$expected_version" '
  . == {
    architecture: "arm64",
    engine: .engine,
    files: .files,
    product: "Gas Can",
    schema: 2,
    source_revision: $revision,
    version: $version
  } and
  (.engine | keys == ["name", "revision", "tag", "url"]) and
  (.engine.name | type == "string" and length > 0) and
  (.engine.url | startswith("https://")) and
  (.engine.tag | type == "string" and length > 0) and
  (.engine.revision | test("^[0-9a-f]{40}$")) and
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
