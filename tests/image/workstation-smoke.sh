#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd -P)
reference_file=${GASCAN_IMAGE_REF_FILE:-"$root/.artifacts/workspace-image-ref"}
container_bin=${CONTAINER_BIN:-container}
source "$root/tests/image/container-cli.sh"
image=$(bash "$root/scripts/validate-connected-image-receipt.sh" "$reference_file")
[[ "$image" =~ ^[a-z0-9][a-z0-9._/-]*:[a-zA-Z0-9._-]+@sha256:[0-9a-f]{64}$ ]] ||
  { printf 'workstation smoke: image reference is not digest-qualified\n' >&2; exit 1; }
image_digest=${image##*@}
local_image=$(approved_local_image "$image")
owner_token=${GASCAN_TEST_OWNER_TOKEN:-$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')}
[[ "$owner_token" =~ ^[0-9a-f]{32}$ ]] || { printf 'workstation smoke: invalid owner token\n' >&2; exit 1; }
name="gascan-image-workstation-test-$owner_token"
tools_volume="gascan-image-workstation-tools-$owner_token"
cache_volume="gascan-image-workstation-cache-$owner_token"
config_volume="gascan-image-workstation-config-$owner_token"
volumes=("$tools_volume" "$cache_volume" "$config_volume")
cleaning=false

owned() {
  local inspect
  inspect=$(bounded_container inspect "$name") || return 1
  printf '%s' "$inspect" | cargo run --quiet --locked --offline \
    --manifest-path "$root/scripts/Cargo.toml" --bin validate-owned-container -- \
    "$name" "$owner_token"
}
owned_image() {
  owned_container_from_approved_image "$name" "$owner_token" "$image_digest" "$local_image"
}
owned_volume() {
  local volume=$1 inspect
  inspect=$(bounded_container volume inspect "$volume") || return 1
  printf '%s' "$inspect" | cargo run --quiet --locked --offline \
    --manifest-path "$root/scripts/Cargo.toml" --bin validate-owned-volume -- \
    "$volume" "$owner_token"
}
volume_inventory_proves_absent() {
  local inventory
  inventory=$(bounded_container volume list --format json) || return 1
  printf '%s' "$inventory" | cargo run --quiet --locked --offline \
    --manifest-path "$root/scripts/Cargo.toml" --bin validate-container-inventory -- \
    "${volumes[@]}"
}
cleanup() {
  $cleaning && return
  cleaning=true
  if owned && owned; then
    bounded_container stop --time 5 "$name" >/dev/null 2>&1 || true
    owned && owned && bounded_container delete "$name" >/dev/null 2>&1 || true
  fi
  local volume
  for volume in "${volumes[@]}"; do
    if owned_volume "$volume" && owned_volume "$volume"; then
      bounded_container volume delete "$volume" >/dev/null 2>&1 || true
    fi
  done
}
on_signal() { trap - EXIT INT TERM; cleanup; exit 130; }
trap cleanup EXIT
trap on_signal INT TERM

for specification in \
  "$tools_volume:10737418240" \
  "$cache_volume:10737418240" \
  "$config_volume:1073741824"
do
  volume=${specification%%:*}
  size=${specification#*:}
  "$container_bin" volume create -s "$size" \
    --label dev.gascan.test=true \
    --label "dev.gascan.test.owner=$owner_token" \
    "$volume" >/dev/null
  owned_volume "$volume"
done

"$container_bin" create --name "$name" --init --label dev.gascan.test=true \
  --label "dev.gascan.test.owner=$owner_token" --network none \
  --volume "$tools_volume:/home/workspace/.local/share/mise" \
  --volume "$cache_volume:/home/workspace/.cache" \
  --volume "$config_volume:/home/workspace/.config/gascan" \
  --env HOME=/home/workspace \
  --env MISE_CACHE_DIR=/home/workspace/.cache/mise \
  --env MISE_DATA_DIR=/home/workspace/.local/share/mise \
  --env MISE_GLOBAL_CONFIG_FILE=/home/workspace/.config/gascan/mise.toml \
  --env MISE_STATE_DIR=/home/workspace/.config/gascan/mise-state \
  --env MISE_SYSTEM_DATA_DIR=/opt/gascan/mise \
  --env PATH=/home/workspace/.local/share/mise/shims:/opt/gascan/mise/shims:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
  "$local_image" >/dev/null
owned_image
"$container_bin" start "$name" >/dev/null
owned_image
"$container_bin" exec "$name" sudo -n install -d -o workspace -g workspace -m 0700 \
  /home/workspace/.local/share/mise \
  /home/workspace/.cache \
  /home/workspace/.config/gascan
"$container_bin" exec "$name" env HOME=/home/workspace \
  /usr/local/bin/configure-workstation-home
"$container_bin" exec "$name" /opt/gascan/tests/workstation-contract.sh
"$container_bin" exec "$name" sh -c '
  set -eu
  nslookup -version 2>&1 | grep -Eq "^nslookup 9[.]"
  curl --fail --silent file:///etc/os-release | grep -Fq "ID=ubuntu"
  wget --version | sed -n "1p" | grep -Fq "GNU Wget"
  rsync --version | sed -n "1p" | grep -Fq "rsync  version"
  lsof -v 2>&1 | grep -Fq "revision:"
  file /bin/sh | grep -Fq "symbolic link to dash"
  printf "{\"ready\":true}\n" | jq -e ".ready == true" >/dev/null
  ps -o comm= -p $$ | grep -Eq "^[[:space:]]*sh$"
  top -b -n 1 | grep -Fq "PID"
  pstree -p $$ | grep -Fq "sh("
  tree --version | sed -n "1p" | grep -Eq "^tree v[0-9]"
  test "$(printf gascan-less | less -F -X)" = gascan-less
'
cleanup
volume_inventory_proves_absent
