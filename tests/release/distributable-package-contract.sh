#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/../.." && pwd -P)
source "$repo_root/packaging/macos/release-common.sh"
fixture=$(mktemp -d "${TMPDIR:-/tmp}/gascan-distributable-contract.XXXXXX")
trap 'rm -rf "$fixture"' EXIT

team=Z548WR4TF8

# A real, genuinely unsigned package. pkgbuild over an empty root is instant.
mkdir "$fixture/empty-root"
pkgbuild --quiet --root "$fixture/empty-root" \
  --identifier dev.gascan.test --version 1 "$fixture/unsigned.pkg"

# Real tools must reject the real unsigned package.
if gascan_assert_distributable_package "$fixture/unsigned.pkg" "$team"; then
  printf 'unsigned package accepted\n' >&2
  exit 1
fi

# Malformed inputs.
if gascan_assert_distributable_package "$fixture/unsigned.pkg" not-a-team; then
  printf 'malformed team identifier accepted\n' >&2
  exit 1
fi
if gascan_assert_distributable_package "$fixture/missing.pkg" "$team"; then
  printf 'missing package accepted\n' >&2
  exit 1
fi

# Stub the signing tools to isolate each individual gate. Each stub forwards
# every subcommand it does not simulate to the real tool.
stub_bin=$fixture/bin
mkdir "$stub_bin"

cat >"$stub_bin/pkgutil" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
if [[ ${1:-} != --check-signature ]]; then
  exec /usr/sbin/pkgutil "$@"
fi
case ${GASCAN_STUB_PKGUTIL:-ok} in
  unsigned)
    printf 'Package "x":\n   Status: no signature\n'; exit 1 ;;
  other-cert)
    printf 'Package "x":\n   Status: signed by a certificate trusted by macOS\n'
    printf '   1. Some Other Certificate\n'; exit 0 ;;
  other-team)
    printf 'Package "x":\n   Status: signed by a Developer ID Installer certificate\n'
    printf '   1. Developer ID Installer: Other LLC (AAAAAAAAAA)\n'; exit 0 ;;
  ok)
    printf 'Package "x":\n   Status: signed by a Developer ID Installer certificate\n'
    printf '   1. Developer ID Installer: Liquescent Development LLC (Z548WR4TF8)\n'; exit 0 ;;
esac
STUB

cat >"$stub_bin/spctl" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
[[ ${GASCAN_STUB_SPCTL:-ok} == ok ]] || exit 3
exit 0
STUB

cat >"$stub_bin/xcrun" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
if [[ ${1:-} != stapler ]]; then
  exec /usr/bin/xcrun "$@"
fi
[[ ${GASCAN_STUB_STAPLER:-ok} == ok ]] || exit 66
exit 0
STUB

cat >"$stub_bin/codesign" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
[[ ${GASCAN_STUB_CODESIGN:-ok} == ok ]] || exit 3
exit 0
STUB

chmod +x "$stub_bin"/*
PATH=$stub_bin:$PATH

# Build a package whose payload holds the three expected executables so the
# per-executable requirement check has something to walk.
mkdir -p "$fixture/root/usr/local/bin"
for binary in gascan gascand gascan-apple-attach; do
  printf '#!/bin/sh\n' >"$fixture/root/usr/local/bin/$binary"
done
pkgbuild --quiet --root "$fixture/root" \
  --identifier dev.gascan.pkg --version 1 "$fixture/payload.pkg"

# With every stub healthy the helper must accept.
gascan_assert_distributable_package "$fixture/payload.pkg" "$team"

# Each gate must fail on its own. The helper is a shell function, so the
# override runs in a subshell rather than through `env`.
assert_rejects() {
  local label=$1
  shift
  if (
    # shellcheck disable=SC2163 # positional parameters are NAME=VALUE pairs
    export "$@"
    gascan_assert_distributable_package "$fixture/payload.pkg" "$team"
  ) 2>/dev/null; then
    printf '%s accepted\n' "$label" >&2
    exit 1
  fi
}

assert_rejects 'unsigned package' GASCAN_STUB_PKGUTIL=unsigned
assert_rejects 'non-Developer-ID certificate' GASCAN_STUB_PKGUTIL=other-cert
assert_rejects 'foreign team signature' GASCAN_STUB_PKGUTIL=other-team
assert_rejects 'Gatekeeper rejection' GASCAN_STUB_SPCTL=reject
assert_rejects 'missing notarization ticket' GASCAN_STUB_STAPLER=reject
assert_rejects 'unsigned executable' GASCAN_STUB_CODESIGN=reject

# The pinned team identifier must appear in release-common.sh.
grep -Fq 'Z548WR4TF8' "$repo_root/packaging/macos/release-common.sh"

printf 'PASS: Gas Can distributable-package contract\n'
