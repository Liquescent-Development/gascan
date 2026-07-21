#!/usr/bin/env bash
set -euo pipefail

host_url=${1:?test-owned host URL required}
script_dir=$(cd "$(dirname "$0")" && pwd -P)

deny_url() {
  "$script_dir/assert-unreachable.sh" "$1"
}

deny_url http://example.com
deny_url http://1.1.1.1
deny_url http://192.0.2.1
deny_url "$host_url"
