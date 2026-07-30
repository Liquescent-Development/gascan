# SSH Workspace and Full Ubuntu Image Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship Gas Can 0.1.15 with a full Ubuntu workspace image and interactive SSH sessions that start in `/workspace`.

**Architecture:** Restore the pinned Ubuntu base with its supported `unminimize` command during image assembly. Add a narrowly guarded directory change to the existing managed interactive Bash hook so SSH automation and home-backed state retain their current semantics.

**Tech Stack:** Dockerfile, Bash, Apple container, Rust release tooling, GitHub Actions/Homebrew.

## Global Constraints

- Keep `/home/workspace` as the account home and managed state root.
- Redirect only interactive SSH shells that begin in `$HOME`.
- Preserve noninteractive SSH commands, SFTP, port forwarding, and editor bootstrap.
- Publish patch version `0.1.15`.

---

### Task 1: Regression contracts

**Files:**
- Modify: `tests/image/shell-home-root-contract.sh`
- Modify: `images/workspace/tests/ssh-contract.sh`
- Modify: `tests/image/workstation-smoke.sh`
- Modify: `tests/release/source-input-contract.sh`

**Interfaces:**
- Consumes: `/etc/gascan/bashrc`, the built workspace image, and the live SSH test harness.
- Produces: executable assertions for image completeness and SSH working-directory behavior.

- [ ] **Step 1: Write failing shell-hook assertions**

Add interactive Bash cases proving `SSH_CONNECTION` plus `PWD=$HOME` changes
to `/workspace`, while a local interactive shell and an SSH shell starting
outside `$HOME` retain their current directory.

- [ ] **Step 2: Write failing connected-image assertions**

Require the built image to have removed `/etc/update-motd.d/60-unminimize`,
provide restored manual-page content, and return `/workspace` from a
pseudo-terminal SSH login. Keep the existing remote `/usr/bin/env` command
and SFTP assertions as regression coverage.

- [ ] **Step 3: Run focused contracts and confirm failure**

Run:

```bash
bash tests/image/shell-home-root-contract.sh
bash tests/release/source-input-contract.sh
```

Expected: at least the new assertions fail because neither behavior exists.

### Task 2: Minimal implementation

**Files:**
- Modify: `images/workspace/Dockerfile`
- Modify: `images/workspace/etc/gascan/bashrc`

**Interfaces:**
- Consumes: Ubuntu's `/usr/local/sbin/unminimize`, `SSH_CONNECTION`, `HOME`, and `PWD`.
- Produces: a full Ubuntu filesystem and guarded interactive SSH workspace entry.

- [ ] **Step 1: Restore the Ubuntu image**

Run `unminimize` noninteractively in `workspace-base` before installing the
locked workstation package set.

- [ ] **Step 2: Add the SSH directory guard**

At the start of the already-interactive Bash hook, change to `/workspace`
only when `SSH_CONNECTION` is nonempty, `PWD` equals `HOME`, and `/workspace`
exists.

- [ ] **Step 3: Run focused contracts**

Run the shell and source-input contracts and require both to pass.

### Task 3: Connected image verification

**Files:**
- Modify: `images/workspace/approved-source.sha256`
- Modify: `images/workspace/approved-image.txt`
- Modify: connected-build receipt under `.artifacts/` (ignored)

**Interfaces:**
- Consumes: the reviewed workspace-image source tree.
- Produces: an approved digest-qualified GHCR image reference.

- [ ] **Step 1: Build the connected image**

Run the repository connected-image build workflow for Apple arm64.

- [ ] **Step 2: Publish and validate the immutable image**

Publish the OCI archive to GHCR with all manifests and preserved digests,
then validate the canonical repository digest.

- [ ] **Step 3: Run image and repository gates**

Run the shell, SSH, workstation, release-contract, and Rust workspace tests
against the approved image.

### Task 4: PR and release

**Files:**
- Modify: `Cargo.lock`
- Modify: `crates/*/Cargo.toml`
- Modify: `README.md`
- Modify: `docs/release/macos-checklist.md`

**Interfaces:**
- Consumes: verified feature commit and approved image digest.
- Produces: merged PR, signed `v0.1.15` tag, notarized package, GitHub release, and Homebrew cask update.

- [ ] **Step 1: Commit and merge the feature**

Push `fix/ssh-workspace-unminimize`, open one PR, wait for required checks,
and squash merge it.

- [ ] **Step 2: Prepare version 0.1.15**

Update every workspace package and release-document version reference from
`0.1.14` to `0.1.15`; regenerate the lockfile and run release gates.

- [ ] **Step 3: Sign and publish**

Run `packaging/macos/release.sh` with the Developer ID identities, notary
profile `AC_PASSWORD`, and `/Users/kiener/code/homebrew-tap`; push the signed
tag and publish the GitHub release and cask.

- [ ] **Step 4: Verify public installation metadata**

Confirm the GitHub release assets, tag signature, package checksum, and
Homebrew cask version/checksum all agree on `0.1.15`.
