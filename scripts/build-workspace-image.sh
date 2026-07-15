#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd -P)
lock="$root/images/workspace/versions.lock"
context="$root/.artifacts/workspace-context"
base_image='ubuntu@sha256:7f622ca8766bccb22f04242ecb6f19f770b2f08827dc4b8c707de5e78a6da7ab'

die() { printf 'workspace image build: %s\n' "$*" >&2; exit 1; }
test -f "$lock" || die "missing image lock"
test -d "$context" || die "missing verified workspace context; run scripts/prefetch-workspace-image.sh"

top_value() {
  awk -F ' = ' -v key="$1" '$1 == key { gsub(/^"|"$/, "", $2); print $2; exit }' "$lock"
}

test "$(top_value base_image)" = "$base_image" || die "base image differs from the reviewed exact digest"
tag=$(top_value workspace_tag)
snapshot=$(top_value ubuntu_snapshot)
[[ "$tag" != *:latest ]] || die "latest workspace tag rejected"

cargo run --quiet --locked --offline --manifest-path "$root/scripts/Cargo.toml" --bin prepare-workspace-context -- --verify "$root" "$lock" "$root/.artifacts" "$context"

inspect=$(container image inspect --format json "$base_image")
inspected=$(printf '%s' "$inspect" | cargo run --quiet --locked --offline \
  --manifest-path "$root/scripts/Cargo.toml" --bin validate-image-inspect)
test "$inspected" = "${base_image#ubuntu@}" || die "exact local linux/arm64 base image is missing"

container build \
  --arch arm64 \
  --tag "$tag" \
  --file "$context/Dockerfile" \
  --build-arg "BASE_IMAGE=$base_image" \
  --build-arg "UBUNTU_SNAPSHOT=$snapshot" \
  "$context"

inspect=$(container image inspect --format json "$tag")
digest=$(printf '%s' "$inspect" | cargo run --quiet --locked --offline \
  --manifest-path "$root/scripts/Cargo.toml" --bin validate-image-inspect)
printf '%s@%s\n' "$tag" "$digest" | tee "$root/.artifacts/workspace-image-ref"
