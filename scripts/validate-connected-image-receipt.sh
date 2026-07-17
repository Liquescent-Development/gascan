#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd -P)
reference_file=${1:?missing reference file}
receipt_file=${2:-"$(dirname "$reference_file")/workspace-image-build.json"}
artifacts=${GASCAN_IMAGE_ARTIFACTS:-"$root/.artifacts"}
lock="$root/images/workspace/versions.lock"
die() { printf 'workspace image receipt: %s\n' "$*" >&2; exit 1; }

test -f "$reference_file" && test ! -L "$reference_file" || die 'missing or unsafe reference'
mode=$(awk -F ' = ' '$1 == "workspace_build_mode" { gsub(/^"|"$/, "", $2); print $2; exit }' "$lock")
if test "$mode" != connected; then
  cat "$reference_file"
  exit 0
fi
test -f "$receipt_file" && test ! -L "$receipt_file" || die 'missing or unsafe JSON receipt'
context_manifest="$artifacts/connected-workspace-context/context-manifest.tsv"
test -f "$context_manifest" && test ! -L "$context_manifest" || die 'missing connected context manifest'
lock_digest=$(shasum -a 256 "$lock" | awk '{print $1}')
context_digest=$(shasum -a 256 "$context_manifest" | awk '{print $1}')
cargo run --quiet --locked --offline --manifest-path "$root/scripts/Cargo.toml" \
  --bin validate-connected-build -- validate-receipt \
  "$reference_file" "$receipt_file" "$lock_digest" "$context_digest"
cat "$reference_file"
