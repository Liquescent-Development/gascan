#!/usr/bin/env bash
set -euo pipefail

read -r quota period </sys/fs/cgroup/cpu.max
test "$quota" != max
test "$quota" -eq "$period"
test "$(cat /sys/fs/cgroup/memory.max)" = 268435456

if timeout 2 sh -c 'while :; do :; done'; then
  printf 'bounded CPU workload unexpectedly completed\n' >&2
  exit 1
else
  test "$?" -eq 124
fi

set +e
timeout 12 python3 -c 'x = bytearray(384 * 1024 * 1024); print(len(x))'
memory_status=$?
set -e
if test "$memory_status" -eq 0; then
  printf 'memory workload exceeded the configured ceiling\n' >&2
  exit 1
fi
test "$memory_status" -ne 124
