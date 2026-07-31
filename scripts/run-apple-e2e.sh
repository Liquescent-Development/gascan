#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

candidate_image=
candidate_runtime_image=
live_acceptance_file=
if test -n "${GASCAN_E2E_CANDIDATE_IMAGE_FILE:-}"; then
  test -f "$GASCAN_E2E_CANDIDATE_IMAGE_FILE" || {
    printf 'apple e2e: candidate receipt is unavailable\n' >&2
    exit 1
  }
  test "$(wc -l <"$GASCAN_E2E_CANDIDATE_IMAGE_FILE" | tr -d ' ')" = 1 || {
    printf 'apple e2e: candidate receipt must contain exactly one line\n' >&2
    exit 1
  }
  IFS= read -r candidate_image <"$GASCAN_E2E_CANDIDATE_IMAGE_FILE"
  printf '%s\n' "$candidate_image" |
    grep -Eq '^[a-z0-9]([a-z0-9._-]*[a-z0-9])?(:[0-9]+)?(/[a-z0-9]([a-z0-9._-]*[a-z0-9])?)*:[a-zA-Z0-9_][a-zA-Z0-9._-]{0,127}@sha256:[0-9a-f]{64}$' || {
      printf 'apple e2e: candidate receipt is not an immutable image\n' >&2
      exit 1
  }
  candidate_runtime_image=${candidate_image%%@sha256:*}
  export GASCAN_E2E_CANDIDATE_IMAGE=$candidate_image
  export GASCAN_E2E_CANDIDATE_RUNTIME_IMAGE=$candidate_runtime_image
  if test "${GASCAN_E2E_PREDECESSOR_IMAGE+x}" = x; then
    predecessor_image=$GASCAN_E2E_PREDECESSOR_IMAGE
  else
    predecessor_file=$root/images/workspace/approved-image.txt
    test ! -L "$predecessor_file" && test -f "$predecessor_file" || {
      printf 'apple e2e: predecessor receipt is unavailable\n' >&2
      exit 1
    }
    if ! predecessor_image=$(
      awk '
        NR == 1 { value = $0; next }
        { exit 1 }
        END {
          if (NR != 1) {
            exit 1
          }
          printf "%s", value
        }
      ' "$predecessor_file"
    ); then
      printf 'apple e2e: predecessor receipt must contain exactly one line\n' >&2
      exit 1
    fi
  fi
  test "$(printf '%s' "$predecessor_image" | wc -l | tr -d ' ')" = 0 &&
    printf '%s\n' "$predecessor_image" |
      grep -Eq '^[a-z0-9][a-z0-9._/-]*:[a-zA-Z0-9._-]+@sha256:[0-9a-f]{64}$' || {
        printf 'apple e2e: predecessor image is not immutable\n' >&2
        exit 1
      }
  export GASCAN_E2E_PREDECESSOR_IMAGE=$predecessor_image
  live_acceptance_file=${GASCAN_E2E_LIVE_ACCEPTANCE_FILE:-"$root/.artifacts/connected-workspace-image-apple-live.txt"}
  rm -f "$live_acceptance_file"
fi

cleanup_root=$("$root/scripts/apple-e2e-session-root.sh")
cargo build -p gascan-e2e --bin gascan-e2e-cli
trusted_cli=$(realpath "$root/target/debug/gascan-e2e-cli")
"$root/scripts/build-apple-attach-helper.sh"
helper_candidate="$root/target/gascan-apple-attach"
if ! test -f "$helper_candidate"; then
  printf 'apple e2e preflight: attach helper is not a regular file: %s\n' "$helper_candidate" >&2
  exit 1
fi
if ! test -x "$helper_candidate"; then
  printf 'apple e2e preflight: attach helper is not executable: %s\n' "$helper_candidate" >&2
  exit 1
fi
trusted_helper=$(realpath "$helper_candidate")
if ! test -f "$trusted_helper" || ! test -x "$trusted_helper"; then
  printf 'apple e2e preflight: canonical attach helper is not usable: %s\n' "$trusted_helper" >&2
  exit 1
fi
export GASCAN_APPLE_ATTACH_HELPER=$trusted_helper
session_root=$(mktemp -d "$cleanup_root/session-XXXXXXXXXXXX")
chmod 700 "$session_root"
export GASCAN_E2E_SESSION_ROOT=$session_root
prepare_scoped_session_root() {
  if ! test -e "$session_root" && ! test -L "$session_root"; then
    mkdir -m 700 "$session_root"
  fi
  test -d "$session_root" && test ! -L "$session_root" || {
    printf 'apple e2e: scoped session root is unsafe: %s\n' "$session_root" >&2
    return 1
  }
  if metadata=$(stat -f '%Lp %u' "$session_root" 2>/dev/null); then
    :
  else
    metadata=$(stat -c '%a %u' "$session_root")
  fi
  test "$metadata" = "700 $(id -u)" || {
    printf 'apple e2e: scoped session root metadata changed: %s\n' "$session_root" >&2
    return 1
  }
}
manifest=
cleanup_scoped() {
  result=0
  if test -n "$manifest" && test -f "$manifest"; then
    "$root/scripts/apple-e2e-cleanup.sh" "$manifest" "$trusted_cli" "$cleanup_root" || result=1
    manifest=
  fi
  if test -e "$session_root" && ! rmdir "$session_root"; then
    printf 'apple e2e: scoped session root cleanup failed: %s\n' "$session_root" >&2
    result=1
  fi
  return "$result"
}
finish() {
  status=$?
  trap - EXIT INT TERM HUP
  cleanup_scoped || status=1
  exit "$status"
}
on_signal() {
  status=$1
  trap - EXIT INT TERM HUP
  cleanup_scoped || status=1
  exit "$status"
}
trap finish EXIT
trap 'on_signal 130' INT
trap 'on_signal 143' TERM
trap 'on_signal 129' HUP

for stale in "$cleanup_root"/*.json; do
  test -e "$stale" || continue
  "$root/scripts/apple-e2e-cleanup.sh" "$stale" "$trusted_cli" "$cleanup_root"
done

./scripts/apple-test-preflight.sh

case ${1-} in
  "")
    tests="apple_lifecycle apple_recovery"
    ;;
  apple_lifecycle|apple_recovery|apple_apply|apple_security)
    tests=$1
    ;;
  *)
    printf 'usage: %s [apple_lifecycle|apple_recovery|apple_apply|apple_security]\n' "$0" >&2
    exit 64
    ;;
esac

accepted_candidate=false
for test_name in $tests; do
  prepare_scoped_session_root
  manifest="$cleanup_root/$test_name-$$.json"
  export GASCAN_E2E_CLEANUP_MANIFEST=$manifest
  cargo test -p gascan-e2e --test "$test_name" -- --ignored --test-threads=1 --nocapture
  if test -f "$manifest"; then
    "$root/scripts/apple-e2e-cleanup.sh" "$manifest" "$trusted_cli" "$cleanup_root"
  fi
  manifest=
  if test "$test_name" = apple_apply && test -n "$candidate_image"; then
    accepted_candidate=true
  fi
done

cleanup_scoped
trap - EXIT INT TERM HUP
if $accepted_candidate; then
  mkdir -p "$(dirname "$live_acceptance_file")"
  live_tmp=$(mktemp "$(dirname "$live_acceptance_file")/.connected-workspace-image-apple-live.XXXXXX")
  printf '%s\n' "$candidate_image" >"$live_tmp"
  mv -f "$live_tmp" "$live_acceptance_file"
fi
