image_cli_timeout=${GASCAN_IMAGE_CLI_TIMEOUT_SECONDS:-10}
case "$image_cli_timeout" in ''|*[!0-9]*) printf 'invalid image controller timeout\n' >&2; exit 1 ;; esac
test "$image_cli_timeout" -gt 0 || { printf 'invalid image controller timeout\n' >&2; exit 1; }

bounded_container() {
  local command_pid result ticks
  set -m
  "$container_bin" "$@" & command_pid=$!
  set +m
  ticks=$((image_cli_timeout * 20))
  while kill -0 "$command_pid" 2>/dev/null && test "$ticks" -gt 0; do
    sleep 0.05
    ticks=$((ticks - 1))
  done
  if kill -0 "$command_pid" 2>/dev/null; then
    kill -TERM -- "-$command_pid" 2>/dev/null || true
    sleep 0.1
    kill -KILL -- "-$command_pid" 2>/dev/null || true
  fi
  if wait "$command_pid"; then result=0; else result=$?; fi
  return "$result"
}

approved_local_image() {
  local approved=$1 tag digest inspect_reference runnable inspect actual
  digest=${approved##*@}
  case "$approved" in
    gascan-workspace:[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]@sha256:[0-9a-f]*)
      tag=${approved%%@*}
      inspect_reference=$tag
      runnable=$tag
      ;;
    ghcr.io/liquescent-development/gascan/workspace:*@sha256:[0-9a-f]*)
      tag=${approved%%@*}
      inspect_reference="${tag%:*}@$digest"
      runnable=$approved
      ;;
    *)
      printf 'approved image is not a supported unique digest-qualified reference\n' >&2
      return 1
      ;;
  esac
  [[ "$digest" =~ ^sha256:[0-9a-f]{64}$ ]] || {
    printf 'approved image digest is invalid\n' >&2
    return 1
  }
  inspect=$(bounded_container image inspect "$inspect_reference") || {
    printf 'approved local image tag is unavailable\n' >&2
    return 1
  }
  actual=$(printf '%s' "$inspect" | cargo run --quiet --locked --offline \
    --manifest-path "$root/scripts/Cargo.toml" --bin validate-connected-build -- "$inspect_reference") || {
    printf 'approved local image inspection is invalid\n' >&2
    return 1
  }
  test "$actual" = "$digest" || {
    printf 'approved local image tag digest changed\n' >&2
    return 1
  }
  printf '%s\n' "$runnable"
}

owned_container_from_approved_image() {
  local name=$1 token=$2 digest=$3 reference=$4 inspect
  inspect=$(bounded_container inspect "$name") || return 1
  printf '%s' "$inspect" | cargo run --quiet --locked --offline \
    --manifest-path "$root/scripts/Cargo.toml" --bin validate-owned-container -- \
    "$name" "$token" "$digest" "$reference"
}
