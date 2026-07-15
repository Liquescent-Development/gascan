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
