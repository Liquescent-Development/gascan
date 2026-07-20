#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/../.." && pwd -P)
fixture=$(mktemp -d "${TMPDIR:-/tmp}/gascan-clean-host-contract.XXXXXX")
trap 'rm -rf "$fixture"' EXIT
mkdir -p "$fixture/bin" "$fixture/root"

cat >"$fixture/bin/pkgutil" <<'EOF'
#!/usr/bin/env bash
[[ ${FIXTURE_RECEIPT:-0} == 0 ]] || exit 0
exit 1
EOF
cat >"$fixture/bin/container" <<'EOF'
#!/usr/bin/env bash
case "$*" in
  'list --all --format json') printf '%s\n' "${FIXTURE_CONTAINERS:-[]}";;
  'volume list --format json') printf '%s\n' "${FIXTURE_VOLUMES:-[]}";;
  'system dns list --format json') printf '%s\n' "${FIXTURE_DNS:-[]}";;
  *) exit 64;;
esac
EOF
chmod 0755 "$fixture/bin/pkgutil" "$fixture/bin/container"
export PATH="$fixture/bin:/usr/bin:/bin:/usr/sbin:/sbin"
source "$repo_root/packaging/macos/release-common.sh"

assert_dirty() {
  if gascan_audit_clean_host fixture "$fixture/runtime" "$fixture/root" >/dev/null 2>&1; then
    printf 'dirty-host fixture passed: %s\n' "$1" >&2
    exit 1
  fi
}

gascan_audit_clean_host clean "$fixture/runtime" "$fixture/root"
export FIXTURE_RECEIPT=1; assert_dirty receipt; export FIXTURE_RECEIPT=0
mkdir -p "$fixture/root/usr/local/bin"; touch "$fixture/root/usr/local/bin/gascan"; assert_dirty installed-path; rm "$fixture/root/usr/local/bin/gascan"
mkdir "$fixture/runtime"; assert_dirty controller-state; rmdir "$fixture/runtime"
export FIXTURE_CONTAINERS='[{"configuration":{"labels":{"dev.gascan.managed-by":"gascan"}}}]'; assert_dirty container; export FIXTURE_CONTAINERS='[]'
export FIXTURE_VOLUMES='[{"configuration":{"labels":{"dev.gascan.managed-by":"gascan"}}}]'; assert_dirty volume; export FIXTURE_VOLUMES='[]'
export FIXTURE_DNS='["gascan-0123456789abcdef0123456789abcdef.test"]'; assert_dirty dns; export FIXTURE_DNS='[]'

printf 'PASS: Gas Can clean-host baseline contract\n'
