#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/../.." && pwd -P)
mode=${1:-live}

case "$mode" in
  --package-only|live) ;;
  *)
    printf 'usage: %s [--package-only]\n' "$0" >&2
    exit 64
    ;;
esac

package=$("$repo_root/packaging/macos/package.sh")
test -f "$package"

payload=$(pkgutil --payload-files "$package")
grep -qx './usr/local/bin/gascan' <<<"$payload"
grep -qx './usr/local/bin/gascand' <<<"$payload"
grep -qx './usr/local/bin/gascan-apple-attach' <<<"$payload"
grep -qx './usr/local/share/gascan/LICENSE' <<<"$payload"
grep -qx './usr/local/share/gascan/default-gascan.toml' <<<"$payload"
grep -qx './usr/local/share/gascan/build-manifest.json' <<<"$payload"
if grep -Eq '/(container|container-apiserver)$' <<<"$payload"; then
  printf 'package must not contain the Apple container runtime\n' >&2
  exit 1
fi

manifest=$(mktemp -d "${TMPDIR:-/tmp}/gascan-release-manifest.XXXXXX")
installed=false
cleanup() {
  if [[ $installed == true ]]; then
    "$repo_root/packaging/macos/uninstall.sh" >/dev/null 2>&1 || true
  fi
  rm -rf "$manifest"
}
on_signal() {
  trap - EXIT INT TERM
  cleanup
  exit 130
}
trap cleanup EXIT
trap on_signal INT TERM
pkgutil --expand "$package" "$manifest/pkg"
payload_root="$manifest/payload"
mkdir -p "$payload_root"
(cd "$payload_root" && gzip -dc "$manifest/pkg/Payload" | cpio -idm --quiet)
jq -e '
  .schema == 1 and
  .architecture == "arm64" and
  (.source_revision | test("^[0-9a-f]{40}$")) and
  (.files | length == 3) and
  ([.files[].path] | sort == ["usr/local/bin/gascan", "usr/local/bin/gascan-apple-attach", "usr/local/bin/gascand"])
' "$payload_root/usr/local/share/gascan/build-manifest.json" >/dev/null
while IFS=$'\t' read -r relative expected; do
  actual=$(shasum -a 256 "$payload_root/$relative" | awk '{print $1}')
  test "$actual" = "$expected"
done < <(jq -r '.files[] | [.path, .sha256] | @tsv' "$payload_root/usr/local/share/gascan/build-manifest.json")

if [[ $mode == --package-only ]]; then
  printf 'PASS: Gas Can macOS package contract\n'
  exit 0
fi

if [[ ${GASCAN_RELEASE_CLEAN_HOST_CONFIRM:-} != YES ]]; then
  printf 'refusing live clean-host mutation without GASCAN_RELEASE_CLEAN_HOST_CONFIRM=YES\n' >&2
  exit 64
fi

"$repo_root/packaging/macos/install.sh" "$package"
installed=true

status=0
/usr/local/bin/gascan doctor --json |
  jq -e '([.checks[] | select(.status != "pass")] | length) == 0' >/dev/null || status=$?
if [[ $status -eq 0 ]]; then
  "$repo_root/packaging/macos/release-smoke.sh" || status=$?
fi
if "$repo_root/packaging/macos/uninstall.sh"; then
  installed=false
else
  status=$?
fi

for binary in gascan gascand gascan-apple-attach; do
  test ! -e "/usr/local/bin/$binary"
done
if container list --all --format json | jq -e \
  '.[] | select(.configuration.id | startswith("gate5-release-"))' >/dev/null; then
  printf 'release gate left a test-owned Apple container behind\n' >&2
  status=1
fi
if container volume list --format json | jq -e \
  '.[] | select(.configuration.name | contains("gate5-release-"))' >/dev/null; then
  printf 'release gate left a test-owned Apple volume behind\n' >&2
  status=1
fi

if [[ $status -ne 0 ]]; then
  printf 'FAIL: Gas Can macOS MVP release gate (status %s)\n' "$status" >&2
  exit "$status"
fi

printf 'PASS: Gas Can macOS MVP release gate\n'
