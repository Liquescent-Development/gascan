#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd -P)
revision=f6b248c5926240856dbea83d1d2c5c90ea1c1456
selector="$root/images/workspace/bin/select-gascamp"
dockerfile="$root/images/workspace/Dockerfile"

test -x "$selector"
grep -Fq 'RUN --mount=type=secret,id=gascamp_read_token,required=true' "$dockerfile"
grep -Fq 'git remote add origin https://github.com/Liquescent-Development/gascamp.git' "$dockerfile"
grep -Fq 'fetch --depth=1 origin "$GASCAMP_REVISION"' "$dockerfile"
grep -Fq 'mise exec -- cargo test --locked' "$dockerfile"
grep -Fq 'mise exec -- cargo build --locked --release --bin camp' "$dockerfile"
! grep -Fq 'bundles/gascamp_source_vendor' "$dockerfile"
! grep -Fq '@github.com' "$dockerfile"
grep -Fq '/opt/gascan/gascamp/bin/camp' "$dockerfile"
grep -Fq 'campd' "$dockerfile"
grep -Fq 'select-gascamp' "$dockerfile"

reference_file=${GASCAN_IMAGE_REF_FILE:-"$root/.artifacts/workspace-image-ref"}
test -f "$reference_file" || {
  printf 'missing Gascamp image reference: %s\n' "$reference_file" >&2
  exit 1
}

container_bin=${CONTAINER_BIN:-container}
image=$(cat "$reference_file")
owner_token=${GASCAN_TEST_OWNER_TOKEN:-$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')}
[[ "$owner_token" =~ ^[0-9a-f]{32}$ ]] || { printf 'invalid owner token\n' >&2; exit 1; }
name="gascan-image-gascamp-test-$owner_token"
cleaning=false
owned() {
  local inspect
  inspect=$("$container_bin" inspect "$name") || return 1
  printf '%s' "$inspect" | cargo run --quiet --locked --offline \
    --manifest-path "$root/scripts/Cargo.toml" --bin validate-owned-container -- "$name" "$owner_token"
}
cleanup() {
  $cleaning && return
  cleaning=true
  if owned; then
    "$container_bin" stop --time 5 "$name" >/dev/null 2>&1 || true
    owned && "$container_bin" delete "$name" >/dev/null 2>&1 || true
  fi
}
on_signal() { trap - EXIT INT TERM; cleanup; exit 130; }
trap cleanup EXIT
trap on_signal INT TERM
"$container_bin" create --name "$name" --label dev.gascan.test=true \
  --label "dev.gascan.test.owner=$owner_token" "$image" >/dev/null
owned
"$container_bin" start "$name" >/dev/null
"$container_bin" exec "$name" bash -ceu '
  revision=f6b248c5926240856dbea83d1d2c5c90ea1c1456
  test "$(cat /opt/gascan/gascamp/REVISION)" = "$revision"
  test "$(stat -c %U:%G /opt/gascan/gascamp/REVISION)" = root:root
  test "$(stat -c %a /opt/gascan/gascamp/REVISION)" = 444
  /opt/gascan/gascamp/bin/camp --version
  test -L /opt/gascan/gascamp/bin/campd
  test "$(readlink /opt/gascan/gascamp/bin/campd)" = camp
  timeout 10 /opt/gascan/gascamp/bin/campd --version

  metadata=$(select-gascamp)
  test "$(printf "%s" "$metadata" | jq -r .source)" = bundled
  test "$(printf "%s" "$metadata" | jq -r .revision)" = "$revision"
  test "$(printf "%s" "$metadata" | jq -r .trusted)" = true

  mkdir -p /workspace/gascamp/bin
  cp /opt/gascan/gascamp/bin/camp /workspace/gascamp/bin/camp
  metadata=$(select-gascamp /workspace/gascamp)
  test "$(printf "%s" "$metadata" | jq -r .source)" = workspace
  test "$(printf "%s" "$metadata" | jq -r .path)" = /workspace/gascamp
  test "$(printf "%s" "$metadata" | jq -r .trusted)" = false

  rm /workspace/gascamp/bin/camp
  ln -s /opt/gascan/gascamp/bin/camp /workspace/gascamp/bin/camp
  ! select-gascamp /workspace/gascamp
  rm /workspace/gascamp/bin/camp
  cp /opt/gascan/gascamp/bin/camp /workspace/gascamp/bin/camp

  ! select-gascamp /opt/gascan/gascamp
  ! select-gascamp /workspace/gascamp-link-outside
  mv /workspace/gascamp /workspace/gascamp-real
  ln -s /opt/gascan/gascamp /workspace/gascamp
  ! select-gascamp /workspace/gascamp
'
