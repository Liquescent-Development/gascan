# SSH Permission Compatibility and Onboarding Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Accept conventional OpenSSH-safe host path permissions while preserving existing safe file modes, and make the README quickstart sufficient to launch and update Claude Code and use Herdr.

**Architecture:** Replace exact existing-path mode checks with an integrity rule that rejects group/other write access while retaining ownership, type, link, and race checks. Carry the observed safe file mode in the existing file identity so atomic replacement uses and verifies that mode. Keep creation modes unchanged, and document the already-supported mise override path rather than adding CLI or provisioning behavior.

**Tech Stack:** Rust 2024, `rustix`, Cargo integration tests, Markdown, mise npm backend.

## Global Constraints

- New SSH directories remain `0700`; new SSH files remain `0600`.
- Existing owner-controlled directories are safe only when group and other write bits are clear.
- Existing owner-controlled regular single-link files are safe only when group and other write bits are clear.
- Installing or removing the managed include preserves an existing safe `~/.ssh/config` mode.
- Symlink, hard-link, foreign-owner, special-file, size, race, and recovery checks remain fail-closed.
- Do not add a Gas Can tool-upgrade command or change provisioning/floating-version behavior.
- Do not promise that unchanged `latest` declarations are remotely re-resolved on every apply.

---

### Task 1: Accept and Preserve Conventional SSH Permissions

**Files:**
- Modify: `crates/gascan/tests/ssh_config.rs`
- Modify: `crates/gascan/src/ssh_config.rs`

**Interfaces:**
- Consumes: `SshConfig::{install,remove,record_offer_receipt}` and the existing descriptor-relative validation and atomic-replacement pipeline.
- Produces: `FileIdentity::mode: u16`; mode-safe `validate_directory_stat` and `validate_file_stat`; atomic replacement that chooses `previous.identity.mode` or `FILE_MODE`.

- [ ] **Step 1: Write failing integration tests for conventional modes and mode preservation**

Replace the old assertions that treat `0755` directories and `0644` files as attacks with explicit safe and unsafe cases:

```rust
#[test]
fn conventional_owner_controlled_modes_are_accepted_and_preserved() -> TestResult {
    let (_temp, config) = fixture()?;
    fs::create_dir(config.ssh_directory_path())?;
    fs::set_permissions(
        config.ssh_directory_path(),
        fs::Permissions::from_mode(0o755),
    )?;
    fs::write(config.user_config_path(), b"Host personal\n")?;
    fs::set_permissions(
        config.user_config_path(),
        fs::Permissions::from_mode(0o644),
    )?;

    assert_eq!(config.install()?, IncludeChange::Changed);
    assert_eq!(
        fs::metadata(config.user_config_path())?.mode() & 0o777,
        0o644
    );
    assert_eq!(config.remove()?, IncludeChange::Changed);
    assert_eq!(
        fs::metadata(config.user_config_path())?.mode() & 0o777,
        0o644
    );
    Ok(())
}

#[test]
fn conventional_managed_directory_modes_are_accepted() -> TestResult {
    let (_temp, config) = fixture()?;
    let gascan = config
        .managed_config_path()
        .parent()
        .and_then(Path::parent)
        .ok_or("managed gascan directory")?;
    fs::create_dir_all(gascan)?;
    fs::set_permissions(gascan, fs::Permissions::from_mode(0o755))?;

    config.record_offer_receipt()?;
    assert!(config.offer_receipt_exists()?);
    Ok(())
}
```

Change the file attack to `0664`, the directory attack to `0775`, and the
stable-code unsafe-directory fixture to `0775`. Assert that unsafe modes remain
unchanged after rejection.

- [ ] **Step 2: Run the focused integration tests and verify the new behavior fails**

Run:

```sh
cargo test -p gascan --test ssh_config
```

Expected: the `0755`/`0644` acceptance tests fail with
`ssh_config_unsafe`; existing attack tests still pass after using writable
modes.

- [ ] **Step 3: Carry the observed safe mode in file identity**

Extend `FileIdentity` and its constructor:

```rust
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileIdentity {
    device: u64,
    inode: u64,
    mode: u16,
}

impl FileIdentity {
    fn from_stat(stat: &rustix::fs::Stat) -> Self {
        Self {
            device: stat.st_dev as u64,
            inode: stat.st_ino,
            mode: (stat.st_mode & 0o7777) as u16,
        }
    }
}
```

This makes concurrent permission changes part of the existing identity
comparisons and makes the previous safe mode available to replacement.

- [ ] **Step 4: Replace exact existing-path checks with no-untrusted-write checks**

Remove the `exact_private_mode` argument from `open_directory`,
`open_child_directory`, and `validate_directory_stat`. Validate directories
with:

```rust
if FileType::from_raw_mode(stat.st_mode) != FileType::Directory
    || stat.st_uid != expected_uid
    || stat.st_mode & 0o022 != 0
{
    return Err(SshConfigError::unsafe_path(
        "SSH directory ownership or permissions are unsafe",
    ));
}
```

Remove `required_mode` from `read_file`, `file_identity`,
`validate_file_stat`, and `entry_has_contents`. Validate files with:

```rust
if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
    || stat.st_uid != expected_uid
    || stat.st_nlink != 1
    || stat.st_mode & 0o022 != 0
{
    return Err(SshConfigError::unsafe_path(
        "SSH configuration ownership, type, links, or permissions are unsafe",
    ));
}
```

Update every caller and the unit test for foreign ownership to the reduced
signature.

- [ ] **Step 5: Preserve the safe existing file mode through atomic replacement**

In `atomic_replace_with_hooks`, select and apply the publication mode:

```rust
let replacement_mode = previous
    .map(|previous| previous.identity.mode)
    .unwrap_or(FILE_MODE);
```

Create the staging file with `replacement_mode`, `fchmod` it to that mode,
validate its safe metadata, and explicitly confirm
`FileIdentity::from_stat(&stat).mode == replacement_mode`. Use mode-agnostic
safe validation for the prior file, target, staging, and recovery checks.

- [ ] **Step 6: Run the focused tests and verify green**

Run:

```sh
cargo test -p gascan --test ssh_config
cargo test -p gascan ssh_config
```

Expected: all SSH config integration and unit tests pass, including attack,
concurrency, and recovery coverage.

- [ ] **Step 7: Format, lint the changed crate, and commit**

Run:

```sh
cargo fmt --all --check
cargo clippy -p gascan --all-targets -- -D warnings
git diff --check
```

Commit:

```sh
git add crates/gascan/src/ssh_config.rs crates/gascan/tests/ssh_config.rs
git commit -m "fix: accept conventional SSH config permissions"
```

---

### Task 2: Make the README a Complete First-Use Path

**Files:**
- Modify: `README.md`

**Interfaces:**
- Consumes: the existing CLI commands, manifest version 1 schema, workstation inventory, and mise override precedence.
- Produces: a concise install-to-Claude/Herdr Quickstart, a complete copyable manifest, and accurate Claude Code override guidance.

- [ ] **Step 1: Rewrite Quickstart around the first five minutes**

Replace the current command inventory-style Quickstart with these explicit
stages:

1. Copy `packaging/macos/default-gascan.toml` or create the shown networked
   manifest.
2. Run `gascan doctor`, then `cd` to the project and run `gascan up .`.
3. Run `gascan shell`.
4. Inside the shell, run `claude --version`, `claude`, `herdr --version`, and
   `herdr`.
5. State that authentication and agent configuration remain sandbox-local and
   persist through `down`, `up`, and container replacement.
6. Show `gascan apply` after manifest edits.
7. End with the essential `status`, `shell`, `down`, and destructive
   `destroy --yes` lifecycle commands.

Use this tools example in the quickstart manifest:

```toml
[tools]
node = "lts"
"npm:@anthropic-ai/claude-code" = "latest"
```

Keep the Quickstart concise by linking to the later SSH, schema, and lifecycle
details rather than duplicating them.

- [ ] **Step 2: Update the full schema example**

Retain every supported top-level section and replace the tools portion with:

```toml
[tools]                         # mise tool name = version
node = "lts"
python = "3.13"
"npm:@anthropic-ai/claude-code" = "latest"
```

Keep `[resources]`, `[storage]`, `[ports]`, and `[ssh]` in the same copyable
example so it remains a complete manifest.

- [ ] **Step 3: Add precise workstation override and Claude upgrade guidance**

In the `[tools]` / default workstation section, add a short example showing
both policies:

```toml
# Follow the latest release when this declaration is first applied.
"npm:@anthropic-ai/claude-code" = "latest"

# Or pin a reviewed release and change this value for controlled upgrades.
"npm:@anthropic-ai/claude-code" = "2.1.218"
```

Explain that adding or changing the entry requires `gascan apply`, the
networked sandbox downloads it into the persistent tools volume, its shim
precedes the immutable bundled fallback, and changing an exact version is the
deterministic upgrade path. Explicitly avoid claiming that an unchanged
`latest` is refreshed on every apply.

- [ ] **Step 4: Verify documentation against source contracts**

Run:

```sh
rg -n 'enum Commands|SshConfigCommand|struct Manifest|pub struct Manifest' \
  crates/gascan crates/gascan-core
rg -n 'npm:@anthropic-ai/claude-code|claude --version|herdr --version|gascan apply' \
  README.md
git diff --check
```

Manually confirm the Quickstart uses only current commands and every full
schema key appears in `docs/reference/manifest.md`.

- [ ] **Step 5: Commit the README revision**

```sh
git add README.md
git commit -m "docs: improve first-use agent workflow"
```

---

### Task 3: Verify, Review, Integrate, and Release

**Files:**
- Verify: entire workspace
- Modify during release bump: six `crates/*/Cargo.toml`, `Cargo.lock`,
  `README.md`, `docs/release/macos-checklist.md`

**Interfaces:**
- Consumes: Tasks 1 and 2, the repository PR workflow, and
  `packaging/macos/release.sh`.
- Produces: merged feature commit, merged patch-version bump, signed pushed
  tag, published GitHub release, updated Homebrew tap, and removed temporary
  worktree.

- [ ] **Step 1: Run complete feature verification**

Run:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo check --locked --workspace --all-targets
git diff --check
```

Run all release contracts because the release follows immediately:

```sh
for contract in tests/release/*-contract.sh; do
  bash "$contract"
done
```

Expected: every command exits zero.

- [ ] **Step 2: Review the complete branch**

Compare the branch with `origin/main` and verify:

- only the spec, plan, SSH implementation/tests, and README changed;
- safe conventional modes are accepted and preserved;
- writable modes and all substitution/race attacks remain rejected;
- documentation does not claim automatic floating-version refresh;
- no user-owned primary-checkout changes are present.

- [ ] **Step 3: Push, open the feature PR, wait for checks, and merge**

```sh
git push -u origin fix/ssh-permissions-docs
gh pr create --base main --head fix/ssh-permissions-docs \
  --title "Accept conventional SSH permissions and improve onboarding" \
  --body "Accept conventional OpenSSH-safe host permissions, preserve safe existing config modes, and improve the first-use Claude Code and Herdr workflow."
pr=$(gh pr view fix/ssh-permissions-docs --json number --jq .number)
gh pr checks "$pr" --watch
gh pr merge "$pr" --squash --delete-branch
```

Confirm `origin/main` contains the squash merge before release work.

- [ ] **Step 4: Create and merge the patch-version bump**

From a clean branch based on updated `origin/main`, increment `0.1.8` to
`0.1.9` in exactly the nine files named by `docs/release/releasing.md`. Run
`cargo update --workspace --offline`, verify metadata and locked checks, commit
the bump, run every release contract, push, create the version PR, wait for
checks, and squash-merge it.

- [ ] **Step 5: Tag and run the release driver**

From clean, synchronized `main`:

```sh
git tag -s v0.1.9 -m "Gas Can 0.1.9"
git push origin v0.1.9
./packaging/macos/release.sh 0.1.9 --check
./packaging/macos/release.sh 0.1.9
```

Verify the public release, package checksum, and Homebrew tap commit reported
by the driver. Do not recreate, move, or overwrite a published tag or release.

- [ ] **Step 6: Clean up the temporary feature and version worktrees**

After confirming both branches are merged and no uncommitted files remain:

```sh
git worktree remove .worktrees/ssh-permissions-docs
git worktree prune
```

Remove any separate version-bump worktree using the same clean-state check.
Confirm `git worktree list` contains neither temporary worktree.
