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
# The same directory also carries narrowly-scoped wrappers around pkgutil,
# codesign, spctl and xcrun: real Apple notarization requires a live network
# submission to Apple's notary service, which a fast, offline contract test
# cannot do. Those wrappers pass every call straight through to the real
# binary unless GASCAN_STUB_TRUST_BYPASS=yes *and* the call matches the
# known fixture package/executables exactly, so they only ever fake the one
# property (a distributable Gas Can package) that cannot be constructed
# offline, and never change behavior for any other invocation.
stub_bin=$fixture/bin
mkdir "$stub_bin"
cat >"$stub_bin/gh" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >>"${GASCAN_STUB_GH_LOG:?}"
case "${1:-} ${2:-}" in
  'release view')
    if [[ " $* " == *' --json '* ]]; then
      printf '%s\n' "${GASCAN_STUB_GH_ASSETS:?}"
      exit 0
    fi
    [[ ${GASCAN_STUB_GH_EXISTING:-no} == yes ]] && exit 0
    exit 1 ;;
  'release upload')
    # Real gh is suspected to gate this on a TTY, but that cannot be proven
    # without a live release. Chatter here regardless, so the assertion below
    # verifies publish.sh's own redirect, not gh's behavior.
    printf 'Uploading gascan.pkg 100%%\n' ;;
esac
exit 0
STUB
chmod +x "$stub_bin/gh"

cat >"$stub_bin/pkgutil" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
if [[ ${GASCAN_STUB_TRUST_BYPASS:-no} == yes && ${1:-} == --check-signature \
      && ${2:-} == "${GASCAN_STUB_FIXTURE_PKG:-}" ]]; then
  printf 'Package "%s":\n' "$(basename "$2")"
  printf '   Status: signed by a developer certificate issued by Apple for distribution\n'
  printf '   Certificate Chain:\n'
  printf '    1. Developer ID Installer: Fixture Test (%s)\n' "${GASCAN_STUB_TEAM:?}"
  exit 0
fi
exec /usr/sbin/pkgutil "$@"
STUB
chmod +x "$stub_bin/pkgutil"

cat >"$stub_bin/spctl" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
if [[ ${GASCAN_STUB_TRUST_BYPASS:-no} == yes && ${1:-} == --assess && ${2:-} == --type \
      && ${3:-} == install && ${4:-} == "${GASCAN_STUB_FIXTURE_PKG:-}" ]]; then
  exit 0
fi
exec /usr/sbin/spctl "$@"
STUB
chmod +x "$stub_bin/spctl"

cat >"$stub_bin/xcrun" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
if [[ ${GASCAN_STUB_TRUST_BYPASS:-no} == yes && ${1:-} == stapler && ${2:-} == validate \
      && ${3:-} == "${GASCAN_STUB_FIXTURE_PKG:-}" ]]; then
  exit 0
fi
exec /usr/bin/xcrun "$@"
STUB
chmod +x "$stub_bin/xcrun"

cat >"$stub_bin/codesign" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
if [[ ${GASCAN_STUB_TRUST_BYPASS:-no} == yes ]]; then
  last=${!#}
  case "$(basename "$last")" in
    gascan|gascan-apple-attach|gascand)
      for arg in "$@"; do
        if [[ $arg == "=anchor apple generic and certificate leaf[subject.OU] = ${GASCAN_STUB_TEAM:-}"* ]]; then
          exit 0
        fi
      done
      ;;
  esac
fi
exec /usr/bin/codesign "$@"
STUB
chmod +x "$stub_bin/codesign"

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

version=$(cd "$repo_root" && cargo metadata --locked --no-deps --format-version 1 |
  jq -er '.packages[] | select(.name == "gascan") | .version')
tag="v$version"
team=$(bash -c "source '$repo_root/packaging/macos/release-common.sh'; printf '%s' \"\$GASCAN_RELEASE_TEAM\"")

# Case A: an annotated, genuinely signed v<version> release tag that does not
# peel to HEAD. Built in its own disposable clone with an ephemeral ed25519
# signing key and allowed-signers file (same technique as
# source-signature-contract.sh), so the property holds for whatever the
# workspace version happens to be, before or after any real release is cut —
# unlike depending on this repository's actual v0.1.1 tag, which is only ever
# true of one snapshot in time. This is a behavioral replacement for the old
# source-text greps for 'refs/tags/' and 'verify-tag', which a mutation
# (replacing the whole tag gate with `true`) sailed straight through.
clone_a=$fixture/clone-a
git clone --quiet "$repo_root" "$clone_a"
ssh-keygen -q -t ed25519 -N '' -C release@example.invalid -f "$fixture/case-a-key"
printf 'release@example.invalid %s\n' "$(cat "$fixture/case-a-key.pub")" \
  >"$fixture/case-a-allowed-signers"
git -C "$clone_a" config user.name release
git -C "$clone_a" config user.email release@example.invalid
git -C "$clone_a" config gpg.format ssh
git -C "$clone_a" config user.signingKey "$fixture/case-a-key"
git -C "$clone_a" config gpg.ssh.allowedSignersFile "$fixture/case-a-allowed-signers"
not_head=$(git -C "$clone_a" rev-parse HEAD~1)
git -C "$clone_a" tag -d "$tag" >/dev/null 2>&1 || true
git -C "$clone_a" tag -s "$tag" -m "publish-contract test fixture: not HEAD" "$not_head"
git -C "$clone_a" verify-tag "$tag" >/dev/null 2>&1

: >"$GASCAN_STUB_GH_LOG"
set +e
case_a_err=$(PATH=$stub_bin:$PATH "$clone_a/packaging/macos/publish.sh" "$fixture/unsigned.pkg" 2>&1 >/dev/null)
case_a_status=$?
set -e
if [[ $case_a_status -eq 0 ]]; then
  printf 'Case A: publish succeeded although tag %s does not point at HEAD\n' "$tag" >&2
  exit 1
fi
if [[ $case_a_err != "release tag $tag does not point at HEAD" ]]; then
  printf 'Case A: unexpected failure message: %s\n' "$case_a_err" >&2
  exit 1
fi
if [[ -s $GASCAN_STUB_GH_LOG ]]; then
  printf 'Case A: gh was contacted before the tag gate failed\n' >&2
  exit 1
fi

# Build a fixture package that satisfies every structural check in
# verify-package.sh, so it reaches gascan_assert_distributable_package
# (Case B) or, once that gate is bypassed (Case C), gh itself. Real macOS
# executables must be thin arm64 for verify-package.sh's `lipo -archs`
# check; every system binary tried (/bin/echo included) is a universal
# x86_64+arm64e binary, so a tiny arm64-only binary is compiled instead.
fixture_root=$fixture/pkgroot
mkdir -p "$fixture_root/usr/local/bin" "$fixture_root/usr/local/share/gascan"
cc -arch arm64 -o "$fixture/thin-arm64" -x c - <<<'int main(void){return 0;}'
for entry in gascan gascan-apple-attach gascand; do
  cp "$fixture/thin-arm64" "$fixture_root/usr/local/bin/$entry"
done
printf 'LICENSE fixture\n' >"$fixture_root/usr/local/share/gascan/LICENSE"
printf '# fixture default config\n' >"$fixture_root/usr/local/share/gascan/default-gascan.toml"

revision=$(git -C "$repo_root" rev-parse --verify HEAD)
sha_gascan=$(shasum -a 256 "$fixture_root/usr/local/bin/gascan" | awk '{print $1}')
sha_attach=$(shasum -a 256 "$fixture_root/usr/local/bin/gascan-apple-attach" | awk '{print $1}')
sha_gascand=$(shasum -a 256 "$fixture_root/usr/local/bin/gascand" | awk '{print $1}')
jq -n --arg rev "$revision" --arg ver "$version" \
  --arg s1 "$sha_gascan" --arg s2 "$sha_attach" --arg s3 "$sha_gascand" '
{
  architecture: "arm64",
  files: [
    {path: "usr/local/bin/gascan", sha256: $s1},
    {path: "usr/local/bin/gascan-apple-attach", sha256: $s2},
    {path: "usr/local/bin/gascand", sha256: $s3}
  ],
  product: "Gas Can",
  schema: 1,
  source_revision: $rev,
  version: $ver
}' >"$fixture_root/usr/local/share/gascan/build-manifest.json"

fixture_pkg=$fixture/gascan-$version-macos-arm64.pkg
pkgbuild --quiet --identifier dev.gascan.pkg --version "$version" --install-location / \
  --root "$fixture_root" "$fixture_pkg"

# Cases B and C need the tag gate to pass, which requires a signed tag
# pointing at HEAD. Retargeting the real tag would mutate this repository's
# refs, so a disposable local clone is used instead, exactly as the review
# that found this gap did it: a real `git clone`, never the working repo.
clone=$fixture/clone
git clone --quiet "$repo_root" "$clone"
git -C "$clone" tag -d "$tag" >/dev/null 2>&1 || true
git -C "$clone" tag -s "$tag" -m "publish-contract test fixture" HEAD
git -C "$clone" verify-tag "$tag" >/dev/null 2>&1

# Case B: the fixture package passes verify-package.sh and therefore reaches
# gascan_assert_distributable_package, which must reject it (it is not
# Developer-ID signed). This is a behavioral replacement for the old
# source-text grep, which a mutation (deleting the
# gascan_assert_distributable_package call) sailed straight through.
: >"$GASCAN_STUB_GH_LOG"
set +e
case_b_err=$(PATH=$stub_bin:$PATH "$clone/packaging/macos/publish.sh" "$fixture_pkg" 2>&1 >/dev/null)
case_b_status=$?
set -e
if [[ $case_b_status -eq 0 ]]; then
  printf 'Case B: publish succeeded with an unsigned package\n' >&2
  exit 1
fi
if [[ $case_b_err != 'package is not signed' ]]; then
  printf 'Case B: distributable gate was not reached; got: %s\n' "$case_b_err" >&2
  exit 1
fi
if [[ -s $GASCAN_STUB_GH_LOG ]]; then
  printf 'Case B: gh was contacted before the distributable gate failed\n' >&2
  exit 1
fi

# Case C: with the distributable gate bypassed (the only realistic way to
# reach this point offline — genuine notarization needs a live submission to
# Apple, so the fixture package can never be genuinely distributable) and an
# existing release simulated, publish.sh must refuse to clobber it and must
# never call `release create`. This is a behavioral replacement for the old
# source-text greps ('--clobber' absence, 'gh release view' presence), which
# a mutation (deleting the whole no-clobber block) sailed straight through.
: >"$GASCAN_STUB_GH_LOG"
export GASCAN_STUB_TRUST_BYPASS=yes
export GASCAN_STUB_FIXTURE_PKG=$fixture_pkg
export GASCAN_STUB_TEAM=$team
export GASCAN_STUB_GH_EXISTING=yes
set +e
case_c_err=$(PATH=$stub_bin:$PATH "$clone/packaging/macos/publish.sh" "$fixture_pkg" 2>&1 >/dev/null)
case_c_status=$?
set -e
unset GASCAN_STUB_TRUST_BYPASS GASCAN_STUB_FIXTURE_PKG GASCAN_STUB_TEAM GASCAN_STUB_GH_EXISTING
if [[ $case_c_status -eq 0 ]]; then
  printf 'Case C: publish succeeded although a release already exists\n' >&2
  exit 1
fi
if [[ $case_c_err != "release $tag already exists; publish a new version instead" ]]; then
  printf 'Case C: unexpected failure message: %s\n' "$case_c_err" >&2
  exit 1
fi
if [[ "$(cat "$GASCAN_STUB_GH_LOG")" != "release view $tag" ]]; then
  printf 'Case C: gh was not called exactly once with release view: %s\n' "$(cat "$GASCAN_STUB_GH_LOG")" >&2
  exit 1
fi

# Case D: the full happy path, distributable gate bypassed exactly as Case C
# and no existing release, so publish.sh runs all the way through. The stub
# gh chatters on `release upload`'s stdout, the same way the real CLI's
# progress message is suspected to -- unprovable without a live release --
# so this is what proves publish.sh's own stdout stays exactly the asset URL
# then the SHA-256 regardless. This is the producing side of that two-line
# contract release.sh asserts; every other publish.sh invocation in this file
# is a failing case captured with `2>&1 >/dev/null`, so nothing tested it
# before.
: >"$GASCAN_STUB_GH_LOG"
export GASCAN_STUB_TRUST_BYPASS=yes
export GASCAN_STUB_FIXTURE_PKG=$fixture_pkg
export GASCAN_STUB_TEAM=$team
export GASCAN_STUB_GH_EXISTING=no
base_name=$(basename "$fixture_pkg")
export GASCAN_STUB_GH_ASSETS="build-manifest.json,$base_name,$base_name.sha256"
set +e
case_d_out=$(PATH=$stub_bin:$PATH "$clone/packaging/macos/publish.sh" "$fixture_pkg")
case_d_status=$?
set -e
unset GASCAN_STUB_TRUST_BYPASS GASCAN_STUB_FIXTURE_PKG GASCAN_STUB_TEAM \
  GASCAN_STUB_GH_EXISTING GASCAN_STUB_GH_ASSETS
if [[ $case_d_status -ne 0 ]]; then
  printf 'Case D: publish failed on the happy path:\n%s\n' "$case_d_out" >&2
  exit 1
fi
case_d_lines=$(grep -c '' <<<"$case_d_out")
[[ $case_d_lines -eq 2 ]] || {
  printf 'Case D: expected exactly two stdout lines, got %s:\n%s\n' \
    "$case_d_lines" "$case_d_out" >&2
  exit 1
}
case_d_url=$(sed -n '1p' <<<"$case_d_out")
case_d_sum=$(sed -n '2p' <<<"$case_d_out")
[[ $case_d_url == https://github.com/*/releases/download/*/* ]] || {
  printf 'Case D: first line is not an asset-URL shape: %s\n' "$case_d_url" >&2
  exit 1
}
[[ $case_d_sum =~ ^[0-9a-f]{64}$ ]] || {
  printf 'Case D: second line is not 64 hex characters: %s\n' "$case_d_sum" >&2
  exit 1
}
# A successful publish records the instant the release becomes public, beside
# the package: local, immediate, and interrupt-proof, which release.sh reads
# from its EXIT trap instead of asking GitHub there.
published_marker="$fixture/$tag.published"
[[ -f $published_marker ]] || {
  printf 'Case D: publish left no %s beside the package\n' "$tag.published" >&2
  exit 1
}

# It must never clobber.
if grep -q -- '--clobber' "$publish"; then
  printf 'publish uses a clobber flag\n' >&2
  exit 1
fi
# The happy path (creating a draft, verifying assets, then un-drafting) can
# only be reached with a real signed tag and a genuinely notarized package,
# neither of which can be constructed offline, so the sequencing is asserted
# structurally instead: every `gh release create` call must request a draft,
# and the un-draft call must appear after the upload call in source order.
if grep -n 'gh release create' "$publish" | grep -v -- '--draft'; then
  printf 'a gh release create call omits --draft\n' >&2
  exit 1
fi
upload_line=$(grep -n 'gh release upload' "$publish" | head -1 | cut -d: -f1)
undraft_line=$(grep -n -- '--draft=false' "$publish" | head -1 | cut -d: -f1)
[[ -n $upload_line && -n $undraft_line && $undraft_line -gt $upload_line ]] || {
  printf -- '--draft=false does not appear after the asset upload\n' >&2
  exit 1
}
# It must publish exactly three assets.
grep -Fq 'build-manifest.json' "$publish"
grep -Fq '.sha256' "$publish"

# C1: the asset-completeness check must derive its expected value through the
# same `sort | join(",")` jq pipeline as the actual `gh release view` value,
# never a hand-ordered string — otherwise codepoint sort places
# build-manifest.json first and the two sides can never be equal.
# shellcheck disable=SC2016 # single quotes are deliberate: asserting literal source text, not expanding it
grep -Fq 'gascan_expected_release_assets "$base"' "$publish" || {
  printf 'publish.sh does not derive expected assets from the shared helper\n' >&2
  exit 1
}
pipeline_count=$(grep -hv '^[[:space:]]*#' "$publish" "$repo_root/packaging/macos/release-common.sh" |
  grep -o 'sort | join(",")' | wc -l | tr -d ' ')
[[ $pipeline_count -eq 2 ]] || {
  printf 'expected two sort | join(",") pipelines guarding release assets, found %s\n' \
    "$pipeline_count" >&2
  exit 1
}

# C2: execute that helper rather than pattern-matching it. The happy path
# cannot run offline, so this is the only place the comparison's actual output
# is observed. It must be raw text in codepoint order: emitting a JSON-encoded
# string wraps it in quotes, which compares unequal to the raw value `gh --jq`
# returns and would reject every release, complete or not.
source "$repo_root/packaging/macos/release-common.sh"
observed_assets=$(gascan_expected_release_assets gascan-9.9.9-macos-arm64.pkg)
want_assets='build-manifest.json,gascan-9.9.9-macos-arm64.pkg,gascan-9.9.9-macos-arm64.pkg.sha256'
[[ $observed_assets == "$want_assets" ]] || {
  printf 'expected asset list is not raw sorted text\n  want: %s\n  got:  %s\n' \
    "$want_assets" "$observed_assets" >&2
  exit 1
}

# C3: the published marker is derived once, in release-common.sh, and both
# publish.sh and release.sh call it rather than rebuilding the path inline --
# the same coupling C1 proves for the expected-asset list. Without this,
# either side can drift and the suite stays green while release.sh's live-
# release warning silently never fires.
release_script=$repo_root/packaging/macos/release.sh
for f in "$publish" "$release_script"; do
  # shellcheck disable=SC2016 # single quotes are deliberate: asserting literal source text, not expanding it
  grep -Fq 'gascan_published_marker "$package" "$version"' "$f" || {
    printf '%s does not derive the published marker from the shared helper\n' "$f" >&2
    exit 1
  }
done
if grep -F '.published"' "$publish" "$release_script"; then
  printf 'the published marker path is still built inline\n' >&2
  exit 1
fi

# C4: execute the helper directly, the same as C2 does for the asset
# pipeline -- the only place its actual output is observed.
observed_marker=$(gascan_published_marker /artifacts/gascan-9.9.9-macos-arm64.pkg 9.9.9)
[[ $observed_marker == /artifacts/v9.9.9.published ]] || {
  printf 'published marker path is wrong\n  want: /artifacts/v9.9.9.published\n  got:  %s\n' \
    "$observed_marker" >&2
  exit 1
}

printf 'PASS: Gas Can publish contract\n'
