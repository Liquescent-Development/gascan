#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd -P)
lock="$root/images/workspace/versions.lock"
die() { printf 'workspace image build: %s\n' "$*" >&2; exit 1; }
test -f "$lock" || die "missing image lock"
workspace_build_mode=$(awk -F ' = ' '$1 == "workspace_build_mode" { gsub(/^"|"$/, "", $2); print $2; exit }' "$lock")
case "$workspace_build_mode" in
  connected) exec "$root/scripts/build-connected-workspace-image.sh" ;;
  offline) exec "$root/scripts/build-offline-workspace-image.sh" ;;
  *) die "unsupported exact workspace build mode" ;;
esac
