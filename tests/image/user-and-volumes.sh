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
reference_file=${GASCAN_IMAGE_REF_FILE:-"$root/.artifacts/workspace-image-ref"}
container_bin=${CONTAINER_BIN:-container}
source "$root/tests/image/container-cli.sh"
test -f "$reference_file" || { printf 'missing image reference: %s\n' "$reference_file" >&2; exit 1; }
image=$(bash "$root/scripts/validate-connected-image-receipt.sh" "$reference_file")
[[ "$image" =~ ^[a-z0-9][a-z0-9._/-]*:[a-zA-Z0-9._-]+@sha256:[0-9a-f]{64}$ ]] || {
  printf 'image reference is not digest-qualified\n' >&2
  exit 1
}
owner_token=${GASCAN_TEST_OWNER_TOKEN:-$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')}
[[ "$owner_token" =~ ^[0-9a-f]{32}$ ]] || { printf 'invalid owner token\n' >&2; exit 1; }
name="gascan-image-user-test-$owner_token"
cleaning=false

owned() {
  local inspect
  inspect=$(bounded_container inspect "$name") || return 1
  printf '%s' "$inspect" | cargo run --quiet --locked --offline \
    --manifest-path "$root/scripts/Cargo.toml" --bin validate-owned-container -- \
    "$name" "$owner_token"
}

cleanup() {
  $cleaning && return
  cleaning=true
  if owned && owned; then
    bounded_container stop --time 5 "$name" >/dev/null 2>&1 || true
    if owned && owned; then
      bounded_container delete "$name" >/dev/null 2>&1 || true
    fi
  fi
}

on_signal() {
  trap - EXIT INT TERM
  cleanup
  exit 130
}

trap cleanup EXIT
trap on_signal INT TERM

"$container_bin" create \
  --name "$name" \
  --label dev.gascan.test=true \
  --label "dev.gascan.test.owner=$owner_token" \
  --mount "type=bind,source=$root,target=/workspace" \
  "$image" >/dev/null
owned
"$container_bin" start "$name" >/dev/null
"$container_bin" exec "$name" bash /workspace/tests/image/user-and-volumes.sh --inside

started=$(date +%s)
owned && owned || { printf 'container ownership changed before stop\n' >&2; exit 1; }
bounded_container stop --time 5 "$name" >/dev/null
elapsed=$(( $(date +%s) - started ))
test "$elapsed" -le 5
