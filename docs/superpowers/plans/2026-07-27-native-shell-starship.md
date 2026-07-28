# Native Shell and Managed Starship Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `gascan shell` open the same complete Bash login environment as SSH and add automatically managed standard, Starship, and Nerd Font prompt modes.

**Architecture:** The daemon substitutes `/bin/bash --login` only when a Shell request has no explicit argv. A validated manifest enum drives a separately hashed `ConfigureShell` provisioning step that invokes one image-owned, fail-closed configurator. The reviewed workspace image supplies Bash completion, a pinned Starship 1.25.1 ARM64 Linux artifact at a stable path, immutable presets, and a login-only Bash hook shared by Gas Can shell and SSH.

**Tech Stack:** Rust 1.85+ edition 2024, Tokio/Tonic/protobuf, Serde/TOML, SHA-256 durable provisioning state, POSIX shell plus Python 3 image helpers, Apple Container 1.1 live tests.

## Global Constraints

- The manifest schema remains `version = 1`; omitted `[shell]` and omitted `shell.prompt` mean `standard`.
- Accepted prompt strings are exactly `standard`, `starship`, and `starship-nerd-font`.
- Empty Shell argv becomes exactly `["/bin/bash", "--login"]`; explicit argv remains byte-for-byte unchanged.
- The managed prompt uses exact root-owned `/opt/gascan/shell/bin/starship`, never a user-controlled `PATH` lookup.
- Managed Starship activation performs no network access and does not run in `gascan run`, setup, health, or other non-interactive sessions.
- Gas Can never edits host dotfiles, installs host fonts, or claims to detect the host terminal font.
- Unsafe links, file types, ownership, or modes in managed shell state fail closed; an unavailable Starship at interactive startup warns once and falls back to Bash.
- The security boundary covers root-managed files, immutable pinned inputs and
  paths, provisioning authority, and Gas Can's own initialization failures.
  Existing and concurrently same-user-mutated interactive Bash state is
  trusted caller state, not an isolation boundary.
- Preserve compatible caller prompt customization supported by pinned
  Starship, including an existing writable `PROMPT_COMMAND`.
- The compatible preset must not require a Nerd Font; the Nerd Font preset requires an already configured host terminal font.
- Preserve the user-owned changes in the primary checkout; all work stays in `.worktrees/native-shell-starship`.
- Every shell command in this repository environment is prefixed with `rtk`.

---

### Task 1: Add the validated shell prompt manifest model

**Files:**
- Modify: `crates/gascan-core/src/manifest.rs`
- Modify: `crates/gascan-core/tests/manifest.rs`

**Interfaces:**
- Produces: `ShellPrompt::{Standard, Starship, StarshipNerdFont}`
- Produces: `Shell::prompt(&self) -> ShellPrompt`
- Produces: `ShellPrompt::as_str(self) -> &'static str`
- Produces: `Manifest::shell(&self) -> &Shell`
- Consumes: existing `RawManifest` validation and `#[serde(deny_unknown_fields)]` policy

- [ ] **Step 1: Write failing manifest tests**

Add tests that load the omitted table, an empty table, every accepted value,
an invalid value, and an unknown field:

```rust
assert_eq!(load("version = 1\n")?.shell().prompt(), ShellPrompt::Standard);
assert_eq!(
    load("version = 1\n[shell]\n")?.shell().prompt(),
    ShellPrompt::Standard
);
for (value, expected) in [
    ("standard", ShellPrompt::Standard),
    ("starship", ShellPrompt::Starship),
    ("starship-nerd-font", ShellPrompt::StarshipNerdFont),
] {
    let manifest = load(&format!("version = 1\n[shell]\nprompt = '{value}'\n"))?;
    assert_eq!(manifest.shell().prompt(), expected);
}
assert!(load("version = 1\n[shell]\nprompt = 'spaceship'\n")
    .unwrap_err().to_string().contains("unknown variant"));
assert!(load("version = 1\n[shell]\ncommand = 'bash'\n")
    .unwrap_err().to_string().contains("unknown field `command`"));
```

- [ ] **Step 2: Run the focused tests and confirm RED**

Run: `rtk cargo test -p gascan-core --test manifest shell_prompt -- --nocapture`

Expected: FAIL because `ShellPrompt` and `Manifest::shell` do not exist.

- [ ] **Step 3: Implement the closed manifest model**

Add the validated public types and raw default:

```rust
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ShellPrompt {
    #[default]
    Standard,
    Starship,
    StarshipNerdFont,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub struct Shell {
    prompt: ShellPrompt,
}

impl Shell {
    pub const fn prompt(&self) -> ShellPrompt { self.prompt }
}

impl ShellPrompt {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Starship => "starship",
            Self::StarshipNerdFont => "starship-nerd-font",
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawShell {
    #[serde(default)]
    prompt: ShellPrompt,
}
```

Add `shell: Shell` to `Manifest`, `shell: RawShell` to `RawManifest`, populate
defaults, validate it without string interpolation, and expose
`pub const fn shell(&self) -> &Shell`.

- [ ] **Step 4: Run focused and crate tests**

Run: `rtk cargo test -p gascan-core --test manifest`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
rtk git add crates/gascan-core/src/manifest.rs crates/gascan-core/tests/manifest.rs
rtk git commit -m "feat: add managed shell prompt configuration"
```

---

### Task 2: Make the default Shell RPC launch login Bash

**Files:**
- Modify: `crates/gascand/src/api.rs`
- Modify: `crates/gascan-e2e/tests/apple_lifecycle.rs`
- Modify: `crates/gascan-e2e/tests/fake_backend.rs`

**Interfaces:**
- Consumes: `v1::ShellRequest.command.argv`
- Produces: default pending-session argv `vec!["/bin/bash", "--login"]`
- Preserves: explicit command argv and existing TTY attachment protocol

- [ ] **Step 1: Write failing API tests for default and explicit argv**

Capture the `ExecRequest` observed by the fake runtime:

```rust
assert_eq!(default_exec.argv, ["/bin/bash", "--login"]);
assert!(default_exec.tty);
assert_eq!(
    explicit_exec.argv,
    ["bash", "--noprofile", "--norc", "-c", "printf explicit"]
);
```

Retain the existing exit-status, resize, signal, and EOF assertions.

- [ ] **Step 2: Run the focused tests and confirm RED**

Run: `rtk cargo test -p gascand shell -- --nocapture`

Expected: FAIL with the observed default argv `["sh"]`.

- [ ] **Step 3: Replace only the empty-argv default**

In the Shell RPC:

```rust
let argv = if command.argv.is_empty() {
    vec!["/bin/bash".to_owned(), "--login".to_owned()]
} else {
    argv_from_wire(command.argv).map_err(ApiInputError::status)?
};
```

Do not change Run requests, CLI argument parsing, terminal raw mode, or the
attach helper.

- [ ] **Step 4: Update lifecycle assertions and run the affected suites**

Run:

```bash
rtk cargo test -p gascand shell
rtk cargo test -p gascan-e2e --test fake_backend shell
```

Expected: PASS. Keep Apple live tests ignored in the ordinary suite.

- [ ] **Step 5: Commit**

```bash
rtk git add crates/gascand/src/api.rs crates/gascand/tests crates/gascan-e2e/tests/apple_lifecycle.rs crates/gascan-e2e/tests/fake_backend.rs
rtk git commit -m "fix: open login bash from gascan shell"
```

---

### Task 3: Plan and present shell configuration as durable provisioning state

**Files:**
- Modify: `crates/gascan-core/src/provision.rs`
- Modify: `crates/gascan-core/tests/provision_plan.rs`
- Modify: `proto/gascan/v1/gascan.proto`
- Modify: `crates/gascan-proto/tests/api_compatibility.rs`
- Modify: `crates/gascand/src/api.rs`
- Modify: `crates/gascan/src/presentation.rs`

**Interfaces:**
- Extends: `AppliedState` with `shell_hash: Option<String>`
- Produces: `ProvisionStep::ConfigureShell` / protobuf numeric value `7`
- Produces: `ProvisionPlan::{shell_changed, desired_shell_hash}`
- Produces: `ProvisionPlan::shell_prompt() -> ShellPrompt`
- Consumes: `Manifest::shell().prompt()`

- [ ] **Step 1: Write failing planner and presentation tests**

Use a versioned domain-separated hash:

```rust
let standard = plan("version = 1\n", AppliedState::empty())?;
assert!(standard.shell_changed());
assert!(standard.steps().contains(&ProvisionStep::ConfigureShell));

let applied = AppliedState::with_hashes(
    Some(standard.desired_tool_hash().to_owned()),
    None,
    Some(standard.desired_shell_hash().to_owned()),
);
let unchanged = plan("version = 1\n", applied)?;
assert!(!unchanged.shell_changed());

let nerd = plan(
    "version = 1\n[shell]\nprompt = 'starship-nerd-font'\n",
    AppliedState::empty(),
)?;
assert_ne!(standard.desired_shell_hash(), nerd.desired_shell_hash());
assert_eq!(nerd.shell_prompt(), ShellPrompt::StarshipNerdFont);
```

Add presentation coverage expecting `Configuring interactive shell` for the
new protobuf step.

- [ ] **Step 2: Run focused tests and confirm RED**

Run:

```bash
rtk cargo test -p gascan-core --test provision_plan
rtk cargo test -p gascan presentation::tests
rtk cargo test -p gascan-proto --test api_compatibility
```

Expected: FAIL because the new state, step, and enum mapping are absent.

- [ ] **Step 3: Implement the planner contract**

Compute the shell hash independently from tool state:

```rust
const MANAGED_SHELL_CONFIG_VERSION: &str = "gascan-managed-shell-v1";

fn desired_shell_hash(prompt: ShellPrompt) -> String {
    let mut hash = Sha256::new();
    hash.update(MANAGED_SHELL_CONFIG_VERSION.as_bytes());
    hash.update([0]);
    hash.update(match prompt {
        ShellPrompt::Standard => b"standard".as_slice(),
        ShellPrompt::Starship => b"starship".as_slice(),
        ShellPrompt::StarshipNerdFont => b"starship-nerd-font".as_slice(),
    });
    format!("sha256:{:x}", hash.finalize())
}
```

Insert `ConfigureShell` after tool installation and before `RunSetup`. Add
protobuf value `PROVISION_STEP_CONFIGURE_SHELL = 7` without renumbering any
existing value. Map the daemon event and render the human message.

- [ ] **Step 4: Run all affected tests**

Run:

```bash
rtk cargo test -p gascan-core --test provision_plan
rtk cargo test -p gascan-proto --test api_compatibility
rtk cargo test -p gascand api
rtk cargo test -p gascan presentation::tests
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
rtk git add crates/gascan-core/src/provision.rs crates/gascan-core/tests/provision_plan.rs proto/gascan/v1/gascan.proto crates/gascan-proto/tests/api_compatibility.rs crates/gascand/src/api.rs crates/gascan/src/presentation.rs
rtk git commit -m "feat: plan managed shell configuration"
```

---

### Task 4: Add reviewed Bash completion and pinned Starship 1.25.1 inputs

**Files:**
- Modify: `tests/image/system-tools.txt`
- Modify: `images/workspace/versions.toml`
- Modify: `images/workspace/versions.lock`
- Modify: `scripts/src/bin/update-image-lock.rs`
- Modify: `images/workspace/bin/install-workstation-artifacts`
- Modify: `scripts/prefetch-connected-workspace-image.sh`
- Modify: `scripts/tests/tool_versions.rs`
- Modify: `scripts/tests/image_lock.rs`
- Modify: `scripts/tests/connected_image_build.rs`
- Modify: `scripts/tests/connected_workspace_context.rs`
- Modify: `scripts/tests/connected_dockerfile.rs`

**Interfaces:**
- Produces: locked Starship `1.25.1` Linux ARM64 musl tarball metadata
- Produces: immutable `/opt/gascan/workstation/bin/starship`
- Produces later stable link target: `/opt/gascan/shell/bin/starship`
- Consumes: existing GitHub release resolver, redirect policy, digest checks, bounded artifact extraction

- [ ] **Step 1: Write failing lock, package, and installer tests**

Require `bash-completion` exactly once in the sorted package list. Extend the
workstation intent/lock allowlists with `starship`, and require:

```rust
assert_eq!(intent["starship"].as_str(), Some("1.25.1"));
assert_eq!(artifact.platform, "linux-arm64");
assert_eq!(artifact.kind, "tar_gz");
assert!(artifact.url.ends_with(
    "/starship-aarch64-unknown-linux-musl.tar.gz"
));
```

Add behavioral installer fixtures whose archive contains exactly one ARM64 ELF
named `starship`; reject duplicate commands, escaping links, wrong ELF
architecture, extra executable candidates, size mismatch, and digest mismatch.

- [ ] **Step 2: Run the scripts tests and confirm RED**

Run:

```bash
rtk cargo test --manifest-path scripts/Cargo.toml --test tool_versions
rtk cargo test --manifest-path scripts/Cargo.toml --test image_lock
rtk cargo test --manifest-path scripts/Cargo.toml --test connected_dockerfile workstation
```

Expected: FAIL because neither reviewed input exists.

- [ ] **Step 3: Extend the reviewed artifact pipeline**

Add `starship = "1.25.1"` to `[workstation]`. Resolve official
`starship/starship` tag `v1.25.1` and the exact
`starship-aarch64-unknown-linux-musl.tar.gz` asset through the existing GitHub
client. Store its release-provided SHA-256, size, platform, and kind in
`versions.lock`. Extend every exact workstation allowlist and bounded size map;
do not add a generic arbitrary-repository path.

Teach the installer to select the sole `starship` file, validate it as ARM64
ELF, install it immutably, and accept only:

```python
"starship": output == f"starship {version}"
```

- [ ] **Step 4: Regenerate and verify reviewed lock/cache inputs**

Run:

```bash
rtk cargo run --manifest-path scripts/Cargo.toml --bin update-image-lock
rtk cargo run --manifest-path scripts/Cargo.toml --bin update-image-lock -- --verify-existing-workstation-lock
```

Expected: PASS with Starship 1.25.1 locked. Inspect `rtk git diff
images/workspace/versions.lock`; explain and separately review any unrelated
tool version movement before retaining it.

- [ ] **Step 5: Run the artifact and context contract suites**

Run:

```bash
rtk cargo test --manifest-path scripts/Cargo.toml --test tool_versions
rtk cargo test --manifest-path scripts/Cargo.toml --test image_lock
rtk cargo test --manifest-path scripts/Cargo.toml --test connected_image_build
rtk cargo test --manifest-path scripts/Cargo.toml --test connected_workspace_context
rtk cargo test --manifest-path scripts/Cargo.toml --test connected_dockerfile
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
rtk git add tests/image/system-tools.txt images/workspace/versions.toml images/workspace/versions.lock scripts/src/bin/update-image-lock.rs images/workspace/bin/install-workstation-artifacts scripts/prefetch-connected-workspace-image.sh scripts/tests
rtk git commit -m "build: bundle bash completion and starship"
```

---

### Task 5: Build the immutable shell hook, presets, and fail-closed configurator

**Files:**
- Create: `images/workspace/etc/gascan/bashrc`
- Create: `images/workspace/etc/gascan/starship.toml`
- Create: `images/workspace/etc/gascan/starship-nerd-font.toml`
- Create: `images/workspace/bin/configure-shell-home`
- Modify: `images/workspace/Dockerfile`
- Modify: `scripts/tests/image_user_contract.rs`
- Modify: `scripts/tests/connected_dockerfile.rs`
- Modify: `scripts/tests/connected_workspace_context.rs`
- Modify: `tests/image/workstation-contract.sh`

**Interfaces:**
- Produces: root-only `/usr/local/bin/configure-shell-home PROMPT`
- Produces: `/etc/gascan/bashrc`
- Produces: `/opt/gascan/shell/presets/{starship,starship-nerd-font}.toml`
- Produces: `/opt/gascan/shell/bin/starship`
- Consumes: exact prompt strings and locked workstation Starship binary

- [ ] **Step 1: Write failing behavioral contracts**

Create a disposable real-Linux root-container contract and host hook fixtures.
Assert:

```rust
assert_eq!(owner(shell_dir), "root:workspace");
assert_eq!(mode(shell_dir), 0o750);
assert_eq!(owner(selector), "root:workspace");
assert_eq!(mode(selector), 0o640);
assert_eq!(read(selector), "starship-nerd-font\n");
assert_eq!(read(config), read(immutable_nerd_preset));
```

Exercise initial standard configuration, enable, switch, disable, retry after
an exact staging file, and rejection of selector/config/directory symlinks,
FIFOs, wrong owners, permissive modes, and unexpected files. Add shell-hook
tests proving non-interactive no-op, standard no-op, exact binary invocation,
root-vs-workspace `STARSHIP_CONFIG` selection, byte-exact selector validation,
direct full-init generation, immutable runtime executable selection, and
one-warning restoration after generation or partial evaluation failure.
Use the production relative symlink topology and prove its exact target,
ownership, and mode are validated while hostile `PATH` remains unused. Prove
full init executes exactly once, a partial failure leaks no prompt,
`PROMPT_COMMAND`, function, variable, or DEBUG-trap state, and pre-existing
Starship environment is restored exactly, including set/unset and
exported/unexported attributes. Add defense-in-depth collisions for every
managed function and
internal variable, readonly config/executable/PATH and prompt-hook variables,
an inherited DEBUG trap that mutates evaluator state, a readonly managed
function introduced after preflight, a writable managed function introduced
after preflight, a DEBUG trap that creates a spoofing `trap()` function, a
self-clearing DEBUG trap that leaves a managed-state collision, and a failed
init command followed by a successful final command. Prove an inherited trap
visible to the hook is captured and manipulated through `builtin trap` and
preserved exactly on fallback. Acknowledge that a sourced hook cannot
authenticate a self-clearing trap that mutates the shell before its first
instruction. Verify the defense-in-depth function and variable comparisons and
rollback behavior without treating them as isolation from adversarial
same-shell state. Prove supported existing `PROMPT_COMMAND` customization is
preserved through pinned Starship's `STARSHIP_PROMPT_COMMAND` path. Prove
rejected cases warn once and leave subsequent Bash commands usable. Prove the
root-owned mode `0600` advisory lock serializes concurrent writers and that the
workspace account can read but cannot mutate or race managed state.

- [ ] **Step 2: Run focused contracts and confirm RED**

Run:

```bash
rtk cargo test --manifest-path scripts/Cargo.toml --test image_user_contract shell
rtk cargo test --manifest-path scripts/Cargo.toml --test connected_dockerfile shell
```

Expected: FAIL because the files and Dockerfile wiring are absent.

- [ ] **Step 3: Implement the configurator and immutable assets**

`configure-shell-home` must accept only:

```python
PROMPTS = {
    "standard": None,
    "starship": "starship.toml",
    "starship-nerd-font": "starship-nerd-font.toml",
}
```

Require effective root, exactly one prompt argument, and the fixed workspace
identity. Compile the target home as `/home/workspace` and ignore inherited
`HOME`, since the exact production sudo invocation resets it to `/root`. Use
directory file descriptors, `lstat`, `O_NOFOLLOW`, exact
UID/GID/mode/link checks, a root-owned transaction lock, reserved
same-directory staging names, parent-directory `fsync`, and atomic `rename`.
The shell directory is `root:workspace` mode `0750`; selector, generated
configuration, and staging files are `root:workspace` mode `0640`. Copy only
the selected immutable preset; standard removes only a verified managed config
and publishes `standard\n`.

The Bash hook begins with:

```bash
case $- in *i*) ;; *) return 0 2>/dev/null || exit 0;; esac
```

It validates the selector byte-for-byte, leaves standard untouched, sets a
workspace shell's `STARSHIP_CONFIG` to the root-owned managed regular file,
and sets a root shell's `STARSHIP_CONFIG` to the immutable preset directly.
Under an immutable-only `PATH`, it exports the pinned
`STARSHIP_EXECUTABLE` and obtains the complete initialization with:

```bash
/opt/gascan/shell/bin/starship init bash --print-full-init
```

The stable path is the exact root-owned relative symlink to the immutable
root-owned workstation binary; validate both the link and its target. Evaluate
the captured full init exactly once in an inherited subshell with effective
`errexit`. At hook entry, capture any visible inherited DEBUG trap through
`builtin trap`; if one is visible, fail closed before validation or
initialization and preserve it exactly. This is a defense-in-depth
compatibility check: the sourced hook cannot authenticate a self-clearing
DEBUG trap or other same-authority mutation that runs before its first
instruction. Use `builtin trap` for every later trap read or manipulation.
Before generation, reject readonly live state and every
pre-existing managed function/internal-variable collision on the exact pinned
1.25.1 surface. Pass config, executable, and immutable `PATH` only through
child environments until validation succeeds. On success, serialize only the
explicit allowlist, snapshot the same live surface, syntax-check and dry-run
the declaration commit, and guard every applied operation. Immediately before
each managed function declaration, compare that it remains absent; immediately
before each managed variable write, compare its exact preflight declaration,
including set/unset and attributes. Never serialize inherited user Starship
definitions and never evaluate full init a second time. Roll back the snapshot
if parent apply unexpectedly fails. Preserve compatible caller customization
that pinned Starship supports: a writable existing `PROMPT_COMMAND` is stored
as `STARSHIP_PROMPT_COMMAND` and executed by `starship_precmd`. If validation,
visible inherited DEBUG state, collision preflight, generation, isolated
evaluation, compare-before-write, or guarded apply fails, do not leave partial
Gas Can initialization state, print
`gascan: Starship prompt unavailable; using standard Bash prompt.` once and
return success.

Wire the image to set `ENV SHELL=/bin/bash`, append the immutable hook at the
end of the image-provided workspace and root login startup chains, copy the
presets/configurator, and create the stable Starship link without making
`/opt/gascan` writable.

- [ ] **Step 4: Add restrained preset contents**

Both TOML files include hostname/sandbox identity, directory, Git branch and
status, command status and duration, plus the supported runtime modules. The
compatible file uses no Private Use Area code points. The Nerd Font file may
use Nerd Font glyphs. Snapshot the exact module order and scan:

```rust
assert!(!compatible.chars().any(|c| ('\u{e000}'..='\u{f8ff}').contains(&c)));
assert!(nerd.chars().any(|c| ('\u{e000}'..='\u{f8ff}').contains(&c)));
```

- [ ] **Step 5: Run image contracts**

Run:

```bash
rtk cargo test --manifest-path scripts/Cargo.toml --test image_user_contract
rtk cargo test --manifest-path scripts/Cargo.toml --test connected_dockerfile
rtk cargo test --manifest-path scripts/Cargo.toml --test connected_workspace_context
rtk bash -n images/workspace/bin/configure-shell-home
rtk container run --rm --network none --user root ... \
  /source/tests/image/shell-home-root-contract.sh /source
```

The Linux contract must invoke the configurator from the workspace account
through the exact Task 6 argv, assert sudo actually resets `HOME` to `/root`,
and prove the fixed `/home/workspace` target still succeeds.

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
rtk git add images/workspace/etc/gascan images/workspace/bin/configure-shell-home images/workspace/Dockerfile scripts/tests tests/image/workstation-contract.sh
rtk git commit -m "feat: add managed bash and starship experience"
```

---

### Task 6: Apply shell selection safely and persist its resolution

**Files:**
- Modify: `crates/gascand/src/service.rs`
- Modify: `crates/gascand/tests/apply_tools.rs`
- Modify: `crates/gascand/tests/apply_setup.rs`
- Modify: `crates/gascand/tests/lifecycle.rs`
- Modify: `crates/gascan-core/src/fake_runtime.rs`

**Interfaces:**
- Consumes: `ProvisionPlan::{shell_changed, shell_prompt, desired_shell_hash}`
- Invokes: `/usr/bin/sudo -n /usr/local/bin/configure-shell-home <validated-prompt>`
- Persists: `shell_hash` beside existing durable provisioning resolution
- Produces: apply-required reason `shell_changed`

- [ ] **Step 1: Write failing service tests**

Assert initial standard configuration, no-op matching state, compatible enable,
Nerd Font switch, disable, and retry:

```rust
assert_exec(
    &calls,
    [
        "/usr/bin/sudo",
        "-n",
        "/usr/local/bin/configure-shell-home",
        "starship",
    ]
);
assert_eq!(after["shell_hash"], desired_shell_hash);
assert!(events.iter().any(|event|
    event["step"] == "configure_shell"
));
```

Inject configurator failure and prove prior applied `shell_hash` remains,
health/setup do not run past the failed boundary, and retry succeeds. Prove a
shell-only change never calls `mise install` or the project setup script.

- [ ] **Step 2: Run focused service tests and confirm RED**

Run:

```bash
rtk cargo test -p gascand --test apply_tools shell
rtk cargo test -p gascand --test apply_setup shell
rtk cargo test -p gascand --test lifecycle shell
```

Expected: FAIL because the daemon does not execute or persist the shell step.

- [ ] **Step 3: Implement the service boundary**

Extend `ProvisionedResolution` and `AppliedState` plumbing with
`shell_hash`. Before setup and Gascamp verification:

```rust
if plan.shell_changed() {
    self.emit_provision_step(
        operation_id,
        ProvisionStep::ConfigureShell,
        sender,
    ).await?;
    self.exec_guest(
        spec.id(),
        ProvisionStep::ConfigureShell,
        "configure_shell",
        [
            "/usr/bin/sudo",
            "-n",
            "/usr/local/bin/configure-shell-home",
            plan.shell_prompt().as_str(),
        ],
        Vec::new(),
    ).await?;
}
```

Include `desired_shell_hash` in the desired fingerprint, `after_provision`
event, stored durable resolution, replacement applied state, and matching
logic. Prefer `shell_changed` over generic `desired_content_changed` when
choosing the apply-required reason.

- [ ] **Step 4: Run daemon and core regression suites**

Run:

```bash
rtk cargo test -p gascan-core
rtk cargo test -p gascand --test apply_tools
rtk cargo test -p gascand --test apply_setup
rtk cargo test -p gascand --test lifecycle
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
rtk git add crates/gascand/src/service.rs crates/gascand/tests crates/gascan-core/src/fake_runtime.rs
rtk git commit -m "feat: apply managed shell prompts"
```

---

### Task 7: Add end-to-end shell parity, offline, and release-smoke coverage

**Files:**
- Modify: `crates/gascan-e2e/tests/apple_lifecycle.rs`
- Modify: `crates/gascan-e2e/tests/apple_apply.rs`
- Modify: `crates/gascan-e2e/tests/apple_common/mod.rs`
- Modify: `packaging/macos/release-smoke.sh`
- Modify: `tests/release/release-smoke-contract.sh`

**Interfaces:**
- Consumes: built approved workspace image and native SSH fixture
- Proves: Gas Can shell/SSH parity, completion, both prompt modes, offline activation

- [ ] **Step 1: Write failing static release and E2E assertions**

The release contract requires checks for:

```text
BASH_VERSION
/usr/share/bash-completion/bash_completion
/opt/gascan/shell/bin/starship --version
gascan shell default argv behavior
standard selector
starship selector
```

Add PTY helpers that wait for a deterministic test marker emitted after shell
startup rather than snapshotting terminal escape timing.

- [ ] **Step 2: Run static tests and confirm RED**

Run:

```bash
rtk cargo test -p gascan-e2e --test fake_backend
rtk bash tests/release/release-smoke-contract.sh
```

Expected: FAIL because smoke and live assertions are not wired.

- [ ] **Step 3: Implement deterministic live coverage**

For standard mode, assert interactive/login Bash flags, `SHELL=/bin/bash`,
readable Bash completion, forwarded `TERM`, clean exit, and unchanged explicit
argv. Apply each Starship mode and compare the selector/config identity seen
through `gascan shell` and SSH. Run the compatible preset in an offline
sandbox and prove no network command or download occurs.

Keep Apple tests ignored behind the existing live gate; do not add a new
always-on Apple dependency.

- [ ] **Step 4: Run non-live E2E and release contracts**

Run:

```bash
rtk cargo test -p gascan-e2e --test fake_backend
rtk bash tests/release/release-smoke-contract.sh
rtk bash -n packaging/macos/release-smoke.sh
```

Expected: PASS.

- [ ] **Step 5: Run the scoped Apple live tests with the approved image**

Run:

```bash
rtk bash ./scripts/run-apple-e2e.sh apple_lifecycle
rtk bash ./scripts/run-apple-e2e.sh apple_apply
```

Expected: PASS, including both entry methods. Preserve and report the known
Apple custom-network teardown defect rather than changing network lifecycle.

- [ ] **Step 6: Commit**

```bash
rtk git add crates/gascan-e2e/tests packaging/macos/release-smoke.sh tests/release/release-smoke-contract.sh
rtk git commit -m "test: cover native shell and managed prompts"
```

---

### Task 8: Document the native shell and complete prompt configuration

**Files:**
- Modify: `README.md`
- Modify: `packaging/macos/default-gascan.toml`
- Create: `tests/release/documentation-contract.sh`

**Interfaces:**
- Documents: default login Bash, completion, prompt values, SSH parity, font prerequisite, explicit argv escape hatch

- [ ] **Step 1: Write failing documentation contract assertions**

Create `tests/release/documentation-contract.sh` with focused fixed-string
assertions over the README and packaged default manifest, requiring these exact
examples:

```toml
[shell]
prompt = "standard"
# prompt = "starship"
# prompt = "starship-nerd-font"
```

Require the quick start to explain that `gascan shell` opens interactive login
Bash with colors and completion, and that Nerd Font installation occurs on the
host terminal, not in the sandbox.

- [ ] **Step 2: Run the documentation contract and confirm RED**

Run: `rtk bash tests/release/documentation-contract.sh`

Expected: FAIL because `[shell]` is undocumented.

- [ ] **Step 3: Update quick start, full manifest, and shell reference**

Document:

- `standard` is backward-compatible and does not activate Starship;
- both Starship modes use Gas Can's pinned offline-capable binary;
- `starship` needs no special font;
- `starship-nerd-font` requires a Nerd Font selected in the macOS terminal;
- the setting affects `gascan shell` and SSH;
- changes take effect through `gascan apply`;
- `gascan shell -- <argv>` bypasses the managed default executable.

- [ ] **Step 4: Run documentation and formatting checks**

Run:

```bash
rtk bash tests/release/documentation-contract.sh
rtk git diff --check
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
rtk git add README.md packaging/macos/default-gascan.toml tests/release
rtk git commit -m "docs: explain native shell and starship prompts"
```

---

### Task 9: Full verification and review handoff

**Files:**
- Verify only; change files only to fix a demonstrated regression

**Interfaces:**
- Consumes: all prior task commits
- Produces: reviewable branch with complete evidence

- [ ] **Step 1: Run formatting and static checks**

Run:

```bash
rtk cargo fmt --all -- --check
rtk cargo clippy --workspace --all-targets -- -D warnings
rtk git diff --check origin/main...HEAD
```

Expected: PASS.

- [ ] **Step 2: Run the complete Rust and scripts suites**

Run:

```bash
rtk cargo test --workspace
rtk cargo test --manifest-path scripts/Cargo.toml
```

Expected: PASS with only the repository's explicitly ignored live cases.

- [ ] **Step 3: Run every release contract**

Run:

```bash
rtk bash -c 'for contract in tests/release/*-contract.sh; do bash "$contract"; done'
```

Expected: every contract exits zero.

- [ ] **Step 4: Inspect scope and history**

Run:

```bash
rtk git status --short
rtk git diff --stat origin/main...HEAD
rtk git log --oneline origin/main..HEAD
```

Expected: clean worktree; only native-shell, managed-Starship, tests,
documentation, and required reviewed image-input changes.

- [ ] **Step 5: Request two-stage review**

Use `superpowers:requesting-code-review`. First verify spec compliance against
`docs/superpowers/specs/2026-07-27-native-shell-starship-design.md`, then
perform code-quality review. Resolve every finding with a new failing test
before changing implementation.
