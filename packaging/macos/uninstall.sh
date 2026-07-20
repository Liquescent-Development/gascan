#!/usr/bin/env bash
set -euo pipefail

remove_data=false
case ${1:-} in
  '') ;;
  --remove-data) remove_data=true ;;
  *) printf 'usage: %s [--remove-data]\n' "$0" >&2; exit 64 ;;
esac
[[ $# -le 1 ]] || { printf 'usage: %s [--remove-data]\n' "$0" >&2; exit 64; }

stop_owned_daemon() {
  command -v gascan >/dev/null || return 0
  local attestation pid executable
  if ! attestation=$(gascan daemon-attest 2>/dev/null); then
    return 0
  fi
  pid=$(jq -er '.pid | select(type == "number" and . > 1 and . < 4294967296)' <<<"$attestation") || {
    printf 'refusing to stop daemon with invalid attestation pid\n' >&2
    return 1
  }
  executable=$(jq -er '.executable | select(type == "string")' <<<"$attestation") || {
    printf 'refusing to stop daemon with invalid executable attestation\n' >&2
    return 1
  }
  [[ $executable == /usr/local/bin/gascand ]] || {
    printf 'refusing to stop unexpected daemon executable: %s\n' "$executable" >&2
    return 1
  }
  kill -TERM "$pid"
  for _ in {1..100}; do
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.05
  done
  if kill -0 "$pid" 2>/dev/null; then
    printf 'installed Gas Can daemon did not stop promptly\n' >&2
    return 1
  fi
  wait "$pid" 2>/dev/null || true
}

if [[ $remove_data == false ]]; then
  printf 'Preserving all sandboxes, volumes, caches, and user state.\n'
else
  command -v gascan >/dev/null || {
    printf 'gascan is required to remove owned data safely\n' >&2
    exit 69
  }
  sandbox_ids=$(gascan list --json | jq -er '[.[]?.sandbox_id] | .[]')
  while IFS= read -r sandbox_id; do
    [[ -n $sandbox_id ]] || continue
    gascan --sandbox "$sandbox_id" destroy --yes
  done <<<"$sandbox_ids"
fi

stop_owned_daemon
sudo rm -f \
  /usr/local/bin/gascan \
  /usr/local/bin/gascand \
  /usr/local/bin/gascan-apple-attach \
  /usr/local/share/gascan/LICENSE \
  /usr/local/share/gascan/default-gascan.toml \
  /usr/local/share/gascan/build-manifest.json
sudo rmdir /usr/local/share/gascan 2>/dev/null || true
if pkgutil --pkg-info dev.gascan.pkg >/dev/null 2>&1; then
  sudo pkgutil --forget dev.gascan.pkg >/dev/null
fi
printf 'Gas Can binaries removed.\n'
