#!/usr/bin/env bash
set -euo pipefail

inside_image() {

cd /tmp
mise --version
test "$MISE_SYSTEM_CONFIG_FILE" = /etc/mise/config.toml
test "$(mise current node)" = 24.18.0
for tool in node python go rust java ruby elixir; do
  mise ls --installed "$tool"
done
node -e 'console.log("node-ok")'
python -c 'print("python-ok")'
printf 'package main\nimport "fmt"\nfunc main(){fmt.Println("go-ok")}\n' >/tmp/main.go
go run /tmp/main.go
test "$(rustc --version | awk '{print $2}')" = "$(mise current rust)"
test "$(cargo --version | awk '{print $2}')" = "$(mise current rust)"
printf 'fn main(){println!("rust-ok");}\n' >/tmp/main.rs
rustc /tmp/main.rs -o /tmp/rust-ok
/tmp/rust-ok
printf 'class Main { public static void main(String[] a){ System.out.println("java-ok"); } }\n' >/tmp/Main.java
javac /tmp/Main.java
java -cp /tmp Main
ruby -e 'puts "ruby-ok"'
elixir -e 'IO.puts("elixir-ok")'
erl -noshell -eval 'true = (erlang:system_info(otp_release) =:= "29"), halt().'

resolved=/tmp/gascan-resolved-tool-versions.json
trap 'rm -f "$resolved"' EXIT
jq --null-input \
  --arg elixir "$(mise current elixir)" \
  --arg erlang "$(mise current erlang)" \
  --arg go "$(mise current go)" \
  --arg java "$(mise current java)" \
  --arg node "$(mise current node)" \
  --arg python "$(mise current python)" \
  --arg ruby "$(mise current ruby)" \
  --arg rust "$(mise current rust)" \
  '$ARGS.named' >"$resolved"
jq --exit-status --slurpfile expected /opt/gascan/image-tool-versions.json \
  '. == $expected[0]' "$resolved" >/dev/null
test "$(stat -c %U:%G /opt/gascan/image-tool-versions.json)" = root:root
test "$(stat -c %a /opt/gascan/image-tool-versions.json)" = 444

node /opt/gascan/tests/playwright-smoke.mjs
git --version
gh --version
cc --version
}

if [[ ${1:-} == --inside ]]; then
  inside_image
  exit 0
fi

root=$(cd "$(dirname "$0")/../.." && pwd -P)
reference_file=${GASCAN_IMAGE_REF_FILE:-"$root/.artifacts/workspace-image-ref"}
container_bin=${CONTAINER_BIN:-container}
source "$root/tests/image/container-cli.sh"
test -f "$reference_file" || { printf 'missing polyglot image reference: %s\n' "$reference_file" >&2; exit 1; }
image=$(bash "$root/scripts/validate-connected-image-receipt.sh" "$reference_file")
[[ "$image" =~ ^[a-z0-9][a-z0-9._/-]*:[a-zA-Z0-9._-]+@sha256:[0-9a-f]{64}$ ]] || {
  printf 'image reference is not digest-qualified\n' >&2
  exit 1
}
image_digest=${image##*@}
local_image=$(approved_local_image "$image")
owner_token=${GASCAN_TEST_OWNER_TOKEN:-$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')}
[[ "$owner_token" =~ ^[0-9a-f]{32}$ ]] || { printf 'invalid owner token\n' >&2; exit 1; }
name="gascan-image-polyglot-test-$owner_token"
tools_volume="gascan-image-polyglot-tools-$owner_token"
cleaning=false

owned() {
  local inspect
  inspect=$(bounded_container inspect "$name") || return 1
  printf '%s' "$inspect" | cargo run --quiet --locked --offline \
    --manifest-path "$root/scripts/Cargo.toml" --bin validate-owned-container -- "$name" "$owner_token"
}
owned_image() {
  owned_container_from_approved_image "$name" "$owner_token" "$image_digest" "$local_image"
}
owned_volume() {
  bounded_container volume inspect "$tools_volume" |
    cargo run --quiet --locked --offline \
    --manifest-path "$root/scripts/Cargo.toml" --bin validate-owned-volume -- \
    "$tools_volume" "$owner_token"
}
volume_inventory_proves_absent() {
  local inventory
  inventory=$(bounded_container volume list --format json) || return 1
  printf '%s' "$inventory" | cargo run --quiet --locked --offline \
    --manifest-path "$root/scripts/Cargo.toml" --bin validate-container-inventory -- \
    "$tools_volume"
}
cleanup() {
  $cleaning && return
  cleaning=true
  if owned && owned; then
    bounded_container stop --time 5 "$name" >/dev/null 2>&1 || true
    owned && owned && bounded_container delete "$name" >/dev/null 2>&1 || true
  fi
  if owned_volume "$tools_volume" && owned_volume "$tools_volume"; then
    bounded_container volume delete "$tools_volume" >/dev/null 2>&1 || true
  fi
}
on_signal() { trap - EXIT INT TERM; cleanup; exit 130; }
trap cleanup EXIT
trap on_signal INT TERM

"$container_bin" volume create -s 10737418240 \
  --label dev.gascan.test=true \
  --label "dev.gascan.test.owner=$owner_token" \
  "$tools_volume" >/dev/null
owned_volume "$tools_volume"

"$container_bin" create --name "$name" --init --label dev.gascan.test=true \
  --label "dev.gascan.test.owner=$owner_token" \
  --mount "type=bind,source=$root,target=/workspace" \
  --volume "$tools_volume:/home/workspace/.local" \
  --env HOME=/home/workspace \
  --env CARGO_HOME=/home/workspace/.local/share/cargo \
  --env RUSTUP_HOME=/home/workspace/.local/share/rustup \
  "$local_image" >/dev/null
owned_image
"$container_bin" start "$name" >/dev/null
owned_image
"$container_bin" exec "$name" sudo -n install -d -o workspace -g workspace -m 0700 \
  /home/workspace/.local /home/workspace/.local/share
"$container_bin" exec "$name" env HOME=/home/workspace CARGO_HOME=/home/workspace/.local/share/cargo RUSTUP_HOME=/home/workspace/.local/share/rustup /usr/local/bin/initialize-rust-home
"$container_bin" exec "$name" bash /workspace/tests/image/polyglot-smoke.sh --inside
cleanup
volume_inventory_proves_absent
