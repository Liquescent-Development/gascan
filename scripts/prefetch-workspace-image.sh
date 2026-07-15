#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd -P)
lock="$root/images/workspace/versions.lock"
artifacts="$root/.artifacts"
context="$artifacts/workspace-context"
base_image='ubuntu@sha256:7f622ca8766bccb22f04242ecb6f19f770b2f08827dc4b8c707de5e78a6da7ab'

die() { printf 'workspace image prefetch: %s\n' "$*" >&2; exit 1; }
test -f "$lock" || die "missing image lock"
mkdir -p "$artifacts/bundles"

top_value() {
  awk -F ' = ' -v key="$1" '$1 == key { gsub(/^"|"$/, "", $2); print $2; exit }' "$lock"
}
section_value() {
  awk -F ' = ' -v section="[$1]" -v key="$2" '
    $0 == section { active=1; next }
    /^\[/ { active=0 }
    active && $1 == key { gsub(/^"|"$/, "", $2); print $2; exit }
  ' "$lock"
}
fetch() {
  cargo run --quiet --locked --offline --manifest-path "$root/scripts/Cargo.toml" \
    --bin fetch-image-artifact -- "$@"
}

test "$(top_value base_image)" = "$base_image" || die "base image differs from the reviewed exact digest"
test "$(section_value workspace_bundles publication)" = published || die "bundle publication state is not published"

mise_url=$(section_value mise url); mise_sha=$(section_value mise sha256)
chromium_url=$(section_value playwright_chromium url); chromium_sha=$(section_value playwright_chromium sha256)
test -n "$mise_url" && test -n "$mise_sha" || die "mise lock record is incomplete"
test -n "$chromium_url" && test -n "$chromium_sha" || die "Chromium lock record is incomplete"
fetch mise "$mise_url" "$mise_sha" "$artifacts/mise-linux-arm64"
fetch chromium "$chromium_url" "$chromium_sha" "$artifacts/playwright-chromium-linux-arm64.zip"

for record in ubuntu_packages mise_runtimes gascamp_source_vendor; do
  section="workspace_bundles.$record"
  url=$(section_value "$section" url); sha=$(section_value "$section" sha256); size=$(section_value "$section" size)
  test -n "$url" && test -n "$sha" && test -n "$size" || die "missing published bundle record $record"
  fetch workspace-bundle "$url" "$sha" "$artifacts/bundles/$record.tar.zst" "$size"
done

reviewed_next="$artifacts/.playwright-chromium-reviewed.$$"
reviewed_old="$artifacts/.playwright-chromium-reviewed.old.$$"
trap 'chmod -R u+w "$reviewed_next" "$reviewed_old" 2>/dev/null || true; rm -rf "$reviewed_next" "$reviewed_old"' EXIT
cargo run --quiet --locked --offline --manifest-path "$root/scripts/Cargo.toml" \
  --bin extract-reviewed-chromium -- "$artifacts/playwright-chromium-linux-arm64.zip" "$reviewed_next"
if test -d "$artifacts/playwright-chromium-reviewed" && diff -qr "$artifacts/playwright-chromium-reviewed" "$reviewed_next" >/dev/null; then
  chmod -R u+w "$reviewed_next"
  rm -rf "$reviewed_next"
else
  if test -e "$artifacts/playwright-chromium-reviewed"; then
    mv "$artifacts/playwright-chromium-reviewed" "$reviewed_old"
  fi
  if ! mv "$reviewed_next" "$artifacts/playwright-chromium-reviewed"; then
    test ! -e "$reviewed_old" || mv "$reviewed_old" "$artifacts/playwright-chromium-reviewed"
    die "could not publish reviewed Chromium tree"
  fi
  if test -e "$reviewed_old"; then chmod -R u+w "$reviewed_old"; rm -rf "$reviewed_old"; fi
fi
trap - EXIT
expected_temp=$(mktemp "$artifacts/.expected-tool-versions.XXXXXX")
trap 'rm -f "$expected_temp"' EXIT
cargo run --quiet --locked --offline --manifest-path "$root/scripts/Cargo.toml" \
  --bin validate-tool-versions -- "$lock" "$root/images/workspace/etc/mise/config.toml" >"$expected_temp"
chmod 0444 "$expected_temp"
mv -f "$expected_temp" "$artifacts/expected-tool-versions.json"
trap - EXIT

container image pull "$base_image"
inspect=$(container image inspect --format json "$base_image")
inspected=$(printf '%s' "$inspect" | cargo run --quiet --locked --offline \
  --manifest-path "$root/scripts/Cargo.toml" --bin validate-image-inspect)
test "$inspected" = "${base_image#ubuntu@}" || die "local base inspect differs from locked digest"

cargo run --quiet --locked --offline --manifest-path "$root/scripts/Cargo.toml" \
  --bin prepare-workspace-context -- --replace "$root" "$lock" "$artifacts" "$context"
