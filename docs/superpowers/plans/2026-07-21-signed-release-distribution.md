# Signed Release Distribution Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let users install Gas Can from a signed, notarized package instead of
cloning the repository and building from source.

**Architecture:** Three local stages. `package.sh` (unchanged) builds and signs.
A new `publish.sh` refuses to publish anything that is not Developer ID signed,
notarized, stapled, and bound to the exact signed release tag, then creates a
GitHub Release as a draft and only clears the draft flag once all three assets
upload. A new `render-cask.sh` deterministically renders the Homebrew cask from
the published version and checksum.

**Tech Stack:** Bash, `pkgutil`, `spctl`, `xcrun stapler`, `codesign`, `gh`,
`jq`, `cargo metadata`, Homebrew cask DSL.

## Global Constraints

- Every shell file starts with `#!/usr/bin/env bash` and `set -euo pipefail`.
- Target is Apple silicon macOS 26 or newer. No Intel or universal builds.
- The package payload stays script-free. Never add `Scripts` to the package.
- Signing identities and notarization credentials are referenced by name only.
  Never accept a private key, password, or API key as a command-line value.
- The release team identifier is exactly `Z548WR4TF8`.
- The package asset name is exactly `gascan-<version>-macos-arm64.pkg`.
- The package identifier is exactly `dev.gascan.pkg`.
- Never overwrite an existing release, and never pass a clobber flag.
- Lint every shell file you touch with
  `shellcheck --severity=warning <files>` before committing.

---

## File Structure

| File | Responsibility |
|---|---|
| `packaging/macos/release-common.sh` (modify) | Add `gascan_assert_distributable_package` — the four distribution-trust assertions. |
| `packaging/macos/publish.sh` (create) | Establish every precondition, then create the draft release, upload assets, and publish. |
| `packaging/macos/render-cask.sh` (create) | Render `Casks/gascan.rb` from a version and SHA-256. |
| `tests/release/distributable-package-contract.sh` (create) | Prove the helper rejects every non-distributable package. |
| `tests/release/publish-contract.sh` (create) | Prove `publish.sh` rejects untagged sources, version mismatches, and existing releases. |
| `tests/release/cask-contract.sh` (create) | Prove the rendered cask matches the uninstall contract. |
| `README.md` (modify) | Lead with install-from-release; move source builds to a secondary section. |
| `docs/release/macos-checklist.md` (modify) | Add the notarization setup, publish, and tap runbook. |

**Naming note:** the spec named a single `publish-contract.sh`. This plan splits
the helper's rejection cases into `distributable-package-contract.sh` so a
reviewer can accept the helper independently of `publish.sh`. Same coverage,
two review gates.

---

### Task 1: Distribution-trust helper

**Files:**
- Modify: `packaging/macos/release-common.sh` (append a new function)
- Test: `tests/release/distributable-package-contract.sh`

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `gascan_assert_distributable_package <package> <team>` — returns 0
  when the package is Developer ID Installer signed by `<team>`, accepted by
  Gatekeeper, carries a stapled notarization ticket, and every payload
  executable satisfies a Developer ID requirement for `<team>`. Returns 64 for
  a malformed team identifier, 66 for a missing package, and 65 for every
  trust failure.

- [ ] **Step 1: Write the failing test**

Create `tests/release/distributable-package-contract.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/../.." && pwd -P)
source "$repo_root/packaging/macos/release-common.sh"
fixture=$(mktemp -d "${TMPDIR:-/tmp}/gascan-distributable-contract.XXXXXX")
trap 'rm -rf "$fixture"' EXIT

team=Z548WR4TF8
other=AAAAAAAAAA

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
  if ( export "$@"
       gascan_assert_distributable_package "$fixture/payload.pkg" "$team" ) 2>/dev/null; then
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
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
chmod +x tests/release/distributable-package-contract.sh
bash tests/release/distributable-package-contract.sh
```

Expected: FAIL with `command not found: gascan_assert_distributable_package`,
or a nonzero exit before any `PASS:` line.

- [ ] **Step 3: Implement the helper**

Append to `packaging/macos/release-common.sh`:

```bash
# The exact Apple Developer team that signs Gas Can releases.
GASCAN_RELEASE_TEAM=Z548WR4TF8

gascan_assert_distributable_package() {
  local package=$1 team=$2 signature work relative
  [[ $team =~ ^[A-Z0-9]{10}$ ]] || {
    printf 'team identifier must be ten uppercase alphanumeric characters\n' >&2
    return 64
  }
  [[ -f $package ]] || {
    printf 'package does not exist: %s\n' "$package" >&2
    return 66
  }
  signature=$(pkgutil --check-signature "$package" 2>&1) || {
    printf 'package is not signed\n' >&2
    return 65
  }
  grep -Fq 'Developer ID Installer' <<<"$signature" || {
    printf 'package is not signed by a Developer ID Installer certificate\n' >&2
    return 65
  }
  grep -Fq "($team)" <<<"$signature" || {
    printf 'package signature does not belong to team %s\n' "$team" >&2
    return 65
  }
  spctl --assess --type install "$package" >/dev/null 2>&1 || {
    printf 'Gatekeeper rejects the package as an install candidate\n' >&2
    return 65
  }
  xcrun stapler validate "$package" >/dev/null 2>&1 || {
    printf 'package has no stapled notarization ticket\n' >&2
    return 65
  }
  work=$(mktemp -d "${TMPDIR:-/tmp}/gascan-distributable.XXXXXX") || return 70
  if ! pkgutil --expand "$package" "$work/pkg" >/dev/null 2>&1; then
    rm -rf "$work"
    printf 'package could not be expanded\n' >&2
    return 65
  fi
  mkdir "$work/root"
  if ! (cd "$work/root" && gzip -dc "$work/pkg/Payload" | cpio -idm --quiet); then
    rm -rf "$work"
    printf 'package payload could not be extracted\n' >&2
    return 65
  fi
  for relative in usr/local/bin/gascan usr/local/bin/gascand \
    usr/local/bin/gascan-apple-attach; do
    if ! codesign --verify --strict \
      -R "=anchor apple generic and certificate leaf[subject.OU] = $team" \
      "$work/root/$relative" >/dev/null 2>&1; then
      rm -rf "$work"
      printf 'executable is not Developer ID signed by team %s: %s\n' \
        "$team" "$relative" >&2
      return 65
    fi
  done
  rm -rf "$work"
}
```

- [ ] **Step 4: Run the test to verify it passes**

```bash
bash tests/release/distributable-package-contract.sh
```

Expected: `PASS: Gas Can distributable-package contract`

- [ ] **Step 5: Lint and commit**

```bash
shellcheck --severity=warning packaging/macos/release-common.sh \
  tests/release/distributable-package-contract.sh
git add packaging/macos/release-common.sh tests/release/distributable-package-contract.sh
git commit -m "feat: assert a package is distributable before release"
```

---

### Task 2: Publish gate

**Files:**
- Create: `packaging/macos/publish.sh`
- Test: `tests/release/publish-contract.sh`

**Interfaces:**
- Consumes: `gascan_assert_distributable_package <package> <team>` and
  `GASCAN_RELEASE_TEAM` from Task 1; the existing
  `gascan_verify_release_source`, `gascan_assert_release_inputs_clean`, and
  `packaging/macos/verify-package.sh`.
- Produces: `packaging/macos/publish.sh PACKAGE.pkg`, which prints the
  published asset URL and its SHA-256 on stdout — Task 3 consumes that SHA-256.

**Design refinement:** `publish.sh` is *stricter* than `package.sh`.
`gascan_verify_release_source` accepts a trusted signed commit that carries no
tag, which is correct for building but wrong for publishing, since the release
is named after the tag. `publish.sh` therefore requires the annotated, signed
`v<version>` tag to peel exactly to `HEAD`, in addition to the shared checks.

- [ ] **Step 1: Write the failing test**

Create `tests/release/publish-contract.sh`:

```bash
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
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
chmod +x tests/release/publish-contract.sh
bash tests/release/publish-contract.sh
```

Expected: FAIL with `publish.sh is not executable`.

- [ ] **Step 3: Implement publish.sh**

Create `packaging/macos/publish.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/../.." && pwd -P)
cd "$repo_root"
source "$repo_root/packaging/macos/release-common.sh"

[[ $# -eq 1 ]] || { printf 'usage: %s PACKAGE.pkg\n' "$0" >&2; exit 64; }
package=$1
[[ -f $package ]] || { printf 'package does not exist: %s\n' "$package" >&2; exit 66; }
for command in cargo gh jq pkgutil shasum; do
  command -v "$command" >/dev/null || {
    printf 'required publish command is unavailable: %s\n' "$command" >&2
    exit 69
  }
done

version=$(cargo metadata --locked --no-deps --format-version 1 |
  jq -er '.packages[] | select(.name == "gascan") | .version')
revision=$(git rev-parse --verify HEAD)
[[ $revision =~ ^[0-9a-f]{40}$ ]] || {
  printf 'source revision is not a full Git object ID\n' >&2
  exit 1
}
tag="v$version"

# Publishing requires the release tag itself, not merely a signed commit.
[[ $(git cat-file -t "refs/tags/$tag" 2>/dev/null) == tag ]] || {
  printf 'release tag %s is missing or not an annotated tag\n' "$tag" >&2
  exit 65
}
git verify-tag "refs/tags/$tag" >/dev/null 2>&1 || {
  printf 'release tag %s does not carry a trusted signature\n' "$tag" >&2
  exit 65
}
[[ $(git rev-parse --verify "refs/tags/$tag^{}") == "$revision" ]] || {
  printf 'release tag %s does not point at HEAD\n' "$tag" >&2
  exit 65
}
gascan_verify_release_source "$repo_root" "$revision" "$version" || {
  printf 'release source is not trusted\n' >&2
  exit 65
}
gascan_assert_release_inputs_clean "$repo_root" "$tag" || exit 65
"$repo_root/packaging/macos/verify-package.sh" "$package" "$revision" "$version"
gascan_assert_distributable_package "$package" "$GASCAN_RELEASE_TEAM" || exit 65

if gh release view "$tag" >/dev/null 2>&1; then
  printf 'release %s already exists; publish a new version instead\n' "$tag" >&2
  exit 65
fi

artifact_dir=$(cd "$(dirname "$package")" && pwd -P)
base=$(basename "$package")
[[ $base == "gascan-$version-macos-arm64.pkg" ]] || {
  printf 'unexpected package file name: %s\n' "$base" >&2
  exit 65
}

work=$(mktemp -d "${TMPDIR:-/tmp}/gascan-publish.XXXXXX")
trap 'rm -rf "$work"' EXIT
pkgutil --expand "$package" "$work/pkg"
mkdir "$work/root"
(cd "$work/root" && gzip -dc "$work/pkg/Payload" | cpio -idm --quiet)
cp "$work/root/usr/local/share/gascan/build-manifest.json" "$work/build-manifest.json"
(cd "$artifact_dir" && shasum -a 256 "$base" >"$work/$base.sha256")
checksum=$(awk '{print $1}' "$work/$base.sha256")

cat >"$work/notes.md" <<EOF_NOTES
Gas Can $version for Apple silicon, macOS 26 or newer.

Install with Homebrew:

    brew tap liquescent-development/tap
    brew install --cask gascan

Or download \`$base\` and open it.

Gas Can requires Apple \`container\` 1.1.0 and its running service, which Gas
Can neither installs nor redistributes. Verify the host with
\`gascan doctor --json\`.

Source revision: \`$revision\`
SHA-256: \`$checksum\`
EOF_NOTES

gh release create "$tag" --draft --title "Gas Can $version" --notes-file "$work/notes.md"
gh release upload "$tag" \
  "$package" "$work/$base.sha256" "$work/build-manifest.json"
assets=$(gh release view "$tag" --json assets --jq '[.assets[].name] | sort | join(",")')
[[ $assets == "$base,$base.sha256,build-manifest.json" ]] || {
  printf 'release assets are incomplete: %s\n' "$assets" >&2
  exit 65
}
gh release edit "$tag" --draft=false >/dev/null

printf 'https://github.com/Liquescent-Development/gascan/releases/download/%s/%s\n' "$tag" "$base"
printf '%s\n' "$checksum"
```

- [ ] **Step 4: Run the test to verify it passes**

```bash
chmod +x packaging/macos/publish.sh
bash tests/release/publish-contract.sh
```

Expected: `PASS: Gas Can publish contract`

- [ ] **Step 5: Lint and commit**

```bash
shellcheck --severity=warning packaging/macos/publish.sh tests/release/publish-contract.sh
git add packaging/macos/publish.sh tests/release/publish-contract.sh
git commit -m "feat: gate release publication on a distributable package"
```

---

### Task 3: Cask rendering

**Files:**
- Create: `packaging/macos/render-cask.sh`
- Test: `tests/release/cask-contract.sh`

**Interfaces:**
- Consumes: the version and SHA-256 that `publish.sh` prints (Task 2).
- Produces: `packaging/macos/render-cask.sh VERSION SHA256`, which writes a
  complete cask to stdout.

- [ ] **Step 1: Write the failing test**

Create `tests/release/cask-contract.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/../.." && pwd -P)
render=$repo_root/packaging/macos/render-cask.sh
[[ -x $render ]] || { printf 'render-cask.sh is not executable\n' >&2; exit 1; }
fixture=$(mktemp -d "${TMPDIR:-/tmp}/gascan-cask-contract.XXXXXX")
trap 'rm -rf "$fixture"' EXIT

version=1.2.3
checksum=$(printf 'x' | shasum -a 256 | awk '{print $1}')

if "$render" 1.2 "$checksum" 2>/dev/null; then
  printf 'malformed version accepted\n' >&2
  exit 1
fi
if "$render" "$version" not-a-checksum 2>/dev/null; then
  printf 'malformed checksum accepted\n' >&2
  exit 1
fi

"$render" "$version" "$checksum" >"$fixture/gascan.rb"
grep -Fq "version \"$version\"" "$fixture/gascan.rb"
grep -Fq "sha256 \"$checksum\"" "$fixture/gascan.rb"
grep -Fq 'depends_on arch: :arm64' "$fixture/gascan.rb"
grep -Fq 'depends_on macos: ">= :tahoe"' "$fixture/gascan.rb"
grep -Fq 'pkgutil: "dev.gascan.pkg"' "$fixture/gascan.rb"
grep -Fq 'container 1.1.0' "$fixture/gascan.rb"

# Rendering must be deterministic.
"$render" "$version" "$checksum" >"$fixture/again.rb"
cmp -s "$fixture/gascan.rb" "$fixture/again.rb" || {
  printf 'cask rendering is not deterministic\n' >&2
  exit 1
}

# The cask's delete list must equal the set uninstall.sh removes, parsed from
# that script so the two can never drift.
awk '/^sudo rm -f/,/^sudo rmdir/' "$repo_root/packaging/macos/uninstall.sh" |
  grep -o '/usr/local/[^ \\]*' | LC_ALL=C sort -u >"$fixture/expected-paths"
grep -o '"/usr/local/[^"]*"' "$fixture/gascan.rb" | tr -d '"' |
  LC_ALL=C sort -u >"$fixture/cask-paths"
cmp -s "$fixture/expected-paths" "$fixture/cask-paths" || {
  printf 'cask uninstall paths differ from uninstall.sh\n' >&2
  diff -u "$fixture/expected-paths" "$fixture/cask-paths" >&2 || true
  exit 1
}

printf 'PASS: Gas Can cask contract\n'
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
chmod +x tests/release/cask-contract.sh
bash tests/release/cask-contract.sh
```

Expected: FAIL with `render-cask.sh is not executable`.

- [ ] **Step 3: Implement render-cask.sh**

Create `packaging/macos/render-cask.sh`. The `#{version}` sequences are
Ruby interpolation and must reach the output literally; the heredoc is
unquoted so only `$version` and `$checksum` expand.

```bash
#!/usr/bin/env bash
set -euo pipefail

[[ $# -eq 2 ]] || { printf 'usage: %s VERSION SHA256\n' "$0" >&2; exit 64; }
version=$1 checksum=$2
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || {
  printf 'version must be MAJOR.MINOR.PATCH\n' >&2
  exit 64
}
[[ $checksum =~ ^[0-9a-f]{64}$ ]] || {
  printf 'checksum must be a lowercase SHA-256 hex digest\n' >&2
  exit 64
}

cat <<EOF_CASK
cask "gascan" do
  version "$version"
  sha256 "$checksum"

  url "https://github.com/Liquescent-Development/gascan/releases/download/v#{version}/gascan-#{version}-macos-arm64.pkg"
  name "Gas Can"
  desc "Secure local sandbox for agentic coding"
  homepage "https://github.com/Liquescent-Development/gascan"

  depends_on macos: ">= :tahoe"
  depends_on arch: :arm64

  pkg "gascan-#{version}-macos-arm64.pkg"

  uninstall pkgutil: "dev.gascan.pkg",
            delete:  [
              "/usr/local/bin/gascan",
              "/usr/local/bin/gascan-apple-attach",
              "/usr/local/bin/gascand",
              "/usr/local/share/gascan/LICENSE",
              "/usr/local/share/gascan/build-manifest.json",
              "/usr/local/share/gascan/default-gascan.toml",
            ]

  caveats <<~EOS
    Gas Can requires Apple container 1.1.0 and its running service. Gas Can does
    not install or redistribute it.

    Verify the host with:
      gascan doctor --json
  EOS
end
EOF_CASK
```

- [ ] **Step 4: Run the test to verify it passes**

```bash
chmod +x packaging/macos/render-cask.sh
bash tests/release/cask-contract.sh
```

Expected: `PASS: Gas Can cask contract`

If the path comparison fails, reconcile by editing the cask's `delete:` list to
match `uninstall.sh` exactly. Do not edit the test to match the cask.

- [ ] **Step 5: Lint and commit**

```bash
shellcheck --severity=warning packaging/macos/render-cask.sh tests/release/cask-contract.sh
git add packaging/macos/render-cask.sh tests/release/cask-contract.sh
git commit -m "feat: render the Homebrew cask from a published release"
```

---

### Task 4: User and maintainer documentation

**Files:**
- Modify: `README.md:17-61` (the `## Install` section)
- Modify: `docs/release/macos-checklist.md:29-38` and `:76-85`

**Interfaces:**
- Consumes: `publish.sh` and `render-cask.sh` from Tasks 2 and 3.
- Produces: nothing consumed by later tasks.

- [ ] **Step 1: Replace the README install section**

Replace everything from `## Install` through the end of the
`### Trusting the release signature` section with:

```markdown
## Install

Gas Can is distributed as a signed, notarized macOS package. Install it with
Homebrew:

```sh
brew tap liquescent-development/tap
brew install --cask gascan
```

Or download `gascan-<version>-macos-arm64.pkg` from the
[latest release](https://github.com/Liquescent-Development/gascan/releases/latest)
and open it. Each release also publishes a `.sha256` checksum and the
`build-manifest.json`, which records the source revision and a SHA-256 for
every installed executable.

Then confirm the host and runtime satisfy the security contract. `doctor`
reports one fact per capability — architecture, macOS version, runtime service,
storage, bind mounts, named volumes, TTY, signals, loopback publishing,
resource limits, and offline isolation:

```sh
gascan doctor --json | jq
```

### Building from source

Building is for contributors; installing a release does not require it.
Packaging refuses to build from an untrusted source revision: the checkout must
be either a trusted signed commit or the exact signed release tag. Build from
the tag rather than from `main`, which moves ahead between releases:

```sh
git checkout v0.1.1
package=$(./packaging/macos/package.sh)
GASCAN_EXPECTED_SOURCE_REVISION=$(git rev-parse HEAD) \
GASCAN_EXPECTED_VERSION=0.1.1 \
  ./packaging/macos/install.sh "$package"
```

Skipping the checkout leaves `HEAD` on a commit the release tag does not
attest, and `package.sh` exits 65 with `release source HEAD needs a trusted
commit signature or exact signed v0.1.1 tag`.

Verification runs through Git's own trust policy, so the tag's signing key must
be one you have chosen to trust. Releases are signed with this SSH key:

```
richard@liquescent.dev ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHyTKmfAwcJcdfKXmj2h3mwfgPaelE6gSMrquAcPmW09
```

Its fingerprint is `SHA256:3NWoJ1nmsLHxd8hAG/BnyriJJpIFXHaW3RtuPYANKc4`. Add it
to a Git allowed-signers file and point Git at it:

```sh
mkdir -p ~/.config/git
printf 'richard@liquescent.dev ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHyTKmfAwcJcdfKXmj2h3mwfgPaelE6gSMrquAcPmW09\n' \
  >> ~/.config/git/allowed_signers
git config --global gpg.ssh.allowedSignersFile ~/.config/git/allowed_signers
git verify-tag v0.1.1
```
```

- [ ] **Step 2: Add the release runbook to the checklist**

Append to `docs/release/macos-checklist.md` a `## Publish` section:

```markdown
## Publish

Notarization requires a stored credential profile once per machine:

```sh
xcrun notarytool store-credentials gascan-notary \
  --key <AuthKey_XXXXXXXXXX.p8> --key-id <KEY_ID> --issuer <ISSUER_UUID>
```

From the signed release tag, build and publish:

```sh
git checkout v<version>
GASCAN_CODESIGN_IDENTITY="Developer ID Application: Liquescent Development LLC (Z548WR4TF8)" \
GASCAN_INSTALLER_SIGNING_IDENTITY="Developer ID Installer: Liquescent Development LLC (Z548WR4TF8)" \
GASCAN_NOTARYTOOL_PROFILE=gascan-notary \
  package=$(./packaging/macos/package.sh)
./packaging/macos/publish.sh "$package"
```

`publish.sh` refuses any package that is not Developer ID signed, notarized,
stapled, and bound to the exact signed tag. It creates the release as a draft,
uploads the package, its checksum, and `build-manifest.json`, and clears the
draft flag only after all three assets are present. It prints the asset URL and
the SHA-256.

Render the cask with that checksum and commit it to the tap:

```sh
./packaging/macos/render-cask.sh <version> <sha256> >Casks/gascan.rb
```

An existing release is never overwritten. A botched release is corrected by
publishing a new version.
```

- [ ] **Step 3: Verify the docs assertions still hold**

The source-signature contract greps the README and checklist. Run it:

```bash
bash tests/release/source-signature-contract.sh
```

Expected: `PASS: Gas Can release source-signature contract`

If it fails, the grep targets moved. Keep the phrases
`trusted signed commit or the exact signed release tag` in `README.md` and
`v<version>` in the checklist.

- [ ] **Step 4: Commit**

```bash
git add README.md docs/release/macos-checklist.md
git commit -m "docs: install from a signed release instead of a source build"
```

---

### Task 5: Tap repository and first publish

**Files:**
- Create (separate repository `Liquescent-Development/homebrew-tap`):
  `Casks/gascan.rb`, `README.md`

**Interfaces:**
- Consumes: `render-cask.sh` output from Task 3 and the checksum printed by
  `publish.sh` from Task 2.
- Produces: the installable tap.

These steps publish to the internet and sign with the maintainer's identities.
They are maintainer actions and must not be automated by an agent.

- [ ] **Step 1: Run the full local suite first**

```bash
cargo test --locked --workspace
for contract in tests/release/*.sh; do bash "$contract"; done
```

Expected: workspace tests pass, and every contract prints its own `PASS:` line.

- [ ] **Step 2: Create the tap repository**

```bash
gh repo create Liquescent-Development/homebrew-tap --public \
  --description "Homebrew tap for Liquescent Development software"
```

- [ ] **Step 3: Build, sign, notarize, and publish a release**

Follow the `## Publish` runbook added in Task 4. Record the SHA-256 it prints.

- [ ] **Step 4: Render and commit the cask**

```bash
./packaging/macos/render-cask.sh <version> <sha256> >/path/to/homebrew-tap/Casks/gascan.rb
```

Commit and push it in the tap repository.

- [ ] **Step 5: Prove the published path works from a clean host**

```bash
brew tap liquescent-development/tap
brew install --cask gascan
gascan doctor --json | jq
```

Expected: Homebrew verifies the checksum, the package installs without a
Gatekeeper prompt, and `doctor` reports its facts.

- [ ] **Step 6: Prove uninstall is clean**

```bash
brew uninstall --cask gascan
test ! -e /usr/local/bin/gascan
pkgutil --pkg-info dev.gascan.pkg && exit 1 || true
```

Expected: no installed paths and no package receipt remain.

---

## Self-Review

**Spec coverage:** `publish.sh` gate (Task 2), `render-cask.sh` (Task 3),
`gascan_assert_distributable_package` (Task 1), publish and cask contracts
(Tasks 1–3), README and checklist (Task 4), tap (Task 5), notarization setup
(Task 4 checklist). The spec's atomic-publish, never-clobber, and script-free
properties are enforced in Task 2 and asserted by its contract.

**Placeholders:** none. Angle-bracketed values in Task 5 are maintainer inputs
that cannot be known in advance — the version, checksum, and Apple credentials.

**Type consistency:** `gascan_assert_distributable_package` takes
`<package> <team>` in Tasks 1 and 2. `GASCAN_RELEASE_TEAM` is defined in Task 1
and consumed in Task 2. `render-cask.sh VERSION SHA256` matches Tasks 3 and 5.
