#!/usr/bin/env bash
set -euo pipefail

tool_root=$(cd "$(dirname "$0")/.." && pwd -P)
root=${GASCAN_GATE_TEST_ROOT:-$tool_root}
artifacts=${GASCAN_GATE_ARTIFACTS:-"$root/.artifacts"}
container_bin=${CONTAINER_BIN:-container}
reference_file="$artifacts/workspace-image-ref"
receipt_file="$artifacts/workspace-image-build.json"
evidence_file="$root/docs/evidence/connected-workspace-image.md"
approved_file="$root/images/workspace/approved-image.txt"
die() { printf 'connected image gate: %s\n' "$*" >&2; exit 1; }
run_tool() { cargo run --quiet --locked --offline --manifest-path "$tool_root/scripts/Cargo.toml" --bin "$1" -- "${@:2}"; }

test -n "${GASCAMP_READ_TOKEN_FILE:-}" || die 'GASCAMP_READ_TOKEN_FILE is required'
case "$GASCAMP_READ_TOKEN_FILE" in /*) ;; *) die 'secret path must be absolute' ;; esac
case "$GASCAMP_READ_TOKEN_FILE" in "$root"|"$root"/*) die 'secret file must be outside the repository' ;; esac
test -f "$GASCAMP_READ_TOKEN_FILE" && test ! -L "$GASCAMP_READ_TOKEN_FILE" || die 'secret file must be a regular non-symlink'
test "$(stat -f %Lp "$GASCAMP_READ_TOKEN_FILE" 2>/dev/null || stat -c %a "$GASCAMP_READ_TOKEN_FILE")" = 600 || die 'secret file mode must be 0600'
command -v "$container_bin" >/dev/null || die 'container controller is unavailable'
if test "$root" = "$tool_root"; then
  test "$(uname -s)" = Darwin || die 'the live connected gate requires macOS'
  sudo -n true >/dev/null 2>&1 || die 'sudo authorization is required'
  owner_token=$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')
else
  owner_token=${GASCAN_TEST_OWNER_TOKEN:-$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')}
fi
[[ "$owner_token" =~ ^[0-9a-f]{32}$ ]] || die 'invalid cleanup ownership token'

names=("gascan-image-user-test-$owner_token" "gascan-image-polyglot-test-$owner_token" "gascan-image-gascamp-test-$owner_token")
cleaning=false
owned() {
  local name=$1 inspect
  inspect=$("$container_bin" inspect "$name" 2>/dev/null) || return 1
  printf '%s' "$inspect" | run_tool validate-owned-container "$name" "$owner_token" >/dev/null
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
  local name
  for name in "${names[@]}"; do cleanup_name "$name"; done
}
assert_absent() {
  local name
  for name in "${names[@]}"; do ! "$container_bin" inspect "$name" >/dev/null 2>&1 || return 1; done
}
on_signal() { trap - EXIT INT TERM; cleanup; assert_absent || exit 1; exit 130; }
trap cleanup EXIT
trap on_signal INT TERM

if test -n "${GASCAN_GATE_TEST_SIGNAL:-}"; then
  "$container_bin" create --name "${names[0]}" --label dev.gascan.test=true --label "dev.gascan.test.owner=$owner_token" test.invalid >/dev/null
  kill -"$GASCAN_GATE_TEST_SIGNAL" $$
fi

GASCAN_GATE_ARTIFACTS="$artifacts" "$root/scripts/prefetch-connected-workspace-image.sh"
build_output=$(GASCAN_GATE_ARTIFACTS="$artifacts" GASCAMP_READ_TOKEN_FILE="$GASCAMP_READ_TOKEN_FILE" "$root/scripts/build-connected-workspace-image.sh")
image=$(GASCAN_IMAGE_ARTIFACTS="$artifacts" "$root/scripts/validate-connected-image-receipt.sh" "$reference_file" "$receipt_file") || die 'build receipt pair is invalid'
[[ "$image" =~ ^[a-z0-9][a-z0-9._/-]*:[a-zA-Z0-9._-]+@sha256:[0-9a-f]{64}$ ]] || die 'receipt reference is not digest-qualified'
test "$(printf '%s\n' "$build_output" | tail -n 1)" = "$image" || die 'build output and receipt reference differ'
inspect=$("$container_bin" image inspect --format json "$image") || die 'built image is unavailable'
inspected_digest=$(printf '%s' "$inspect" | run_tool validate-image-inspect) || die 'structured image inspection is invalid'
test "$inspected_digest" = "${image##*@}" || die 'inspection digest differs from receipt'

for smoke in user-and-volumes.sh polyglot-smoke.sh gascamp-smoke.sh; do
  GASCAN_IMAGE_REF_FILE="$reference_file" GASCAN_IMAGE_ARTIFACTS="$artifacts" GASCAN_TEST_OWNER_TOKEN="$owner_token" CONTAINER_BIN="$container_bin" CALLS="${CALLS:-}" FAIL_SMOKE="${FAIL_SMOKE:-}" \
    "$root/tests/image/$smoke"
done
cleanup
assert_absent || die 'current-run container residue remains'

mkdir -p "$(dirname "$evidence_file")" "$(dirname "$approved_file")"
evidence_tmp=$(mktemp "$(dirname "$evidence_file")/.connected-workspace-image.XXXXXX")
approved_tmp=$(mktemp "$(dirname "$approved_file")/.approved-image.XXXXXX")
publication_cleanup() { status=$?; rm -f "$evidence_tmp" "$approved_tmp"; exit "$status"; }
trap publication_cleanup EXIT
lock_digest=$(shasum -a 256 "$root/images/workspace/versions.lock" | awk '{print $1}')
receipt_digest=$(shasum -a 256 "$receipt_file" | awk '{print $1}')
printf '# Connected workspace image evidence\n\n- status: `PASS`\n- platform: `linux/arm64`\n- image: `%s`\n- owner token: `%s`\n- versions lock SHA-256: `%s`\n- build receipt SHA-256: `%s`\n- final current-token residue: `absent`\n' "$image" "$owner_token" "$lock_digest" "$receipt_digest" >"$evidence_tmp"
printf '%s' "$image" >"$approved_tmp"
assert_absent || die 'current-run container residue appeared before publication'
mv -f "$approved_tmp" "$approved_file"
mv -f "$evidence_tmp" "$evidence_file"
trap - EXIT INT TERM
printf '%s\n' "$image"
