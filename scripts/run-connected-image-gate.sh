#!/usr/bin/env bash
set -euo pipefail

tool_root=$(cd "$(dirname "$0")/.." && pwd -P)
configured_root=${GASCAN_GATE_TEST_ROOT:-$tool_root}
root=$(cd "$configured_root" 2>/dev/null && pwd -P) || { printf 'connected image gate: configured root is unavailable\n' >&2; exit 1; }
artifacts=${GASCAN_GATE_ARTIFACTS:-"$root/.artifacts"}
container_bin=${CONTAINER_BIN:-container}
reference_file="$artifacts/workspace-image-ref"
receipt_file="$artifacts/workspace-image-build.json"
evidence_file="$root/docs/evidence/connected-workspace-image.md"
approved_file="$root/images/workspace/approved-image.txt"
die() { printf 'connected image gate: %s\n' "$*" >&2; exit 1; }
run_tool() { cargo run --quiet --locked --offline --manifest-path "$tool_root/scripts/Cargo.toml" --bin "$1" -- "${@:2}"; }
cli_timeout=${GASCAN_GATE_CLI_TIMEOUT_SECONDS:-10}
case "$cli_timeout" in ''|*[!0-9]*) die 'controller timeout must be a positive integer' ;; esac
test "$cli_timeout" -gt 0 || die 'controller timeout must be a positive integer'
run_bounded() {
  local timeout_seconds=$1 command_pid result ticks
  shift
  set -m
  "$@" & command_pid=$!
  set +m
  ticks=$((timeout_seconds * 20))
  while kill -0 "$command_pid" 2>/dev/null && test "$ticks" -gt 0; do sleep 0.05; ticks=$((ticks - 1)); done
  if kill -0 "$command_pid" 2>/dev/null; then
    kill -TERM -- "-$command_pid" 2>/dev/null || true
    sleep 0.1
    kill -KILL -- "-$command_pid" 2>/dev/null || true
  fi
  if wait "$command_pid"; then result=0; else result=$?; fi
  return "$result"
}
controller() { run_bounded "$cli_timeout" "$container_bin" "$@"; }

# Retire prior acceptance markers before any validation or connected work.
# Preserve the checked-in PENDING record, but never let a failed rerun retain
# an earlier PASS marker.
rm -f "$approved_file"
if test -f "$evidence_file" && grep -Fq 'status: `PASS`' "$evidence_file"; then rm -f "$evidence_file"; fi

for name in $(compgen -e); do
  case "$name" in
    GASCAMP_*TOKEN*|GITHUB_TOKEN|GH_TOKEN|GITLAB_TOKEN|DOCKER_AUTH_CONFIG|HTTP_AUTHORIZATION|AUTHORIZATION|AWS_ACCESS_KEY_ID|AWS_SECRET_ACCESS_KEY|AWS_SESSION_TOKEN|BUILD_TOKEN|BUILD_*_TOKEN|*_BUILD_TOKEN|*_BUILD_*_TOKEN|BUILD_CREDENTIAL|BUILD_*_CREDENTIAL|*_BUILD_CREDENTIAL|*_BUILD_*_CREDENTIAL|BUILD_PASSWORD|BUILD_*_PASSWORD|*_BUILD_PASSWORD|*_BUILD_*_PASSWORD|BUILD_SECRET|BUILD_*_SECRET|*_BUILD_SECRET|*_BUILD_*_SECRET)
      test -z "${!name:-}" || die "authentication input is forbidden: $name"
      ;;
  esac
done
command -v "$container_bin" >/dev/null || die 'container controller is unavailable'
if test "$root" = "$tool_root"; then
  test "$(uname -s)" = Darwin || die 'the live connected gate requires macOS'
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
inventory_proves_absent() {
  local inventory
  inventory=$(controller list --all --format json 2>/dev/null) || return 1
  printf '%s' "$inventory" | run_tool validate-container-inventory "$@" >/dev/null
}
classify_name() {
  local name=$1 inspect
  if inspect=$(controller inspect "$name" 2>/dev/null); then
    if printf '%s' "$inspect" | run_tool validate-owned-container "$name" "$owner_token" >/dev/null; then printf 'owned\n'; return 0; fi
    printf 'foreign\n'; return 2
  fi
  if inventory_proves_absent "$name"; then printf 'absent\n'; return 0; fi
  printf 'indeterminate\n'; return 3
}
owned() {
  test "$(classify_name "$1")" = owned
}
cleanup_name() {
  local name=$1 state
  state=$(classify_name "$name") || return 1
  test "$state" = absent && return 0
  test "$state" = owned || return 1
  owned "$name" || return 1
  controller stop --time 5 "$name" >/dev/null 2>&1 || true
  owned "$name" && owned "$name" || return 1
  controller delete "$name" >/dev/null 2>&1 || true
  inventory_proves_absent "$name"
}
cleanup() {
  $cleaning && return
  cleaning=true
  local name result=0
  for name in "${names[@]}"; do cleanup_name "$name" || result=1; done
  return "$result"
}
assert_absent() {
  inventory_proves_absent "${names[@]}"
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
finish() { status=$?; rollback_publication; cleanup || status=1; exit "$status"; }
on_signal() { code=$1; trap - EXIT INT TERM; rollback_publication; cleanup; assert_absent || exit 1; exit "$code"; }
trap cleanup EXIT
trap 'on_signal 130' INT
trap 'on_signal 143' TERM

if test -n "${GASCAN_GATE_TEST_SIGNAL:-}"; then
  "$container_bin" create --name "${names[0]}" --label dev.gascan.test=true --label "dev.gascan.test.owner=$owner_token" test.invalid >/dev/null
  kill -"$GASCAN_GATE_TEST_SIGNAL" $$
fi

GASCAN_GATE_ARTIFACTS="$artifacts" "$root/scripts/prefetch-connected-workspace-image.sh"
build_output=$(GASCAN_GATE_ARTIFACTS="$artifacts" "$root/scripts/build-connected-workspace-image.sh")
image=$(GASCAN_IMAGE_ARTIFACTS="$artifacts" "$root/scripts/validate-connected-image-receipt.sh" "$reference_file" "$receipt_file") || die 'build receipt pair is invalid'
[[ "$image" =~ ^[a-z0-9][a-z0-9._/-]*:[a-zA-Z0-9._-]+@sha256:[0-9a-f]{64}$ ]] || die 'receipt reference is not digest-qualified'
test "$(printf '%s\n' "$build_output" | tail -n 1)" = "$image" || die 'build output and receipt reference differ'
inspect=$("$container_bin" image inspect "$image") || die 'built image is unavailable'
inspected_digest=$(printf '%s' "$inspect" | run_tool validate-connected-build "${image%%@*}") || die 'structured image inspection is invalid'
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
