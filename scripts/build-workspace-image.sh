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
  local url=$1 expected=$2 destination=$3 temporary actual
  test ${#expected} -eq 64 || { printf 'invalid SHA-256 for %s\n' "$destination" >&2; exit 1; }
  temporary="$destination.tmp.$$"
  trap 'rm -f "$temporary"' RETURN
  curl --fail --silent --show-error --location --proto '=https' --tlsv1.2 --output "$temporary" "$url"
  actual=$(shasum -a 256 "$temporary" | awk '{print $1}')
  test "$actual" = "$expected" || { printf 'SHA-256 mismatch for %s\n' "$url" >&2; exit 1; }
  mv "$temporary" "$destination"
  trap - RETURN
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

container build \
  --arch arm64 \
  --tag "$tag" \
  --file "$root/images/workspace/Dockerfile" \
  --build-arg "BASE_IMAGE=$base_image" \
  --build-arg "UBUNTU_SNAPSHOT=$snapshot" \
  "$root"

inspect=$(container image inspect --format json "$tag")
digest=$(printf '%s' "$inspect" | sed -n 's/.*"digest"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -1)
test -n "$digest" || { printf 'built image inspect omitted digest\n' >&2; exit 1; }
printf '%s@%s\n' "$tag" "$digest" | tee "$artifacts/workspace-image-ref"
