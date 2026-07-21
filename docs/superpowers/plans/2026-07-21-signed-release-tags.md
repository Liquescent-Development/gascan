# Signed Release Tag Packaging Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the documented macOS package command work from a GitHub-merged release commit without weakening exact-source signature verification.

**Architecture:** Add one release-common trust helper that accepts either the exact source commit's trusted signature or the trusted annotated `v<workspace-version>` tag pointing exactly at that commit. Exercise the helper with a real isolated SSH-signing fixture, update the package entry point and README, then merge before creating and verifying the real `v0.1.0` tag on the resulting `main` commit.

**Tech Stack:** Bash 3.2, Git SSH commit/tag signatures, Cargo metadata, macOS `pkgbuild`, existing shell release contracts.

## Global Constraints

- Only `HEAD` or the exact annotated tag `v<workspace-version>` may attest the packaged source revision.
- A signed ancestor, matching tree, lightweight tag, arbitrary tag, or version tag pointing elsewhere must not authorize packaging.
- The package manifest and installer remain bound to the full `HEAD` object ID and exact semantic version.
- Existing dirty, untracked, and ignored release-input checks remain unchanged.
- Preserve `.tokensave/` and every unrelated user file.
- All repository shell commands are prefixed with `rtk`.

---

### Task 1: Exact signed source attestation

**Files:**
- Create: `tests/release/source-signature-contract.sh`
- Modify: `packaging/macos/release-common.sh`
- Modify: `packaging/macos/package.sh`

**Interfaces:**
- Consumes: repository path, full source commit ID, and Cargo workspace version.
- Produces: `gascan_verify_release_source REPO REVISION VERSION`, returning zero only for a trusted commit signature or the exact trusted annotated version tag.

- [ ] **Step 1: Write the failing source-signature contract**

Create `tests/release/source-signature-contract.sh` with an isolated Git repository, an ephemeral ED25519 key, and Git's SSH allowed-signers policy. The contract must make a trusted signed commit pass, make an unsigned child fail, reject a lightweight `v0.1.0` tag, accept a trusted annotated `v0.1.0` tag on that exact child, reject a signed `v9.9.9` tag as a substitute, and reject a trusted `v0.1.0` tag that points to the parent:

```bash
#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/../.." && pwd -P)
source "$repo_root/packaging/macos/release-common.sh"
fixture=$(mktemp -d "${TMPDIR:-/tmp}/gascan-source-signature-contract.XXXXXX")
trap 'rm -rf "$fixture"' EXIT

ssh-keygen -q -t ed25519 -N '' -C release@example.invalid -f "$fixture/signing-key"
printf 'release@example.invalid %s\n' "$(cat "$fixture/signing-key.pub")" >"$fixture/allowed-signers"
git -C "$fixture" init -q
git -C "$fixture" config user.name release
git -C "$fixture" config user.email release@example.invalid
git -C "$fixture" config gpg.format ssh
git -C "$fixture" config user.signingKey "$fixture/signing-key"
git -C "$fixture" config gpg.ssh.allowedSignersFile "$fixture/allowed-signers"
printf 'signed\n' >"$fixture/source"
git -C "$fixture" add source
git -C "$fixture" commit -Sqm signed
signed=$(git -C "$fixture" rev-parse HEAD)
gascan_verify_release_source "$fixture" "$signed" 0.1.0

printf 'unsigned\n' >>"$fixture/source"
git -C "$fixture" add source
git -C "$fixture" -c commit.gpgsign=false commit -qm unsigned
unsigned=$(git -C "$fixture" rev-parse HEAD)
if gascan_verify_release_source "$fixture" "$unsigned" 0.1.0; then
  printf 'unsigned source accepted\n' >&2
  exit 1
fi

git -C "$fixture" tag v0.1.0
if gascan_verify_release_source "$fixture" "$unsigned" 0.1.0; then
  printf 'lightweight release tag accepted\n' >&2
  exit 1
fi
git -C "$fixture" tag -d v0.1.0 >/dev/null

git -C "$fixture" tag -s v9.9.9 -m wrong-name "$unsigned"
if gascan_verify_release_source "$fixture" "$unsigned" 0.1.0; then
  printf 'arbitrary signed tag accepted\n' >&2
  exit 1
fi

git -C "$fixture" tag -s v0.1.0 -m wrong-target "$signed"
if gascan_verify_release_source "$fixture" "$unsigned" 0.1.0; then
  printf 'signed release tag for another commit accepted\n' >&2
  exit 1
fi
git -C "$fixture" tag -d v0.1.0 >/dev/null

git -C "$fixture" tag -s v0.1.0 -m release "$unsigned"
gascan_verify_release_source "$fixture" "$unsigned" 0.1.0

printf 'PASS: Gas Can release source-signature contract\n'
```

- [ ] **Step 2: Run the contract and verify RED**

Run:

```bash
rtk ./tests/release/source-signature-contract.sh
```

Expected: FAIL before fixture setup because `gascan_verify_release_source` is undefined.

- [ ] **Step 3: Implement the minimal trust helper**

Add this function to `packaging/macos/release-common.sh`:

```bash
gascan_verify_release_source() {
  local repo=$1 revision=$2 version=$3 tag object_type target
  git -C "$repo" verify-commit "$revision" >/dev/null 2>&1 && return 0
  [[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || return 1
  tag="v$version"
  object_type=$(git -C "$repo" cat-file -t "refs/tags/$tag" 2>/dev/null) || return 1
  [[ $object_type == tag ]] || return 1
  git -C "$repo" verify-tag "refs/tags/$tag" >/dev/null 2>&1 || return 1
  target=$(git -C "$repo" rev-parse --verify "refs/tags/$tag^{}") || return 1
  [[ $target == "$revision" ]]
}
```

Replace the direct `git verify-commit` block in `packaging/macos/package.sh` with:

```bash
gascan_verify_release_source "$repo_root" "$revision" "$version" || {
  printf 'release source HEAD needs a trusted commit signature or exact signed v%s tag\n' "$version" >&2
  exit 65
}
```

- [ ] **Step 4: Run focused GREEN verification**

Run:

```bash
rtk ./tests/release/source-signature-contract.sh
rtk ./tests/release/source-input-contract.sh
rtk bash -n packaging/macos/package.sh packaging/macos/release-common.sh tests/release/source-signature-contract.sh
rtk shellcheck --severity=warning packaging/macos/package.sh packaging/macos/release-common.sh tests/release/source-signature-contract.sh
```

Expected: both contracts print `PASS`; syntax and ShellCheck exit zero without diagnostics.

- [ ] **Step 5: Commit the accepted trust boundary**

```bash
rtk git add packaging/macos/package.sh packaging/macos/release-common.sh tests/release/source-signature-contract.sh
rtk git diff --cached --check
rtk git commit -S -m "fix: trust exact signed release tags"
```

Expected: one signed commit containing only the helper, package call site, and source-signature contract.

---

### Task 2: Document and globally verify the release path

**Files:**
- Modify: `README.md`
- Modify: `docs/release/macos-checklist.md`

**Interfaces:**
- Consumes: `gascan_verify_release_source` from Task 1.
- Produces: user-facing source-trust prerequisites and complete pre-merge verification evidence.

- [ ] **Step 1: Add a documentation contract that fails on the old README**

Extend `tests/release/source-signature-contract.sh` before its PASS line:

```bash
grep -Fq 'trusted signed commit or the exact signed release tag' "$repo_root/README.md"
grep -Fq 'v<version>' "$repo_root/docs/release/macos-checklist.md"
```

- [ ] **Step 2: Run the documentation assertion and verify RED**

Run:

```bash
rtk ./tests/release/source-signature-contract.sh
```

Expected: FAIL at the README assertion because the current README does not explain source attestation.

- [ ] **Step 3: Document the exact prerequisite**

Add this sentence immediately before the README package command:

```markdown
The checkout must be a trusted signed commit or the exact signed release tag
(`v0.1.0` for this version); packaging rejects unsigned source revisions.
```

In `docs/release/macos-checklist.md`, state that packaging accepts a trusted signed `HEAD` or a trusted annotated `v<version>` tag pointing exactly to `HEAD`, and explicitly rejects lightweight tags, arbitrary tags, signed ancestors, and matching trees.

- [ ] **Step 4: Run focused and global verification**

Run:

```bash
rtk ./tests/release/source-signature-contract.sh
rtk ./tests/release/source-input-contract.sh
rtk ./tests/release/installer-contract.sh
rtk cargo fmt --all -- --check
rtk cargo clippy --workspace --all-targets --all-features -- -D warnings
rtk cargo test --workspace
rtk git diff --check
```

Expected: all three release contracts print `PASS`; formatting, strict Clippy, the full workspace suite, and diff checks exit zero.

- [ ] **Step 5: Commit the documentation**

```bash
rtk git add README.md docs/release/macos-checklist.md tests/release/source-signature-contract.sh
rtk git diff --cached --check
rtk git commit -S -m "docs: explain release source signatures"
```

Expected: one signed commit containing the documentation and its executable assertion.

---

### Task 3: Review, merge, publish, and prove `v0.1.0`

**Files:**
- No tracked file changes.
- Creates remote annotated tag: `v0.1.0`.
- Creates ignored local artifact: `.artifacts/release/gascan-0.1.0-macos-arm64.pkg`.

**Interfaces:**
- Consumes: the reviewed Task 1 and Task 2 commits merged into `main`.
- Produces: a trusted release tag on the exact merged commit and live proof that the README package flow succeeds.

- [ ] **Step 1: Obtain independent code review**

Have a fresh reviewer compare the fix branch with `origin/main`, run the source-signature contract, inspect the exact-tag trust boundary, and report Critical/Important/Minor findings. Resolve every Critical or Important finding before continuing.

- [ ] **Step 2: Open and merge the correction PR**

Push the fix branch, create a PR titled `fix: allow signed release tags for packaging`, confirm GitHub reports it mergeable with all configured checks passing, and merge it with a merge commit so signed task commits remain in history.

- [ ] **Step 3: Create the trusted release tag on merged `main`**

After fetching the merged remote, run:

```bash
rtk git fetch origin main
merge_revision=$(rtk git rev-parse origin/main)
rtk git tag -s v0.1.0 "$merge_revision" -m 'Gas Can macOS MVP v0.1.0'
rtk git verify-tag v0.1.0
rtk git push origin v0.1.0
```

Expected: local verification reports the configured trusted ED25519 signer and the remote accepts `v0.1.0`.

- [ ] **Step 4: Verify the exact README package flow from tagged `main`**

Update the local `main` checkout to the fetched merge commit, then run the README commands exactly:

```bash
package=$(rtk ./packaging/macos/package.sh)
GASCAN_EXPECTED_SOURCE_REVISION=$(rtk git rev-parse HEAD) \
GASCAN_EXPECTED_VERSION=0.1.0 \
  rtk ./packaging/macos/install.sh "$package"
rtk gascan doctor --json | rtk jq
```

Expected: packaging prints the `.pkg` path, installation succeeds, and doctor returns valid JSON for the exact supported Apple Container runtime.

- [ ] **Step 5: Verify final repository and remote state**

Run:

```bash
rtk git verify-tag v0.1.0
test "$(rtk git rev-list -n 1 v0.1.0)" = "$(rtk git rev-parse origin/main)"
rtk git status --short --branch
```

Expected: the tag verifies and peels to `origin/main`; status contains only the pre-existing `.tokensave/` directory and ignored build artifacts.
