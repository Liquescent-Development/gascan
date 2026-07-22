#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/../.." && pwd -P)
cd "$repo_root"
source "$repo_root/packaging/macos/release-common.sh"

[[ $# -eq 1 ]] || { printf 'usage: %s PACKAGE.pkg\n' "$0" >&2; exit 64; }
package=$1
[[ -f $package ]] || { printf 'package does not exist: %s\n' "$package" >&2; exit 66; }
for command in cargo gh jq pkgutil shasum; do
  command -v "$command" >/dev/null || {
    printf 'required publish command is unavailable: %s\n' "$command" >&2
    exit 69
  }
done

version=$(cargo metadata --locked --no-deps --format-version 1 |
  jq -er '.packages[] | select(.name == "gascan") | .version')
revision=$(git rev-parse --verify HEAD)
[[ $revision =~ ^[0-9a-f]{40}$ ]] || {
  printf 'source revision is not a full Git object ID\n' >&2
  exit 1
}
tag="v$version"

# Publishing requires the release tag itself, not merely a signed commit.
[[ $(git cat-file -t "refs/tags/$tag" 2>/dev/null) == tag ]] || {
  printf 'release tag %s is missing or not an annotated tag\n' "$tag" >&2
  exit 65
}
git verify-tag "refs/tags/$tag" >/dev/null 2>&1 || {
  printf 'release tag %s does not carry a trusted signature\n' "$tag" >&2
  exit 65
}
[[ $(git rev-parse --verify "refs/tags/$tag^{}") == "$revision" ]] || {
  printf 'release tag %s does not point at HEAD\n' "$tag" >&2
  exit 65
}
gascan_verify_release_source "$repo_root" "$revision" "$version" || {
  printf 'release source is not trusted\n' >&2
  exit 65
}
gascan_assert_release_inputs_clean "$repo_root" "$tag" || exit 65
"$repo_root/packaging/macos/verify-package.sh" "$package" "$revision" "$version"
gascan_assert_distributable_package "$package" "$GASCAN_RELEASE_TEAM" || exit 65

if gh release view "$tag" >/dev/null 2>&1; then
  printf 'release %s already exists; publish a new version instead\n' "$tag" >&2
  exit 65
fi

artifact_dir=$(cd "$(dirname "$package")" && pwd -P)
base=$(basename "$package")
[[ $base == "gascan-$version-macos-arm64.pkg" ]] || {
  printf 'unexpected package file name: %s\n' "$base" >&2
  exit 65
}

work=$(mktemp -d "${TMPDIR:-/tmp}/gascan-publish.XXXXXX")
trap 'rm -rf "$work"' EXIT
pkgutil --expand "$package" "$work/pkg"
mkdir "$work/root"
(cd "$work/root" && gzip -dc "$work/pkg/Payload" | cpio -idm --quiet)
cp "$work/root/usr/local/share/gascan/build-manifest.json" "$work/build-manifest.json"
(cd "$artifact_dir" && shasum -a 256 "$base" >"$work/$base.sha256")
checksum=$(awk '{print $1}' "$work/$base.sha256")

cat >"$work/notes.md" <<EOF_NOTES
Gas Can $version for Apple silicon, macOS 26 or newer.

Install with Homebrew:

    brew tap liquescent-development/tap
    brew trust liquescent-development/tap
    brew install --cask gascan

Or download \`$base\` and open it.

Gas Can requires Apple \`container\` 1.1.0 and its running service, which Gas
Can neither installs nor redistributes. Verify the host with
\`gascan doctor --json\`.

Source revision: \`$revision\`
SHA-256: \`$checksum\`
EOF_NOTES

gh release create "$tag" --draft --title "Gas Can $version" --notes-file "$work/notes.md" \
  --verify-tag --target "$revision" >/dev/null
gh release upload "$tag" \
  "$package" "$work/$base.sha256" "$work/build-manifest.json" >/dev/null
assets=$(gh release view "$tag" --json assets --jq '[.assets[].name] | sort | join(",")')
expected=$(gascan_expected_release_assets "$base")
[[ $assets == "$expected" ]] || {
  printf 'release assets are incomplete: %s\n' "$assets" >&2
  exit 65
}
gh release edit "$tag" --draft=false >/dev/null

printf 'https://github.com/Liquescent-Development/gascan/releases/download/%s/%s\n' "$tag" "$base"
printf '%s\n' "$checksum"
