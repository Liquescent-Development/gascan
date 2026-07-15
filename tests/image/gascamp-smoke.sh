#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd -P)
revision=f6b248c5926240856dbea83d1d2c5c90ea1c1456
selector="$root/images/workspace/bin/select-gascamp"
dockerfile="$root/images/workspace/Dockerfile"

test -x "$selector"
grep -Fq "printf '%s\\n' $revision > /out/REVISION" "$dockerfile"
grep -Fq 'bundles/gascamp_source_vendor/tree/source/' "$dockerfile"
grep -Fq 'bundles/gascamp_source_vendor/tree/vendor/' "$dockerfile"
grep -Fq '/opt/gascan/mise/installs/rust/1.97.0/bin/cargo test --locked --offline --frozen' "$dockerfile"
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
"$container_bin" run --rm "$image" bash -ceu '
  revision=f6b248c5926240856dbea83d1d2c5c90ea1c1456
  test "$(cat /opt/gascan/gascamp/REVISION)" = "$revision"
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
