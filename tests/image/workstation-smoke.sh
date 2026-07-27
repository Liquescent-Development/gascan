#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd -P)
reference_file=${GASCAN_IMAGE_REF_FILE:-"$root/.artifacts/workspace-image-ref"}
container_bin=${CONTAINER_BIN:-container}
source "$root/tests/image/container-cli.sh"
image=$(bash "$root/scripts/validate-connected-image-receipt.sh" "$reference_file")
[[ "$image" =~ ^[a-z0-9][a-z0-9._/-]*:[a-zA-Z0-9._-]+@sha256:[0-9a-f]{64}$ ]] ||
  { printf 'workstation smoke: image reference is not digest-qualified\n' >&2; exit 1; }
image_digest=${image##*@}
local_image=$(approved_local_image "$image")
owner_token=${GASCAN_TEST_OWNER_TOKEN:-$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')}
[[ "$owner_token" =~ ^[0-9a-f]{32}$ ]] || { printf 'workstation smoke: invalid owner token\n' >&2; exit 1; }
name="gascan-image-workstation-test-$owner_token"
network_name="gascan-image-workstation-network-test-$owner_token"
tools_volume="gascan-image-workstation-tools-$owner_token"
cache_volume="gascan-image-workstation-cache-$owner_token"
config_volume="gascan-image-workstation-config-$owner_token"
volumes=("$tools_volume" "$cache_volume" "$config_volume")
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
  local volume=$1
  bounded_container volume inspect "$volume" |
    cargo run --quiet --locked --offline \
    --manifest-path "$root/scripts/Cargo.toml" --bin validate-owned-volume -- \
    "$volume" "$owner_token"
}
volume_inventory_proves_absent() {
  local inventory
  inventory=$(bounded_container volume list --format json) || return 1
  printf '%s' "$inventory" | cargo run --quiet --locked --offline \
    --manifest-path "$root/scripts/Cargo.toml" --bin validate-container-inventory -- \
    "${volumes[@]}"
}
cleanup() {
  $cleaning && return
  cleaning=true
  if owned && owned; then
    bounded_container stop --time 5 "$name" >/dev/null 2>&1 || true
    owned && owned && bounded_container delete "$name" >/dev/null 2>&1 || true
  fi
  local original_name=$name
  name=$network_name
  if owned && owned; then
    bounded_container stop --time 5 "$name" >/dev/null 2>&1 || true
    owned && owned && bounded_container delete "$name" >/dev/null 2>&1 || true
  fi
  name=$original_name
  local volume
  for volume in "${volumes[@]}"; do
    if owned_volume "$volume" && owned_volume "$volume"; then
      bounded_container volume delete "$volume" >/dev/null 2>&1 || true
    fi
  done
}
on_signal() { trap - EXIT INT TERM; cleanup; exit 130; }
trap cleanup EXIT
trap on_signal INT TERM

for specification in \
  "$tools_volume:10737418240" \
  "$cache_volume:10737418240" \
  "$config_volume:1073741824"
do
  volume=${specification%%:*}
  size=${specification#*:}
  "$container_bin" volume create -s "$size" \
    --label dev.gascan.test=true \
    --label "dev.gascan.test.owner=$owner_token" \
    "$volume" >/dev/null
  owned_volume "$volume"
done

"$container_bin" create --name "$name" --init --label dev.gascan.test=true \
  --label "dev.gascan.test.owner=$owner_token" --network none \
  --volume "$tools_volume:/home/workspace/.local" \
  --volume "$cache_volume:/home/workspace/.cache" \
  --volume "$config_volume:/home/workspace/.config" \
  --env HOME=/home/workspace \
  --env XDG_DATA_HOME=/home/workspace/.local/share \
  --env XDG_CACHE_HOME=/home/workspace/.cache \
  --env XDG_CONFIG_HOME=/home/workspace/.config \
  --env CARGO_HOME=/home/workspace/.local/share/cargo \
  --env MISE_CARGO_HOME=/home/workspace/.local/share/cargo \
  --env RUSTUP_HOME=/home/workspace/.local/share/rustup \
  --env MISE_RUSTUP_HOME=/home/workspace/.local/share/rustup \
  --env NPM_CONFIG_PREFIX=/home/workspace/.local \
  --env NPM_CONFIG_CACHE=/home/workspace/.cache/npm \
  --env GOPATH=/home/workspace/.local/share/go \
  --env GOCACHE=/home/workspace/.cache/go-build \
  --env GOMODCACHE=/home/workspace/.cache/go-mod \
  --env PYTHONUSERBASE=/home/workspace/.local \
  --env GEM_HOME=/home/workspace/.local/share/gem \
  --env MIX_HOME=/home/workspace/.local/share/mix \
  --env HEX_HOME=/home/workspace/.local/share/hex \
  --env REBAR_CACHE_DIR=/home/workspace/.cache/rebar3 \
  --env MISE_CACHE_DIR=/home/workspace/.cache/mise \
  --env MISE_DATA_DIR=/home/workspace/.local/share/mise \
  --env MISE_GLOBAL_CONFIG_FILE=/home/workspace/.config/gascan/mise.toml \
  --env MISE_STATE_DIR=/home/workspace/.config/gascan/mise-state \
  --env MISE_SYSTEM_DATA_DIR=/opt/gascan/mise \
  --env PATH=/home/workspace/.local/bin:/home/workspace/.local/share/cargo/bin:/home/workspace/.local/share/go/bin:/home/workspace/.local/share/gem/bin:/home/workspace/.local/share/mise/shims:/opt/gascan/mise/shims:/usr/local/sbin:/usr/local/bin:/opt/gascan/workstation/bin:/usr/sbin:/usr/bin:/sbin:/bin \
  "$local_image" >/dev/null
owned_image
"$container_bin" start "$name" >/dev/null
owned_image
"$container_bin" exec "$name" sudo -n install -d -o workspace -g workspace -m 0700 \
  /home/workspace/.local \
  /home/workspace/.cache \
  /home/workspace/.local/share
"$container_bin" exec "$name" sudo -n install -d -o root -g workspace -m 1770 \
  /home/workspace/.config
"$container_bin" exec "$name" /usr/local/bin/initialize-rust-home
"$container_bin" exec "$name" env HOME=/home/workspace \
  /usr/local/bin/configure-workstation-home
"$container_bin" exec "$name" sudo -n chown root:workspace /home/workspace/.config/gascan
"$container_bin" exec "$name" sudo -n chmod 1770 /home/workspace/.config/gascan
"$container_bin" exec "$name" /opt/gascan/tests/workstation-contract.sh
"$container_bin" exec "$name" sh -c '
  set -eu
  cd /tmp
  fixture=./gascan-local-install-smoke
  rm -rf "$fixture"
  mkdir -p \
    "$fixture/rust-app/src" \
    "$fixture/rust-bin/src" \
    "$fixture/npm-bin" \
    "$fixture/go-bin" \
    "$fixture/python-bin" \
    "$fixture/ruby-bin/bin" \
    "$XDG_CONFIG_HOME/gascan-local-smoke"

  printf "%s\n" \
    "[package]" \
    "name = \"gascan-rust-app\"" \
    "version = \"0.1.0\"" \
    "edition = \"2024\"" >"$fixture/rust-app/Cargo.toml"
  printf "%s\n" "fn main() { println!(\"rust-app-local-ok\"); }" \
    >"$fixture/rust-app/src/main.rs"
  test "$(cargo run --manifest-path "$fixture/rust-app/Cargo.toml")" = rust-app-local-ok

  printf "%s\n" \
    "[package]" \
    "name = \"gascan-rust-local\"" \
    "version = \"0.1.0\"" \
    "edition = \"2024\"" >"$fixture/rust-bin/Cargo.toml"
  printf "%s\n" "fn main() { println!(\"rust-install-local-ok\"); }" \
    >"$fixture/rust-bin/src/main.rs"
  cargo install --path "$fixture/rust-bin"

  printf "%s\n" \
    "{\"name\":\"gascan-npm-local\",\"version\":\"1.0.0\",\"bin\":{\"gascan-npm-local\":\"cli.js\"}}" \
    >"$fixture/npm-bin/package.json"
  printf "%s\n" "#!/usr/bin/env node" "console.log(\"npm-local-ok\")" \
    >"$fixture/npm-bin/cli.js"
  chmod 0755 "$fixture/npm-bin/cli.js"
  npm install --global "$fixture/npm-bin" >/dev/null

  printf "%s\n" "module example.com/gascan/go-local" "go 1.26" \
    >"$fixture/go.mod"
  printf "%s\n" \
    "package main" \
    "import \"fmt\"" \
    "func main() { fmt.Println(\"go-local-ok\") }" >"$fixture/go-bin/main.go"
  (cd "$fixture" && go install ./go-bin)

  printf "%s\n" \
    "from setuptools import setup" \
    "setup(name=\"gascan-python-local\", version=\"0.1.0\", scripts=[\"gascan-python-local\"])" \
    >"$fixture/python-bin/setup.py"
  printf "%s\n" \
    "#!/usr/bin/env python" \
    "print(\"python-local-ok\")" >"$fixture/python-bin/gascan-python-local"
  chmod 0755 "$fixture/python-bin/gascan-python-local"
  python -m pip install --user --no-deps "$fixture/python-bin" >/dev/null

  printf "%s\n" \
    "Gem::Specification.new do |spec|" \
    "  spec.name = \"gascan-ruby-local\"" \
    "  spec.version = \"0.1.0\"" \
    "  spec.summary = \"Gas Can local smoke\"" \
    "  spec.authors = [\"Gas Can\"]" \
    "  spec.files = [\"bin/gascan-ruby-local\"]" \
    "  spec.bindir = \"bin\"" \
    "  spec.executables = [\"gascan-ruby-local\"]" \
    "end" >"$fixture/ruby-bin/gascan-ruby-local.gemspec"
  printf "%s\n" "#!/usr/bin/env ruby" "puts \"ruby-local-ok\"" \
    >"$fixture/ruby-bin/bin/gascan-ruby-local"
  chmod 0755 "$fixture/ruby-bin/bin/gascan-ruby-local"
  gem build "$fixture/ruby-bin/gascan-ruby-local.gemspec" \
    --output "$fixture/ruby-bin.gem" >/dev/null
  gem install --local "$fixture/ruby-bin.gem" >/dev/null

  assert_user_command()
  {
    command_path=$(realpath -m "$(command -v "$1")")
    case "$command_path" in /home/workspace/.local/*) ;; *) exit 1 ;; esac
    test "$("$1")" = "$2"
  }
  assert_user_command gascan-rust-local rust-install-local-ok
  assert_user_command gascan-npm-local npm-local-ok
  assert_user_command go-bin go-local-ok
  assert_user_command gascan-python-local python-local-ok
  assert_user_command gascan-ruby-local ruby-local-ok
  printf "xdg-local-ok\n" >"$XDG_CONFIG_HOME/gascan-local-smoke/config"
  test "$(cat "$XDG_CONFIG_HOME/gascan-local-smoke/config")" = xdg-local-ok
'
"$container_bin" exec "$name" sh -c '
  set -eu
  nslookup -version 2>&1 | grep -Eq "^nslookup 9[.]"
  curl --fail --silent file:///etc/os-release | grep -Fq "ID=ubuntu"
  wget --version | sed -n "1p" | grep -Fq "GNU Wget"
  rsync --version | sed -n "1p" | grep -Fq "rsync  version"
  lsof -v 2>&1 | grep -Fq "revision:"
  file /bin/sh | grep -Fq "symbolic link to dash"
  printf "{\"ready\":true}\n" | jq -e ".ready == true" >/dev/null
  ps -o comm= -p $$ | grep -Eq "^[[:space:]]*sh$"
  top -b -n 1 | grep -Fq "PID"
  pstree -p $$ | grep -Fq "sh("
  tree --version | sed -n "1p" | grep -Eq "^tree v[0-9]"
  test "$(printf gascan-less | less -F -X)" = gascan-less
'
owned_image
bounded_container stop --time 5 "$name" >/dev/null
owned_image
bounded_container delete "$name" >/dev/null

name=$network_name
"$container_bin" create --name "$name" --init --label dev.gascan.test=true \
  --label "dev.gascan.test.owner=$owner_token" \
  --volume "$tools_volume:/home/workspace/.local" \
  --volume "$cache_volume:/home/workspace/.cache" \
  --volume "$config_volume:/home/workspace/.config" \
  --env HOME=/home/workspace \
  --env XDG_DATA_HOME=/home/workspace/.local/share \
  --env XDG_CACHE_HOME=/home/workspace/.cache \
  --env XDG_CONFIG_HOME=/home/workspace/.config \
  --env CARGO_HOME=/home/workspace/.local/share/cargo \
  --env MISE_CARGO_HOME=/home/workspace/.local/share/cargo \
  --env RUSTUP_HOME=/home/workspace/.local/share/rustup \
  --env MISE_RUSTUP_HOME=/home/workspace/.local/share/rustup \
  --env PATH=/home/workspace/.local/bin:/home/workspace/.local/share/cargo/bin:/home/workspace/.local/share/go/bin:/home/workspace/.local/share/gem/bin:/home/workspace/.local/share/mise/shims:/opt/gascan/mise/shims:/usr/local/sbin:/usr/local/bin:/opt/gascan/workstation/bin:/usr/sbin:/usr/bin:/sbin:/bin \
  "$local_image" >/dev/null
owned_image
"$container_bin" start "$name" >/dev/null
owned_image
"$container_bin" exec "$name" sudo -n install -d -o workspace -g workspace -m 0700 \
  /home/workspace/.local /home/workspace/.cache
"$container_bin" exec "$name" sudo -n install -d -o root -g workspace -m 1770 \
  /home/workspace/.config
"$container_bin" exec "$name" /usr/local/bin/initialize-rust-home
"$container_bin" exec "$name" sh -c '
  set -eu
  cd /tmp
  fixture=./gascan-network-install-smoke
  rm -rf "$fixture"
  mkdir -p "$fixture/rust-network/src"
  printf "%s\n" \
    "[package]" \
    "name = \"gascan-rust-network\"" \
    "version = \"0.1.0\"" \
    "edition = \"2024\"" \
    "" \
    "[dependencies]" \
    "cfg-if = \"=1.0.4\"" >"$fixture/rust-network/Cargo.toml"
  printf "%s\n" \
    "fn main() { cfg_if::cfg_if! { if #[cfg(unix)] { println!(\"cargo-network-ok\"); } else { compile_error!(\"expected Unix\"); } } }" \
    >"$fixture/rust-network/src/main.rs"
  test "$(cargo run --quiet --manifest-path "$fixture/rust-network/Cargo.toml")" = cargo-network-ok
  test -d "$CARGO_HOME/registry"
  test -n "$(find "$CARGO_HOME/registry" -mindepth 1 -print -quit)"
  rustup component add rust-src
  rustup component list --installed | grep -Fqx "rust-src-$(rustc -vV | sed -n "s/^host: //p")"
'
cleanup
volume_inventory_proves_absent
