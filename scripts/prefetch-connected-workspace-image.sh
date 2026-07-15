#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd -P)
lock="$root/images/workspace/versions.lock"
artifacts="$root/.artifacts"
context="$artifacts/connected-workspace-context"

die() { printf 'connected workspace prefetch: %s\n' "$*" >&2; exit 1; }
test -f "$lock" || die "missing image lock"
mkdir -p "$artifacts"

run_tool() {
  cargo run --quiet --locked --offline --manifest-path "$root/scripts/Cargo.toml" \
    --bin "$1" -- "${@:2}"
}

connected_lock=()
while IFS= read -r value; do
  connected_lock[${#connected_lock[@]}]=$value
done < <(run_tool prepare-workspace-context --connected-lock "$lock")
test "${#connected_lock[@]}" -eq 5 || die "connected lock parser returned an invalid record"
base_image=${connected_lock[0]}
mise_url=${connected_lock[1]}
mise_sha=${connected_lock[2]}
chromium_url=${connected_lock[3]}
chromium_sha=${connected_lock[4]}

run_tool fetch-image-artifact mise "$mise_url" "$mise_sha" "$artifacts/mise-linux-arm64" >/dev/null
run_tool fetch-image-artifact chromium "$chromium_url" "$chromium_sha" "$artifacts/playwright-chromium-linux-arm64.zip" >/dev/null
run_tool extract-reviewed-chromium "$artifacts/playwright-chromium-linux-arm64.zip" "$artifacts/playwright-chromium-reviewed" >/dev/null

expected_temp=$(mktemp "$artifacts/.expected-tool-versions.XXXXXX")
trap 'rm -f "$expected_temp"' EXIT
run_tool validate-tool-versions "$lock" "$root/images/workspace/etc/mise/config.toml" >"$expected_temp"
chmod 0444 "$expected_temp"
mv -f "$expected_temp" "$artifacts/expected-tool-versions.json"
trap - EXIT

container image pull "$base_image" >/dev/null
inspect=$(container image inspect --format json "$base_image")
inspected=$(printf '%s' "$inspect" | run_tool validate-image-inspect)
test "$inspected" = "${base_image#ubuntu@}" || die "local base inspect differs from locked digest"

run_tool prepare-workspace-context --mode connected --replace \
  "$root" "$lock" "$artifacts" "$context"
