#!/usr/bin/env bash
set -euo pipefail

inside_image() {
  test "$(id -un)" = workspace
  test "$(id -u)" = 1000
  test "$(id -g)" = 1000
  test "$(sudo -n id -u)" = 0

  for directory in \
    /opt/gascan/mise \
    /home/workspace/.cache \
    /home/workspace/.config/gascan
  do
    test "$(stat -c %U "$directory")" = workspace
    test "$(stat -c %G "$directory")" = workspace
  done

  test ! -e /run/host-services/ssh-auth.sock
  test ! -e /var/run/docker.sock
  for status in /proc/[0-9]*/status; do
    ! grep -q '^State:.*Z' "$status"
  done
}

if [[ ${1:-} == --inside ]]; then
  inside_image
  exit 0
fi

root=$(cd "$(dirname "$0")/../.." && pwd -P)
reference_file="$root/.artifacts/workspace-image-ref"
test -f "$reference_file" || { printf 'missing image reference: %s\n' "$reference_file" >&2; exit 1; }
image=$(cat "$reference_file")
name="gascan-image-user-test-$$-$(date +%s)"
created=false

cleanup() {
  if $created; then
    container stop --time 5 "$name" >/dev/null 2>&1 || true
    container delete "$name" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT INT TERM

container run --detach \
  --name "$name" \
  --label dev.gascan.test=true \
  --mount "type=bind,source=$root,target=/workspace" \
  "$image" >/dev/null
created=true
container exec "$name" bash /workspace/tests/image/user-and-volumes.sh --inside

started=$(date +%s)
container stop --time 5 "$name" >/dev/null
elapsed=$(( $(date +%s) - started ))
test "$elapsed" -le 5
container delete "$name" >/dev/null
created=false
