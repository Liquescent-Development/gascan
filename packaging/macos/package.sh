#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/../.." && pwd -P)
cd "$repo_root"

if [[ $(uname -s) != Darwin || $(uname -m) != arm64 ]]; then
  printf 'Gas Can macOS packages must be built natively on Apple silicon\n' >&2
  exit 69
fi
for command in cargo jq lipo pkgbuild shasum strip swift xcrun; do
  command -v "$command" >/dev/null || {
    printf 'required packaging command is unavailable: %s\n' "$command" >&2
    exit 69
  }
done
if [[ -n ${GASCAN_CODESIGN_IDENTITY:-} ]]; then
  command -v codesign >/dev/null || {
    printf 'codesign is required when GASCAN_CODESIGN_IDENTITY is set\n' >&2
    exit 69
  }
fi

version=$(cargo metadata --locked --no-deps --format-version 1 |
  jq -er '.packages[] | select(.name == "gascan") | .version')
revision=$(git rev-parse --verify HEAD)
[[ $revision =~ ^[0-9a-f]{40}$ ]] || {
  printf 'source revision is not a full Git object ID\n' >&2
  exit 1
}
git verify-commit "$revision" >/dev/null 2>&1 || {
  printf 'release source HEAD does not have a trusted Git signature\n' >&2
  exit 65
}
release_inputs=(Cargo.toml Cargo.lock crates helpers scripts/build-apple-attach-helper.sh packaging/macos LICENSE)
if [[ -n $(git status --porcelain --untracked-files=all -- "${release_inputs[@]}") ]]; then
  printf 'release source inputs are not clean at %s\n' "$revision" >&2
  exit 65
fi

artifact_root=${GASCAN_RELEASE_ARTIFACT_DIR:-$repo_root/.artifacts/release}
mkdir -p "$artifact_root"
artifact_root=$(cd "$artifact_root" && pwd -P)
work=$(mktemp -d "${TMPDIR:-/tmp}/gascan-package.XXXXXX")
trap 'rm -rf "$work"' EXIT
root="$work/root"
install -d -m 0755 "$root/usr/local/bin" "$root/usr/local/share/gascan"

cargo build --locked --release -p gascan -p gascand >&2
"$repo_root/scripts/build-apple-attach-helper.sh" >&2
for binary in gascan gascand gascan-apple-attach; do
  source_path="$repo_root/target/release/$binary"
  if [[ $binary == gascan-apple-attach ]]; then
    source_path="$repo_root/target/gascan-apple-attach"
  fi
  [[ $(lipo -archs "$source_path") == arm64 ]] || {
    printf 'release binary is not native arm64 Mach-O: %s\n' "$source_path" >&2
    exit 1
  }
  install -m 0755 "$source_path" "$root/usr/local/bin/$binary"
  strip -x "$root/usr/local/bin/$binary"
done

if [[ -n ${GASCAN_CODESIGN_IDENTITY:-} ]]; then
  for binary in gascan gascand gascan-apple-attach; do
    codesign --force --options runtime --timestamp \
      --sign "$GASCAN_CODESIGN_IDENTITY" "$root/usr/local/bin/$binary" >&2
  done
fi

install -m 0644 "$repo_root/LICENSE" "$root/usr/local/share/gascan/LICENSE"
install -m 0644 "$repo_root/packaging/macos/default-gascan.toml" \
  "$root/usr/local/share/gascan/default-gascan.toml"

files_json='[]'
for binary in gascan gascan-apple-attach gascand; do
  relative="usr/local/bin/$binary"
  digest=$(shasum -a 256 "$root/$relative" | awk '{print $1}')
  files_json=$(jq -cn --argjson files "$files_json" --arg path "$relative" --arg sha "$digest" \
    '$files + [{path: $path, sha256: $sha}]')
done
jq -nS \
  --arg architecture arm64 \
  --arg source_revision "$revision" \
  --arg version "$version" \
  --argjson files "$files_json" \
  '{schema: 1, product: "Gas Can", version: $version, architecture: $architecture, source_revision: $source_revision, files: $files}' \
  >"$root/usr/local/share/gascan/build-manifest.json"
chmod 0644 "$root/usr/local/share/gascan/build-manifest.json"

package="$artifact_root/gascan-$version-macos-arm64.pkg"
pkgbuild_args=(
  --root "$root"
  --identifier dev.gascan.pkg
  --version "$version"
  --install-location /
  --ownership recommended
)
if [[ -n ${GASCAN_INSTALLER_SIGNING_IDENTITY:-} ]]; then
  pkgbuild_args+=(--sign "$GASCAN_INSTALLER_SIGNING_IDENTITY")
fi
pkgbuild "${pkgbuild_args[@]}" "$package" >&2

[[ $(git rev-parse --verify HEAD) == "$revision" ]] || {
  printf 'source HEAD changed during package build\n' >&2
  exit 65
}
if [[ -n $(git status --porcelain --untracked-files=all -- "${release_inputs[@]}") ]]; then
  printf 'release source inputs changed during package build\n' >&2
  exit 65
fi
"$repo_root/packaging/macos/verify-package.sh" "$package" "$revision" "$version"

if [[ -n ${GASCAN_NOTARYTOOL_PROFILE:-} ]]; then
  xcrun notarytool submit "$package" \
    --keychain-profile "$GASCAN_NOTARYTOOL_PROFILE" --wait >&2
  xcrun stapler staple "$package" >&2
fi

printf '%s\n' "$package"
