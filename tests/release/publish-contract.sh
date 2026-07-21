#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/../.." && pwd -P)
fixture=$(mktemp -d "${TMPDIR:-/tmp}/gascan-publish-contract.XXXXXX")
trap 'rm -rf "$fixture"' EXIT

publish=$repo_root/packaging/macos/publish.sh
[[ -x $publish ]] || { printf 'publish.sh is not executable\n' >&2; exit 1; }

# usage
if "$publish" 2>/dev/null; then
  printf 'missing argument accepted\n' >&2
  exit 1
fi
[[ $("$publish" 2>&1 >/dev/null | head -1) == usage:* ]]

# missing package
if "$publish" "$fixture/absent.pkg" 2>/dev/null; then
  printf 'missing package accepted\n' >&2
  exit 1
fi

# A stub gh that records its invocation and can simulate an existing release.
stub_bin=$fixture/bin
mkdir "$stub_bin"
cat >"$stub_bin/gh" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >>"${GASCAN_STUB_GH_LOG:?}"
case "${1:-} ${2:-}" in
  'release view')
    [[ ${GASCAN_STUB_GH_EXISTING:-no} == yes ]] && exit 0
    exit 1 ;;
esac
exit 0
STUB
chmod +x "$stub_bin/gh"

# An unsigned package can never be published, even with every git gate happy.
mkdir "$fixture/empty-root"
pkgbuild --quiet --root "$fixture/empty-root" \
  --identifier dev.gascan.test --version 1 "$fixture/unsigned.pkg"
export GASCAN_STUB_GH_LOG=$fixture/gh.log
: >"$GASCAN_STUB_GH_LOG"
if PATH=$stub_bin:$PATH "$publish" "$fixture/unsigned.pkg" 2>/dev/null; then
  printf 'unsigned package published\n' >&2
  exit 1
fi
if grep -q 'release create' "$GASCAN_STUB_GH_LOG"; then
  printf 'publish contacted GitHub before trust succeeded\n' >&2
  exit 1
fi

# The script must require the exact signed tag, not merely a signed commit.
grep -Fq 'refs/tags/' "$publish"
grep -Fq 'verify-tag' "$publish"
# It must never clobber.
if grep -q -- '--clobber' "$publish"; then
  printf 'publish uses a clobber flag\n' >&2
  exit 1
fi
# It must create the release as a draft and clear the flag only at the end.
grep -Fq -- '--draft' "$publish"
grep -Fq -- '--draft=false' "$publish"
# It must publish exactly three assets.
grep -Fq 'build-manifest.json' "$publish"
grep -Fq '.sha256' "$publish"

printf 'PASS: Gas Can publish contract\n'
