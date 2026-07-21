#!/usr/bin/env bash
set -euo pipefail

tool_root=$(cd "$(dirname "$0")/.." && pwd -P)
root=${GASCAN_GATE_TEST_ROOT:-$tool_root}
artifacts=${GASCAN_GATE_ARTIFACTS:-"$root/.artifacts"}
lock="$root/images/workspace/versions.lock"
container_bin=${CONTAINER_BIN:-container}
if test "$root" = "$tool_root"; then
  isolator=''
else
  isolator=${GASCAN_GATE_SANDBOX_BIN:-}
fi

die() { printf 'offline image gate: %s\n' "$*" >&2; exit 1; }
test -f "$lock" || die "missing versions.lock"
publication=$(awk -F ' = ' '$0 == "[workspace_bundles]" { active=1; next } /^\[/ { active=0 } active && $1 == "publication" { gsub(/^"|"$/, "", $2); print $2; exit }' "$lock")
test "$publication" = published || die "workspace bundle publication is not published"

mode=${1:-}
case "$mode" in cold|warm) test $# -eq 1 || die "usage: $0 cold|warm|corrupt BUNDLE" ;; corrupt) test $# -eq 2 || die "usage: $0 corrupt BUNDLE" ;; *) die "usage: $0 cold|warm|corrupt BUNDLE" ;; esac
if test "$mode" != corrupt && test "$root" = "$tool_root"; then
  die "Apple 1.1 has no reviewed builder-VM network isolator; Gate remains PENDING"
fi

owner_token=${GASCAN_TEST_OWNER_TOKEN:-$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')}
[[ "$owner_token" =~ ^[0-9a-f]{32}$ ]] || die "invalid cleanup ownership token"
cleaning=false
cold_backup=''
cold_cache=false

owned() {
  local name=$1
  local inspect
  inspect=$("$container_bin" inspect "$name" 2>/dev/null) || return 1
  printf '%s' "$inspect" | cargo run --quiet --locked --offline --manifest-path "$tool_root/scripts/Cargo.toml" \
    --bin validate-owned-container -- "$name" "$owner_token" >/dev/null
}
cleanup_name() {
  local name=$1
  if owned "$name"; then
    "$container_bin" stop --time 5 "$name" >/dev/null 2>&1 || true
    owned "$name" && "$container_bin" delete "$name" >/dev/null 2>&1 || true
  fi
}
cleanup() {
  $cleaning && return
  cleaning=true
  cleanup_name "gascan-image-user-test-$owner_token"
  cleanup_name "gascan-image-polyglot-test-$owner_token"
  cleanup_name "gascan-image-gascamp-test-$owner_token"
  if $cold_cache; then
    rm -rf "$artifacts"
    if test -n "$cold_backup"; then mv "$cold_backup" "$artifacts"; fi
  fi
}
assert_current_run_absent() {
  local name
  for name in "gascan-image-user-test-$owner_token" \
    "gascan-image-polyglot-test-$owner_token" "gascan-image-gascamp-test-$owner_token"; do
    ! "$container_bin" inspect "$name" >/dev/null 2>&1 || return 1
  done
}
on_signal() { trap - EXIT INT TERM; cleanup; assert_current_run_absent || exit 1; exit 130; }
trap cleanup EXIT
trap on_signal INT TERM

if test "$mode" = corrupt; then
  record=$2
  case "$record" in ubuntu_packages|mise_runtimes|gascamp_source_vendor) ;; *) die "unknown bundle record: $record" ;; esac
  archive="$artifacts/bundles/$record.tar.zst"
  test -f "$archive" || die "missing bundle archive: $record"
  temp=$(mktemp -d "${TMPDIR:-/tmp}/gascan-corrupt-bundle.XXXXXX")
  trap 'rm -rf "$temp"; cleanup' EXIT
  mkdir "$temp/bundles"
  for bundle in ubuntu_packages mise_runtimes gascamp_source_vendor; do
    cp "$artifacts/bundles/$bundle.tar.zst" "$temp/bundles/$bundle.tar.zst"
  done
  printf '\001' >>"$temp/bundles/$record.tar.zst"
  if GASCAN_WORKSPACE_ARTIFACTS="$temp" "$root/scripts/verify-workspace-image-inputs.sh"; then
    die "corrupt bundle was accepted: $record"
  fi
  die "corruption rejected before Apple build: $record"
fi

artifact_hashes() {
  for path in \
    "$artifacts/bundles/ubuntu_packages.tar.zst" \
    "$artifacts/bundles/mise_runtimes.tar.zst" \
    "$artifacts/bundles/gascamp_source_vendor.tar.zst" \
    "$artifacts/mise-linux-arm64" \
    "$artifacts/playwright-chromium-linux-arm64.zip" \
    "$artifacts/expected-tool-versions.json" \
    "$artifacts/workspace-context/context-manifest.tsv" \
    "$lock"
  do
    test -f "$path" || die "missing exact artifact hash input: $path"
    test ! -L "$path" || die "symlink artifact hash input rejected: $path"
    digest=$(shasum -a 256 "$path" | awk '{print $1}')
    printf '%s  %s\n' "$digest" "$(basename "$path")"
  done | LC_ALL=C sort
}

if test "$mode" = cold; then
  cold_backup="${artifacts}.offline-gate-backup-$owner_token"
  test ! -e "$cold_backup" || die "cold cache backup collision"
  if test -e "$artifacts"; then mv "$artifacts" "$cold_backup"; else cold_backup=''; fi
  mkdir -m 0700 "$artifacts"
  cold_cache=true
  base_image=$(awk -F ' = ' '$1 == "base_image" { gsub(/^"|"$/, "", $2); print $2; exit }' "$lock")
  test -n "$base_image" || die "missing locked base image"
  if "$container_bin" image inspect --format json "$base_image" >/dev/null 2>&1; then
    die "cold mode requires the exact locked base image to be absent"
  fi
  GASCAN_GATE_ARTIFACTS="$artifacts" "$root/scripts/prefetch-workspace-image.sh"
fi
before=$(artifact_hashes)
test -n "$before" || die "missing artifact hash evidence"

if test "${GASCAN_GATE_TEST_SIGNAL:-}" = TERM; then
  name="gascan-image-gascamp-test-$owner_token"
  "$container_bin" create --name "$name" --label dev.gascan.test=true --label "dev.gascan.test.owner=$owner_token" test.invalid >/dev/null
  kill -TERM $$
fi

reference_file="$artifacts/workspace-image-ref"
rm -f "$reference_file"
test -n "$isolator" && test -x "$isolator" || die "supported Apple builder network isolator is unavailable"
build_output=$("$isolator" run -- env GASCAN_GATE_ARTIFACTS="$artifacts" "$root/scripts/build-workspace-image.sh")
if test "${GASCAN_GATE_TEST_BUILD_EVIDENCE:-}" = missing; then rm -f "$reference_file"; fi
if test -n "${GASCAN_GATE_TEST_MUTATE_AFTER_BUILD:-}"; then
  printf 'changed' >>"$artifacts/bundles/$GASCAN_GATE_TEST_MUTATE_AFTER_BUILD.tar.zst"
fi
after=$(artifact_hashes)
test "$before" = "$after" || die "artifact hashes changed during Apple build"

test -f "$reference_file" && test ! -L "$reference_file" && test -O "$reference_file" || die "missing or unsafe image reference evidence"
test "$(wc -l <"$reference_file" | tr -d ' ')" = 1 || die "image reference evidence must be exactly one line"
image=$(cat "$reference_file")
[[ "$image" =~ ^[a-z0-9][a-z0-9._/-]*:[a-zA-Z0-9._-]+@sha256:[0-9a-f]{64}$ ]] || die "malformed image reference evidence"
workspace_tag=$(awk -F ' = ' '$1 == "workspace_tag" { gsub(/^"|"$/, "", $2); print $2; exit }' "$lock")
test "${image%@sha256:*}" = "$workspace_tag" || die "image reference tag differs from versions.lock"
test "$(printf '%s\n' "$build_output" | tail -n 1)" = "$image" || die "build output and image reference differ"
inspect=$("$container_bin" image inspect --format json "$image")
inspected_digest=$(printf '%s' "$inspect" | cargo run --quiet --locked --offline \
  --manifest-path "$tool_root/scripts/Cargo.toml" --bin validate-image-inspect)
test "$inspected_digest" = "${image##*@}" || die "image reference digest differs from structured linux/arm64 inspect"

for smoke in user-and-volumes.sh polyglot-smoke.sh gascamp-smoke.sh; do
  GASCAN_IMAGE_REF_FILE="$reference_file" GASCAN_TEST_OWNER_TOKEN="$owner_token" CONTAINER_BIN="$container_bin" \
    "$root/tests/image/$smoke"
done

cleanup
assert_current_run_absent || die "current-run container remains after cleanup"
trap - EXIT INT TERM
lock_digest=$(shasum -a 256 "$lock" | awk '{print $1}')
printf 'mode=%s\nplatform=linux/arm64\nversions-lock-sha256=%s\nimage=%s\nartifacts-sha256:\n%s\n' \
  "$mode" "$lock_digest" "$image" "$after"
