# Developer Onboarding and Nested Starship Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix Starship in nested interactive shells and add secure, persistent, guided Git, GitHub CLI, and GitLab CLI configuration for each Gas Can sandbox.

**Architecture:** The macOS `gascan` CLI owns user interaction, host-default discovery, sandbox selection, and secret streaming. A reusable guest-session runner executes argv without a shell and transports bounded stdin/stdout/stderr over the existing Run/Attach API. The immutable workspace image owns persistent Git/SSH home layout and a narrow, no-secret developer-home helper; native `git`, `ssh-keygen`, `gh`, and `glab` own their configuration formats.

**Tech Stack:** Rust 1.85 workspace, Clap, Tokio/Tonic streaming, Bash/Python image helpers, Ubuntu 24.04 ARM64 workspace image, Git/OpenSSH, GitHub CLI, GitLab CLI, Apple Container, signed/notarized macOS release tooling.

## Global Constraints

- Work only in the isolated `feat/developer-onboarding` worktree.
- Preserve the existing protobuf major version and wire compatibility; reuse the existing Run/Attach messages.
- Never accept a token through argv or an environment variable.
- Never print, serialize, log, or persist a token in Gas Can-owned state.
- Tokens may exist only in a redaction-aware in-memory value and the guest CLI stdin stream.
- Host Git defaults come only from `git config --global`, never repository or worktree configuration.
- The sandbox key is Ed25519, passwordless, per-sandbox, persistent, mode `0600`, and used for SSH authentication and commit/tag signing.
- SSH is the default Git protocol; HTTPS remains an explicit choice.
- OpenSSH host-key checking remains enabled.
- First-run setup is optional, TTY-only, non-blocking, and never changes a successful `up` result into failure.
- Existing valid setup is reused; unsafe managed paths fail closed without deletion.
- The onboarding receipt contains only versioned completion or decline state.
- The nested-shell correction must retain immutable input, readonly collision, function collision, DEBUG trap, BLE, syntax, guarded-apply, and rollback protections.
- The release target is `0.1.17` from current `0.1.16`.
- Every shell command in this environment is prefixed with `rtk`.

Design: `docs/superpowers/specs/2026-07-30-developer-onboarding-design.md`

---

## File Structure

| Path | Responsibility |
| --- | --- |
| `images/workspace/etc/gascan/bashrc` | Transactional Starship startup, including safe nested-shell reset. |
| `images/workspace/bin/configure-workstation-home` | Conventional persistent home links and managed-directory preflight. |
| `images/workspace/bin/configure-developer-home` | No-secret Git identity, key, SSH, receipt, and status mutations inside the guest. |
| `images/workspace/Dockerfile` | Install helper and export persistent Git configuration path. |
| `crates/gascan/src/guest.rs` | Bounded non-TTY Run/Attach execution with exact argv/stdin and captured output. |
| `crates/gascan/src/configure/mod.rs` | Public setup models, coordinator, and summary. |
| `crates/gascan/src/configure/host.rs` | Global Git defaults and host `gh`/`glab` account/token discovery. |
| `crates/gascan/src/configure/prompt.rs` | Line prompts, hidden token input, cancellation, and terminal restoration. |
| `crates/gascan/src/configure/git.rs` | Persistent Git/key setup through the guest helper. |
| `crates/gascan/src/configure/forge.rs` | GitHub/GitLab login, verification, and key registration adapters. |
| `crates/gascan/src/configure/onboarding.rs` | Aggregate walkthrough, focused commands, first-use receipt logic. |
| `crates/gascan/src/cli.rs` | Clap surface, command dispatch, and post-`up` offer integration. |
| `crates/gascan/tests/configure_cli.rs` | Process-level CLI and secret-leak contracts. |
| `images/workspace/tests/workstation-contract.sh` | Persistent developer-home, permissions, and credential-isolation contract. |
| `tests/image/shell-home-root-contract.sh` | Shell-hook production fixture contract. |
| `scripts/tests/image_user_contract.rs` | Host-runnable image helper and nested-Starship unit contracts. |
| `crates/gascan-e2e/tests/apple_apply.rs` | Live persistence and image-replacement proof. |
| `README.md` | Quickstart and full user-facing configuration reference. |

---

### Task 1: Permit clean Starship initialization in nested Bash

**Files:**
- Modify: `images/workspace/etc/gascan/bashrc`
- Modify: `scripts/tests/image_user_contract.rs`
- Modify: `tests/image/shell-home-root-contract.sh`

**Interfaces:**
- Consumes: writable inherited `STARSHIP_*` environment declarations from a parent Starship-enabled Bash.
- Produces: `__gascan_starship_clear_runtime`, which unsets only the reviewed Starship runtime variables inside the isolated evaluator before full initialization.

- [ ] **Step 1: Add a failing nested-shell Rust contract**

Add a test that initializes the fixture hook, exports the resulting Starship
state, and launches a second interactive Bash that sources the same hook:

```rust
#[test]
fn nested_interactive_bash_reinitializes_inherited_starship_state() {
    let temporary = tempfile::tempdir().unwrap();
    let (hook, _shell_dir, _starship, log) = hook_fixture(&temporary);
    let command = r#"
        . "$GASCAN_TEST_HOOK"
        export STARSHIP_CONFIG STARSHIP_EXECUTABLE STARSHIP_SHELL STARSHIP_SESSION_KEY
        GASCAN_TEST_HOOK="$GASCAN_TEST_HOOK" GASCAN_TEST_LOG="$GASCAN_TEST_LOG" \
          /bin/bash --noprofile --norc -ic \
          '. "$GASCAN_TEST_HOOK"; printf "PS1=%s\n" "$PS1"'
    "#;
    let output = run_hook(&hook, command, &log, false, false, true);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("PS1=managed-starship"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("Starship prompt unavailable"));
}
```

- [ ] **Step 2: Run the focused test and prove the current guard fails**

Run:

```sh
rtk cargo test --manifest-path scripts/Cargo.toml nested_interactive_bash_reinitializes_inherited_starship_state -- --exact
```

Expected: FAIL because `__gascan_starship_preflight` rejects inherited
`STARSHIP_SHELL` or `STARSHIP_SESSION_KEY`.

- [ ] **Step 3: Add production shell-contract coverage**

In `shell-home-root-contract.sh`, initialize one Bash and launch a nested
interactive Bash. Assert exactly zero fallback warnings, the managed prompt,
and two full-init invocations. Keep the existing attacker variable cases.

- [ ] **Step 4: Implement isolated runtime clearing**

Move all reviewed internal Starship variables into the writable-variable
readonly check. Remove their blanket “already declared” rejection. Add:

```bash
__gascan_starship_clear_runtime()
{
    unset STARSHIP_PREEXEC_READY STARSHIP_START_TIME STARSHIP_CMD_STATUS \
        STARSHIP_PIPE_STATUS STARSHIP_END_TIME STARSHIP_DURATION \
        STARSHIP_PROMPT_COMMAND STARSHIP_DEBUG_TRAP STARSHIP_SHELL \
        STARSHIP_SESSION_KEY
}
```

Call it only inside `__gascan_starship_build_commit` before evaluating the
fresh initialization. Preserve capture, guarded replacement, rollback, and
cleanup of helper functions.

- [ ] **Step 5: Run all shell-hook security tests**

Run:

```sh
rtk cargo test --manifest-path scripts/Cargo.toml image_user_contract
rtk bash tests/image/shell-home-root-contract.sh
```

Expected: nested-shell tests PASS and every existing collision, immutable
input, DEBUG trap, BLE, rollback, and failure-injection case remains PASS.

- [ ] **Step 6: Commit the nested-shell correction**

```sh
rtk git add images/workspace/etc/gascan/bashrc scripts/tests/image_user_contract.rs tests/image/shell-home-root-contract.sh
rtk git diff --cached --check
rtk git commit -m "fix: initialize Starship in nested shells"
```

---

### Task 2: Establish the persistent Git and SSH home contract

**Files:**
- Modify: `images/workspace/bin/configure-workstation-home`
- Modify: `images/workspace/Dockerfile`
- Modify: `images/workspace/tests/workstation-contract.sh`
- Modify: `scripts/tests/image_user_contract.rs`

**Interfaces:**
- Produces: `GIT_CONFIG_GLOBAL=/home/workspace/.config/gascan/git/config`.
- Produces: `/home/workspace/.ssh -> .config/gascan/git/ssh`.
- Produces: user-owned mode-`0700` managed directories
  `/home/workspace/.config/gascan/git` and `.../git/ssh`.

- [ ] **Step 1: Add failing home-layout tests**

Extend the workstation-helper fixture to require:

```sh
test "$(readlink "$HOME/.ssh")" = .config/gascan/git/ssh
test "$(stat -c %U:%G:%a "$HOME/.config/gascan/git")" = workspace:workspace:700
test "$(stat -c %U:%G:%a "$HOME/.config/gascan/git/ssh")" = workspace:workspace:700
test "$GIT_CONFIG_GLOBAL" = /home/workspace/.config/gascan/git/config
```

Add hostile cases for a preexisting regular `~/.ssh`, wrong symlink target,
unmarked Git directory, and symlinked SSH directory; all must fail before any
mutation.

- [ ] **Step 2: Run the focused image-user contracts**

```sh
rtk cargo test --manifest-path scripts/Cargo.toml configure_workstation_home
```

Expected: FAIL because the Git directories, link, and environment variable do
not exist.

- [ ] **Step 3: Extend preflight and publication**

Add `"$gascan_root/git"` and `"$gascan_root/git/ssh"` to the existing managed
directory preflight/creation list. Add:

```sh
preflight_link "$HOME/.ssh" ".config/gascan/git/ssh"
```

Create the relative symlink only after all preflight checks pass. Keep marker,
mode, and owner validation identical to the other managed user directories.

- [ ] **Step 4: Export the exact global Git config path**

Add to the final image:

```dockerfile
ENV GIT_CONFIG_GLOBAL=/home/workspace/.config/gascan/git/config
```

Update `workstation-contract.sh` so the conventional `.ssh` path is accepted
only when it is the exact managed relative symlink and its resolved directory
stays under the config volume. Continue rejecting `/root/.ssh`,
`/workspace/.ssh`, host SSH-agent sockets, and host private paths.

- [ ] **Step 5: Run the workstation contracts**

```sh
rtk cargo test --manifest-path scripts/Cargo.toml configure_workstation_home
rtk bash images/workspace/tests/workstation-contract.sh
```

Expected: PASS, including all hostile collision cases.

- [ ] **Step 6: Commit the persistent layout**

```sh
rtk git add images/workspace/bin/configure-workstation-home images/workspace/Dockerfile images/workspace/tests/workstation-contract.sh scripts/tests/image_user_contract.rs
rtk git diff --cached --check
rtk git commit -m "feat: persist sandbox Git and SSH configuration"
```

---

### Task 3: Extract a bounded programmatic guest runner

**Files:**
- Create: `crates/gascan/src/guest.rs`
- Modify: `crates/gascan/src/lib.rs`
- Modify: `crates/gascan/src/cli.rs`
- Create: `crates/gascan/tests/guest_runner.rs`

**Interfaces:**
- Produces:

```rust
pub(crate) struct Secret(Vec<u8>);
impl Secret {
    pub(crate) fn new(bytes: Vec<u8>) -> Self;
    pub(crate) fn expose(&self) -> &[u8];
}

pub(crate) struct GuestCommand {
    pub(crate) argv: Vec<Vec<u8>>,
    pub(crate) environment: Vec<v1::EnvironmentVariable>,
    pub(crate) stdin: Option<Secret>,
}

pub(crate) struct GuestOutput {
    pub(crate) code: i32,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
}

#[tonic::async_trait]
pub(crate) trait GuestRunner {
    async fn execute(
        &mut self,
        selector: v1::SandboxSelector,
        command: GuestCommand,
    ) -> Result<GuestOutput, CliError>;
    async fn execute_interactive(
        &mut self,
        selector: v1::SandboxSelector,
        argv: Vec<Vec<u8>>,
    ) -> Result<i32, CliError>;
}

pub(crate) struct ClientGuestRunner<'a> {
    client: &'a mut Client,
}
```

- Consumes: existing `Run`, first `OperationEvent.session_token`, and `Attach`
  streaming frames. No protobuf change.

- [ ] **Step 1: Write failing fake-daemon guest-runner tests**

Cover exact argv preservation, zero-length stdin, multi-frame secret stdin,
separate stdout/stderr capture, nonzero exit, server error, missing token,
missing exit, a one-MiB output bound, and interactive TTY delegation with
terminal restoration. Use the sentinel
`gascan-test-secret-7d9f3a` and assert `Debug` and every error omit it.

- [ ] **Step 2: Run the new test target**

```sh
rtk cargo test -p gascan --test guest_runner
```

Expected: FAIL because `guest` and `GuestCommand` do not exist.

- [ ] **Step 3: Implement redaction-aware secret ownership**

`Secret` implements a constant `Debug` value (`Secret([REDACTED])`), never
`Display`, and overwrites its vector with zero bytes in `Drop`. No error path
formats stdin. Keep the type private to the Gas Can crate.

- [ ] **Step 4: Implement bounded Run/Attach execution**

Send the first Run request with `tty: false`; send stdin frames directly from
the owned secret, then one Close frame. Capture stdout and stderr separately,
reject either stream exceeding `1024 * 1024` bytes, and require one Exit frame.
Never spawn the host stdin forwarding tasks used by public `gascan run`.

- [ ] **Step 5: Refactor shared session validation**

Move token validation, attach-frame error conversion, and frame-size constants
out of `cli.rs` into `guest.rs`. Keep interactive/raw-terminal behavior in
one shared TTY attachment helper used by `cli.rs` and
`ClientGuestRunner::execute_interactive`; public `run` and `shell` output
semantics must not change.

- [ ] **Step 6: Run focused and existing attach tests**

```sh
rtk cargo test -p gascan --test guest_runner
rtk cargo test -p gascan cli::
rtk cargo test -p gascand --test attach_bridge
rtk cargo test -p gascan-e2e --test fake_backend
```

Expected: PASS with no wire fixture changes.

- [ ] **Step 7: Commit the guest runner**

```sh
rtk git add crates/gascan/src/guest.rs crates/gascan/src/lib.rs crates/gascan/src/cli.rs crates/gascan/tests/guest_runner.rs
rtk git diff --cached --check
rtk git commit -m "refactor: add bounded guest command execution"
```

---

### Task 4: Add host defaults, account discovery, and hidden secret input

**Files:**
- Create: `crates/gascan/src/configure/mod.rs`
- Create: `crates/gascan/src/configure/host.rs`
- Create: `crates/gascan/src/configure/prompt.rs`
- Modify: `crates/gascan/src/lib.rs`
- Create: `crates/gascan/tests/configure_host.rs`

**Interfaces:**
- Produces:

```rust
pub(crate) struct GitDefaults {
    pub(crate) name: Option<String>,
    pub(crate) email: Option<String>,
}

pub(crate) struct HostAccount {
    pub(crate) hostname: String,
    pub(crate) login: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Forge { GitHub, GitLab }

pub(crate) trait HostDiscovery {
    fn git_defaults(&self) -> Result<GitDefaults, ConfigureError>;
    fn accounts(&self, forge: Forge) -> Result<Vec<HostAccount>, ConfigureError>;
    fn token(&self, forge: Forge, account: &HostAccount) -> Result<Secret, ConfigureError>;
}

pub(crate) trait Prompter {
    fn confirm(&mut self, prompt: &str, default: bool) -> Result<bool, ConfigureError>;
    fn line(&mut self, prompt: &str, default: Option<&str>) -> Result<Option<String>, ConfigureError>;
    fn secret(&mut self, prompt: &str) -> Result<Option<Secret>, ConfigureError>;
}

pub(crate) enum ConfigureError {
    Cancelled,
    Io(std::io::Error),
    HostCommand { category: &'static str, message: String },
    GuestCommand { category: &'static str, message: String },
    InvalidOutput { category: &'static str },
    UnsafeState { path: String, remedy: String },
}
```

- [ ] **Step 1: Write failing host-discovery tests**

Use fake executables that record argv. Assert Git discovery invokes exactly:

```text
git config --global --get user.name
git config --global --get user.email
```

from `/`, and never invokes `git config --get` without `--global`.

For GitHub, fixture `gh auth status --json hosts` and
`gh auth token --hostname HOST`. For GitLab, fixture
`glab auth status --all` and `glab config get token --global --host HOST`.
Cover missing executables, unauthenticated accounts, multiple enterprise
hosts, malformed output, and sentinel-secret redaction.

- [ ] **Step 2: Write failing PTY prompt tests**

Use `rustix_openpty` to prove:

- normal lines echo;
- secret bytes do not echo;
- newline terminates the secret;
- EOF and Ctrl-C return cancellation;
- termios is restored after success, I/O failure, and unwind.

- [ ] **Step 3: Run the focused tests**

```sh
rtk cargo test -p gascan --test configure_host
```

Expected: FAIL because the configure modules are absent.

- [ ] **Step 4: Implement host command execution without a shell**

Use `std::process::Command` with fixed programs and discrete arguments.
Bound stdout and stderr to one MiB. Parse only the documented machine output
where available; the GitLab status parser accepts only explicit authenticated
host records and rejects ambiguous lines. Remove `GH_TOKEN`, `GITHUB_TOKEN`,
`GH_ENTERPRISE_TOKEN`, `GITHUB_ENTERPRISE_TOKEN`, `GITLAB_TOKEN`,
`GITLAB_ACCESS_TOKEN`, and `OAUTH_TOKEN` from host discovery subprocesses so
an ambient secret is never inherited; users with environment-only credentials
use the hidden-entry path.

- [ ] **Step 5: Implement hidden input with restoration**

Duplicate the terminal descriptor, clear only `LocalModes::ECHO`, read through
the duplicate, and restore in `Drop`. Trim one trailing CR/LF but preserve all
other token bytes. Empty input cancels instead of authenticating with an empty
token.

- [ ] **Step 6: Verify no secret crosses an observable boundary**

Have fake host CLIs return `gascan-test-secret-7d9f3a`; assert it is absent
from recorded argv, environment, formatted errors, `Debug`, stdout, and
stderr. Token retrieval stdout is consumed, not forwarded.

- [ ] **Step 7: Commit host discovery and prompts**

```sh
rtk git add crates/gascan/src/configure crates/gascan/src/lib.rs crates/gascan/tests/configure_host.rs
rtk git diff --cached --check
rtk git commit -m "feat: discover host developer credentials safely"
```

---

### Task 5: Add the no-secret guest developer-home helper and Git setup

**Files:**
- Create: `images/workspace/bin/configure-developer-home`
- Modify: `images/workspace/Dockerfile`
- Modify: `images/workspace/tests/workstation-contract.sh`
- Modify: `scripts/tests/image_user_contract.rs`
- Create: `crates/gascan/src/configure/git.rs`
- Create: `crates/gascan/tests/configure_git.rs`

**Interfaces:**
- Guest helper commands:

```text
configure-developer-home status
configure-developer-home git --sandbox-id ID --name NAME --email EMAIL --protocol ssh|https
configure-developer-home ssh-host --hostname HOST
configure-developer-home receipt status
configure-developer-home receipt complete
configure-developer-home receipt decline
```

- `status` emits bounded JSON containing configured identity, protocol, public
  key, fingerprint, and receipt state. It never emits a private key or token.
- Produces:

```rust
#[derive(Clone, Copy, Debug, Eq, PartialEq, clap::ValueEnum)]
pub(crate) enum GitProtocol { Ssh, Https }

pub(crate) struct GitRequest {
    pub(crate) sandbox_id: String,
    pub(crate) name: String,
    pub(crate) email: String,
    pub(crate) protocol: GitProtocol,
}

pub(crate) struct GitSetup {
    pub(crate) name: String,
    pub(crate) email: String,
    pub(crate) protocol: GitProtocol,
    pub(crate) public_key: String,
    pub(crate) fingerprint: String,
}

pub(crate) async fn configure_git<R: GuestRunner>(
    runner: &mut R,
    selector: v1::SandboxSelector,
    request: GitRequest,
) -> Result<GitSetup, ConfigureError>;

pub(crate) async fn configure_ssh_host<R: GuestRunner>(
    runner: &mut R,
    selector: v1::SandboxSelector,
    hostname: &str,
) -> Result<(), ConfigureError>;
```

- [ ] **Step 1: Write failing helper security contracts**

Create fixtures for a clean persistent home and assert exact output and:

```sh
test "$(stat -c %a "$HOME/.config/gascan/git/ssh")" = 700
test "$(stat -c %a "$HOME/.config/gascan/git/ssh/id_ed25519")" = 600
test "$(stat -c %a "$HOME/.config/gascan/git/ssh/id_ed25519.pub")" = 644
git config --global --get gpg.format | grep -Fx ssh
git config --global --get commit.gpgsign | grep -Fx true
git config --global --get tag.gpgsign | grep -Fx true
```

Run the helper twice and assert the private-key hash is unchanged. Add
preexisting FIFO, symlink, wrong owner, permissive mode, hard-link, invalid
private key, partial pair, unsafe Git config, unsafe receipt, and staging-file
cases; all fail before replacement or deletion.

- [ ] **Step 2: Run the helper tests and prove failure**

```sh
rtk cargo test --manifest-path scripts/Cargo.toml configure_developer_home
```

Expected: FAIL because the helper is absent.

- [ ] **Step 3: Implement strict preflight and key generation**

Use Python `os.open(..., O_NOFOLLOW)`, `fstat`, bounded reads, and same-directory
exclusive staging. Validate exact owner, type, mode, and link count. Generate
with:

```text
/usr/bin/ssh-keygen -q -t ed25519 -N "" -C gascan-SANDBOX_ID -f STAGED_PATH
```

Publish the pair only after both staged files validate and directories are
synced. Reuse an existing valid pair.

- [ ] **Step 4: Configure Git and OpenSSH**

Write the persistent Git config through `git config --file STAGING` and
atomically publish it. Set the six approved values from the design. For SSH
protocol, `ssh-host` validates a DNS hostname and atomically publishes sorted
managed host stanzas selecting the persistent key with `IdentitiesOnly yes`
while retaining normal host-key checking. Repeated hosts are idempotent and
multiple enterprise hosts coexist. For HTTPS, Git setup does not add a host
stanza; focused forge setup adds one only when its selected protocol is SSH.

- [ ] **Step 5: Implement versioned receipt operations**

Use exact contents:

```text
gascan-developer-onboarding-v1 complete
gascan-developer-onboarding-v1 declined
```

`status` reports `pending`, `complete`, or `declined`. Publication is atomic,
mode `0600`, and contains no setup values.

- [ ] **Step 6: Implement the Rust Git adapter**

Invoke only the helper with discrete argv. Parse the bounded JSON status,
validate hostname-independent public-key and SHA256 fingerprint shapes, and
return `GitSetup`. Do not duplicate filesystem mutation in the host CLI.

- [ ] **Step 7: Run helper, adapter, and signing tests**

```sh
rtk cargo test --manifest-path scripts/Cargo.toml configure_developer_home
rtk cargo test -p gascan --test configure_git
rtk bash images/workspace/tests/workstation-contract.sh
```

Include a local temporary Git repository that creates a commit and annotated
tag and verifies both with `git verify-commit` and `git verify-tag` using the
generated public key in an `allowedSignersFile`.

- [ ] **Step 8: Commit Git setup**

```sh
rtk git add images/workspace/bin/configure-developer-home images/workspace/Dockerfile images/workspace/tests/workstation-contract.sh scripts/tests/image_user_contract.rs crates/gascan/src/configure/git.rs crates/gascan/tests/configure_git.rs
rtk git diff --cached --check
rtk git commit -m "feat: configure persistent Git identity and signing"
```

---

### Task 6: Add GitHub and GitLab authentication and key registration

**Files:**
- Create: `crates/gascan/src/configure/forge.rs`
- Create: `crates/gascan/tests/configure_forge.rs`

**Interfaces:**
- Produces:

```rust
pub(crate) struct ForgeRequest {
    pub(crate) forge: Forge,
    pub(crate) hostname: String,
    pub(crate) protocol: GitProtocol,
    pub(crate) token: Secret,
    pub(crate) key: GitSetup,
}

pub(crate) struct ForgeSetup {
    pub(crate) forge: Forge,
    pub(crate) hostname: String,
    pub(crate) login: String,
    pub(crate) authenticated: bool,
    pub(crate) authentication_key: RegistrationState,
    pub(crate) signing_key: RegistrationState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RegistrationState {
    Existing,
    Added,
    Skipped,
    Failed,
}
```

- [ ] **Step 1: Write failing GitHub adapter tests**

The fake guest runner must observe exactly:

```text
gh auth login --hostname HOST --git-protocol ssh --skip-ssh-key --with-token
gh auth status --hostname HOST
gh api --hostname HOST user/keys
gh api --hostname HOST user/ssh_signing_keys
gh api --hostname HOST --method POST user/keys --raw-field title=TITLE --raw-field key=PUBLIC_KEY
gh api --hostname HOST --method POST user/ssh_signing_keys --raw-field title=TITLE --raw-field key=PUBLIC_KEY
```

Cover existing matching keys, one missing role, both missing roles, rejected
token, missing scope, enterprise host, malformed JSON, and partial success.

- [ ] **Step 2: Write failing GitLab adapter tests**

Observe exactly:

```text
glab auth login --hostname HOST --git-protocol ssh --stdin
glab auth status --hostname HOST
glab api --hostname HOST /user/keys
glab api --hostname HOST --method POST /user/keys --raw-field title=TITLE --raw-field key=PUBLIC_KEY --raw-field usage_type=auth_and_signing
```

Cover existing `auth_and_signing`, a signing-only collision, registration,
self-managed host, and partial success.

- [ ] **Step 3: Run the focused forge tests**

```sh
rtk cargo test -p gascan --test configure_forge
```

Expected: FAIL because the forge adapter is absent.

- [ ] **Step 4: Implement login with exact secret stdin**

Move the `Secret` into one guest command. Append exactly one newline only when
the native CLI requires it. Clear the secret immediately after Attach closes.
Disable CLI update checks and color with non-secret reviewed environment
variables so errors are deterministic.

- [ ] **Step 5: Implement verified idempotent registration**

Parse JSON key lists, compare decoded public-key algorithm and body rather than
titles, and register only missing roles. Key titles are
`Gas Can <sandbox-id>` and contain no canonical project path. Treat an
already-present matching key as success. Before registering an SSH-role key,
call `configure_ssh_host` for the selected hostname. After successful
registration, call `execute_interactive` with `ssh -T git@HOST` so ordinary
strict host-key checking displays and records a new host fingerprint. Treat
the documented GitHub/GitLab “authenticated, no shell access” response as
verification even when the server returns its conventional nonzero code.

- [ ] **Step 6: Implement redacted partial failures**

Return structured states for authenticated/key-registration outcomes. A
registration failure retains authenticated success and names only the native
command category, hostname, stable error, and `gascan configure gh|glab`
retry. Run the sentinel-secret assertions across every failure fixture.

- [ ] **Step 7: Run all forge tests**

```sh
rtk cargo test -p gascan --test configure_forge
rtk cargo test -p gascan --test guest_runner
```

Expected: PASS and no sentinel secret in test artifacts.

- [ ] **Step 8: Commit forge configuration**

```sh
rtk git add crates/gascan/src/configure/forge.rs crates/gascan/tests/configure_forge.rs
rtk git diff --cached --check
rtk git commit -m "feat: configure GitHub and GitLab authentication"
```

---

### Task 7: Build the configure CLI and guided coordinator

**Files:**
- Create: `crates/gascan/src/configure/onboarding.rs`
- Modify: `crates/gascan/src/configure/mod.rs`
- Modify: `crates/gascan/src/cli.rs`
- Create: `crates/gascan/tests/configure_cli.rs`

**Interfaces:**
- Clap surface:

```rust
Configure {
    #[command(subcommand)]
    command: Option<ConfigureCommand>,
}

enum ConfigureCommand {
    Git,
    Gh {
        #[arg(long)] hostname: Option<String>,
        #[arg(long)] token_stdin: bool,
        #[arg(long, value_enum, default_value_t = GitProtocol::Ssh)]
        git_protocol: GitProtocol,
    },
    Glab {
        #[arg(long)] hostname: Option<String>,
        #[arg(long)] token_stdin: bool,
        #[arg(long, value_enum, default_value_t = GitProtocol::Ssh)]
        git_protocol: GitProtocol,
    },
}

pub(crate) trait ConfigureIo: Prompter {
    fn write_out(&mut self, text: &str) -> Result<(), ConfigureError>;
    fn write_err(&mut self, text: &str) -> Result<(), ConfigureError>;
    fn stdin_is_terminal(&self) -> bool;
    fn stderr_is_terminal(&self) -> bool;
}
```

- [ ] **Step 1: Add failing Clap and process-level tests**

Cover the four public forms, global `--sandbox`, invalid protocol, forbidden
`--token`, unexpected values, token-stdin with TTY and pipe, focused commands,
no sandbox, multiple sandboxes, stopped sandbox, and aggregate non-TTY refusal.

- [ ] **Step 2: Add failing coordinator tests with fakes**

Exercise:

- host global defaults accepted and edited;
- no host defaults;
- SSH default and HTTPS selection;
- existing valid Git/key state;
- GitHub/GitLab host import confirmation;
- hidden token fallback;
- multiple and enterprise hosts;
- per-section skip;
- cancellation;
- offline route detection;
- auth success plus registration failure;
- concise final summary.

Offline detection invokes a no-secret guest probe for a usable default route
before remote sections. No protocol change is needed.

- [ ] **Step 3: Run the focused CLI test**

```sh
rtk cargo test -p gascan --test configure_cli
```

Expected: FAIL because Clap has no `configure` command.

- [ ] **Step 4: Implement focused command dispatch**

Resolve the sandbox with the existing selector, fetch status, require
`ActualState::Running`, and create one `ClientGuestRunner`. `git` is
interactive. `gh` and `glab` use hidden input by default or read stdin exactly
once with `--token-stdin`.

- [ ] **Step 5: Implement the aggregate guide**

Use the approved order: Git/key, GitHub, GitLab, summary. Show current values
before confirmation. A skipped component is explicit. Cancellation returns a
clean status without receipt mutation. Errors report retained components and
the focused retry command.

- [ ] **Step 6: Implement host-import choice**

List detected accounts by hostname/login, ask for explicit confirmation, then
retrieve the selected token only after confirmation. If retrieval fails,
offer hidden entry without printing retrieval output.

- [ ] **Step 7: Verify presentation and secret isolation**

Capture stdout/stderr for every fake flow. Assert the summary includes only
name, email, hostname, account, protocol, public fingerprint, and registration
state. Assert the sentinel token is absent.

- [ ] **Step 8: Run Gas Can CLI tests**

```sh
rtk cargo test -p gascan --test configure_cli
rtk cargo test -p gascan --test configure_host
rtk cargo test -p gascan --test configure_git
rtk cargo test -p gascan --test configure_forge
rtk cargo test -p gascan
```

Expected: PASS.

- [ ] **Step 9: Commit the configure command**

```sh
rtk git add crates/gascan/src/configure crates/gascan/src/cli.rs crates/gascan/tests/configure_cli.rs
rtk git diff --cached --check
rtk git commit -m "feat: add guided developer configuration"
```

---

### Task 8: Integrate the non-blocking first-`up` offer

**Files:**
- Modify: `crates/gascan/src/cli.rs`
- Modify: `crates/gascan/src/configure/onboarding.rs`
- Modify: `crates/gascan/tests/configure_cli.rs`

**Interfaces:**
- Produces:

```rust
pub(crate) enum OfferResult {
    Suppressed,
    Pending,
    Declined,
    Completed,
    Cancelled,
}

pub(crate) async fn offer_after_up(
    client: &mut Client,
    selector: v1::SandboxSelector,
    io: &mut dyn ConfigureIo,
) -> Result<OfferResult, ConfigureError>;
```

- [ ] **Step 1: Add failing post-`up` tests**

Cover successful interactive `up`, declined receipt, completed receipt,
pending receipt, cancellation, setup error, receipt status error, receipt
write error, redirected input, redirected stderr, CI variables, JSON, failed
`up`, and multiple existing sandboxes.

Derive the exact new sandbox selector from the already resolved project root
by loading the same manifest and constructing `SandboxSpec`; never use
“the only sandbox” after `up`.

- [ ] **Step 2: Prove the current CLI never offers setup**

```sh
rtk cargo test -p gascan --test configure_cli first_up_
```

Expected: FAIL because only the host SSH include offer exists.

- [ ] **Step 3: Refactor post-success offers**

Keep the existing SSH include offer behavior. After human `up` returns zero,
check the developer receipt for the exact selector. Suppress for JSON, CI, or
non-TTY. Print:

```text
Set up Git, GitHub, and GitLab for this sandbox now? [Y/n]
```

On decline, write the declined receipt and print
`Run 'gascan configure' whenever you are ready.`

- [ ] **Step 4: Preserve successful `up` on every onboarding failure**

Convert receipt/setup errors into one warning with the retry command. Return
the original zero exit code. Never run an offer after an operation failure or
nonzero operation result.

- [ ] **Step 5: Run post-`up` and existing SSH-offer tests**

```sh
rtk cargo test -p gascan --test configure_cli first_up_
rtk cargo test -p gascan cli::tests::preserve_up
rtk cargo test -p gascan --test ssh_config
```

Expected: PASS, with each offer appearing at most once per successful first
use.

- [ ] **Step 6: Commit first-use onboarding**

```sh
rtk git add crates/gascan/src/cli.rs crates/gascan/src/configure/onboarding.rs crates/gascan/tests/configure_cli.rs
rtk git diff --cached --check
rtk git commit -m "feat: offer developer setup on first sandbox start"
```

---

### Task 9: Build, publish, pin, and live-test the workspace image

**Files:**
- Modify: `images/workspace/approved-image.txt`
- Modify: `images/workspace/approved-source.sha256`
- Modify: `docs/evidence/connected-workspace-image.md`
- Modify: `crates/gascan-e2e/tests/apple_apply.rs`
- Modify: image receipts produced by the repository approval workflow only

**Interfaces:**
- Produces: one immutable public `linux/arm64` workspace image reference whose
  digest contains Tasks 1, 2, and 5.
- Produces: updated approved image/source pins consumed by released Gas Can.

- [ ] **Step 1: Add the live persistence test**

Extend `apple_apply.rs` to create Git identity/key state, write a fake
credential sentinel through the native CLI config path, record key/config
hashes, stop/start, replace the image, and assert:

- identity, public fingerprint, private-key hash, and native auth config
  persist;
- the private key is mode `0600` and never appears in output;
- the onboarding receipt persists;
- nested Bash initializes Starship without warning after replacement.

- [ ] **Step 2: Run connected image preflight and locked input verification**

```sh
rtk bash ./scripts/apple-test-preflight.sh
rtk bash ./scripts/verify-workspace-image-inputs.sh
rtk cargo check --manifest-path scripts/Cargo.toml
```

Expected: PASS before the live build.

- [ ] **Step 3: Prefetch and build the connected image**

Use the repository workflow exactly:

```sh
rtk bash ./scripts/prefetch-connected-workspace-image.sh
rtk bash ./scripts/build-connected-workspace-image.sh
rtk bash ./scripts/run-connected-image-gate.sh --prebuilt
```

Do not replace a failed gate with an ad hoc image build.

- [ ] **Step 4: Run live setup and nested-shell checks against the candidate**

```sh
rtk cargo test -p gascan-e2e --test apple_apply -- --ignored developer_configuration_persists
rtk bash images/workspace/tests/workstation-contract.sh
```

Expected: PASS on the candidate digest.

- [ ] **Step 5: Publish the immutable GHCR image and rebind the receipts**

Read `.artifacts/workspace-image-ref` and derive the never-reused remote name
from the locked tag and complete digest:

```sh
rtk bash -c '
set -euo pipefail
receipt=.artifacts/workspace-image-build.json
reference_file=.artifacts/workspace-image-ref
local_reference=$(jq -er .reference "$receipt")
local_tag=${local_reference%@*}
digest=${local_reference##*@}
digest_hex=${digest#sha256:}
test ${#digest_hex} -eq 64
locked_tag=$(awk -F " = " '"'"'$1 == "workspace_tag" {
  gsub(/^"|"$/, "", $2); print $2
}'"'"' images/workspace/versions.lock)
locked_tag=${locked_tag#gascan-workspace:}
remote_tag=ghcr.io/liquescent-development/gascan/workspace:${locked_tag}-${digest_hex}
remote_reference=$remote_tag@$digest
container image tag "$local_tag" "$remote_tag"
headers=$(mktemp .artifacts/.workspace-registry-headers.XXXXXX)
receipt_tmp=$(mktemp .artifacts/.workspace-image-build.public.XXXXXX)
reference_tmp=$(mktemp .artifacts/.workspace-image-ref.public.XXXXXX)
trap '"'"'rm -f "$headers" "$receipt_tmp" "$reference_tmp"'"'"' EXIT
token=$(curl --fail --silent --show-error \
  "https://ghcr.io/token?scope=repository:liquescent-development/gascan/workspace:pull" |
  jq -er .token)
status=$(curl --silent --show-error --output /dev/null \
  --dump-header "$headers" --write-out "%{http_code}" \
  --header "Authorization: Bearer $token" \
  --header "Accept: application/vnd.oci.image.manifest.v1+json, application/vnd.oci.image.index.v1+json" \
  "https://ghcr.io/v2/liquescent-development/gascan/workspace/manifests/${remote_tag##*:}")
case $status in
  200)
    existing=$(awk '"'"'tolower($1) == "docker-content-digest:" {
      gsub(/\r/, "", $2); print $2
    }'"'"' "$headers")
    test "$existing" = "$digest"
    ;;
  404)
    container image push --platform linux/arm64 "$remote_tag"
    ;;
  *)
    printf "unexpected GHCR manifest status: %s\n" "$status" >&2
    exit 1
    ;;
esac
container image pull "$remote_reference"
inspect=$(container image inspect "$remote_tag")
printf "%s" "$inspect" |
  cargo run --quiet --locked --offline --manifest-path scripts/Cargo.toml \
    --bin validate-connected-build -- "$remote_tag" >/dev/null
jq --arg reference "$remote_reference" --arg tag "$remote_tag" \
  ".reference = \$reference | .tag = \$tag" "$receipt" >"$receipt_tmp"
printf "%s\n" "$remote_reference" >"$reference_tmp"
bash scripts/validate-connected-image-receipt.sh \
  "$reference_tmp" "$receipt_tmp" >/dev/null
mv -f "$receipt_tmp" "$receipt"
mv -f "$reference_tmp" "$reference_file"
rm -f "$headers"
trap - EXIT
'
```

The block proceeds only on `404`, or on `200` when the descriptor digest
already equals `$digest`. Any other status or digest stops the task. Never
move or overwrite an existing remote tag.

- [ ] **Step 6: Re-run the live gate against the public digest and approve it**

```sh
rtk bash ./scripts/run-connected-image-gate.sh --prebuilt
rtk env GASCAN_E2E_CANDIDATE_IMAGE_FILE=.artifacts/workspace-image-ref ./scripts/run-apple-e2e.sh
rtk bash ./scripts/approve-connected-workspace-image.sh
```

The gate must publish exact
`.artifacts/connected-workspace-image-candidate.txt` and
`.artifacts/connected-workspace-image-apple-live.txt` receipts matching the
public digest before approval. Approval atomically updates the evidence,
approved image, and approved source fingerprint.

- [ ] **Step 7: Verify the new pins**

```sh
rtk bash ./scripts/validate-connected-image-receipt.sh .artifacts/workspace-image-ref .artifacts/workspace-image-build.json
rtk cargo run --quiet --locked --offline --manifest-path scripts/Cargo.toml --bin update-image-lock -- --verify-existing-workstation-lock
rtk git diff --check
```

Expected: source digest, image digest, and approved pins agree.

- [ ] **Step 8: Commit image pins and live coverage**

```sh
rtk git add images/workspace/approved-image.txt images/workspace/approved-source.sha256 docs/evidence/connected-workspace-image.md crates/gascan-e2e/tests/apple_apply.rs
rtk git diff --cached --check
rtk git commit -m "test: approve developer onboarding workspace image"
```

---

### Task 10: Document, verify, review, and merge the feature

**Files:**
- Modify: `README.md`
- Modify: `packaging/macos/release-smoke.sh`
- Modify: `scripts/tests/macos_release_smoke.rs`

**Interfaces:**
- Produces: documented commands and a release smoke that proves identity,
  signed commit/tag creation, nested Starship, and credential persistence
  without real tokens.
- Produces: `GASCAN_RELEASE_GASCAND`, defaulting to
  `/usr/local/bin/gascand`, so branch smoke uses a matching CLI and daemon.

- [ ] **Step 1: Add failing README and release-smoke contracts**

Require the README to contain every public command, `--hostname`,
`--token-stdin`, SSH/HTTPS choice, enterprise hosts, persistence/security
model, focused retries, offline behavior, and destroy cleanup. Add a smoke
fixture that configures Git with fake forge CLIs and verifies a signed commit
and tag. Add a contract proving the smoke uses
`GASCAN_RELEASE_GASCAND` for daemon attestation and shutdown rather than a
hard-coded installed daemon.

- [ ] **Step 2: Update the quickstart and reference**

The quickstart shows:

```sh
gascan up .
# accept the optional developer setup
gascan configure
gascan configure git
gascan configure gh
gascan configure glab
```

Explain host import, hidden input, global-only defaults, native credential
files, no Gas Can vault, per-sandbox key revocation, GitHub double
registration, GitLab `auth_and_signing`, enterprise hostnames, and verification
with `git log --show-signature -1`.

- [ ] **Step 3: Run formatting and focused suites**

```sh
rtk cargo fmt --all -- --check
rtk cargo test -p gascan
rtk cargo test --manifest-path scripts/Cargo.toml
rtk bash tests/image/shell-home-root-contract.sh
rtk bash images/workspace/tests/workstation-contract.sh
rtk bash tests/release/release-smoke-contract.sh
rtk git diff --check
```

Expected: PASS.

- [ ] **Step 4: Run the full workspace and release contracts**

```sh
rtk cargo test --locked --workspace --all-targets
rtk cargo clippy --locked --workspace --all-targets -- -D warnings
rtk bash -c 'for c in tests/release/*-contract.sh; do bash "$c" >/dev/null || exit; done'
rtk cargo build -p gascan -p gascand
rtk env GASCAN_RELEASE_GASCAN="$PWD/target/debug/gascan" GASCAN_RELEASE_GASCAND="$PWD/target/debug/gascand" ./packaging/macos/release-smoke.sh
```

Expected: all contracts and the branch-built CLI/daemon smoke PASS.

- [ ] **Step 5: Commit documentation and smoke coverage**

```sh
rtk git add README.md packaging/macos/release-smoke.sh scripts/tests/macos_release_smoke.rs
rtk git diff --cached --check
rtk git commit -m "docs: explain developer onboarding"
```

- [ ] **Step 6: Perform two-stage code review**

Invoke `superpowers:requesting-code-review`. Review first against the approved
design and this plan, then for code quality/security. Fix every valid finding
with a failing regression test, rerun the affected suite, and commit each
coherent repair.

- [ ] **Step 7: Run final branch verification**

Invoke `superpowers:verification-before-completion`, then run:

```sh
rtk git status --short
rtk git diff origin/main...HEAD --check
rtk cargo test --locked --workspace --all-targets
rtk cargo clippy --locked --workspace --all-targets -- -D warnings
rtk cargo test --manifest-path scripts/Cargo.toml
rtk bash ./scripts/validate-connected-image-receipt.sh
```

Expected: clean branch, all PASS.

- [ ] **Step 8: Push, open, verify, and squash-merge the feature PR**

```sh
rtk git push -u origin feat/developer-onboarding
rtk gh pr create --base main --head feat/developer-onboarding --title "Add guided developer onboarding" --body-file /tmp/gascan-developer-onboarding-pr.md
rtk gh pr checks --watch
rtk gh pr merge --squash --delete-branch
```

Record the PR URL and squash commit. Do not delete the active worktree until
the release is complete.

---

### Task 11: Bump to 0.1.17 and publish the release

**Files:**
- Modify: six `crates/*/Cargo.toml` workspace package versions
- Modify: `Cargo.lock`
- Modify: `README.md`
- Modify: `docs/release/macos-checklist.md`

**Interfaces:**
- Produces: merged release PR, signed annotated `v0.1.17`, notarized package,
  GitHub release, and Homebrew cask `0.1.17`.

- [ ] **Step 1: Sync and create the release branch**

Fast-forward a clean release worktree to `origin/main`, verify the feature
squash commit is present, and create `release/v0.1.17`. Do not use the dirty
root worktree.

- [ ] **Step 2: Apply the exact nine-file version bump**

Change only the six workspace package `version` fields, root `Cargo.lock` via:

```sh
rtk cargo update --workspace --offline
```

and the version references in `README.md` and
`docs/release/macos-checklist.md`. Leave `scripts/Cargo.lock` and release
contract fixture versions unchanged.

- [ ] **Step 3: Commit and verify the bump**

```sh
rtk git add crates/*/Cargo.toml Cargo.lock README.md docs/release/macos-checklist.md
rtk git diff --cached --check
rtk git commit -S -m "release: prepare Gas Can 0.1.17"
rtk cargo metadata --locked --no-deps --format-version 1
rtk cargo check --locked --workspace --all-targets
rtk bash -c 'for c in tests/release/*-contract.sh; do bash "$c" >/dev/null || exit; done'
```

Expected: signed commit, exact version `0.1.17`, all gates PASS.

- [ ] **Step 4: Push, review, and squash-merge the release PR**

```sh
rtk git push -u origin release/v0.1.17
rtk gh pr create --base main --head release/v0.1.17 --title "Release Gas Can 0.1.17" --body-file /tmp/gascan-v0.1.17-pr.md
rtk gh pr checks --watch
rtk gh pr merge --squash --delete-branch
```

Record the release squash commit.

- [ ] **Step 5: Synchronize `origin/main` and create the provenance tag**

```sh
rtk git fetch origin main --tags
rtk git tag -s v0.1.17 origin/main -m "Gas Can 0.1.17"
rtk git cat-file -t refs/tags/v0.1.17
rtk git verify-tag refs/tags/v0.1.17
rtk git rev-parse 'refs/tags/v0.1.17^{}'
rtk git push origin refs/tags/v0.1.17:refs/tags/v0.1.17
```

Expected: annotated `tag`, trusted signature, target equals the release squash
commit. Never recreate, move, or overwrite the tag.

- [ ] **Step 6: Run the documented release preflight**

From a clean checkout of the tag:

```sh
rtk ./packaging/macos/release.sh 0.1.17 --check
```

Expected: `all release preconditions pass`. Resolve failures using
`docs/release/releasing.md`; do not bypass a gate.

- [ ] **Step 7: Sign, notarize, publish, and update Homebrew**

```sh
rtk ./packaging/macos/release.sh 0.1.17
```

Expected: accepted notarization, stapled package, verified GitHub asset,
published GitHub release, signed tap commit, and pushed cask `0.1.17`.

- [ ] **Step 8: Verify public distribution**

```sh
rtk gh release view v0.1.17 --json tagName,isDraft,isPrerelease,url
rtk brew update
rtk brew info --cask gascan
```

Confirm the release is public/non-draft, the cask reports `0.1.17`, and the
download SHA256 matches the release driver output.

- [ ] **Step 9: Clean temporary branches and worktrees**

After all public verification succeeds, remove only the completed
developer-onboarding and release worktrees and their merged local branches.
Preserve the user's dirty root worktree and the unrelated `provisioning` and
`signed-release-tags` worktrees.
