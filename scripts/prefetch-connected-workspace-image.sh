#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd -P)
lock="$root/images/workspace/versions.lock"
artifacts="$root/.artifacts"
context="$artifacts/connected-workspace-context"
package_manifest="$root/images/workspace/workstation-package.json"
package_lock="$root/images/workspace/workstation-package-lock.json"

die() { printf 'connected workspace prefetch: %s\n' "$*" >&2; exit 1; }
test -f "$lock" || die "missing image lock"
mkdir -p "$artifacts"
umask 077

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

workstation_records=$(mktemp "$artifacts/.workstation-records.XXXXXX")
workstation_records_after=$(mktemp "$artifacts/.workstation-records-after.XXXXXX")
workstation_staging=$(mktemp -d "$artifacts/.workstation-prefetch.XXXXXX")
workstation="$workstation_staging/workstation"
trap 'rm -f "$workstation_records" "$workstation_records_after"; rm -rf "$workstation_staging"' EXIT
run_tool prepare-workspace-context --workstation-lock \
  "$lock" "$package_manifest" "$package_lock" >"$workstation_records"
mkdir -m 0700 "$workstation"
while IFS=$'\t' read -r kind relative class url digest bound; do
  case "$kind" in
    receipt)
      test "$relative" = "prefetch-lock.sha256" || die "invalid workstation receipt path"
      printf '%s\n' "$class" >"$workstation/$relative"
      chmod 0444 "$workstation/$relative"
      ;;
    native|npm)
      case "$relative" in
        ""|/*|*../*|../*|*/../*|*/..) die "unsafe workstation artifact path" ;;
      esac
      case "$class" in
        workstation-github|workstation-gitlab|workstation-npm|workstation-npm-native) ;;
        *) die "invalid workstation artifact class" ;;
      esac
      mkdir -p "$(dirname "$workstation/$relative")"
      run_tool fetch-image-artifact "$class" "$url" "$digest" \
        "$workstation/$relative" "$bound" >/dev/null
      ;;
    *) die "invalid workstation lock record" ;;
  esac
done <"$workstation_records"
run_tool prepare-workspace-context --workstation-lock \
  "$lock" "$package_manifest" "$package_lock" >"$workstation_records_after"
cmp -s "$workstation_records" "$workstation_records_after" ||
  die "workstation lock changed during prefetch"
run_tool prepare-workspace-context --publish-workstation-cache \
  "$lock" "$package_manifest" "$package_lock" \
  "$workstation_staging" "$artifacts/workstation"
rm -f "$workstation_records" "$workstation_records_after"
rm -rf "$workstation_staging"
trap - EXIT

expected_temp=$(mktemp "$artifacts/.expected-tool-versions.XXXXXX")
trap 'rm -f "$expected_temp"' EXIT
run_tool validate-tool-versions "$lock" "$root/images/workspace/etc/mise/config.toml" >"$expected_temp"
chmod 0444 "$expected_temp"
mv -f "$expected_temp" "$artifacts/expected-tool-versions.json"
trap - EXIT

container image pull --platform linux/arm64 "$base_image" >/dev/null
inspect=$(container image inspect "$base_image")
inspected=$(printf '%s' "$inspect" | run_tool validate-image-inspect)
test "$inspected" = "${base_image#ubuntu@}" || die "local base inspect differs from locked digest"

run_tool prepare-workspace-context --mode connected --replace \
  "$root" "$lock" "$artifacts" "$context"
