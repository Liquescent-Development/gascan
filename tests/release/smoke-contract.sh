#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/../.." && pwd -P)
fixture=$(mktemp -d "${TMPDIR:-/tmp}/gascan-smoke-contract.XXXXXX")
trap 'rm -rf "$fixture"' EXIT
mkdir -p "$fixture/bin" "$fixture/root" "$fixture/tmp"
log=$fixture/sudo.log
dns_state=$fixture/dns

# shellcheck source=../../packaging/macos/release-common.sh
source "$repo_root/packaging/macos/release-common.sh"

destroyed_id=gate5-release-123-456-0123456789ab
destroyed='[{"actual_state":"absent","sandbox_id":"gate5-release-123-456-0123456789ab"}]'
gascan_assert_destroyed_controller_record "$destroyed" "$destroyed_id"

assert_destroyed_record_rejected() {
  local inventory=$1 label=$2
  if gascan_assert_destroyed_controller_record "$inventory" "$destroyed_id" >/dev/null 2>&1; then
    printf 'invalid destroyed controller record passed: %s\n' "$label" >&2
    exit 1
  fi
}
assert_destroyed_record_rejected '[]' missing
assert_destroyed_record_rejected '[{"actual_state":"running","sandbox_id":"gate5-release-123-456-0123456789ab"}]' running
assert_destroyed_record_rejected '[{"actual_state":"absent","sandbox_id":"gate5-release-123-456-0123456789ab"},{"actual_state":"absent","sandbox_id":"gate5-release-123-456-0123456789ab"}]' duplicate
assert_destroyed_record_rejected '[{"actual_state":"absent","sandbox_id":"another-sandbox-0123456789ab"}]' wrong-id
assert_destroyed_record_rejected '{}' malformed
gascan_assert_destroyed_controller_record \
  '[{"actual_state":"running","sandbox_id":"unrelated-sandbox-0123456789ab"},{"actual_state":"absent","sandbox_id":"gate5-release-123-456-0123456789ab"}]' \
  "$destroyed_id"

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

release_smoke=$repo_root/packaging/macos/release-smoke.sh
for required in \
  '/home/workspace/.local' \
  '/home/workspace/.cache' \
  '/home/workspace/.config' \
  'CARGO_HOME=/home/workspace/.local/share/cargo' \
  'RUSTUP_HOME=/home/workspace/.local/share/rustup' \
  'test "$GOBIN" = /home/workspace/.local/bin' \
  'cfg-if = \"=1.0.4\"' \
  'cargo run --manifest-path "$fixture/rust-app/Cargo.toml"' \
  'cargo install --path "$fixture/rust-bin"' \
  'npm pack "$fixture/npm-bin" --pack-destination "$fixture"' \
  'npm install --global "$fixture/gascan-release-npm-local-1.0.0.tgz"' \
  'go install ./go-bin' \
  'python -m zipfile -c ../gascan_release_python_local-0.1.0-py3-none-any.whl' \
  '"$fixture/gascan_release_python_local-0.1.0-py3-none-any.whl"' \
  'gem build gascan-release-ruby-local.gemspec --output ../ruby-bin.gem' \
  'gem install --local "$fixture/ruby-bin.gem"' \
  '$XDG_CONFIG_HOME/gascan-release-smoke/config'
do
  grep -F "$required" "$release_smoke" >/dev/null || {
    printf 'release smoke omits writable runtime-home proof: %s\n' "$required" >&2
    exit 1
  }
done
grep -F 'configuration.mounts' "$release_smoke" >/dev/null || {
  printf 'release smoke does not inspect exact managed mount targets\n' >&2
  exit 1
}

readme=$repo_root/README.md
for required in \
  'pre-0.1.10' \
  'gascan destroy --yes' \
  'gascan up .' \
  'approximately 1.5 GiB' \
  'tools = "20GiB"' \
  'cache = "10GiB"' \
  'config = "2GiB"' \
  'cargo run' \
  'rustup component add rust-src' \
  'npm install -g typescript' \
  'go install golang.org/x/tools/gopls@latest' \
  'python -m pip install --user ruff' \
  'gem install bundler'
do
  grep -F "$required" "$readme" >/dev/null || {
    printf 'README omits writable storage migration guidance: %s\n' "$required" >&2
    exit 1
  }
done

for required in \
  '| `tools` | `10GiB` | `/home/workspace/.local` |' \
  '| `cache` | `10GiB` | `/home/workspace/.cache` |' \
  '| `config` | `1GiB` | `/home/workspace/.config` |'
do
  grep -F "$required" "$readme" >/dev/null || {
    printf 'README omits exact version-2 storage row: %s\n' "$required" >&2
    exit 1
  }
done

manifest_reference=$repo_root/docs/reference/manifest.md
for required in \
  '| `tools` | `"10GiB"` | `/home/workspace/.local` |' \
  '| `cache` | `"10GiB"` | `/home/workspace/.cache` |' \
  '| `config` | `"1GiB"` | `/home/workspace/.config` |'
do
  grep -F "$required" "$manifest_reference" >/dev/null || {
    printf 'manifest reference omits version-2 storage mount: %s\n' "$required" >&2
    exit 1
  }
done
for obsolete in \
  '| `tools` | `10GiB` | `/home/workspace/.local/share/mise` |' \
  '| `config` | `1GiB` | `/home/workspace/.config/gascan` |' \
  '| `tools` | `"10GiB"` | `/home/workspace/.local/share/mise` |' \
  '| `config` | `"1GiB"` | `/home/workspace/.config/gascan` |' \
  '`/home/workspace/.local/share/mise` managed tools volume' \
  'managed `/home/workspace/.config/gascan` volume'
do
  if grep -F "$obsolete" "$readme" "$manifest_reference" >/dev/null; then
    printf 'public documentation retains obsolete storage boundary: %s\n' "$obsolete" >&2
    exit 1
  fi
done

checklist=$repo_root/docs/release/macos-checklist.md
for required in \
  'managed storage layout version 2' \
  '/home/workspace/.local' \
  '/home/workspace/.cache' \
  '/home/workspace/.config' \
  'crates.io dependency' \
  'local Rust, npm, Go, Python, and Ruby installs' \
  'XDG configuration'
do
  grep -F "$required" "$checklist" >/dev/null || {
    printf 'release checklist omits installed writable-home verification: %s\n' "$required" >&2
    exit 1
  }
done

write_fake() {
  local name=$1 body=$2
  printf '#!/usr/bin/env bash\nset -euo pipefail\n%s\n' "$body" >"$fixture/bin/$name"
  chmod 0755 "$fixture/bin/$name"
}

write_fake python3 '
if [[ ${1:-} == -c ]]; then printf "54321\n"; exit 0; fi
exec /bin/sleep 300'
write_fake ps '
if [[ ${2:-} == 4242 ]]; then
  [[ -f $FIXTURE_SUDO_LOG.daemon ]] || exit 1
  case "${4:-}" in
    command=) cat "$FIXTURE_SUDO_LOG.gascand" ;;
    lstart=) printf "Mon Jan  1 00:00:00 2024\n" ;;
    *) exit 64 ;;
  esac
else
  printf "Mon Jan  1 00:00:00 2024\n"
fi'
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
write_fake kill '
if [[ $* == "-TERM 4242" ]]; then
  rm -f "$FIXTURE_SUDO_LOG.daemon"
  printf "kill:%s\n" "$*" >>"$FIXTURE_SUDO_LOG.daemon-log"
else
  exec /bin/kill "$@"
fi'
write_fake gascan '
[[ -z ${GASCAN_RELEASE_SENTINEL_SECRET+x} ]] || exit 88
if declare -F compgen >/dev/null; then
  compgen -e >/dev/null
  exit 90
fi
case "$*" in
  daemon-attest)
    [[ -z ${GASCAN_APPLE_ATTACH_HELPER+x} ]] || exit 87
    [[ -f $FIXTURE_SUDO_LOG.daemon ]] || exit 1
    jq -nc --arg executable "$(cat "$FIXTURE_SUDO_LOG.gascand")" \
      "{\"pid\":4242,\"executable\":\$executable,\"start_identity\":\"Mon Jan  1 00:00:00 2024\",\"instance_token\":\"fixture-instance-token\"}"
    ;;
  "daemon status --json")
    [[ -z ${GASCAN_APPLE_ATTACH_HELPER+x} ]] || exit 87
    if [[ -f $FIXTURE_SUDO_LOG.daemon ]]; then
      printf "{\"state\":\"running\",\"health\":\"healthy\"}\n"
    else
      printf "{\"state\":\"stopped\",\"health\":\"stopped\"}\n"
    fi
    ;;
  "up "*)
    [[ ${CI:-} == 1 ]] || exit 91
    exit 42
    ;;
  *)
    [[ ${GASCAN_APPLE_ATTACH_HELPER:-} == "$(cat "$FIXTURE_SUDO_LOG.attach-helper")" ]] || exit 86
    [[ ! -f $FIXTURE_SUDO_LOG.daemon ]] || exit 89
    exit 42
    ;;
esac'
write_fake gascan-apple-attach 'exit 0'
write_fake gascand 'exit 0'

/usr/bin/python3 - "$release_smoke" "$fixture/configure-git-driver.py" <<'PY'
import pathlib
import sys

source = pathlib.Path(sys.argv[1]).read_text()
prefix = "gascan_configure_git_from_host_fixture() {\n  python3 - \"$gascan_bin\" \"$sandbox_id\" \"$host_git_config\" <<'PY'\n"
start = source.index(prefix) + len(prefix)
end = source.index("\nPY\n}", start)
pathlib.Path(sys.argv[2]).write_text(source[start:end])
PY
write_fake current-configure-gascan '
[[ $* == "--sandbox fixture-sandbox configure git" ]] || exit 64
printf "Host defaults: Gas Can Release <release-smoke@example.test>\\n"
printf "Use this identity with SSH transport and signed commits? [Y/n] "
if ! IFS= read -r -t 2 answer; then
  exit 75
fi
[[ -z $answer ]] || exit 76
printf "Git: Gas Can Release <release-smoke@example.test>; protocol ssh;\\n"
'
cat >"$fixture/release-host-gitconfig" <<'HOST_GIT_CONFIG'
[user]
	name = Gas Can Release
	email = release-smoke@example.test
HOST_GIT_CONFIG
status=0
output=$(/usr/bin/python3 "$fixture/configure-git-driver.py" \
  "$fixture/bin/current-configure-gascan" fixture-sandbox "$fixture/release-host-gitconfig" 2>&1) || status=$?
[[ $status -eq 0 ]] || {
  printf 'release smoke current host-default configure-git driver failed: status=%s\n%s\n' \
    "$status" "$output" >&2
  exit 1
}


run_smoke() {
  PATH="$fixture/bin:$PATH" \
  TMPDIR="$fixture/tmp" \
  FIXTURE_DNS_STATE="$dns_state" \
  FIXTURE_SUDO_LOG="$log" \
  FIXTURE_CREATE_STATUS="${FIXTURE_CREATE_STATUS:-0}" \
  GASCAN_RELEASE_TESTING=YES \
  GASCAN_RELEASE_APPLE_ATTACH_HELPER="$fixture/bin/gascan-apple-attach" \
  GASCAN_RELEASE_GASCAN="$fixture/bin/gascan" \
  GASCAN_RELEASE_GASCAND="$fixture/bin/gascand" \
    "$repo_root/packaging/macos/release-smoke.sh" 2>&1
}

run_smoke_with_exported_compgen() {
  /usr/bin/env -i \
    PATH="$fixture/bin:$PATH" \
    TMPDIR="$fixture/tmp" \
    FIXTURE_DNS_STATE="$dns_state" \
    FIXTURE_SUDO_LOG="$log" \
    FIXTURE_CREATE_STATUS=0 \
    GASCAN_RELEASE_ENV_SANITIZED=1 \
    GASCAN_RELEASE_TESTING=YES \
    GASCAN_RELEASE_APPLE_ATTACH_HELPER="$fixture/bin/gascan-apple-attach" \
    GASCAN_RELEASE_GASCAN="$fixture/bin/gascan" \
    GASCAN_RELEASE_GASCAND="$fixture/bin/gascand" \
    HOME="$HOME" \
    LOGNAME="${LOGNAME:-}" \
    USER="${USER:-}" \
    /bin/bash --noprofile --norc -c '
      compgen() {
        printf "invoked\n" >"$FIXTURE_SUDO_LOG.function-invoked"
        builtin compgen "$@"
      }
      export -f compgen
      exec /bin/bash "$@"
    ' bash "$repo_root/packaging/macos/release-smoke.sh" 2>&1
}

realpath "$fixture/bin/gascan-apple-attach" >"$log.attach-helper"
realpath "$fixture/bin/gascand" >"$log.gascand"

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

: >"$log"
status=0
output=$(GASCAN_RELEASE_ENV_SANITIZED=1 \
  GASCAN_RELEASE_SENTINEL_SECRET=must-not-reach-child run_smoke) || status=$?
[[ $status -eq 42 ]] || {
  printf 'spoofed sanitizer marker exposed caller state to child: status=%s\n%s\n' \
    "$status" "$output" >&2
  exit 1
}
[[ $(wc -l <"$log" | tr -d ' ') -eq 2 ]]
[[ ! -e $dns_state ]]

: >"$log"
status=0
output=$(run_smoke_with_exported_compgen) || status=$?
[[ $status -eq 42 ]] || {
  printf 'spoofed sanitizer marker exposed exported function to child: status=%s\n%s\n' \
    "$status" "$output" >&2
  exit 1
}
[[ ! -e $log.function-invoked ]] || {
  printf 'spoofed exported function was invoked by release smoke\n' >&2
  exit 1
}
[[ $(wc -l <"$log" | tr -d ' ') -eq 2 ]]
[[ ! -e $dns_state ]]

: >"$log"
: >"$log.daemon-log"
printf 'different-helper\n' >"$log.daemon"
[[ $(< "$log.daemon") != "$(< "$log.attach-helper")" ]]
status=0
output=$(run_smoke) || status=$?
[[ $status -eq 42 ]] || {
  printf 'release smoke reused pre-existing same-path daemon: status=%s\n%s\n' \
    "$status" "$output" >&2
  exit 1
}
[[ ! -e $log.daemon ]]
grep -qx 'kill:-TERM 4242' "$log.daemon-log"
[[ $(wc -l <"$log" | tr -d ' ') -eq 2 ]]
[[ ! -e $dns_state ]]

printf 'PASS: Gas Can release smoke command contract\n'
