#!/usr/bin/env bash
set -euo pipefail

host_url=${1:?test-owned host URL required}

deny_url() {
  if timeout 4 curl --silent --show-error --fail --max-time 2 "$1" >/dev/null 2>&1; then
    printf 'offline target unexpectedly reachable: %s\n' "$1" >&2
    exit 1
  fi
}

deny_url http://example.com
deny_url http://1.1.1.1
deny_url http://192.0.2.1
deny_url "$host_url"
