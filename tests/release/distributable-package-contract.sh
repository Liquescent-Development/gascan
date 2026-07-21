#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/../.." && pwd -P)
source "$repo_root/packaging/macos/release-common.sh"
fixture=$(mktemp -d "${TMPDIR:-/tmp}/gascan-distributable-contract.XXXXXX")
trap 'rm -rf "$fixture"' EXIT

team=Z548WR4TF8

# The exit code is part of the published interface: publish.sh branches on it.
# Every rejection asserts the exact status, never merely "nonzero".
assert_status() {
  local label=$1 expected=$2 rc=0
  shift 2
  ( "$@" ) 2>/dev/null || rc=$?
  [[ $rc -eq $expected ]] || {
    printf '%s: expected exit %s, got %s\n' "$label" "$expected" "$rc" >&2
    exit 1
  }
}

# Run the helper with stub overrides exported into the subshell assert_status
# already provides.
run_helper() {
  local package=$1 team_argument=$2
  shift 2
  if [[ $# -gt 0 ]]; then
    # shellcheck disable=SC2163 # positional parameters are NAME=VALUE pairs
    export "$@"
  fi
  gascan_assert_distributable_package "$package" "$team_argument"
}

# A real, genuinely unsigned package. pkgbuild over an empty root is instant.
mkdir "$fixture/empty-root"
pkgbuild --quiet --root "$fixture/empty-root" \
  --identifier dev.gascan.test --version 1 "$fixture/unsigned.pkg"

# Real tools must reject the real unsigned package.
assert_status 'real unsigned package' 65 \
  gascan_assert_distributable_package "$fixture/unsigned.pkg" "$team"

assert_status 'missing package' 66 \
  gascan_assert_distributable_package "$fixture/missing.pkg" "$team"

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
# Each case trips exactly one gate: the certificate lines below satisfy every
# gate except the one the case is named for.
case ${GASCAN_STUB_PKGUTIL:-ok} in
  untrusted)
    printf 'Package "x":\n   Status: signed by an untrusted certificate\n'
    printf '   1. Developer ID Installer: Liquescent Development LLC (Z548WR4TF8)\n'
    exit 1 ;;
  other-cert)
    printf 'Package "x":\n   Status: signed by a certificate trusted by macOS\n'
    printf '   1. Apple Development: Liquescent Development LLC (Z548WR4TF8)\n'; exit 0 ;;
  other-team)
    printf 'Package "x":\n   Status: signed by a Developer ID Installer certificate\n'
    printf '   1. Developer ID Installer: Other LLC (AAAAAAAAAA)\n'; exit 0 ;;
  ok)
    printf 'Package "x":\n   Status: signed by a Developer ID Installer certificate\n'
    printf '   1. Developer ID Installer: Liquescent Development LLC (Z548WR4TF8)\n'; exit 0 ;;
  *)
    printf 'pkgutil stub: unknown scenario: %s\n' "${GASCAN_STUB_PKGUTIL:-}" >&2
    exit 70 ;;
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

# The requirement string is the substance of the strongest gate, so the stub
# validates what it was handed before it consults the scenario variable.
cat >"$stub_bin/codesign" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
strict=false
requirement=
while (($#)); do
  case $1 in
    --strict) strict=true ;;
    -R) requirement=${2:-}; shift ;;
  esac
  shift
done
[[ $strict == true ]] || {
  printf 'codesign stub: --strict is missing\n' >&2; exit 90
}
[[ $requirement == *'subject.OU] = Z548WR4TF8'* ]] || {
  printf 'codesign stub: requirement does not pin the team: %s\n' "$requirement" >&2
  exit 91
}
[[ $requirement == *'certificate 1[field.1.2.840.113635.100.6.2.6] exists'* ]] || {
  printf 'codesign stub: requirement lacks the Developer ID intermediate OID: %s\n' \
    "$requirement" >&2
  exit 92
}
[[ $requirement == *'certificate leaf[field.1.2.840.113635.100.6.1.13] exists'* ]] || {
  printf 'codesign stub: requirement lacks the Developer ID leaf OID: %s\n' \
    "$requirement" >&2
  exit 93
}
[[ ${GASCAN_STUB_CODESIGN:-ok} == ok ]] || exit 3
exit 0
STUB

chmod +x "$stub_bin"/*
PATH=$stub_bin:$PATH

# Build a package whose payload is exactly what a real Gas Can package ships,
# so it satisfies the shared allowlist and the per-executable requirement check
# has something to walk.
mkdir -p "$fixture/root/usr/local/bin" "$fixture/root/usr/local/share/gascan"
for binary in gascan gascand gascan-apple-attach; do
  printf '#!/bin/sh\n' >"$fixture/root/usr/local/bin/$binary"
done
for payload_file in LICENSE build-manifest.json default-gascan.toml; do
  printf 'fixture\n' >"$fixture/root/usr/local/share/gascan/$payload_file"
done
pkgbuild --quiet --root "$fixture/root" \
  --identifier dev.gascan.pkg --version 1 "$fixture/payload.pkg"

# Payloads that differ from the allowlist, and a payload carrying scripts.
cp -R "$fixture/root" "$fixture/extra-root"
printf '#!/bin/sh\n' >"$fixture/extra-root/usr/local/bin/EXTRA-UNSIGNED-BINARY"
pkgbuild --quiet --root "$fixture/extra-root" \
  --identifier dev.gascan.pkg --version 1 "$fixture/extra.pkg"

# An unsigned binary outside usr/local/bin: the directory the old walk never read.
cp -R "$fixture/root" "$fixture/outside-root"
mkdir "$fixture/outside-root/usr/local/libexec"
printf '#!/bin/sh\n' >"$fixture/outside-root/usr/local/libexec/EVIL-UNSIGNED-BINARY"
pkgbuild --quiet --root "$fixture/outside-root" \
  --identifier dev.gascan.pkg --version 1 "$fixture/outside.pkg"

# A symlink among the executables: invisible to a `find -type f` walk.
cp -R "$fixture/root" "$fixture/symlink-root"
ln -s /bin/sh "$fixture/symlink-root/usr/local/bin/EVIL-SYMLINK"
pkgbuild --quiet --root "$fixture/symlink-root" \
  --identifier dev.gascan.pkg --version 1 "$fixture/symlink.pkg"

cp -R "$fixture/root" "$fixture/incomplete-root"
rm "$fixture/incomplete-root/usr/local/bin/gascand"
pkgbuild --quiet --root "$fixture/incomplete-root" \
  --identifier dev.gascan.pkg --version 1 "$fixture/incomplete.pkg"

mkdir "$fixture/scripts"
printf '#!/bin/sh\nexit 0\n' >"$fixture/scripts/postinstall"
chmod +x "$fixture/scripts/postinstall"
pkgbuild --quiet --root "$fixture/root" --scripts "$fixture/scripts" \
  --identifier dev.gascan.pkg --version 1 "$fixture/scripted.pkg"

# With every stub healthy the helper must accept.
gascan_assert_distributable_package "$fixture/payload.pkg" "$team"

# The malformed-team gate is asserted against the package that otherwise
# passes, so the team gate is the only possible reason for the rejection.
assert_status 'malformed team identifier' 64 \
  run_helper "$fixture/payload.pkg" not-a-team

# Each gate must fail on its own, with the trust-failure status.
assert_status 'untrusted package signature' 65 \
  run_helper "$fixture/payload.pkg" "$team" GASCAN_STUB_PKGUTIL=untrusted
assert_status 'non-Developer-ID certificate' 65 \
  run_helper "$fixture/payload.pkg" "$team" GASCAN_STUB_PKGUTIL=other-cert
assert_status 'foreign team signature' 65 \
  run_helper "$fixture/payload.pkg" "$team" GASCAN_STUB_PKGUTIL=other-team
assert_status 'Gatekeeper rejection' 65 \
  run_helper "$fixture/payload.pkg" "$team" GASCAN_STUB_SPCTL=reject
assert_status 'missing notarization ticket' 65 \
  run_helper "$fixture/payload.pkg" "$team" GASCAN_STUB_STAPLER=reject
assert_status 'unsigned executable' 65 \
  run_helper "$fixture/payload.pkg" "$team" GASCAN_STUB_CODESIGN=reject

# The whole payload is gated, not three names.
assert_status 'payload with an extra executable' 65 \
  gascan_assert_distributable_package "$fixture/extra.pkg" "$team"
assert_status 'payload with a binary outside usr/local/bin' 65 \
  gascan_assert_distributable_package "$fixture/outside.pkg" "$team"
assert_status 'payload with a symlink among the executables' 65 \
  gascan_assert_distributable_package "$fixture/symlink.pkg" "$team"
assert_status 'payload missing an executable' 65 \
  gascan_assert_distributable_package "$fixture/incomplete.pkg" "$team"
assert_status 'payload carrying installer scripts' 65 \
  gascan_assert_distributable_package "$fixture/scripted.pkg" "$team"

# The requirement the helper hands codesign must be one macOS can actually
# parse. A stub can only compare text; csreq is the real parser, and it needs no
# signing identity. Without this, a requirement macOS rejects as malformed --
# which would reject every legitimately signed package -- would ship green.
requirement=$(gascan_developer_id_requirement "$team")
csreq -r- -b /dev/null <<<"$requirement" || {
  printf 'the Developer ID requirement does not compile: %s\n' "$requirement" >&2
  exit 1
}
[[ $requirement == *"subject.OU] = $team"* ]] || {
  printf 'the Developer ID requirement does not pin the team it was given: %s\n' \
    "$requirement" >&2
  exit 1
}
# Proof that the compile check discriminates: the same string plus an unbalanced
# group keeps every substring the codesign stub inspects, and must not compile.
if csreq -r- -b /dev/null <<<"$requirement and (((" 2>/dev/null; then
  printf 'csreq accepted an unparseable requirement; the compile check is vacuous\n' >&2
  exit 1
fi

# The pinned team identifier must appear in release-common.sh.
grep -Fq 'Z548WR4TF8' "$repo_root/packaging/macos/release-common.sh"

printf 'PASS: Gas Can distributable-package contract\n'
