#!/usr/bin/env bash
set -euo pipefail

gascan_bin=${GASCAN_RELEASE_GASCAN:-/usr/local/bin/gascan}
[[ -x $gascan_bin ]] || { printf 'installed gascan is unavailable\n' >&2; exit 69; }

root=$(mktemp -d "${TMPDIR:-/tmp}/gascan-release-root.XXXXXX")
name="gate5-release-$PPID-$$"
sandbox_id=

cleanup() {
  if [[ -n $sandbox_id ]]; then
    "$gascan_bin" --sandbox "$sandbox_id" destroy --yes >/dev/null 2>&1 || true
  fi
  rm -rf "$root"
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

attestation=$("$gascan_bin" daemon-attest)
daemon_pid=$(jq -er '.pid | select(type == "number" and . > 1)' <<<"$attestation")
daemon_executable=$(jq -er '.executable' <<<"$attestation")
[[ $daemon_executable == /usr/local/bin/gascand ]] || {
  printf 'refusing to stop unexpected daemon executable: %s\n' "$daemon_executable" >&2
  exit 1
}
kill -TERM "$daemon_pid"
for _ in {1..100}; do
  kill -0 "$daemon_pid" 2>/dev/null || break
  sleep 0.05
done
kill -0 "$daemon_pid" 2>/dev/null && { printf 'daemon did not stop promptly\n' >&2; exit 1; }
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
if "$gascan_bin" --sandbox "$sandbox_id" run -- curl --silent --show-error --max-time 3 https://example.com; then
  printf 'offline sandbox reached the public network\n' >&2
  exit 1
fi
if "$gascan_bin" --sandbox "$sandbox_id" run -- curl --silent --show-error --max-time 3 http://1.1.1.1; then
  printf 'offline sandbox reached a public IP\n' >&2
  exit 1
fi
"$gascan_bin" --sandbox "$sandbox_id" destroy --yes
sandbox_id=

if "$gascan_bin" list --json | jq -e --arg prefix "$name" '.[] | select(.sandbox_id | startswith($prefix + "-"))' >/dev/null; then
  printf 'release smoke left controller state behind\n' >&2
  exit 1
fi

printf 'PASS: installed Gas Can release smoke\n'
