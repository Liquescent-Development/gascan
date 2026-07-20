#!/usr/bin/env bash
set -euo pipefail

port=${1:?guest port required}
case $port in ''|*[!0-9]*) exit 64 ;; esac

nohup python3 -m http.server "$port" --bind 0.0.0.0 \
  </dev/null >/workspace/security-port.log 2>&1 &
printf '%s\n' "$!" >/workspace/security-port.pid
for _ in {1..50}; do
  if curl --silent --fail --max-time 1 "http://127.0.0.1:$port" >/dev/null; then
    exit 0
  fi
  sleep 0.1
done
printf 'guest listener did not become reachable\n' >&2
exit 1
