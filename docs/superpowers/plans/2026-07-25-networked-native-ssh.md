# Networked Native SSH Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give networked Gas Can sandboxes secure native loopback SSH, polished `gascan ssh`, and VS Code Remote SSH aliases without a daemon TCP relay.

**Architecture:** Apple Container publishes guest port 22 directly on host IPv4 loopback. Gas Can owns the Ed25519 client identity, persistent guest host identity, strict OpenSSH trust/config generations, lifecycle readiness, status, and CLI integration. Offline sandboxes do not receive SSH and explicitly enabling SSH for one is rejected.

**Tech Stack:** Rust, Tokio, Apple Container structured JSON, OpenSSH client/server, SQLite resolution records, tonic/protobuf, Clap, shell image contracts, live Apple acceptance.

**Prerequisites already committed on this branch:**

- Validated SSH manifest/control-plane types: `efcf206`.
- SSH identity persistence schema and status types: `2a960fb`, `0acdc70`.
- Locked-down guest OpenSSH service and persistent Ed25519 host key: `7412541`, `3f3a836`.
- Initial host identity/config implementation: `943cf1e`.
- Authoritative reduced design: `8277bb0`.

**Design:** `docs/superpowers/specs/2026-07-25-networked-native-ssh-design.md`

## Global Constraints

- Networked sandboxes enable SSH by default; offline sandboxes disable it by default.
- An offline manifest with explicit `ssh.enabled = true` is rejected. Gas Can never silently changes network mode.
- Native SSH publication is exactly IPv4 `127.0.0.1:<host-port>:22`; never wildcard, LAN, IPv6, or an offline runtime port.
- Explicit ports are used exactly and never fall back. Automatic selection retries detected creation collisions within a fixed bound.
- The guest accepts only the generated Ed25519 public key. Private key bytes never enter the guest, generated config, logs, errors, events, or JSON.
- Passwords, keyboard-interactive login, root login, agent forwarding, remote forwarding, host credential import, and host-key bypass remain prohibited.
- Reusable aliases permit local and dynamic forwarding to guest loopback for VS Code.
- Generated trust uses immutable known-host generations and an atomic config commit point.
- JSON and noninteractive commands never prompt or mutate `~/.ssh/config`.
- All subprocess arguments are discrete strings; no shell command construction.
- The custom `SshBridge`, exec-to-`nc` transport, listener registry, and offline SSH acceptance do not exist.
- Until Task 3 switches the daemon to the control-plane compiler, legacy
  `PolicyCompiler::compile` and `compile_for_image` calls without SSH control
  input emit `GASCAN_SSH_ENABLED=0` and no SSH port. Only the
  `*_with_control_plane` APIs materialize enabled SSH. This transition keeps
  every intermediate commit testable and is not used by the final daemon.

---

### Task 1: Resolve Networked-Only SSH and Native Runtime Ports

**Files:**

- Modify: `crates/gascan-core/src/manifest.rs`
- Modify: `crates/gascan-core/src/policy.rs`
- Modify: `crates/gascan-core/src/runtime.rs`
- Modify: `crates/gascan-core/tests/manifest.rs`
- Modify: `crates/gascan-core/tests/policy.rs`
- Modify: `crates/gascan-apple/src/inspect.rs`
- Modify: `crates/gascan-apple/src/translate.rs`
- Modify: `crates/gascan-apple/tests/inspect.rs`
- Modify: `crates/gascan-apple/tests/translate.rs`
- Modify: `crates/gascan-apple/tests/fixtures/container-running-1.0.json`

**Interfaces:**

```rust
pub struct ControlPlanePolicy<'a> {
    pub ssh_authorized_key: Option<&'a str>,
    pub ssh_host_port: Option<u16>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RuntimePort {
    pub host_address: IpAddr,
    pub host_port: u16,
    pub guest_port: u16,
}

pub struct RuntimeSandbox {
    // existing fields
    pub(crate) ports: Vec<RuntimePort>,
}

impl RuntimeSandbox {
    pub fn ports(&self) -> &[RuntimePort];
}
```

- [ ] **Step 1: Write failing manifest-resolution tests**

Add literal fixtures proving:

```rust
// absent [ssh]
("network = 'networked'", true),
("network = 'offline'", false),

// explicit
("network = 'networked'\n[ssh]\nenabled = false", false),
("network = 'networked'\n[ssh]\nenabled = true\nhost_port = 2222", true),
```

Require `network = "offline"` plus `[ssh]\nenabled = true` to return:

```text
ssh requires network = "networked"; disable SSH or enable sandbox networking
```

Retain rejection of unknown fields, ports below 1024, and a host port while disabled.

- [ ] **Step 2: Run manifest tests and verify RED**

```bash
rtk cargo test -p gascan-core --test manifest ssh
```

Expected: the absent offline case is still enabled and explicit offline SSH is still accepted.

- [ ] **Step 3: Implement network-dependent manifest resolution**

Represent raw enablement as `Option<bool>` so absence remains distinguishable from explicit `true`. Resolve:

```rust
let enabled = raw_ssh.enabled.unwrap_or(network == NetworkMode::Networked);
```

Validate explicit offline `true` before constructing the sealed `Ssh`.

- [ ] **Step 4: Write failing native-policy and translation tests**

Require `compile_for_image_with_control_plane` with networked SSH, a valid public key, and host port `22222` to produce exactly one additional internal port:

```rust
RuntimePort {
    host_address: IpAddr::V4(Ipv4Addr::LOCALHOST),
    host_port: 22222,
    guest_port: 22,
}
```

Prove:

- a disabled/offline request emits `GASCAN_SSH_ENABLED=0`, no authorized key, and no SSH port;
- an enabled request requires both an authorized key and host port;
- legacy compiler calls without control-plane input remain SSH-disabled until
  Task 3 replaces the daemon call site;
- user application ports remain unchanged and cannot claim the selected SSH host port;
- Apple translation emits `--publish 127.0.0.1:22222:22`;
- non-loopback SSH ports remain impossible.

- [ ] **Step 5: Add runtime observation tests for `publishedPorts`**

Extend the structured inspect fixture with:

```json
"publishedPorts": [
  {
    "hostAddress": "127.0.0.1",
    "hostPort": 22222,
    "containerPort": 22,
    "protocol": "tcp"
  }
]
```

Require `AppleInspector::inspect` to return the exact `RuntimePort`. Reject UDP, wildcard/non-loopback addresses, zero ports, duplicate mappings, and malformed port values as structured runtime output errors.

- [ ] **Step 6: Run policy and Apple tests and verify RED**

```bash
rtk cargo test -p gascan-core --test policy ssh
rtk cargo test -p gascan-apple --test translate
rtk cargo test -p gascan-apple --test inspect
```

Expected: missing control-plane port handling and missing inspected port mappings fail.

- [ ] **Step 7: Implement internal native port compilation and inspection**

Compile application ports first, reject a collision with the SSH host port, and append the SSH mapping only when both validated control-plane fields exist. Extend `RuntimeSandbox` and Apple structured inspection with the exact `publishedPorts` schema. Preserve current immutable-image, ownership, and state validation.

- [ ] **Step 8: Run focused and regression suites**

```bash
rtk cargo test -p gascan-core --test manifest ssh
rtk cargo test -p gascan-core --test policy ssh
rtk cargo test -p gascan-core --test backend_contract
rtk cargo test -p gascan-apple --test translate
rtk cargo test -p gascan-apple --test inspect
rtk cargo clippy -p gascan-core -p gascan-apple --all-targets -- -D warnings
rtk cargo fmt --all -- --check
rtk git diff --check
```

- [ ] **Step 9: Commit**

```bash
rtk git add crates/gascan-core/src/manifest.rs crates/gascan-core/src/policy.rs \
  crates/gascan-core/src/runtime.rs crates/gascan-core/tests/manifest.rs \
  crates/gascan-core/tests/policy.rs crates/gascan-apple/src/inspect.rs \
  crates/gascan-apple/src/translate.rs crates/gascan-apple/tests/inspect.rs \
  crates/gascan-apple/tests/translate.rs \
  crates/gascan-apple/tests/fixtures/container-running-1.0.json
rtk git commit -m "feat: publish networked sandbox SSH natively"
```

### Task 2: Finish Generation-Consistent Host Trust

**Files:**

- Modify: `crates/gascand/src/ssh.rs`
- Modify: `crates/gascand/src/ssh/identity.rs`
- Modify: `crates/gascand/src/ssh/config.rs`
- Modify: `crates/gascand/tests/ssh_identity.rs`
- Modify: `crates/gascand/tests/ssh_config.rs`

**Interfaces:**

```rust
pub struct HostIdentity {
    private_key: Utf8PathBuf,
    public_key: String,
    fingerprint: String,
}

impl HostIdentity {
    pub fn private_key(&self) -> &Utf8Path;
    pub fn public_key(&self) -> &str;
    pub fn fingerprint(&self) -> &str;
}

pub struct PreparedSshFiles {
    generation: String,
    known_hosts: Utf8PathBuf,
    config: Vec<u8>,
}

impl PreparedSshFiles {
    pub fn generation(&self) -> &str;
    pub fn known_hosts(&self) -> &Utf8Path;
}

pub fn prepare_openssh_files(
    paths: &SshPaths,
    identity: &HostIdentity,
    hosts: &[ManagedSshHost],
) -> Result<PreparedSshFiles, SshError>;

pub fn commit_openssh_files(
    paths: &SshPaths,
    prepared: PreparedSshFiles,
) -> Result<(), SshError>;

pub fn readiness_ssh_args(
    paths: &SshPaths,
    identity: &HostIdentity,
    host: &ManagedSshHost,
    generation_known_hosts: &Utf8Path,
) -> Result<Vec<OsString>, SshError>;
```

- [ ] **Step 1: Write failing tests for every open Task 4 Important finding**

Add tests proving:

1. Injected failure before config commit leaves the previous config pointing to its previous immutable known-host generation.
2. Config-target rejection occurs before any active generation changes.
3. Readiness for an unpublished host still passes explicit discrete `HostName=127.0.0.1`, port, user, identity, `HostKeyAlias`, generation known-hosts, `StrictHostKeyChecking=yes`, `IdentitiesOnly=yes`, `BatchMode=yes`, `ForwardAgent=no`, and `ClearAllForwardings=yes`.
4. XDG/HOME paths containing `$` are rejected before OpenSSH rendering.
5. A fabricated `HostIdentity` cannot be constructed outside the identity module, and publication reloads/revalidates the managed on-disk pair.
6. Replacing the private-key pathname after its descriptor is opened cannot affect `ssh-keygen -y` validation on macOS.

- [ ] **Step 2: Run focused tests and verify RED**

```bash
rtk cargo test -p gascand --test ssh_identity
rtk cargo test -p gascand --test ssh_config
```

Expected: mixed-generation publication, alias-only readiness, `$` path expansion, public identity fields, and pathname reopening remain.

- [ ] **Step 3: Implement immutable generations and descriptor inheritance**

Use generation names derived from the complete rendered known-host bytes:

```text
known_hosts.<lowercase sha256>
```

Create a missing generation with no-replace semantics, verify an existing generation byte-for-byte, then stage config referencing that immutable path. The final config rename is the only publication commit point. Keep the prior generation; bounded cleanup may delete only generations not referenced by either the old or newly committed config.

Make `HostIdentity` fields private. Revalidate its public/fingerprint values against the managed on-disk pair before preparation.

For `ssh-keygen -y`, duplicate the already-open private descriptor, deliberately clear `FD_CLOEXEC` only on that child descriptor, pass `/dev/fd/<n>` as a discrete argument, close it in the parent after spawn, and retain pre/post inode checks. Never recover a pathname from the descriptor.

- [ ] **Step 4: Implement explicit readiness arguments**

Do not depend on a published alias for readiness. Return discrete `-o` arguments containing every security-critical value, followed by:

```text
127.0.0.1 /usr/bin/true
```

The reusable committed stanza remains forwarding-capable and contains no `ClearAllForwardings`.

- [ ] **Step 5: Run focused and daemon regressions**

```bash
rtk cargo test -p gascand --test ssh_identity
rtk cargo test -p gascand --test ssh_config
rtk cargo test -p gascand --test socket_security
rtk cargo test -p gascand
rtk cargo clippy -p gascand --all-targets -- -D warnings
rtk cargo fmt --all -- --check
rtk git diff --check
```

- [ ] **Step 6: Commit**

```bash
rtk git add crates/gascand/src/ssh.rs crates/gascand/src/ssh/identity.rs \
  crates/gascand/src/ssh/config.rs crates/gascand/tests/ssh_identity.rs \
  crates/gascand/tests/ssh_config.rs
rtk git commit -m "fix: finalize managed SSH trust publication"
```

### Task 3: Integrate Native SSH With Lifecycle and Reconciliation

**Files:**

- Create: `crates/gascand/src/ssh/port.rs`
- Create: `crates/gascand/src/ssh/manager.rs`
- Modify: `crates/gascand/src/ssh.rs`
- Modify: `crates/gascand/src/service.rs`
- Modify: `crates/gascand/src/reconcile.rs`
- Modify: `crates/gascand/src/main.rs`
- Modify: `crates/gascand/tests/lifecycle.rs`
- Modify: `crates/gascand/tests/reconcile.rs`
- Modify: `crates/gascand/tests/apply_setup.rs`
- Modify: `crates/gascan-core/src/fake_runtime.rs`

**Interfaces:**

```rust
const AUTOMATIC_PORT_ATTEMPTS: usize = 8;

pub struct PortReservation {
    port: u16,
    listener: std::net::TcpListener,
}

impl PortReservation {
    pub fn port(&self) -> u16;
    pub fn release(self);
}

pub struct PreparedSshCreate {
    identity: HostIdentity,
    host_port: u16,
    reservation: Option<PortReservation>,
}

impl PreparedSshCreate {
    pub fn identity(&self) -> &HostIdentity;
    pub fn host_port(&self) -> u16;
    pub fn release_reservation(&mut self);
    pub fn control_plane(&self) -> ControlPlanePolicy<'_>;
}

pub struct SshManager;

impl SshManager {
    pub async fn prepare_create(
        &self,
        spec: &SandboxSpec,
    ) -> Result<Option<PreparedSshCreate>, ServiceError>;

    pub async fn activate(
        &self,
        id: &SandboxId,
        runtime: &impl RuntimeBackend,
        expected: Option<&SshResolution>,
    ) -> Result<Option<ActiveSsh>, ServiceError>;

    pub fn deactivate(&self, id: &SandboxId) -> Result<(), ServiceError>;
}
```

`PreparedSshCreate` owns the validated `HostIdentity` and either an explicit port or a `PortReservation`. It supplies `ControlPlanePolicy` only immediately before native container creation.

- [ ] **Step 1: Write failing automatic-port tests**

Require one IPv4 `127.0.0.1:0` reservation, a port in `1024..=65535`, explicit-port bypass of automatic reservation, exactly eight retry attempts for retryable automatic collisions, and no retry/fallback for an explicit collision.

- [ ] **Step 2: Write failing lifecycle tests**

Cover:

- networked default create injects the public key and native loopback mapping;
- offline default create injects no key and publishes no SSH port;
- up validates the inspected `127.0.0.1:<port>:22` mapping and Ed25519 host key, performs explicit strict readiness, persists fingerprints, then commits the alias;
- down removes the alias before stop;
- up after down restores the same native mapping;
- apply preserves host/client fingerprints while accepting a new automatic port;
- destroy removes alias/trust/config-volume identity but retains the install client identity;
- readiness/host-key/config failures publish no alias and preserve existing rollback behavior;
- daemon restart reconstructs aliases only for owned running networked sandboxes with one exact native SSH mapping;
- stopped, offline, disabled, malformed, and unhealthy records remain unpublished;
- one broken sandbox cannot block other reconstruction or daemon startup.

- [ ] **Step 3: Run lifecycle tests and verify RED**

```bash
rtk cargo test -p gascand --test lifecycle ssh
rtk cargo test -p gascand --test reconcile ssh
rtk cargo test -p gascand --test apply_setup ssh
```

Expected: service does not yet create identities, native ports, readiness probes, resolutions, or aliases.

- [ ] **Step 4: Implement `SshManager` and service sequencing**

Use only fixed runtime exec arguments to read:

```text
/usr/bin/cat /home/workspace/.config/gascan/ssh/host/ssh_host_ed25519_key.pub
```

Parse and fingerprint that key with the same Ed25519 parser used for host trust. Inspect the runtime mapping rather than trusting the requested port. Prepare a trust generation, run explicit strict readiness, update `SshResolution`, then commit config. No operation reports success before that commit.

Classify native create errors caused by an unavailable automatic host port as retryable only within `AUTOMATIC_PORT_ATTEMPTS`. An explicit collision returns `ssh_port_unavailable` immediately.

- [ ] **Step 5: Implement restart reconciliation**

After existing resource reconciliation, collect validated `ManagedSshHost` values from owned running records and inspected native mappings. Publish one complete deterministic generation. Record per-sandbox findings without aborting daemon startup.

- [ ] **Step 6: Run lifecycle and daemon suites**

```bash
rtk cargo test -p gascand --test lifecycle
rtk cargo test -p gascand --test reconcile
rtk cargo test -p gascand --test apply_setup
rtk cargo test -p gascand --test daemon_idle
rtk cargo test -p gascand
rtk cargo clippy -p gascand --all-targets -- -D warnings
rtk cargo fmt --all -- --check
rtk git diff --check
```

- [ ] **Step 7: Commit**

```bash
rtk git add crates/gascand/src/ssh/port.rs crates/gascand/src/ssh/manager.rs \
  crates/gascand/src/ssh.rs crates/gascand/src/service.rs \
  crates/gascand/src/reconcile.rs crates/gascand/src/main.rs \
  crates/gascand/tests/lifecycle.rs crates/gascand/tests/reconcile.rs \
  crates/gascand/tests/apply_setup.rs crates/gascan-core/src/fake_runtime.rs
rtk git commit -m "feat: manage native SSH lifecycle"
```

### Task 4: Expose SSH Status, CLI, Aliases, and Safe Include Management

**Files:**

- Modify: `proto/gascan/v1/gascan.proto`
- Modify: `crates/gascan-proto/src/lib.rs`
- Modify: `crates/gascand/src/api.rs`
- Modify: `crates/gascan/src/client.rs`
- Modify: `crates/gascan/src/cli.rs`
- Create: `crates/gascan/src/ssh_config.rs`
- Modify: `crates/gascan/src/main.rs`
- Modify: `crates/gascan/src/presentation.rs`
- Create: `crates/gascan/tests/ssh_cli.rs`
- Create: `crates/gascan/tests/ssh_config.rs`

**Interfaces:**

```proto
message SshStatus {
  bool enabled = 1;
  bool active = 2;
  optional string host = 3;
  optional uint32 port = 4;
  optional string alias = 5;
  optional string host_key_fingerprint = 6;
  optional string client_key_fingerprint = 7;
}

message SandboxStatus {
  // existing fields 1..8
  SshStatus ssh = 9;
}
```

```text
gascan ssh [--sandbox <id>] [-- <command>...]
gascan ssh-config install
gascan ssh-config remove
gascan ssh-config path
```

- [ ] **Step 1: Write failing protobuf/status tests**

Append field 9 without reusing reserved field 7 or image field 8. Require inactive status to omit host/port/alias, active status to report exactly `127.0.0.1`, a valid port, stable alias, and both fingerprints. Advance API minor version from 2 to 3.

- [ ] **Step 2: Write failing CLI execution tests**

Require:

```text
/usr/bin/ssh -F <absolute-managed-config> gascan-<sandbox-id>
```

With a remote command, append every `OsString` unchanged after the alias. Never invoke a shell. Inherit stdio and propagate OpenSSH exit/signal status. Disabled offline status says SSH requires a networked sandbox; inactive networked status gives the recovery command.

- [ ] **Step 3: Write failing include-management tests**

Install exactly:

```text
# >>> gascan managed ssh include >>>
Include ~/.config/gascan/ssh/config
# <<< gascan managed ssh include <<<
```

Require idempotent install/removal, byte and line-ending preservation outside the block, atomic replacement, `~/.ssh` mode `0700`, config mode `0600`, and rejection of symlink/hard-link/FIFO/ownership/permission attacks. Removal deletes only the exact block. `path` prints the absolute managed config path.

- [ ] **Step 4: Implement append-only mappings and CLI**

Use the existing sandbox-selection rules. Human status renders:

```text
SSH gascan-<id> (127.0.0.1:<port>)
```

JSON emits separate structured fields. `gascan ssh` obtains status first, then executes only the system SSH client with discrete arguments.

- [ ] **Step 5: Implement interactive first-use offer**

After the first successful interactive human `up`, prompt only when stdin and stderr are TTYs, the include is absent, and the safe `include-offer-v1` receipt is absent:

```text
Add Gas Can's generated SSH hosts to ~/.ssh/config? [Y/n]
```

Yes invokes the same safe installer. No prints `gascan ssh-config install`. Record the receipt after either interactive answer. JSON, piped, CI, and other noninteractive modes neither prompt, mutate the host file, nor record the receipt.

- [ ] **Step 6: Run API, CLI, and presentation suites**

```bash
rtk cargo test -p gascan-proto
rtk cargo test -p gascand
rtk cargo test -p gascan --test ssh_cli
rtk cargo test -p gascan --test ssh_config
rtk cargo test -p gascan presentation
rtk cargo clippy -p gascan-proto -p gascand -p gascan --all-targets -- -D warnings
rtk cargo fmt --all -- --check
rtk git diff --check
```

- [ ] **Step 7: Commit**

```bash
rtk git add proto/gascan/v1/gascan.proto crates/gascan-proto/src/lib.rs \
  crates/gascand/src/api.rs crates/gascan/src/client.rs \
  crates/gascan/src/cli.rs crates/gascan/src/ssh_config.rs \
  crates/gascan/src/main.rs crates/gascan/src/presentation.rs \
  crates/gascan/tests/ssh_cli.rs crates/gascan/tests/ssh_config.rs
rtk git commit -m "feat: add native SSH CLI and status"
```

### Task 5: Verify, Approve, Merge, and Release Native SSH

**Files:**

- Modify: `crates/gascan-core/src/doctor.rs`
- Modify: `crates/gascand/src/main.rs`
- Modify: `crates/gascand/tests/doctor_state.rs`
- Modify: `crates/gascan-e2e/tests/apple_apply.rs`
- Modify: `crates/gascan-e2e/tests/apple_common/mod.rs`
- Modify: `README.md`
- Create: `docs/reference/manifest.md`
- Modify: `images/workspace/approved-image.txt`
- Modify: `docs/evidence/connected-workspace-image.md`

**Interfaces:**

- Doctor facts: `ssh.client`, `ssh.identity`, `ssh.config`, `ssh.native_publish`.
- Release-blocking Apple evidence for native loopback SSH.

- [ ] **Step 1: Write failing doctor and live acceptance tests**

Doctor checks `/usr/bin/ssh`, managed identity safety, generated config validity with `ssh -G -F`, and Apple native loopback-publish capability. Human output remains concise; exact causes/paths remain in failure detail and JSON.

The live Apple test must prove:

1. Networked default SSH publishes only `127.0.0.1:<port>:22`.
2. Public-key login and `gascan ssh -- printf '%s' <argument>` preserve exact arguments.
3. VS Code-style local forwarding reaches a guest-loopback service.
4. Remote and agent forwarding fail.
5. Host/client fingerprints survive down/up and image replacement.
6. Daemon restart reconstructs a working alias from `publishedPorts`.
7. Explicit-port collision returns actionable `ssh_port_unavailable`.
8. Destroy removes alias, sandbox trust generation, host key, config-volume auth sentinel, and all owned resources.
9. Offline default publishes no SSH port; explicit offline SSH fails validation.

- [ ] **Step 2: Run focused tests and verify RED**

```bash
rtk cargo test -p gascand --test doctor_state ssh
rtk cargo test -p gascan doctor
rtk cargo test -p gascan-e2e --test apple_apply native_ssh -- --ignored --nocapture
```

Expected: doctor facts and fresh image/runtime evidence are absent.

- [ ] **Step 3: Implement doctor checks and concise documentation**

Document networked defaults, offline rejection, `[ssh]`, automatic/explicit ports, aliases, direct SSH, VS Code Remote SSH, include install/removal, identity lifetime, fingerprint failure behavior, down/up/apply persistence, destroy semantics, and troubleshooting.

- [ ] **Step 4: Rebuild and approve the final image**

Run the repository's connected prefetch/build/gate workflow. The candidate gate must execute the guest SSH contract and workstation contract. Run live native SSH acceptance against the exact immutable candidate. Only then run the separate approval step that atomically updates `approved-image.txt` and connected-image evidence. Never overwrite an existing registry tag.

- [ ] **Step 5: Run complete verification**

```bash
rtk cargo fmt --all -- --check
rtk cargo clippy --workspace --all-targets --all-features -- -D warnings
rtk cargo test --workspace
rtk cargo test --manifest-path scripts/Cargo.toml
rtk bash ./scripts/apple-test-preflight.sh
rtk cargo test -p gascan-e2e --test apple_apply native_ssh -- --ignored --nocapture
rtk git diff --check
```

Require scoped Apple cleanup to report no test-owned containers, networks, volumes, listeners, or processes.

- [ ] **Step 6: Commit release evidence**

```bash
rtk git add crates/gascan-core/src/doctor.rs crates/gascand/src/main.rs \
  crates/gascand/tests/doctor_state.rs crates/gascan-e2e/tests/apple_apply.rs \
  crates/gascan-e2e/tests/apple_common/mod.rs README.md \
  docs/reference/manifest.md images/workspace/approved-image.txt \
  docs/evidence/connected-workspace-image.md
rtk git commit -m "feat: verify native sandbox SSH"
```

## Post-Plan Delivery

These actions occur only after all five task reviews and the required
whole-branch review pass:

1. Push the branch and create the pull request.
2. Wait for required checks and squash-merge after approval.
3. Synchronize local `main` with `origin/main`.
4. Use the repository release driver to bump `0.1.7` to `0.1.8`.
5. Run release preflight without bypasses.
6. Create the new immutable tag.
7. Run publish and GitHub-release phases.
8. Verify release artifacts and the GitHub release reference the merged commit
   and approved image.

Do not delete, move, or overwrite an existing tag.
