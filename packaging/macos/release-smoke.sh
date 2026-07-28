#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/../.." && pwd -P)
source "$repo_root/packaging/macos/release-common.sh"

gascan_bin=${GASCAN_RELEASE_GASCAN:-/usr/local/bin/gascan}
[[ -x $gascan_bin ]] || { printf 'installed gascan is unavailable\n' >&2; exit 69; }

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
        if process.poll() is not None:
            raise SystemExit(
                "default shell exited before marker: "
                + captured[-4096:].decode("utf-8", "backslashreplace")
            )
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
            continue
        try:
            chunk = os.read(controller, 16384)
        except OSError as error:
            if error.errno == errno.EIO:
                chunk = b""
            else:
                raise
        if not chunk:
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
        b"""printf 'GASCAN_RELEASE_SHELL_BEGIN\\n'
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

"$gascan_bin" up "$root"
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
  'STARSHIP_FUNCTION='
do
  grep -Fx "$required" <<<"$standard_shell" >/dev/null
done
grep -E '^BASH_VERSION=.+$' <<<"$standard_shell" >/dev/null
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
"$gascan_bin" up "$root"
"$gascan_bin" --sandbox "$sandbox_id" run -- test "$(cat "$root/.gascan/setup-result")" = initial
"$gascan_bin" apply "$root"
"$gascan_bin" --sandbox "$sandbox_id" run -- bash -lc '
  test "$(cat /workspace/.gascan/setup-result)" = applied
  test "$(/workspace/gascamp/bin/camp)" = local-gascamp-ok
  /usr/local/bin/select-gascamp /workspace/gascamp | jq -e ".source == \"workspace\" and .trusted == false" >/dev/null
'

"$gascan_bin" --sandbox "$sandbox_id" down
"$gascan_bin" up "$root"
"$gascan_bin" --sandbox "$sandbox_id" run -- test -f /workspace/.gascan/setup-result

gascan_stop_attested_daemon "$gascan_bin" /usr/local/bin/gascand
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
"$gascan_bin" up "$root"
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
  'STARSHIP_FUNCTION=function'
do
  grep -Fx "$required" <<<"$starship_shell" >/dev/null
done
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
  'STARSHIP_FUNCTION=function'
do
  grep -Fx "$required" <<<"$nerd_shell" >/dev/null
done
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
