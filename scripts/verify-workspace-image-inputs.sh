#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd -P)
lock="$root/images/workspace/versions.lock"
artifacts=${GASCAN_WORKSPACE_ARTIFACTS:-"$root/.artifacts"}
temp=$(mktemp -d "${TMPDIR:-/tmp}/gascan-verify-workspace-inputs.XXXXXX")
trap 'rm -rf "$temp"' EXIT INT TERM

for record in ubuntu_packages mise_runtimes gascamp_source_vendor; do
  archive="$artifacts/bundles/$record.tar.zst"
  test -f "$archive" && test ! -L "$archive" || {
    printf 'workspace input verification: missing or unsafe %s\n' "$record" >&2
    exit 1
  }
  cargo run --quiet --locked --offline --manifest-path "$root/scripts/Cargo.toml" \
    --bin validate-workspace-bundle -- "$lock" "$record" "$archive" "$temp/$record"
done
