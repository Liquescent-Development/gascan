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

# Retire prior acceptance markers before any validation or connected work.
# Preserve the checked-in PENDING record, but never let a failed rerun retain
# an earlier PASS marker.
rm -f "$approved_file"
if test -f "$evidence_file" && grep -Fq 'status: `PASS`' "$evidence_file"; then rm -f "$evidence_file"; fi

test -n "${GASCAMP_READ_TOKEN_FILE:-}" || die 'GASCAMP_READ_TOKEN_FILE is required'
case "$GASCAMP_READ_TOKEN_FILE" in /*) ;; *) die 'secret path must be absolute' ;; esac
test -f "$GASCAMP_READ_TOKEN_FILE" && test ! -L "$GASCAMP_READ_TOKEN_FILE" || die 'secret file must be a regular non-symlink'
test "$(stat -f %Lp "$GASCAMP_READ_TOKEN_FILE" 2>/dev/null || stat -c %a "$GASCAMP_READ_TOKEN_FILE")" = 600 || die 'secret file mode must be 0600'
test "$(stat -f %u "$GASCAMP_READ_TOKEN_FILE" 2>/dev/null || stat -c %u "$GASCAMP_READ_TOKEN_FILE")" = "$(id -u)" || die 'secret file must be owned by the current user'
canonical_root=$(cd "$root" && pwd -P)
canonical_secret=$(cd "$(dirname "$GASCAMP_READ_TOKEN_FILE")" && printf '%s/%s\n' "$(pwd -P)" "$(basename "$GASCAMP_READ_TOKEN_FILE")")
case "$canonical_secret" in "$canonical_root"|"$canonical_root"/*) die 'secret file must be outside the repository' ;; esac
command -v "$container_bin" >/dev/null || die 'container controller is unavailable'
if test "$root" = "$tool_root"; then
  test "$(uname -s)" = Darwin || die 'the live connected gate requires macOS'
  sudo -n true >/dev/null 2>&1 || die 'sudo authorization is required'
  owner_token=$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')
else
  if test -n "${GASCAN_TEST_OWNER_TOKEN:-}"; then
    owner_token=$GASCAN_TEST_OWNER_TOKEN
  elif test -n "${GASCAN_GATE_RANDOM_BIN:-}"; then
    owner_token=$("$GASCAN_GATE_RANDOM_BIN")
  else
    owner_token=$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')
  fi
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
  if owned "$name" && owned "$name"; then
    "$container_bin" stop --time 5 "$name" >/dev/null 2>&1 || true
    owned "$name" && owned "$name" && "$container_bin" delete "$name" >/dev/null 2>&1 || true
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
evidence_tmp=''
approved_tmp=''
published_evidence=false
committed=false
rollback_publication() {
  test -z "$evidence_tmp" || rm -f "$evidence_tmp"
  test -z "$approved_tmp" || rm -f "$approved_tmp"
  if $published_evidence && ! $committed; then rm -f "$evidence_file" "$approved_file"; fi
}
finish() { status=$?; rollback_publication; cleanup; exit "$status"; }
on_signal() { code=$1; trap - EXIT INT TERM; rollback_publication; cleanup; assert_absent || exit 1; exit "$code"; }
trap cleanup EXIT
trap 'on_signal 130' INT
trap 'on_signal 143' TERM

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
trap finish EXIT
lock_digest=$(shasum -a 256 "$root/images/workspace/versions.lock" | awk '{print $1}')
receipt_digest=$(shasum -a 256 "$receipt_file" | awk '{print $1}')
printf '# Connected workspace image evidence\n\n- status: `PASS`\n- platform: `linux/arm64`\n- image: `%s`\n- versions lock SHA-256: `%s`\n- build receipt SHA-256: `%s`\n- final current-token residue: `absent`\n' "$image" "$lock_digest" "$receipt_digest" >"$evidence_tmp"
printf '%s' "$image" >"$approved_tmp"
assert_absent || die 'current-run container residue appeared before publication'
if test "${GASCAN_GATE_TEST_PUBLICATION_BOUNDARY:-}" = after-stage; then
  case "${GASCAN_GATE_TEST_PUBLICATION_ACTION:-}" in FAIL) false ;; INT) kill -INT $$ ;; TERM) kill -TERM $$ ;; esac
fi
mv -f "$evidence_tmp" "$evidence_file"
evidence_tmp=''
published_evidence=true
if test "${GASCAN_GATE_TEST_PUBLICATION_BOUNDARY:-}" = after-evidence; then
  case "${GASCAN_GATE_TEST_PUBLICATION_ACTION:-}" in FAIL) false ;; INT) kill -INT $$ ;; TERM) kill -TERM $$ ;; esac
fi
mv -f "$approved_tmp" "$approved_file"
approved_tmp=''
committed=true
trap - EXIT INT TERM
printf '%s\n' "$image"
