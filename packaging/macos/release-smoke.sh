#!/bin/bash -p

gascan_release_environment_is_sanitized() {
  builtin local name
  [[ -z $(builtin export -pf) ]] || return 1
  [[ ${GASCAN_RELEASE_ENV_SANITIZED:-} == 1 ]] || return 1
  while IFS= builtin read -r name; do
    case $name in
      FIXTURE_CREATE_STATUS|FIXTURE_DNS_STATE|FIXTURE_SUDO_LOG|\
      GASCAN_RELEASE_APPLE_ATTACH_HELPER|GASCAN_RELEASE_ENV_SANITIZED|\
      GASCAN_RELEASE_GASCAN|GASCAN_RELEASE_GASCAND|GASCAN_RELEASE_TESTING|\
      GASCAN_RELEASE_TEST_SIGNAL_AFTER_TRAPS|HOME|LOGNAME|PATH|TMPDIR|USER|\
      PWD|SHLVL) ;;
      *) return 1 ;;
    esac
  done < <(builtin compgen -e)
}

if [[ $- != *p* ]] || ! gascan_release_environment_is_sanitized; then
  release_path=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin
  if [[ ${GASCAN_RELEASE_TESTING:-} == YES ]]; then
    release_path=$PATH
  fi
  /usr/bin/env -i \
    FIXTURE_CREATE_STATUS="${FIXTURE_CREATE_STATUS:-}" \
    FIXTURE_DNS_STATE="${FIXTURE_DNS_STATE:-}" \
    FIXTURE_SUDO_LOG="${FIXTURE_SUDO_LOG:-}" \
    GASCAN_RELEASE_ENV_SANITIZED=1 \
    GASCAN_RELEASE_APPLE_ATTACH_HELPER="${GASCAN_RELEASE_APPLE_ATTACH_HELPER:-}" \
    GASCAN_RELEASE_GASCAN="${GASCAN_RELEASE_GASCAN:-}" \
    GASCAN_RELEASE_GASCAND="${GASCAN_RELEASE_GASCAND:-}" \
    GASCAN_RELEASE_TESTING="${GASCAN_RELEASE_TESTING:-}" \
    GASCAN_RELEASE_TEST_SIGNAL_AFTER_TRAPS="${GASCAN_RELEASE_TEST_SIGNAL_AFTER_TRAPS:-}" \
    HOME="$HOME" \
    LOGNAME="${LOGNAME:-}" \
    PATH="$release_path" \
    TMPDIR="${TMPDIR:-/tmp}" \
    USER="${USER:-}" \
    /bin/bash --noprofile --norc -p "$0" "$@"
else
  builtin set -euo pipefail
  builtin set +p

repo_root=$(cd "$(dirname "$0")/../.." && pwd -P)
source "$repo_root/packaging/macos/release-common.sh"

gascan_bin=${GASCAN_RELEASE_GASCAN:-/usr/local/bin/gascan}
gascand_bin=${GASCAN_RELEASE_GASCAND:-/usr/local/bin/gascand}
apple_attach_bin=${GASCAN_RELEASE_APPLE_ATTACH_HELPER:-/usr/local/bin/gascan-apple-attach}
[[ -x $gascan_bin ]] || { printf 'installed gascan is unavailable\n' >&2; exit 69; }
[[ -x $gascand_bin ]] || { printf 'installed gascand is unavailable\n' >&2; exit 69; }
[[ -x $apple_attach_bin ]] || { printf 'installed attach helper is unavailable\n' >&2; exit 69; }
apple_attach_bin=$(realpath "$apple_attach_bin") || { printf 'attach helper path is unavailable\n' >&2; exit 69; }
export GASCAN_DAEMON=$gascand_bin

gascan_release_preflight_daemon() {
  local status
  if "$gascan_bin" daemon-attest >/dev/null 2>&1; then
    if ! gascan_stop_attested_daemon "$gascan_bin" "$gascand_bin"; then
      printf 'release smoke refused unsafe or mismatched pre-existing Gas Can daemon\n' >&2
      return 1
    fi
  fi
  status=$("$gascan_bin" daemon status --json 2>/dev/null) || {
    printf 'release smoke could not prove the selected daemon is stopped\n' >&2
    return 1
  }
  if ! jq -e '.state == "stopped" and .health == "stopped"' <<<"$status" >/dev/null; then
    printf 'release smoke could not prove the selected daemon is stopped\n' >&2
    return 1
  fi
}

gascan_release_up() {
  "$gascan_bin" up "$root" </dev/null
}

gascan_default_shell_probe() {
  python3 - "$gascan_bin" "$sandbox_id" <<'PY'
import errno
import os
import pty
import select
import subprocess
import sys
import time

gascan, sandbox_id = sys.argv[1:]
controller, user = pty.openpty()
environment = os.environ.copy()
environment["TERM"] = "gascan-release-term"
command = [gascan, "--sandbox", sandbox_id, "shell"]
process = subprocess.Popen(
    command,
    stdin=user,
    stdout=user,
    stderr=user,
    close_fds=True,
    env=environment,
)
os.close(user)
captured = bytearray()


def read_until(marker, deadline):
    while marker not in captured:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            process.kill()
            process.wait()
            raise SystemExit(
                "default shell marker timed out: "
                + captured[-4096:].decode("utf-8", "backslashreplace")
            )
        readable, _, _ = select.select([controller], [], [], min(remaining, 0.1))
        if not readable:
            if process.poll() is not None:
                raise SystemExit(
                    "default shell exited before marker: "
                    + captured[-4096:].decode("utf-8", "backslashreplace")
                )
            continue
        try:
            chunk = os.read(controller, 16384)
        except OSError as error:
            if error.errno == errno.EIO:
                chunk = b""
            else:
                raise
        if not chunk:
            if process.poll() is not None:
                raise SystemExit(
                    "default shell exited before marker: "
                    + captured[-4096:].decode("utf-8", "backslashreplace")
                )
            continue
        captured.extend(chunk)
        if len(captured) > 1024 * 1024:
            raise SystemExit("default shell output exceeded its limit")


try:
    os.write(
        controller,
        b"stty -echo; printf 'GASCAN_%s\\n' SHELL_INPUT_READY\n",
    )
    read_until(b"GASCAN_SHELL_INPUT_READY", time.monotonic() + 15)
    os.write(
        controller,
        b"""PROMPT_COMMAND=; PS1= PS2=
printf 'GASCAN_RELEASE_SHELL_BEGIN\\n'
printf 'BASH_VERSION=%s\\n' "${BASH_VERSION:-}"
case $- in *i*) printf 'INTERACTIVE=yes\\n';; *) printf 'INTERACTIVE=no\\n';; esac
if shopt -q login_shell; then printf 'LOGIN=yes\\n'; else printf 'LOGIN=no\\n'; fi
printf 'SHELL=%s\\n' "${SHELL:-}"
if test -r /usr/share/bash-completion/bash_completion; then
    printf 'COMPLETION=/usr/share/bash-completion/bash_completion\\n'
else
    printf 'COMPLETION=missing\\n'
fi
printf 'TERM=%s\\n' "${TERM:-}"
printf 'SELECTOR=%s\\n' "$(< /home/workspace/.config/gascan/shell/prompt)"
printf 'STARSHIP_CONFIG=%s\\n' "${STARSHIP_CONFIG:-}"
printf 'STARSHIP_EXECUTABLE=%s\\n' "${STARSHIP_EXECUTABLE:-}"
printf 'STARSHIP_FUNCTION=%s\\n' "$(type -t starship_precmd || true)"
/bin/bash --login -i -c 'printf "NESTED_STARSHIP_CONFIG=%s\\n" "${STARSHIP_CONFIG:-}"; printf "NESTED_STARSHIP_EXECUTABLE=%s\\n" "${STARSHIP_EXECUTABLE:-}"; printf "NESTED_STARSHIP_FUNCTION=%s\\n" "$(type -t starship_precmd || true)"'
printf 'GASCAN_RELEASE_SHELL_END\\n'
exit 0
""",
    )
    read_until(b"GASCAN_RELEASE_SHELL_END", time.monotonic() + 30)
    try:
        status = process.wait(timeout=10)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()
        raise SystemExit("default shell did not exit cleanly")
    if status != 0:
        raise SystemExit(f"default shell exited with {status}")
finally:
    try:
        if process.poll() is None:
            try:
                process.terminate()
            except ProcessLookupError:
                pass
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                try:
                    process.kill()
                except ProcessLookupError:
                    pass
                process.wait()
        else:
            process.wait()
    finally:
        os.close(controller)

normalized = bytes(captured).replace(b"\r", b"")
begin = b"GASCAN_RELEASE_SHELL_BEGIN\n"
end = b"GASCAN_RELEASE_SHELL_END\n"
start = normalized.find(begin)
if start < 0:
    raise SystemExit("default shell output omitted begin marker")
start += len(begin)
finish = normalized.find(end, start)
if finish < 0:
    raise SystemExit("default shell output omitted end marker")
sys.stdout.buffer.write(normalized[start:finish])
PY
}

gascan_assert_shell_field() {
  local selector=$1 required=$2 captured=$3 field
  field=${required%%=*}
  if grep -Fx -- "$required" <<<"$captured" >/dev/null; then
    return 0
  fi
  printf 'shell probe field mismatch: selector=%s field=%s expected=%s\n' \
    "$selector" "$field" "$required" >&2
  printf 'captured shell output (last 4096 characters):\n%s\n' \
    "${captured: -4096}" >&2
  return 1
}

gascan_assert_shell_pattern() {
  local selector=$1 field=$2 pattern=$3 captured=$4
  if grep -E -- "$pattern" <<<"$captured" >/dev/null; then
    return 0
  fi
  printf 'shell probe pattern mismatch: selector=%s field=%s expected=%s\n' \
    "$selector" "$field" "$pattern" >&2
  printf 'captured shell output (last 4096 characters):\n%s\n' \
    "${captured: -4096}" >&2
  return 1
}

gascan_configure_git_from_host_fixture() {
  python3 - "$gascan_bin" "$sandbox_id" "$host_git_config" <<'PY'
import errno
import os
import pty
import select
import subprocess
import sys
import time

gascan, sandbox_id, host_git_config = sys.argv[1:]
controller, user = pty.openpty()
environment = os.environ.copy()
environment["TERM"] = "gascan-release-term"
environment["GIT_CONFIG_GLOBAL"] = host_git_config
process = subprocess.Popen(
    [gascan, "--sandbox", sandbox_id, "configure", "git"],
    stdin=user,
    stdout=user,
    stderr=user,
    close_fds=True,
    env=environment,
)
os.close(user)
captured = bytearray()


def read_once(timeout):
    readable, _, _ = select.select([controller], [], [], timeout)
    if not readable:
        return False
    try:
        chunk = os.read(controller, 16384)
    except OSError as error:
        if error.errno == errno.EIO:
            return False
        raise
    if not chunk:
        return False
    captured.extend(chunk)
    if len(captured) > 1024 * 1024:
        raise SystemExit("developer configuration output exceeded its limit")
    return True


def answer_prompt(marker):
    deadline = time.monotonic() + 30
    while marker not in captured:
        if time.monotonic() >= deadline:
            raise SystemExit(
                "developer configuration prompt timed out: "
                + captured[-4096:].decode("utf-8", "backslashreplace")
            )
        if not read_once(0.1) and process.poll() is not None:
            raise SystemExit(
                "developer configuration exited before prompt: "
                + captured[-4096:].decode("utf-8", "backslashreplace")
            )
    os.write(controller, b"\n")


try:
    for marker in [
        b"Git name: ",
        b"Git email: ",
        b"Git protocol (ssh or https): ",
    ]:
        answer_prompt(marker)
    deadline = time.monotonic() + 120
    while process.poll() is None:
        if time.monotonic() >= deadline:
            process.kill()
            process.wait()
            raise SystemExit("developer configuration did not exit")
        read_once(0.1)
    while read_once(0):
        pass
    status = process.wait()
    if status != 0:
        raise SystemExit(
            f"developer configuration exited with {status}: "
            + captured[-4096:].decode("utf-8", "backslashreplace")
        )
finally:
    try:
        if process.poll() is None:
            process.kill()
        process.wait()
    finally:
        os.close(controller)

normalized = bytes(captured).replace(b"\r", b"")
for expected in [
    b"Host defaults: Gas Can Release <release-smoke@example.test>",
    b"Git: Gas Can Release <release-smoke@example.test>; protocol ssh;",
]:
    if expected not in normalized:
        raise SystemExit(
            "developer configuration omitted imported identity evidence: "
            + normalized[-4096:].decode("utf-8", "backslashreplace")
        )
PY
}

root=$(mktemp -d "${TMPDIR:-/tmp}/gascan-release-root.XXXXXX")
name="gate5-release-$PPID-$$"
sandbox_id=
destroyed_sandbox_id=
dns_domain=
server_pid=
server_start=

cleanup() {
  local cleanup_status=0 observed_start
  if [[ -n $sandbox_id ]]; then
    "$gascan_bin" --sandbox "$sandbox_id" destroy --yes >/dev/null 2>&1 || cleanup_status=1
  fi
  if [[ -n $dns_domain ]]; then
    sudo -n container system dns delete "$dns_domain" >/dev/null 2>&1 || cleanup_status=1
    dns_inventory=$(container system dns list --format json 2>/dev/null) || cleanup_status=1
    jq -e --arg domain "$dns_domain" 'type == "array" and all(.[]; . != $domain)' <<<"${dns_inventory:-}" >/dev/null 2>&1 || cleanup_status=1
  fi
  if [[ -n $server_pid ]]; then
    observed_start=$(ps -p "$server_pid" -o lstart= 2>/dev/null | sed 's/^ *//;s/ *$//' || true)
    if [[ $observed_start == "$server_start" ]]; then
      kill "$server_pid" 2>/dev/null || cleanup_status=1
    elif [[ -n $observed_start ]]; then
      printf 'refusing reused host-server pid during cleanup\n' >&2
      cleanup_status=1
    fi
  fi
  rm -rf "$root"
  if [[ $cleanup_status -ne 0 ]]; then
    printf 'release smoke cleanup left recorded resources\n' >&2
  fi
  return "$cleanup_status"
}
on_exit() {
  local original=$? cleanup_status=0
  trap - EXIT INT TERM
  cleanup || cleanup_status=$?
  if [[ $original -ne 0 ]]; then exit "$original"; fi
  exit "$cleanup_status"
}
trap on_exit EXIT
trap 'exit 130' INT TERM
gascan_release_test_signal
gascan_release_preflight_daemon
export GASCAN_APPLE_ATTACH_HELPER=$apple_attach_bin

mkdir -p "$root/.gascan"
cat >"$root/.gascan/setup.sh" <<'SETUP'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "${GASCAN_RELEASE_SETUP_VALUE:-initial}" > /workspace/.gascan/setup-result
SETUP
chmod 0755 "$root/.gascan/setup.sh"
cat >"$root/gascan.toml" <<EOF_MANIFEST
version = 1
name = "$name"
network = "networked"
user = "workspace"
gascamp = "bundled"
setup = ".gascan/setup.sh"

[resources]
cpus = 1
memory = "1GiB"

[tools]
elixir = "1.20.2-otp-29"
erlang = "29.0.3"
go = "1.26.5"
java = "25.0.2"
node = "24.18.0"
python = "3.14.6"
ruby = "3.4.10"
rust = "1.97.0"
EOF_MANIFEST

host_git_config=$root/.gascan/release-host-gitconfig
cat >"$host_git_config" <<'HOST_GIT_CONFIG'
[user]
	name = Gas Can Release
	email = release-smoke@example.test
HOST_GIT_CONFIG
chmod 0600 "$host_git_config"

mkdir -p "$root/.gascan/fake-forges"
cat >"$root/.gascan/fake-forges/gh" <<'FAKE_GH'
#!/usr/bin/env bash
set -euo pipefail
log=${XDG_CONFIG_HOME:?}/gascan/release-forge.log
mkdir -p "$(dirname "$log")"
printf 'gh argv:' >>"$log"
printf ' <%s>' "$@" >>"$log"
printf '\n' >>"$log"
case "${1:-} ${2:-}" in
  'auth login')
    token=$(cat)
    [[ $token == gascan-release-fake-token ]]
    unset token
    with_token=
    hostname=
    protocol=
    while (($#)); do
      case $1 in
        --skip-ssh-key)
          printf 'gh auth login rejected unsupported --skip-ssh-key\n' >&2
          exit 64
          ;;
        --with-token) with_token=1 ;;
        --hostname) shift; hostname=${1:-} ;;
        --git-protocol) shift; protocol=${1:-} ;;
      esac
      shift
    done
    [[ -n $with_token && $hostname == github.enterprise.test && $protocol == https ]]
    mkdir -p "${GH_CONFIG_DIR:?}"
    printf '%s\n' \
      'github.enterprise.test:' \
      '    user: gascan-release-fake-gh' \
      '    git_protocol: https' >"$GH_CONFIG_DIR/hosts.yml"
    chmod 0600 "$GH_CONFIG_DIR/hosts.yml"
    ;;
  'auth status')
    [[ -f ${GH_CONFIG_DIR:?}/hosts.yml ]]
    printf '%s\n' \
      'github.enterprise.test' \
      '  ✓ Logged in to github.enterprise.test account gascan-release-fake-gh (/home/workspace/.config/gh/hosts.yml)' \
      '  - Active account: true' \
      '  - Git operations protocol: https' >&2
    ;;
  'api --hostname')
    method=GET
    endpoint=
    key=
    while (($#)); do
      case $1 in
        --method) shift; method=${1:-} ;;
        user/keys|user/ssh_signing_keys) endpoint=$1 ;;
        key=*) key=${1#key=} ;;
      esac
      shift
    done
    if [[ $method == POST ]]; then
      [[ -n $endpoint && -n $key ]]
      [[ $key == "$(< /home/workspace/.config/gascan/git/ssh/id_ed25519.pub)" ]]
      touch "$XDG_CONFIG_HOME/gascan/release-gh-key-${endpoint//\//_}"
      jq -nc --arg key "$key" \
        '{id:17,key:$key,title:"Gas Can release",created_at:"2026-07-31T00:00:00Z",verified:true,read_only:false}'
    elif [[ -f $XDG_CONFIG_HOME/gascan/release-gh-key-${endpoint//\//_} ]]; then
      key=$(< /home/workspace/.config/gascan/git/ssh/id_ed25519.pub)
      jq -nc --arg key "$key" \
        '[{id:17,key:$key,title:"Gas Can release",created_at:"2026-07-31T00:00:00Z",verified:true,read_only:false}]'
    else
      printf '[]\n'
    fi
    ;;
  *) exit 64 ;;
esac
FAKE_GH
chmod 0755 "$root/.gascan/fake-forges/gh"

cat >"$root/.gascan/fake-forges/glab" <<'FAKE_GLAB'
#!/usr/bin/env bash
set -euo pipefail
log=${XDG_CONFIG_HOME:?}/gascan/release-forge.log
mkdir -p "$(dirname "$log")"
printf 'glab:%s\n' "$*" >>"$log"
case "${1:-} ${2:-}" in
  'auth login')
    token=$(cat)
    [[ $token == gascan-release-fake-token ]]
    mkdir -p "${GLAB_CONFIG_DIR:?}"
    printf '%s\n' \
      'hosts:' \
      '  gitlab.enterprise.test:' \
      '    user: gascan-release-fake-glab' \
      '    git_protocol: https' >"$GLAB_CONFIG_DIR/config.yml"
    chmod 0600 "$GLAB_CONFIG_DIR/config.yml"
    ;;
  'auth status')
    [[ -f ${GLAB_CONFIG_DIR:?}/config.yml ]]
    printf '%s\n' \
      'gitlab.enterprise.test' \
      '  ✓ Logged in to gitlab.enterprise.test as gascan-release-fake-glab (/home/workspace/.config/glab-cli/config.yml)' \
      '  ✓ Git operations for gitlab.enterprise.test configured to use https protocol.'
    ;;
  'api --hostname')
    method=GET
    key=
    usage=
    while (($#)); do
      case $1 in
        --method) shift; method=${1:-} ;;
        key=*) key=${1#key=} ;;
        usage_type=*) usage=${1#usage_type=} ;;
      esac
      shift
    done
    if [[ $method == POST ]]; then
      [[ -n $key && $usage == auth_and_signing ]]
      [[ $key == "$(< /home/workspace/.config/gascan/git/ssh/id_ed25519.pub)" ]]
      jq -nc --arg key "$key" \
        '{id:23,title:"Gas Can release",key:$key,created_at:"2026-07-31T00:00:00Z",usage_type:"auth_and_signing"}'
    else
      printf '[]\n'
    fi
    ;;
  *) exit 64 ;;
esac
FAKE_GLAB
chmod 0755 "$root/.gascan/fake-forges/glab"

expected_versions=$(gascan_lock_section_json "$repo_root/images/workspace/versions.lock" tools)
expected_gascamp=$(gascan_lock_section_json "$repo_root/images/workspace/versions.lock" gascamp | jq -er '.revision')
expected_starship=$(
  gascan_lock_section_json "$repo_root/images/workspace/versions.lock" workstation_artifacts.starship |
    jq -er '.version'
)

port=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')
python3 -m http.server "$port" --bind 127.0.0.1 --directory "$root" >/dev/null 2>&1 &
server_pid=$!
server_start=$(ps -p "$server_pid" -o lstart= | sed 's/^ *//;s/ *$//')
domain_token=$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')
candidate_domain="gascan-$domain_token.test"
dns_inventory=$(container system dns list --format json)
jq -e --arg domain "$candidate_domain" 'type == "array" and all(.[]; . != $domain)' <<<"$dns_inventory" >/dev/null
dns_domain=$candidate_domain
sudo -n container system dns create --localhost 203.0.113.113 "$dns_domain" >/dev/null
dns_inventory=$(container system dns list --format json)
jq -e --arg domain "$dns_domain" 'type == "array" and ([.[] | select(. == $domain)] | length) == 1' \
  <<<"$dns_inventory" >/dev/null
host_url="http://$dns_domain:$port"

gascan_release_up
sandbox_id=$("$gascan_bin" list --json | jq -er --arg name "$name" \
  '[.[] | select(.sandbox_id | startswith($name + "-"))] | if length == 1 then .[0].sandbox_id else error("release sandbox identity is ambiguous") end')
inspect=$(container inspect "$sandbox_id")
jq -e '
  type == "array" and length == 1 and
  ([.[0].configuration.mounts[]
    | select(.type.volume? != null)
    | .destination] | sort) ==
  ["/home/workspace/.cache", "/home/workspace/.config", "/home/workspace/.local"]
' <<<"$inspect" >/dev/null

"$gascan_bin" --sandbox "$sandbox_id" run -- bash -lc '
  test "$(id -u)" = 1000
  test "$(sudo -n id -u)" = 0
  MISE_OFFLINE=true mise --version
  node -e "console.log(\"node-ok\")"
  python -c "print(\"python-ok\")"
  go version
  rustc --version
  java --version
  ruby --version
  elixir --version
  /opt/gascan/gascamp/bin/camp --version
  test "$(cat /workspace/.gascan/setup-result)" = initial
'
"$gascan_bin" --sandbox "$sandbox_id" run -- bash -lc '
  set -euo pipefail
  test "CARGO_HOME=$CARGO_HOME" = CARGO_HOME=/home/workspace/.local/share/cargo
  test "RUSTUP_HOME=$RUSTUP_HOME" = RUSTUP_HOME=/home/workspace/.local/share/rustup
  test "$XDG_DATA_HOME" = /home/workspace/.local/share
  test "$XDG_CACHE_HOME" = /home/workspace/.cache
  test "$XDG_CONFIG_HOME" = /home/workspace/.config
  test "$NPM_CONFIG_PREFIX" = /home/workspace/.local
  test "$GOPATH" = /home/workspace/.local/share/go
  test "$GOBIN" = /home/workspace/.local/bin
  test "$(go env GOBIN)" = "$GOBIN"
  test "$PYTHONUSERBASE" = /home/workspace/.local
  test "$GEM_HOME" = /home/workspace/.local/share/gem
  test -w /home/workspace/.local
  test -w /home/workspace/.cache
  test -w /home/workspace/.config

  fixture=/workspace/.gascan/release-write-smoke
  rm -rf "$fixture"
  mkdir -p \
    "$fixture/rust-app/src" \
    "$fixture/rust-bin/src" \
    "$fixture/npm-bin" \
    "$fixture/go-bin" \
    "$fixture/python-wheel/gascan_release_python_local-0.1.0.dist-info" \
    "$fixture/ruby-bin/bin" \
    "$XDG_CONFIG_HOME/gascan-release-smoke"

  printf "%s\n" \
    "[package]" \
    "name = \"gascan-release-rust-app\"" \
    "version = \"0.1.0\"" \
    "edition = \"2024\"" \
    "" \
    "[dependencies]" \
    "cfg-if = \"=1.0.4\"" >"$fixture/rust-app/Cargo.toml"
  printf "%s\n" \
    "fn main() {" \
    "    cfg_if::cfg_if! {" \
    "        if #[cfg(unix)] { println!(\"release-cargo-network-ok\"); }" \
    "        else { compile_error!(\"release smoke requires Unix\"); }" \
    "    }" \
    "}" >"$fixture/rust-app/src/main.rs"
  test "$(cargo run --manifest-path "$fixture/rust-app/Cargo.toml")" = release-cargo-network-ok
  test -d "$CARGO_HOME/registry"

  printf "%s\n" \
    "[package]" \
    "name = \"gascan-release-rust-local\"" \
    "version = \"0.1.0\"" \
    "edition = \"2024\"" >"$fixture/rust-bin/Cargo.toml"
  printf "%s\n" \
    "fn main() { println!(\"release-rust-local-ok\"); }" \
    >"$fixture/rust-bin/src/main.rs"
  cargo install --path "$fixture/rust-bin"

  printf "%s\n" \
    "{\"name\":\"gascan-release-npm-local\",\"version\":\"1.0.0\",\"bin\":{\"gascan-release-npm-local\":\"cli.js\"}}" \
    >"$fixture/npm-bin/package.json"
  printf "%s\n" "#!/usr/bin/env node" "console.log(\"release-npm-local-ok\")" \
    >"$fixture/npm-bin/cli.js"
  chmod 0755 "$fixture/npm-bin/cli.js"
  npm pack "$fixture/npm-bin" --pack-destination "$fixture" >/dev/null
  npm install --global "$fixture/gascan-release-npm-local-1.0.0.tgz" >/dev/null

  printf "%s\n" "module example.com/gascan/release-local" "go 1.26" \
    >"$fixture/go.mod"
  printf "%s\n" \
    "package main" \
    "import \"fmt\"" \
    "func main() { fmt.Println(\"release-go-local-ok\") }" \
    >"$fixture/go-bin/main.go"
  (cd "$fixture" && go install ./go-bin)

  printf "%s\n" \
    "def main():" \
    "    print(\"release-python-local-ok\")" \
    >"$fixture/python-wheel/gascan_release_python_local.py"
  printf "%s\n" \
    "Metadata-Version: 2.1" \
    "Name: gascan-release-python-local" \
    "Version: 0.1.0" \
    >"$fixture/python-wheel/gascan_release_python_local-0.1.0.dist-info/METADATA"
  printf "%s\n" \
    "Wheel-Version: 1.0" \
    "Generator: gascan-release-smoke" \
    "Root-Is-Purelib: true" \
    "Tag: py3-none-any" \
    >"$fixture/python-wheel/gascan_release_python_local-0.1.0.dist-info/WHEEL"
  printf "%s\n" \
    "[console_scripts]" \
    "gascan-release-python-local = gascan_release_python_local:main" \
    >"$fixture/python-wheel/gascan_release_python_local-0.1.0.dist-info/entry_points.txt"
  printf "%s\n" \
    "gascan_release_python_local.py,," \
    "gascan_release_python_local-0.1.0.dist-info/METADATA,," \
    "gascan_release_python_local-0.1.0.dist-info/WHEEL,," \
    "gascan_release_python_local-0.1.0.dist-info/entry_points.txt,," \
    "gascan_release_python_local-0.1.0.dist-info/RECORD,," \
    >"$fixture/python-wheel/gascan_release_python_local-0.1.0.dist-info/RECORD"
  (cd "$fixture/python-wheel" &&
    python -m zipfile -c ../gascan_release_python_local-0.1.0-py3-none-any.whl \
      gascan_release_python_local.py gascan_release_python_local-0.1.0.dist-info)
  python -m pip install --user --no-deps \
    "$fixture/gascan_release_python_local-0.1.0-py3-none-any.whl" >/dev/null

  printf "%s\n" \
    "Gem::Specification.new do |spec|" \
    "  spec.name = \"gascan-release-ruby-local\"" \
    "  spec.version = \"0.1.0\"" \
    "  spec.summary = \"Gas Can release smoke\"" \
    "  spec.authors = [\"Gas Can\"]" \
    "  spec.files = [\"bin/gascan-release-ruby-local\"]" \
    "  spec.bindir = \"bin\"" \
    "  spec.executables = [\"gascan-release-ruby-local\"]" \
    "end" >"$fixture/ruby-bin/gascan-release-ruby-local.gemspec"
  printf "%s\n" "#!/usr/bin/env ruby" "puts \"release-ruby-local-ok\"" \
    >"$fixture/ruby-bin/bin/gascan-release-ruby-local"
  chmod 0755 "$fixture/ruby-bin/bin/gascan-release-ruby-local"
  (cd "$fixture/ruby-bin" &&
    gem build gascan-release-ruby-local.gemspec --output ../ruby-bin.gem >/dev/null)
  gem install --local "$fixture/ruby-bin.gem" >/dev/null

  assert_local_command()
  {
    command_path=$(realpath -m "$(command -v "$1")")
    case "$command_path" in /home/workspace/.local/*) ;; *) return 1 ;; esac
    test "$("$1")" = "$2"
  }
  assert_local_command gascan-release-rust-local release-rust-local-ok
  assert_local_command gascan-release-npm-local release-npm-local-ok
  assert_local_command go-bin release-go-local-ok
  assert_local_command gascan-release-python-local release-python-local-ok
  assert_local_command gascan-release-ruby-local release-ruby-local-ok

  printf "release-xdg-config-ok\n" >"$XDG_CONFIG_HOME/gascan-release-smoke/config"
  test "$(cat "$XDG_CONFIG_HOME/gascan-release-smoke/config")" = release-xdg-config-ok
'
"$gascan_bin" --sandbox "$sandbox_id" run -- bash -lc '
  install -m 0755 /workspace/.gascan/fake-forges/gh "$HOME/.local/bin/gh"
  install -m 0755 /workspace/.gascan/fake-forges/glab "$HOME/.local/bin/glab"
  test "$(command -v gh)" = "$HOME/.local/bin/gh"
  test "$(command -v glab)" = "$HOME/.local/bin/glab"
'
gascan_configure_git_from_host_fixture
fake_forge_token=gascan-release-fake-token
github_configure_output=$(printf '%s' "$fake_forge_token" |
  "$gascan_bin" --sandbox "$sandbox_id" configure gh --hostname github.enterprise.test --token-stdin --git-protocol https)
grep -Fx \
  'GitHub: gascan-release-fake-gh at github.enterprise.test; protocol https; authentication configured; authentication key added; signing key added' \
  <<<"$github_configure_output" >/dev/null || {
  printf 'release smoke GitHub configure summary omitted added key results\n' >&2
  exit 1
}
github_configure_retry_output=$(printf '%s' "$fake_forge_token" |
  "$gascan_bin" --sandbox "$sandbox_id" configure gh --hostname github.enterprise.test --token-stdin --git-protocol https)
grep -Fx \
  'GitHub: gascan-release-fake-gh at github.enterprise.test; protocol https; authentication configured; authentication key existing; signing key existing' \
  <<<"$github_configure_retry_output" >/dev/null || {
  printf 'release smoke GitHub configure summary omitted existing key results\n' >&2
  exit 1
}
for transcript in "$github_configure_output" "$github_configure_retry_output"; do
  ! grep -F -- "$fake_forge_token" <<<"$transcript" >/dev/null || {
    printf 'release smoke GitHub configure transcript leaked fixture token\n' >&2
    exit 1
  }
done
printf '%s' "$fake_forge_token" |
  "$gascan_bin" --sandbox "$sandbox_id" configure glab --hostname gitlab.enterprise.test --token-stdin --git-protocol https
unset fake_forge_token
"$gascan_bin" --sandbox "$sandbox_id" run -- bash -lc '
  set -euo pipefail
  test "$(git config --global user.name)" = "Gas Can Release"
  test "$(git config --global user.email)" = release-smoke@example.test
  test "$(git config --global gpg.format)" = ssh
  test "$(git config --global commit.gpgsign)" = true
  test "$(git config --global tag.gpgsign)" = true
  public_key=/home/workspace/.config/gascan/git/ssh/id_ed25519.pub
  private_key=/home/workspace/.config/gascan/git/ssh/id_ed25519
  test -s "$public_key"
  test "$(stat -c %a "$private_key")" = 600
  allowed_signers=$XDG_CONFIG_HOME/gascan/git/allowed_signers
  printf "%s %s\n" release-smoke@example.test "$(cat "$public_key")" >"$allowed_signers"
  chmod 0600 "$allowed_signers"
  git config --global gpg.ssh.allowedSignersFile "$allowed_signers"
  sha256sum "$private_key" | cut -d " " -f 1 >"$XDG_CONFIG_HOME/gascan/developer-key.sha256"

  signed_repo=/workspace/.gascan/release-signed-repo
  rm -rf "$signed_repo"
  git init -q "$signed_repo"
  printf "signed release smoke\n" >"$signed_repo/evidence.txt"
  (
    cd "$signed_repo"
    git add evidence.txt
    git commit -q -m "Verify Gas Can release signing"
    git tag -s -m "Verify Gas Can release tag" gascan-release-signed
    git verify-commit HEAD
    git verify-tag gascan-release-signed
    git cat-file commit HEAD | grep -F "gpgsig -----BEGIN SSH SIGNATURE-----" >/dev/null
    git cat-file tag gascan-release-signed | grep -F -- "-----BEGIN SSH SIGNATURE-----" >/dev/null
  )

  test "$(stat -c %a "$GH_CONFIG_DIR/hosts.yml")" = 600
  test "$(stat -c %a "$GLAB_CONFIG_DIR/config.yml")" = 600
  ! grep -R -F gascan-release-fake-token "$GH_CONFIG_DIR" "$GLAB_CONFIG_DIR"
  log=$XDG_CONFIG_HOME/gascan/release-forge.log
  grep -Fx "gh argv: <auth> <login> <--hostname> <github.enterprise.test> <--git-protocol> <https> <--with-token>" "$log" >/dev/null
  grep -F "gh argv: <api> <--hostname> <github.enterprise.test> <--method> <POST> <user/keys>" "$log" >/dev/null
  grep -F "gh argv: <api> <--hostname> <github.enterprise.test> <--method> <POST> <user/ssh_signing_keys>" "$log" >/dev/null
  grep -F "glab:api --hostname gitlab.enterprise.test --method POST /user/keys" "$log" |
    grep -F "usage_type=auth_and_signing" >/dev/null
  ! grep -F gascan-release-fake-token "$log" >/dev/null || {
    printf "release smoke fake forge log leaked fixture token\\n" >&2
    exit 1
  }
'
"$gascan_bin" --sandbox "$sandbox_id" run -- \
  /opt/gascan/shell/bin/starship --version |
  grep -Fx "starship $expected_starship"
standard_shell=$(gascan_default_shell_probe)
for required in \
  'INTERACTIVE=yes' \
  'LOGIN=yes' \
  'SHELL=/bin/bash' \
  'COMPLETION=/usr/share/bash-completion/bash_completion' \
  'TERM=gascan-release-term' \
  'SELECTOR=standard' \
  'STARSHIP_CONFIG=' \
  'STARSHIP_EXECUTABLE=' \
  'STARSHIP_FUNCTION=' \
  'NESTED_STARSHIP_CONFIG=' \
  'NESTED_STARSHIP_EXECUTABLE=' \
  'NESTED_STARSHIP_FUNCTION='
do
  gascan_assert_shell_field standard "$required" "$standard_shell"
done
gascan_assert_shell_pattern standard BASH_VERSION '^BASH_VERSION=.+$' "$standard_shell"
"$gascan_bin" --sandbox "$sandbox_id" run -- curl --fail --silent --show-error --max-time 4 "$host_url" >/dev/null
version_check=$(cat <<'VERSION_CHECK'
  actual=/tmp/gascan-release-versions.json
  jq -n --arg elixir "$(mise current elixir)" --arg erlang "$(mise current erlang)" \
    --arg go "$(mise current go)" --arg java "$(mise current java)" \
    --arg node "$(mise current node)" --arg python "$(mise current python)" \
    --arg ruby "$(mise current ruby)" --arg rust "$(mise current rust)" '$ARGS.named' >"$actual"
  jq -e --argjson expected "$1" ". == \$expected" "$actual" >/dev/null
  test "$(cat /opt/gascan/gascamp/REVISION)" = "$2"
VERSION_CHECK
)
"$gascan_bin" --sandbox "$sandbox_id" run -- bash -lc "$version_check" _ "$expected_versions" "$expected_gascamp"

mkdir -p "$root/gascamp/bin"
cat >"$root/gascamp/bin/camp" <<'LOCAL_CAMP'
#!/usr/bin/env sh
printf 'local-gascamp-ok\n'
LOCAL_CAMP
chmod 0755 "$root/gascamp/bin/camp"
sed -i '' 's/gascamp = "bundled"/gascamp = "\/workspace\/gascamp"/' "$root/gascan.toml"
sed -i '' 's/GASCAN_RELEASE_SETUP_VALUE:-initial/GASCAN_RELEASE_SETUP_VALUE:-applied/' "$root/.gascan/setup.sh"
gascan_release_up
"$gascan_bin" --sandbox "$sandbox_id" run -- test "$(cat "$root/.gascan/setup-result")" = initial
"$gascan_bin" apply "$root"
"$gascan_bin" --sandbox "$sandbox_id" run -- bash -lc '
  test "$(cat /workspace/.gascan/setup-result)" = applied
  test "$(/workspace/gascamp/bin/camp)" = local-gascamp-ok
  /usr/local/bin/select-gascamp /workspace/gascamp | jq -e ".source == \"workspace\" and .trusted == false" >/dev/null
'

"$gascan_bin" --sandbox "$sandbox_id" down
gascan_release_up
"$gascan_bin" --sandbox "$sandbox_id" run -- bash -lc '
  set -euo pipefail
  credential_persistence_fail()
  {
    local field=$1 status=$2 output=${3:-}
    output=${output//gascan-release-fake-token/[REDACTED]}
    printf "credential persistence check failed: field=%s exit=%s\n" \
      "$field" "$status" >&2
    printf "credential persistence safe output (last 4096 characters):\n%s\n" \
      "${output: -4096}" >&2
    return "$status"
  }
  persistence_check()
  {
    local field=$1 output status
    shift
    if output=$("$@" 2>&1); then
      return 0
    else
      status=$?
    fi
    credential_persistence_fail "$field" "$status" "$output"
  }

  persistence_check setup.result test -f /workspace/.gascan/setup-result
  persistence_check git.private_key_checksum bash -c '\''
    private_key=/home/workspace/.config/gascan/git/ssh/id_ed25519
    test "$(sha256sum "$private_key" | cut -d " " -f 1)" = \
      "$(< "$XDG_CONFIG_HOME/gascan/developer-key.sha256")"
  '\''
  persistence_check git.user_name bash -c '\''
    test "$(git config --global user.name)" = "Gas Can Release"
  '\''
  persistence_check git.user_email bash -c '\''
    test "$(git config --global user.email)" = release-smoke@example.test
  '\''
  persistence_check git.verify_commit \
    git -C /workspace/.gascan/release-signed-repo verify-commit HEAD
  persistence_check git.verify_tag \
    git -C /workspace/.gascan/release-signed-repo verify-tag gascan-release-signed
  persistence_check forge.github.config_mode bash -c '\''
    test "$(stat -c %a "$GH_CONFIG_DIR/hosts.yml")" = 600
  '\''
  persistence_check forge.gitlab.config_mode bash -c '\''
    test "$(stat -c %a "$GLAB_CONFIG_DIR/config.yml")" = 600
  '\''
  persistence_check forge.github.auth_status \
    gh auth status --hostname github.enterprise.test
  persistence_check forge.gitlab.auth_status \
    glab auth status --hostname gitlab.enterprise.test

  token_scan_status=0
  grep -R -F gascan-release-fake-token "$GH_CONFIG_DIR" "$GLAB_CONFIG_DIR" >/dev/null 2>&1 || \
    token_scan_status=$?
  case $token_scan_status in
    1) ;;
    0) credential_persistence_fail forge.config_token_absence 1 \
         "fixture token found in forge configuration" ;;
    *) credential_persistence_fail forge.config_token_absence "$token_scan_status" \
         "forge configuration token scan failed" ;;
  esac
'

gascan_stop_attested_daemon "$gascan_bin" "$gascand_bin"
"$gascan_bin" --sandbox "$sandbox_id" status --json >/dev/null
"$gascan_bin" --sandbox "$sandbox_id" run -- true

"$gascan_bin" --sandbox "$sandbox_id" destroy --yes
sandbox_id=

cat >"$root/gascan.toml" <<EOF_OFFLINE
version = 1
name = "$name"
network = "offline"
user = "workspace"

[shell]
prompt = "starship"
EOF_OFFLINE
gascan_release_up
sandbox_id=$("$gascan_bin" list --json | jq -er --arg name "$name" \
  '[.[] | select(.sandbox_id | startswith($name + "-"))] | if length == 1 then .[0].sandbox_id else error("offline sandbox identity is ambiguous") end')
inspect=$(container inspect "$sandbox_id")
jq -e --arg id "$sandbox_id" '
  type == "array" and length == 1 and .[0].configuration.id == $id and
  .[0].configuration.labels."dev.gascan.managed-by" == "gascan" and
  .[0].configuration.labels."dev.gascan.sandbox-id" == $id and
  .[0].configuration.networks == []
' <<<"$inspect" >/dev/null
starship_shell=$(gascan_default_shell_probe)
for required in \
  'INTERACTIVE=yes' \
  'LOGIN=yes' \
  'SHELL=/bin/bash' \
  'COMPLETION=/usr/share/bash-completion/bash_completion' \
  'TERM=gascan-release-term' \
  'SELECTOR=starship' \
  'STARSHIP_CONFIG=/home/workspace/.config/gascan/shell/starship.toml' \
  'STARSHIP_EXECUTABLE=/opt/gascan/shell/bin/starship' \
  'STARSHIP_FUNCTION=function' \
  'NESTED_STARSHIP_CONFIG=/home/workspace/.config/gascan/shell/starship.toml' \
  'NESTED_STARSHIP_EXECUTABLE=/opt/gascan/shell/bin/starship' \
  'NESTED_STARSHIP_FUNCTION=function'
do
  gascan_assert_shell_field starship "$required" "$starship_shell"
done
if grep -i 'warning' <<<"$starship_shell" >/dev/null; then
  printf 'nested Starship shell emitted a warning\n' >&2
  exit 1
fi
if "$gascan_bin" --sandbox "$sandbox_id" run -- curl --fail --silent --show-error --max-time 3 "$host_url"; then
  printf 'offline sandbox reached the test-owned endpoint\n' >&2
  exit 1
fi
if "$gascan_bin" --sandbox "$sandbox_id" run -- curl --fail --silent --show-error --max-time 3 http://1.1.1.1; then
  printf 'offline sandbox reached a public IP\n' >&2
  exit 1
fi
if "$gascan_bin" --sandbox "$sandbox_id" run -- getent hosts example.com; then
  printf 'offline sandbox resolved public DNS\n' >&2
  exit 1
fi
if "$gascan_bin" --sandbox "$sandbox_id" run -- sudo -n curl --fail --silent --show-error --max-time 3 "$host_url"; then
  printf 'offline guest root reached the test-owned endpoint\n' >&2
  exit 1
fi
if "$gascan_bin" --sandbox "$sandbox_id" run -- sudo -n curl --fail --silent --show-error --max-time 3 http://1.1.1.1; then
  printf 'offline guest root reached a public IP\n' >&2
  exit 1
fi
if "$gascan_bin" --sandbox "$sandbox_id" run -- sudo -n getent hosts example.com; then
  printf 'offline guest root resolved public DNS\n' >&2
  exit 1
fi
sed -i '' 's/prompt = "starship"/prompt = "starship-nerd-font"/' "$root/gascan.toml"
"$gascan_bin" apply "$root"
nerd_shell=$(gascan_default_shell_probe)
for required in \
  'SELECTOR=starship-nerd-font' \
  'STARSHIP_CONFIG=/home/workspace/.config/gascan/shell/starship.toml' \
  'STARSHIP_EXECUTABLE=/opt/gascan/shell/bin/starship' \
  'STARSHIP_FUNCTION=function' \
  'NESTED_STARSHIP_CONFIG=/home/workspace/.config/gascan/shell/starship.toml' \
  'NESTED_STARSHIP_EXECUTABLE=/opt/gascan/shell/bin/starship' \
  'NESTED_STARSHIP_FUNCTION=function'
do
  gascan_assert_shell_field starship-nerd-font "$required" "$nerd_shell"
done
if grep -i 'warning' <<<"$nerd_shell" >/dev/null; then
  printf 'nested Nerd Font Starship shell emitted a warning\n' >&2
  exit 1
fi
destroyed_sandbox_id=$sandbox_id
"$gascan_bin" --sandbox "$destroyed_sandbox_id" destroy --yes
sandbox_id=
sudo -n container system dns delete "$dns_domain"
dns_inventory=$(container system dns list --format json)
jq -e --arg domain "$dns_domain" 'type == "array" and all(.[]; . != $domain)' <<<"$dns_inventory" >/dev/null
dns_domain=
[[ $(ps -p "$server_pid" -o lstart= 2>/dev/null | sed 's/^ *//;s/ *$//' || true) == "$server_start" ]]
kill "$server_pid"
server_pid=
server_start=

controller_inventory=$("$gascan_bin" list --json)
if ! gascan_assert_destroyed_controller_record "$controller_inventory" "$destroyed_sandbox_id"; then
  printf 'release smoke did not retain the exact destroyed controller record\n' >&2
  exit 1
fi

printf 'PASS: installed Gas Can release smoke\n'
fi
