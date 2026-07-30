#!/usr/bin/env bash
set -euo pipefail

managed_root=/home/workspace/.config/gascan/ssh
host_key="$managed_root/host/ssh_host_ed25519_key"
authorized_keys="$managed_root/authorized_keys"
sshd_config="$managed_root/sshd_config"

if test "${1:-}" = --inside; then
  test $# -eq 1
  config_root=/home/workspace/.config
  gascan_root=/home/workspace/.config/gascan
  test "$(stat -c %U:%G "$config_root")" = root:workspace
  test "$(stat -c %a "$config_root")" = 1770
  test "$(stat -c %U:%G "$gascan_root")" = root:workspace
  test "$(stat -c %a "$gascan_root")" = 1770
  replacement="$config_root/workspace-non-ssh-state"
  mkdir "$replacement"
  printf 'workspace-write-ok\n' >"$replacement/config"
  test "$(cat "$replacement/config")" = workspace-write-ok
  if mv "$managed_root" "$managed_root.moved" 2>/dev/null; then
    printf 'ssh contract: workspace renamed managed SSH state\n' >&2
    exit 1
  fi
  if rm -rf "$managed_root" 2>/dev/null; then
    printf 'ssh contract: workspace removed managed SSH state\n' >&2
    exit 1
  fi
  test -d "$managed_root" && test ! -L "$managed_root"
  if ln -s "$replacement" "$managed_root" 2>/dev/null; then
    printf 'ssh contract: workspace replaced managed SSH state\n' >&2
    exit 1
  fi
  rm -rf "$replacement"

  for specification in \
    "$managed_root:root:root:700:directory" \
    "$managed_root/host:root:root:700:directory" \
    "$host_key:root:root:600:regular file" \
    "$host_key.pub:root:root:644:regular file" \
    "$authorized_keys:root:root:600:regular file" \
    "$sshd_config:root:root:600:regular file"
  do
    path=${specification%%:*}
    remainder=${specification#*:}
    owner=${remainder%%:*}
    remainder=${remainder#*:}
    group=${remainder%%:*}
    remainder=${remainder#*:}
    mode=${remainder%%:*}
    kind=${remainder#*:}
    sudo -n /usr/bin/test ! -L "$path"
    test "$(sudo -n stat -c %F "$path")" = "$kind"
    test "$(sudo -n stat -c %U:%G "$path")" = "$owner:$group"
    test "$(sudo -n stat -c %a "$path")" = "$mode"
    test "$kind" = directory || test "$(sudo -n stat -c %h "$path")" = 1
  done

  test "$(sudo -n wc -l "$authorized_keys" | awk '{print $1}')" = 1
  sudo -n awk 'NF >= 2 && $1 == "ssh-ed25519" { found = 1 } END { exit !found }' \
    "$authorized_keys"
  sudo -n ssh-keygen -l -f "$authorized_keys" >/dev/null
  sudo -n ssh-keygen -y -P '' -f "$host_key" >/dev/null

  for directive in \
    'PasswordAuthentication no' \
    'PermitRootLogin no' \
    'AuthorizedKeysFile none' \
    "AuthorizedKeysCommand /bin/cat $authorized_keys" \
    'AuthorizedKeysCommandUser root' \
    'AllowAgentForwarding no' \
    'AllowTcpForwarding local' \
    'SetEnv HOME=/home/workspace USER=workspace LOGNAME=workspace LANG=C.UTF-8 LC_ALL=C.UTF-8 XDG_DATA_HOME=/home/workspace/.local/share XDG_CACHE_HOME=/home/workspace/.cache XDG_CONFIG_HOME=/home/workspace/.config CARGO_HOME=/home/workspace/.local/share/cargo MISE_CARGO_HOME=/home/workspace/.local/share/cargo RUSTUP_HOME=/home/workspace/.local/share/rustup MISE_RUSTUP_HOME=/home/workspace/.local/share/rustup NPM_CONFIG_PREFIX=/home/workspace/.local NPM_CONFIG_CACHE=/home/workspace/.cache/npm GOPATH=/home/workspace/.local/share/go GOBIN=/home/workspace/.local/bin GOCACHE=/home/workspace/.cache/go-build GOMODCACHE=/home/workspace/.cache/go-mod PYTHONUSERBASE=/home/workspace/.local GEM_HOME=/home/workspace/.local/share/gem MIX_HOME=/home/workspace/.local/share/mix HEX_HOME=/home/workspace/.local/share/hex REBAR_CACHE_DIR=/home/workspace/.cache/rebar3 MISE_CACHE_DIR=/home/workspace/.cache/mise MISE_DATA_DIR=/home/workspace/.local/share/mise MISE_GLOBAL_CONFIG_FILE=/home/workspace/.config/gascan/mise.toml MISE_SYSTEM_CONFIG_FILE=/etc/mise/config.toml MISE_STATE_DIR=/home/workspace/.config/gascan/mise-state MISE_SYSTEM_DATA_DIR=/opt/gascan/mise PATH=/home/workspace/.local/bin:/home/workspace/.local/share/cargo/bin:/home/workspace/.local/share/go/bin:/home/workspace/.local/share/gem/bin:/home/workspace/.local/share/mise/shims:/opt/gascan/mise/shims:/usr/local/sbin:/usr/local/bin:/opt/gascan/workstation/bin:/usr/sbin:/usr/bin:/sbin:/bin'
  do
    sudo -n grep -Fqx "$directive" "$sshd_config"
  done
  effective=$(sudo -n /usr/sbin/sshd -T -f "$sshd_config")
  for directive in \
    'listenaddress 0.0.0.0:22' \
    'passwordauthentication no' \
    'kbdinteractiveauthentication no' \
    'permitrootlogin no' \
    'pubkeyauthentication yes' \
    'authenticationmethods publickey' \
    'authorizedkeysfile none' \
    "authorizedkeyscommand /bin/cat $authorized_keys" \
    'authorizedkeyscommanduser root' \
    'allowusers workspace' \
    'permituserenvironment no' \
    'allowagentforwarding no' \
    'allowtcpforwarding local' \
    'allowstreamlocalforwarding no' \
    'permitopen 127.0.0.1:*' \
    'gatewayports no' \
    'permittunnel no' \
    'x11forwarding no' \
    'strictmodes yes' \
    'setenv HOME=/home/workspace' \
    'setenv USER=workspace' \
    'setenv LOGNAME=workspace' \
    'setenv LANG=C.UTF-8' \
    'setenv LC_ALL=C.UTF-8' \
    'setenv XDG_DATA_HOME=/home/workspace/.local/share' \
    'setenv XDG_CACHE_HOME=/home/workspace/.cache' \
    'setenv XDG_CONFIG_HOME=/home/workspace/.config' \
    'setenv CARGO_HOME=/home/workspace/.local/share/cargo' \
    'setenv MISE_CARGO_HOME=/home/workspace/.local/share/cargo' \
    'setenv RUSTUP_HOME=/home/workspace/.local/share/rustup' \
    'setenv MISE_RUSTUP_HOME=/home/workspace/.local/share/rustup' \
    'setenv NPM_CONFIG_PREFIX=/home/workspace/.local' \
    'setenv NPM_CONFIG_CACHE=/home/workspace/.cache/npm' \
    'setenv GOPATH=/home/workspace/.local/share/go' \
    'setenv GOBIN=/home/workspace/.local/bin' \
    'setenv GOCACHE=/home/workspace/.cache/go-build' \
    'setenv GOMODCACHE=/home/workspace/.cache/go-mod' \
    'setenv PYTHONUSERBASE=/home/workspace/.local' \
    'setenv GEM_HOME=/home/workspace/.local/share/gem' \
    'setenv MIX_HOME=/home/workspace/.local/share/mix' \
    'setenv HEX_HOME=/home/workspace/.local/share/hex' \
    'setenv REBAR_CACHE_DIR=/home/workspace/.cache/rebar3' \
    'setenv MISE_CACHE_DIR=/home/workspace/.cache/mise' \
    'setenv MISE_DATA_DIR=/home/workspace/.local/share/mise' \
    'setenv MISE_GLOBAL_CONFIG_FILE=/home/workspace/.config/gascan/mise.toml' \
    'setenv MISE_SYSTEM_CONFIG_FILE=/etc/mise/config.toml' \
    'setenv MISE_STATE_DIR=/home/workspace/.config/gascan/mise-state' \
    'setenv MISE_SYSTEM_DATA_DIR=/opt/gascan/mise' \
    'setenv PATH=/home/workspace/.local/bin:/home/workspace/.local/share/cargo/bin:/home/workspace/.local/share/go/bin:/home/workspace/.local/share/gem/bin:/home/workspace/.local/share/mise/shims:/opt/gascan/mise/shims:/usr/local/sbin:/usr/local/bin:/opt/gascan/workstation/bin:/usr/sbin:/usr/bin:/sbin:/bin'
  do
    printf '%s\n' "$effective" | grep -Fqx "$directive"
  done
  printf '%s\n' "$effective" |
    awk '$1 == "subsystem" && $2 == "sftp" && $3 == "internal-sftp" && NF == 3 { found = 1 } END { exit !found }'

  pgrep -a -x sshd | grep -F -- "-D -e -f $sshd_config" >/dev/null
  listeners=$(sudo -n ss -ltnp)
  printf '%s\n' "$listeners" | awk '$4 == "0.0.0.0:22" { found = 1 } END { exit !found }'
  ! printf '%s\n' "$listeners" |
    awk '$4 ~ /:22$/ && $4 != "0.0.0.0:22" { found = 1 } END { exit !found }'
  printf 'ssh-contract-inside-ok\n'
  exit 0
fi

test $# -eq 0
root=$(cd "$(dirname "$0")/../../.." && pwd -P)
reference_file=${GASCAN_IMAGE_REF_FILE:-"$root/.artifacts/workspace-image-ref"}
container_bin=${CONTAINER_BIN:-container}
source "$root/tests/image/container-cli.sh"
owner_token=${GASCAN_TEST_OWNER_TOKEN:-$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')}
[[ "$owner_token" =~ ^[0-9a-f]{32}$ ]] ||
  { printf 'ssh contract: invalid owner token\n' >&2; exit 1; }
name="gascan-image-ssh-test-$owner_token"
config_volume="gascan-image-ssh-config-$owner_token"

if test -n "${GASCAN_GATE_TEST_ROOT:-}" && test -n "${CALLS:-}"; then
  test "${FAIL_SMOKE:-}" != ssh-contract.sh
  "$container_bin" create --name "$name" --label dev.gascan.test=true \
    --label "dev.gascan.test.owner=$owner_token" test.invalid >/dev/null
  for _ in 1 2; do
    inspect=$("$container_bin" inspect "$name")
    printf '%s' "$inspect" | cargo run --quiet --locked --offline \
      --manifest-path "$root/scripts/Cargo.toml" --bin validate-owned-container -- \
      "$name" "$owner_token" >/dev/null
  done
  "$container_bin" delete "$name" >/dev/null
  exit 0
fi

image=$(bash "$root/scripts/validate-connected-image-receipt.sh" "$reference_file")
[[ "$image" =~ ^[a-z0-9][a-z0-9._/-]*:[a-zA-Z0-9._-]+@sha256:[0-9a-f]{64}$ ]] ||
  { printf 'ssh contract: image reference is not digest-qualified\n' >&2; exit 1; }
image_digest=${image##*@}
local_image=$(approved_local_image "$image")
temporary=$(mktemp -d "${TMPDIR:-/tmp}/gascan-ssh-contract.XXXXXX")
private_key="$temporary/id_ed25519"
known_hosts="$temporary/known_hosts"
proxy="$temporary/proxy"
agent_pid=
forward_pid=
cleaning=false

owned() {
  local inspect
  inspect=$(bounded_container inspect "$name") || return 1
  printf '%s' "$inspect" | cargo run --quiet --locked --offline \
    --manifest-path "$root/scripts/Cargo.toml" --bin validate-owned-container -- \
    "$name" "$owner_token"
}
owned_image() {
  owned_container_from_approved_image "$name" "$owner_token" "$image_digest" "$local_image"
}
owned_volume() {
  local inspect
  inspect=$(bounded_container volume inspect "$config_volume") || return 1
  printf '%s' "$inspect" | cargo run --quiet --locked --offline \
    --manifest-path "$root/scripts/Cargo.toml" --bin validate-owned-volume -- \
    "$config_volume" "$owner_token"
}
cleanup() {
  $cleaning && return
  cleaning=true
  test -z "$forward_pid" || kill "$forward_pid" >/dev/null 2>&1 || true
  test -z "$agent_pid" || kill "$agent_pid" >/dev/null 2>&1 || true
  if owned && owned; then
    bounded_container stop --time 5 "$name" >/dev/null 2>&1 || true
    owned && owned && bounded_container delete "$name" >/dev/null 2>&1 || true
  fi
  if owned_volume && owned_volume; then
    bounded_container volume delete "$config_volume" >/dev/null 2>&1 || true
  fi
  rm -rf "$temporary"
}
trap cleanup EXIT INT TERM

ssh-keygen -q -t ed25519 -N '' -C gascan-contract -f "$private_key"
chmod 0600 "$private_key"
authorized_key=$(cat "$private_key.pub")

"$container_bin" volume create -s 1073741824 \
  --label dev.gascan.test=true \
  --label "dev.gascan.test.owner=$owner_token" \
  "$config_volume" >/dev/null
owned_volume
"$container_bin" create --name "$name" --init --label dev.gascan.test=true \
  --label "dev.gascan.test.owner=$owner_token" --network none \
  --volume "$config_volume:/home/workspace/.config" \
  --env GASCAN_SSH_ENABLED=1 \
  --env "GASCAN_SSH_AUTHORIZED_KEY=$authorized_key" \
  "$local_image" >/dev/null
owned_image
"$container_bin" start "$name" >/dev/null
owned_image

ready=false
for _ in {1..100}; do
  if "$container_bin" exec "$name" sh -c 'ss -ltn | awk '"'"'$4 == "0.0.0.0:22" { found = 1 } END { exit !found }'"'"''; then
    ready=true
    break
  fi
  sleep 0.05
done
$ready || { printf 'ssh contract: guest listener did not become ready\n' >&2; exit 1; }
"$container_bin" exec "$name" /opt/gascan/tests/ssh-contract.sh --inside

fingerprint_before=$("$container_bin" exec "$name" sudo -n ssh-keygen -l -f "$host_key")
"$container_bin" exec "$name" sudo -n cat "$host_key.pub" |
  awk '{print "gascan-contract " $1 " " $2}' >"$known_hosts"
chmod 0600 "$known_hosts"
{
  printf '%s\n' '#!/bin/sh' 'set -eu'
  printf 'exec %q exec --interactive %q nc 127.0.0.1 22\n' "$container_bin" "$name"
} >"$proxy"
chmod 0700 "$proxy"

ssh_options=(
  ssh -F /dev/null
  -o "ProxyCommand=$proxy"
  -o BatchMode=yes
  -o ConnectTimeout=5
  -o IdentitiesOnly=yes
  -o StrictHostKeyChecking=yes
  -o UserKnownHostsFile="$known_hosts"
  -o HostKeyAlias=gascan-contract
  -i "$private_key"
)
"${ssh_options[@]}" workspace@127.0.0.1 true
test "$("${ssh_options[@]}" workspace@127.0.0.1 /bin/pwd)" = /home/workspace ||
  { printf 'ssh contract: noninteractive command directory changed\n' >&2; exit 1; }
interactive_directory=$(
  printf 'pwd\nexit\n' |
    "${ssh_options[@]}" -tt workspace@127.0.0.1 2>/dev/null |
    tr -d '\r'
)
printf '%s\n' "$interactive_directory" | grep -Fqx /workspace ||
  { printf 'ssh contract: interactive login did not enter /workspace\n' >&2; exit 1; }
remote_environment=$("${ssh_options[@]}" workspace@127.0.0.1 /usr/bin/env)
for variable in \
  'HOME=/home/workspace' \
  'USER=workspace' \
  'LOGNAME=workspace' \
  'LANG=C.UTF-8' \
  'LC_ALL=C.UTF-8' \
  'XDG_DATA_HOME=/home/workspace/.local/share' \
  'XDG_CACHE_HOME=/home/workspace/.cache' \
  'XDG_CONFIG_HOME=/home/workspace/.config' \
  'CARGO_HOME=/home/workspace/.local/share/cargo' \
  'MISE_CARGO_HOME=/home/workspace/.local/share/cargo' \
  'RUSTUP_HOME=/home/workspace/.local/share/rustup' \
  'MISE_RUSTUP_HOME=/home/workspace/.local/share/rustup' \
  'NPM_CONFIG_PREFIX=/home/workspace/.local' \
  'NPM_CONFIG_CACHE=/home/workspace/.cache/npm' \
  'GOPATH=/home/workspace/.local/share/go' \
  'GOBIN=/home/workspace/.local/bin' \
  'GOCACHE=/home/workspace/.cache/go-build' \
  'GOMODCACHE=/home/workspace/.cache/go-mod' \
  'PYTHONUSERBASE=/home/workspace/.local' \
  'GEM_HOME=/home/workspace/.local/share/gem' \
  'MIX_HOME=/home/workspace/.local/share/mix' \
  'HEX_HOME=/home/workspace/.local/share/hex' \
  'REBAR_CACHE_DIR=/home/workspace/.cache/rebar3' \
  'MISE_CACHE_DIR=/home/workspace/.cache/mise' \
  'MISE_DATA_DIR=/home/workspace/.local/share/mise' \
  'MISE_GLOBAL_CONFIG_FILE=/home/workspace/.config/gascan/mise.toml' \
  'MISE_SYSTEM_CONFIG_FILE=/etc/mise/config.toml' \
  'MISE_STATE_DIR=/home/workspace/.config/gascan/mise-state' \
  'MISE_SYSTEM_DATA_DIR=/opt/gascan/mise' \
  'PATH=/home/workspace/.local/bin:/home/workspace/.local/share/cargo/bin:/home/workspace/.local/share/go/bin:/home/workspace/.local/share/gem/bin:/home/workspace/.local/share/mise/shims:/opt/gascan/mise/shims:/usr/local/sbin:/usr/local/bin:/opt/gascan/workstation/bin:/usr/sbin:/usr/bin:/sbin:/bin'
do
  printf '%s\n' "$remote_environment" | grep -Fqx "$variable"
done
sftp -F /dev/null -b /dev/null \
  -o "ProxyCommand=$proxy" \
  -o BatchMode=yes \
  -o IdentitiesOnly=yes \
  -o StrictHostKeyChecking=yes \
  -o UserKnownHostsFile="$known_hosts" \
  -o HostKeyAlias=gascan-contract \
  -i "$private_key" workspace@127.0.0.1

! ssh -F /dev/null -o "ProxyCommand=$proxy" -o BatchMode=yes \
  -o ConnectTimeout=5 -o PubkeyAuthentication=no -o PasswordAuthentication=yes \
  -o KbdInteractiveAuthentication=no \
  -o StrictHostKeyChecking=yes -o UserKnownHostsFile="$known_hosts" \
  -o HostKeyAlias=gascan-contract workspace@127.0.0.1 true
! "${ssh_options[@]}" root@127.0.0.1 true
! "${ssh_options[@]}" -o ExitOnForwardFailure=yes \
  -R 127.0.0.1:0:127.0.0.1:22 workspace@127.0.0.1 true

local_port=$(python3 -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1]);s.close()')
"${ssh_options[@]}" -o ExitOnForwardFailure=yes -N \
  -L "127.0.0.1:$local_port:127.0.0.1:22" workspace@127.0.0.1 &
forward_pid=$!
forward_ready=false
for _ in {1..100}; do
  if nc -z 127.0.0.1 "$local_port"; then forward_ready=true; break; fi
  sleep 0.05
done
$forward_ready || { printf 'ssh contract: allowed local forwarding failed\n' >&2; exit 1; }
kill "$forward_pid"
wait "$forward_pid" || true
forward_pid=

agent_socket="$temporary/agent.sock"
ssh-agent -D -a "$agent_socket" >"$temporary/agent.log" 2>&1 &
agent_pid=$!
for _ in {1..100}; do test -S "$agent_socket" && break; sleep 0.05; done
test -S "$agent_socket"
SSH_AUTH_SOCK=$agent_socket ssh-add "$private_key" >/dev/null
SSH_AUTH_SOCK=$agent_socket "${ssh_options[@]}" -A workspace@127.0.0.1 \
  'test -z "${SSH_AUTH_SOCK:-}"'
kill "$agent_pid"
wait "$agent_pid" || true
agent_pid=

"$container_bin" stop --time 5 "$name" >/dev/null
owned_image
"$container_bin" start "$name" >/dev/null
owned_image
for _ in {1..100}; do
  "$container_bin" exec "$name" sh -c 'ss -ltn | grep -Fq "0.0.0.0:22"' && break
  sleep 0.05
done
"$container_bin" exec "$name" /opt/gascan/tests/ssh-contract.sh --inside
fingerprint_after=$("$container_bin" exec "$name" sudo -n ssh-keygen -l -f "$host_key")
test "$fingerprint_after" = "$fingerprint_before" ||
  { printf 'ssh contract: host-key fingerprint changed across restart\n' >&2; exit 1; }

cleanup
trap - EXIT INT TERM
printf 'ssh-contract-ok\n'
