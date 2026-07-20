#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/../.." && pwd -P)
source "$repo_root/packaging/macos/release-common.sh"

remove_data=false
case ${1:-} in
  '') ;;
  --remove-data) remove_data=true ;;
  *) printf 'usage: %s [--remove-data]\n' "$0" >&2; exit 64 ;;
esac
[[ $# -le 1 ]] || { printf 'usage: %s [--remove-data]\n' "$0" >&2; exit 64; }

if [[ $remove_data == false ]]; then
  printf 'Preserving all sandboxes, volumes, caches, and user state.\n'
else
  command -v gascan >/dev/null || {
    printf 'gascan is required to remove owned data safely\n' >&2
    exit 69
  }
  sandbox_json=$(gascan list --json)
  jq -e '
    type == "array" and
    all(.[]; type == "object" and (.sandbox_id | type == "string" and length > 0)) and
    ([.[].sandbox_id] | length == (unique | length))
  ' <<<"$sandbox_json" >/dev/null || {
    printf 'sandbox inventory is malformed or ambiguous\n' >&2
    exit 65
  }
  sandbox_ids=$(jq -r '.[].sandbox_id' <<<"$sandbox_json")
  while IFS= read -r sandbox_id; do
    [[ -n $sandbox_id ]] || continue
    gascan --sandbox "$sandbox_id" destroy --yes
  done <<<"$sandbox_ids"
fi

gascan_stop_attested_daemon gascan /usr/local/bin/gascand
if [[ $remove_data == true ]]; then
  runtime_root=${XDG_RUNTIME_DIR:-/tmp/gascan-$(id -u)}/gascan
  if [[ -e $runtime_root || -L $runtime_root ]]; then
    [[ -d $runtime_root && ! -L $runtime_root ]] || {
      printf 'refusing unsafe controller-state path: %s\n' "$runtime_root" >&2
      exit 65
    }
    owner=$(stat -f '%u' "$runtime_root")
    mode=$(stat -f '%Lp' "$runtime_root")
    [[ $owner == "$(id -u)" && $mode == 700 ]] || {
      printf 'refusing controller-state directory with unsafe ownership or mode\n' >&2
      exit 65
    }
    rm -rf "$runtime_root"
  fi
fi
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
