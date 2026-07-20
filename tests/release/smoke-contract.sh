#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/../.." && pwd -P)
fixture=$(mktemp -d "${TMPDIR:-/tmp}/gascan-smoke-contract.XXXXXX")
trap 'rm -rf "$fixture"' EXIT
mkdir -p "$fixture/bin" "$fixture/root" "$fixture/tmp"
log=$fixture/sudo.log
dns_state=$fixture/dns

[[ $(grep -F 'name = "$name"' "$repo_root/packaging/macos/release-smoke.sh" | wc -l | tr -d ' ') -eq 2 ]] || {
  printf 'release smoke must preserve one sandbox identity across network modes\n' >&2
  exit 1
}
! grep -F 'name = "$name-offline"' "$repo_root/packaging/macos/release-smoke.sh" >/dev/null || {
  printf 'release smoke changes identity at one canonical root\n' >&2
  exit 1
}
grep -F 'MISE_OFFLINE=true mise --version' "$repo_root/packaging/macos/release-smoke.sh" >/dev/null || {
  printf 'release smoke mise version check is not command-scoped offline\n' >&2
  exit 1
}

write_fake() {
  local name=$1 body=$2
  printf '#!/usr/bin/env bash\nset -euo pipefail\n%s\n' "$body" >"$fixture/bin/$name"
  chmod 0755 "$fixture/bin/$name"
}

write_fake python3 '
if [[ ${1:-} == -c ]]; then printf "54321\n"; exit 0; fi
exec /bin/sleep 300'
write_fake ps 'printf "Mon Jan  1 00:00:00 2024\n"'
write_fake container '
case "$*" in
  "system dns list --format json")
    if [[ -f $FIXTURE_DNS_STATE ]]; then
      jq -Rn --arg domain "$(cat "$FIXTURE_DNS_STATE")" "[\$domain]"
    else
      printf "[]\n"
    fi
    ;;
  *) exit 64;;
esac'
write_fake sudo '
printf "%s\n" "$*" >>"$FIXTURE_SUDO_LOG"
case "$*" in
  "-n container system dns create "*)
    printf "%s\n" "${!#}" >"$FIXTURE_DNS_STATE"
    exit "${FIXTURE_CREATE_STATUS:-0}"
    ;;
  "-n container system dns delete "*) rm -f "$FIXTURE_DNS_STATE";;
  *) exit 64;;
esac'
write_fake gascan 'exit 42'

run_smoke() {
  PATH="$fixture/bin:$PATH" \
  TMPDIR="$fixture/tmp" \
  FIXTURE_DNS_STATE="$dns_state" \
  FIXTURE_SUDO_LOG="$log" \
  FIXTURE_CREATE_STATUS="${FIXTURE_CREATE_STATUS:-0}" \
  GASCAN_RELEASE_GASCAN="$fixture/bin/gascan" \
    "$repo_root/packaging/macos/release-smoke.sh" 2>&1
}

status=0
output=$(run_smoke) || status=$?
[[ $status -eq 42 ]] || { printf 'release smoke returned %s, expected 42\n%s\n' "$status" "$output" >&2; exit 1; }
[[ $(wc -l <"$log" | tr -d ' ') -eq 2 ]] || { printf 'unexpected sudo invocation count\n' >&2; exit 1; }
create_argv=$(sed -n '1p' "$log")
delete_argv=$(sed -n '2p' "$log")
[[ $create_argv =~ ^-n\ container\ system\ dns\ create\ --localhost\ 203\.0\.113\.113\ gascan-[0-9a-f]{32}\.test$ ]] || {
  printf 'DNS create argv is not exact: %s\n' "$create_argv" >&2
  exit 1
}
[[ $delete_argv =~ ^-n\ container\ system\ dns\ delete\ gascan-[0-9a-f]{32}\.test$ ]] || {
  printf 'DNS cleanup argv is not exact: %s\n' "$delete_argv" >&2
  exit 1
}
[[ ! -e $dns_state ]] || { printf 'DNS fixture state remains\n' >&2; exit 1; }
! compgen -G "$fixture/tmp/gascan-release-root.*" >/dev/null

: >"$log"
status=0
output=$(FIXTURE_CREATE_STATUS=44 run_smoke) || status=$?
[[ $status -eq 44 ]] || { printf 'ambiguous DNS create returned %s, expected 44\n%s\n' "$status" "$output" >&2; exit 1; }
[[ $(wc -l <"$log" | tr -d ' ') -eq 2 ]] || { printf 'ambiguous DNS create was not reconciled by one cleanup delete\n' >&2; exit 1; }
create_argv=$(sed -n '1p' "$log")
delete_argv=$(sed -n '2p' "$log")
[[ $create_argv =~ ^-n\ container\ system\ dns\ create\ --localhost\ 203\.0\.113\.113\ gascan-[0-9a-f]{32}\.test$ ]]
[[ $delete_argv =~ ^-n\ container\ system\ dns\ delete\ gascan-[0-9a-f]{32}\.test$ ]]
[[ ${create_argv##* } == "${delete_argv##* }" ]] || { printf 'DNS cleanup used a different identity\n' >&2; exit 1; }
[[ ! -e $dns_state ]] || { printf 'ambiguous DNS create fixture state remains\n' >&2; exit 1; }
! compgen -G "$fixture/tmp/gascan-release-root.*" >/dev/null

printf 'PASS: Gas Can release smoke command contract\n'
