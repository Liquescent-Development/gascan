#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/../.." && pwd -P)
source "$repo_root/packaging/macos/release-common.sh"

gascan_bin=${GASCAN_RELEASE_GASCAN:-/usr/local/bin/gascan}
[[ -x $gascan_bin ]] || { printf 'installed gascan is unavailable\n' >&2; exit 69; }

root=$(mktemp -d "${TMPDIR:-/tmp}/gascan-release-root.XXXXXX")
name="gate5-release-$PPID-$$"
sandbox_id=
dns_domain=
server_pid=
server_start=

cleanup() {
  local original=$? cleanup_status=0 observed_start
  trap - EXIT INT TERM
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
  if [[ $original -ne 0 ]]; then exit "$original"; fi
  exit "$cleanup_status"
}
on_signal() {
  trap - EXIT INT TERM
  cleanup
  exit 130
}
trap cleanup EXIT
trap on_signal INT TERM

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
memory = "256MiB"

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

port=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')
python3 -m http.server "$port" --bind 127.0.0.1 --directory "$root" >/dev/null 2>&1 &
server_pid=$!
server_start=$(ps -p "$server_pid" -o lstart= | sed 's/^ *//;s/ *$//')
domain_token=$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')
candidate_domain="gascan-$domain_token.test"
dns_inventory=$(container system dns list --format json)
jq -e --arg domain "$candidate_domain" 'type == "array" and all(.[]; . != $domain)' <<<"$dns_inventory" >/dev/null
sudo -n container system dns create --localhost "$candidate_domain" >/dev/null
dns_domain=$candidate_domain
dns_inventory=$(container system dns list --format json)
jq -e --arg domain "$dns_domain" 'type == "array" and ([.[] | select(. == $domain)] | length) == 1' \
  <<<"$dns_inventory" >/dev/null
host_url="http://$dns_domain:$port"

"$gascan_bin" up "$root"
sandbox_id=$("$gascan_bin" list --json | jq -er --arg name "$name" \
  '[.[] | select(.sandbox_id | startswith($name + "-"))] | if length == 1 then .[0].sandbox_id else error("release sandbox identity is ambiguous") end')

"$gascan_bin" --sandbox "$sandbox_id" run -- bash -lc '
  test "$(id -u)" = 1000
  test "$(sudo -n id -u)" = 0
  mise --version
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
name = "$name-offline"
network = "offline"
user = "workspace"
EOF_OFFLINE
"$gascan_bin" up "$root"
sandbox_id=$("$gascan_bin" list --json | jq -er --arg name "$name-offline" \
  '[.[] | select(.sandbox_id | startswith($name + "-"))] | if length == 1 then .[0].sandbox_id else error("offline sandbox identity is ambiguous") end')
inspect=$(container inspect "$sandbox_id")
jq -e --arg id "$sandbox_id" '
  type == "array" and length == 1 and .[0].configuration.id == $id and
  .[0].configuration.labels."dev.gascan.managed-by" == "gascan" and
  .[0].configuration.labels."dev.gascan.sandbox-id" == $id and
  .[0].configuration.networks == []
' <<<"$inspect" >/dev/null
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
if "$gascan_bin" --sandbox "$sandbox_id" run -- sudo -n getent hosts example.com; then
  printf 'offline guest root resolved public DNS\n' >&2
  exit 1
fi
"$gascan_bin" --sandbox "$sandbox_id" destroy --yes
sandbox_id=
sudo -n container system dns delete "$dns_domain"
dns_inventory=$(container system dns list --format json)
jq -e --arg domain "$dns_domain" 'type == "array" and all(.[]; . != $domain)' <<<"$dns_inventory" >/dev/null
dns_domain=
[[ $(ps -p "$server_pid" -o lstart= 2>/dev/null | sed 's/^ *//;s/ *$//' || true) == "$server_start" ]]
kill "$server_pid"
server_pid=
server_start=

if "$gascan_bin" list --json | jq -e --arg prefix "$name" '.[] | select(.sandbox_id | startswith($prefix + "-"))' >/dev/null; then
  printf 'release smoke left controller state behind\n' >&2
  exit 1
fi

printf 'PASS: installed Gas Can release smoke\n'
