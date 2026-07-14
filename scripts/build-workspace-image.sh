#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd -P)
lock="$root/images/workspace/versions.lock"
artifacts="$root/.artifacts"

test -f "$lock" || { printf 'missing image lock: %s\n' "$lock" >&2; exit 1; }
mkdir -p "$artifacts"

top_value() {
  awk -F ' = ' -v key="$1" '$1 == key { gsub(/^"|"$/, "", $2); print $2; exit }' "$lock"
}

section_value() {
  awk -F ' = ' -v section="[$1]" -v key="$2" '
    $0 == section { active=1; next }
    /^\[/ { active=0 }
    active && $1 == key { gsub(/^"|"$/, "", $2); print $2; exit }
  ' "$lock"
}

fetch_verified() {
  cargo run --quiet --locked --offline \
    --manifest-path "$root/scripts/Cargo.toml" --bin fetch-image-artifact -- \
    "$1" "$2" "$3"
}

base_image=$(top_value base_image)
snapshot=$(top_value ubuntu_snapshot)
tag=$(top_value workspace_tag)
mise_url=$(section_value mise url)
mise_sha=$(section_value mise sha256)
chromium_url=$(section_value playwright_chromium url)
chromium_sha=$(section_value playwright_chromium sha256)

[[ "$base_image" == ubuntu@sha256:* ]] || { printf 'mutable base image rejected\n' >&2; exit 1; }
[[ "$tag" != *:latest ]] || { printf 'latest workspace tag rejected\n' >&2; exit 1; }
fetch_verified "$mise_url" "$mise_sha" "$artifacts/mise-linux-arm64"
fetch_verified "$chromium_url" "$chromium_sha" "$artifacts/playwright-chromium-linux-arm64.zip"
rm -rf "$artifacts/playwright-chromium-reviewed"
cargo run --quiet --locked --offline \
  --manifest-path "$root/scripts/Cargo.toml" --bin extract-reviewed-chromium -- \
  "$artifacts/playwright-chromium-linux-arm64.zip" \
  "$artifacts/playwright-chromium-reviewed"

expected_temp=$(mktemp "$artifacts/.expected-tool-versions.XXXXXX")
trap 'rm -f "$expected_temp"' EXIT
cargo run --quiet --locked --offline \
  --manifest-path "$root/scripts/Cargo.toml" --bin validate-tool-versions -- \
  "$lock" "$root/images/workspace/etc/mise/config.toml" >"$expected_temp"
chmod 0444 "$expected_temp"
mv "$expected_temp" "$artifacts/expected-tool-versions.json"
trap - EXIT

container build \
  --arch arm64 \
  --tag "$tag" \
  --file "$root/images/workspace/Dockerfile" \
  --build-arg "BASE_IMAGE=$base_image" \
  --build-arg "UBUNTU_SNAPSHOT=$snapshot" \
  "$root"

inspect=$(container image inspect --format json "$tag")
digest=$(printf '%s' "$inspect" | cargo run --quiet --locked --offline \
  --manifest-path "$root/scripts/Cargo.toml" --bin validate-image-inspect)
printf '%s@%s\n' "$tag" "$digest" | tee "$artifacts/workspace-image-ref"
