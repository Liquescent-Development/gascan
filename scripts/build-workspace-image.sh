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
ubuntu_snapshot=$(top_value ubuntu_snapshot)
[[ "$tag" != *:latest ]] || die "latest workspace tag rejected"

context_manifest=$(cargo run --quiet --locked --offline --manifest-path "$root/scripts/Cargo.toml" --bin prepare-workspace-context -- --verify "$root" "$lock" "$root/.artifacts" "$context")
[[ "$context_manifest" =~ ^[0-9a-f]{64}$ ]] || die "context verifier did not emit a canonical manifest digest"

snapshot_helper='/Library/PrivilegedHelperTools/dev.gascan.snapshot-workspace-context'
helper_identity=$(cargo run --quiet --locked --offline --manifest-path "$root/scripts/Cargo.toml" --bin snapshot-helper-identity -- "$snapshot_helper") || die "snapshot helper identity is unsafe"
IFS=$'\t' read -r helper_sha256 helper_device helper_inode <<<"$helper_identity"
receipt=''
cleanup_snapshot() {
  test -z "$receipt" || sudo -n "$snapshot_helper" --self "$helper_sha256" "$helper_device" "$helper_inode" finish "$receipt"
}
trap cleanup_snapshot EXIT INT TERM
receipt=$(sudo -n "$snapshot_helper" --self "$helper_sha256" "$helper_device" "$helper_inode" create "$context" "$context_manifest") || die "root snapshot creation is unavailable"
build_context_snapshot=$(sudo -n "$snapshot_helper" --self "$helper_sha256" "$helper_device" "$helper_inode" path "$receipt") || die "root snapshot validation failed"
test -d "$build_context_snapshot" || die "root snapshot path is unavailable"

inspect=$(container image inspect --format json "$base_image")
inspected=$(printf '%s' "$inspect" | cargo run --quiet --locked --offline \
  --manifest-path "$root/scripts/Cargo.toml" --bin validate-image-inspect)
test "$inspected" = "${base_image#ubuntu@}" || die "exact local linux/arm64 base image is missing"

container build \
  --arch arm64 \
  --tag "$tag" \
  --file "$build_context_snapshot/Dockerfile" \
  --build-arg "BASE_IMAGE=$base_image" \
  --build-arg "UBUNTU_SNAPSHOT=$ubuntu_snapshot" \
  "$build_context_snapshot"

sudo -n "$snapshot_helper" --self "$helper_sha256" "$helper_device" "$helper_inode" finish "$receipt"
receipt=''
trap - EXIT INT TERM

inspect=$(container image inspect --format json "$tag")
digest=$(printf '%s' "$inspect" | cargo run --quiet --locked --offline \
  --manifest-path "$root/scripts/Cargo.toml" --bin validate-image-inspect)
printf '%s@%s\n' "$tag" "$digest" | tee "$root/.artifacts/workspace-image-ref"
