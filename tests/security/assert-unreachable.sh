#!/usr/bin/env bash
set -euo pipefail

url=${1:?URL required}

command -v curl >/dev/null 2>&1 || {
  printf 'curl is required for the offline connectivity probe\n' >&2
  exit 69
}

# Any completed HTTP response proves transport reachability, regardless of
# application status. Curl's exit status remains nonzero for DNS, connect, and
# bounded transport failures because --fail is intentionally absent.
if timeout 4 curl --silent --show-error --connect-timeout 2 --max-time 3 \
  --output /dev/null "$url"; then
  printf 'offline target unexpectedly reachable: %s\n' "$url" >&2
  exit 1
fi
