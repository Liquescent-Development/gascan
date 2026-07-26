# Managed SSH Access Implementation Plan

> **Superseded on 2026-07-25:** Do not execute this plan. The approved
> networked-only native publication replacement is
> `docs/superpowers/plans/2026-07-25-networked-native-ssh.md`. In particular,
> do not implement the TCP bridge or offline SSH tasks below.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give every sandbox secure, persistent, loopback-only SSH access that works for people, automation, and Remote SSH clients without weakening offline isolation.

**Architecture:** A daemon-owned TCP listener on host loopback bridges raw bytes through the existing Apple exec channel to guest loopback port 22. Gas Can generates the client identity and guest host identity, publishes a strict generated OpenSSH config, and manages listener/config lifecycle alongside the sandbox. The Apple runtime network remains private and receives no published port.

**Tech Stack:** Rust, Tokio TCP and byte streams, existing runtime exec sessions, OpenSSH client/server, SQLite resolution records, tonic/protobuf, Clap, fake-runtime integration tests, live Apple acceptance.

**Prerequisites:**

- Complete `docs/superpowers/plans/2026-07-23-image-resolution-upgrades.md`.
- Complete `docs/superpowers/plans/2026-07-23-default-workstation-image.md` through the OpenSSH and `nc` image contract.

**Design:** `docs/superpowers/specs/2026-07-23-default-ssh-workstation-design.md`

## Global Constraints

- Default SSH is enabled for networked and offline sandboxes.
- The only host listener is IPv4 loopback; default port selection uses one atomic `127.0.0.1:0` bind.
- SSH never becomes a manifest application port or an Apple runtime `--publish`.
- Guest SSH listens only on guest loopback and accepts only the generated public key.
- Passwords, root login, agent forwarding, remote forwarding, host credential import, and host key bypass are prohibited.
- Local and dynamic forwarding are allowed, but forwarded destinations are restricted to guest loopback.
- Private keys are created with restrictive permissions and never cross into the guest.
- Sandbox host keys and third-party auth state survive down/up and image replacement because they live on the config volume; destroy removes them with that volume.
- JSON and noninteractive commands never prompt or mutate `~/.ssh/config`.
- Generated files are deterministic, atomically replaced, and reject symlink or ownership attacks.
- All subprocess arguments are passed as discrete strings; no shell command construction.

---

### Task 1: Add a Validated SSH Manifest Policy

**Files:**
- Modify: `crates/gascan-core/src/manifest.rs`
- Modify: `crates/gascan-core/src/policy.rs`
- Modify: `crates/gascan-core/tests/manifest.rs`
- Modify: `crates/gascan-core/tests/policy.rs`

**Interfaces:**

```rust
pub struct Ssh {
    enabled: bool,
    host_port: Option<u16>,
}

impl Ssh {
    pub const fn enabled(&self) -> bool;
    pub const fn host_port(&self) -> Option<u16>;
}
```

- [ ] **Step 1: Write failing manifest tests**

Cover absent `[ssh]` as `enabled = true`, explicit disable, explicit high port,
unknown fields, `host_port = 0`, and privileged ports `1..=1023`. Require:

```toml
[ssh]
enabled = true
# omitted means an OS-selected loopback port
host_port = 22222
```

Reject `host_port` when `enabled = false`.

- [ ] **Step 2: Run tests and confirm RED**

```bash
rtk cargo test -p gascan-core --test manifest ssh
```

Expected: `Manifest::ssh()` and validation do not exist.

- [ ] **Step 3: Implement sealed validated policy**

Add `ssh` to `Manifest`, `RawManifest`, defaults, serialization, accessors, and
unknown-field validation. Keep fields private so callers cannot construct an
unvalidated port.

Add a daemon-only policy input:

```rust
pub struct ControlPlanePolicy<'a> {
    pub ssh_authorized_key: Option<&'a str>,
}

pub fn compile_with_control_plane(
    manifest: &Manifest,
    capabilities: &RuntimeCapabilities,
    control: ControlPlanePolicy<'_>,
) -> Result<CreateRequest, PolicyError>;
```

When enabled, seal only the public key and `GASCAN_SSH_ENABLED=1` into the
create request. When disabled use `GASCAN_SSH_ENABLED=0`. Never put the private
key, host port, or host path into guest environment.

- [ ] **Step 4: Run focused tests**

```bash
rtk cargo test -p gascan-core --test manifest ssh
rtk cargo test -p gascan-core --test policy ssh
```

Expected: all pass, including offline manifests producing SSH control policy
without a published runtime port.

- [ ] **Step 5: Commit**

```bash
rtk git add crates/gascan-core/src/manifest.rs crates/gascan-core/src/policy.rs \
  crates/gascan-core/tests/manifest.rs crates/gascan-core/tests/policy.rs
rtk git commit -m "feat: add validated SSH policy"
```

### Task 2: Persist SSH Resolution and Public Status

**Files:**
- Create: `crates/gascand/migrations/004_ssh_resolution.sql`
- Modify: `crates/gascand/src/store.rs`
- Modify: `crates/gascand/tests/store.rs`
- Modify: `crates/gascan-core/src/sandbox.rs`
- Modify: `crates/gascan-core/tests/sandbox_identity.rs`

**Interfaces:**

```rust
pub struct SshResolution {
    pub version: u32,
    pub details: serde_json::Value,
}

pub struct SshStatus {
    pub enabled: bool,
    pub active: bool,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub alias: Option<String>,
    pub host_key_fingerprint: Option<String>,
    pub client_key_fingerprint: Option<String>,
}
```

- [ ] **Step 1: Write failing schema and round-trip tests**

Require schema version 4, migration from the checked-in version-3 fixture,
nullable `ssh_resolution_version/details`, and lossless round trips. Reject
invalid JSON and a details column without a version. Advance the existing
"newer schema" rejection fixture from version 4 to version 5.

Resolution version 1 stores only:

```json
{
  "enabled": true,
  "host_key_fingerprint": "SHA256:...",
  "client_key_fingerprint": "SHA256:..."
}
```

The ephemeral host port and active flag must not be durable.

- [ ] **Step 2: Run store tests and confirm RED**

```bash
rtk cargo test -p gascand --test store ssh_resolution
```

- [ ] **Step 3: Add migration, types, and store update**

Extend `SandboxRecord`, every query/projection, insert/update methods, fixtures,
schema validation, and migration accounting. Add one transactionally scoped
`update_ssh_resolution` operation used only after host/client identity
validation succeeds.

- [ ] **Step 4: Run store and identity tests**

```bash
rtk cargo test -p gascand --test store
rtk cargo test -p gascan-core --test sandbox_identity
```

- [ ] **Step 5: Commit**

```bash
rtk git add crates/gascand/migrations/004_ssh_resolution.sql \
  crates/gascand/src/store.rs crates/gascand/tests/store.rs \
  crates/gascan-core/src/sandbox.rs crates/gascan-core/tests/sandbox_identity.rs
rtk git commit -m "feat: persist sandbox SSH identity"
```

### Task 3: Start a Locked-Down Guest OpenSSH Service

**Files:**
- Modify: `images/workspace/bin/gascan-entrypoint`
- Create: `images/workspace/bin/start-gascan-sshd`
- Modify: `images/workspace/Dockerfile`
- Modify: `scripts/tests/connected_dockerfile.rs`
- Modify: `scripts/tests/image_user_contract.rs`
- Modify: `scripts/run-connected-image-gate.sh`
- Modify: `scripts/tests/connected_image_gate.rs`
- Create: `images/workspace/tests/ssh-contract.sh`

**Interfaces:**
- Consumes: `GASCAN_SSH_ENABLED` and `GASCAN_SSH_AUTHORIZED_KEY`
- Persists: `/home/workspace/.config/gascan/ssh/host/`
- Listens: guest `127.0.0.1:22`

- [ ] **Step 1: Write failing image-policy tests**

Require a fixed entrypoint and a generated `sshd_config` containing:

```text
ListenAddress 127.0.0.1
Port 22
PasswordAuthentication no
KbdInteractiveAuthentication no
PermitRootLogin no
PubkeyAuthentication yes
AllowUsers workspace
AllowAgentForwarding no
AllowTcpForwarding local
PermitOpen 127.0.0.1:*
GatewayPorts no
PermitTunnel no
X11Forwarding no
StrictModes yes
```

Reject wildcard listen addresses, `authorized_keys` outside managed config,
runtime-generated passwords, and host key files outside the config volume.

- [ ] **Step 2: Run image tests and confirm RED**

```bash
rtk cargo test --manifest-path scripts/Cargo.toml --test connected_dockerfile ssh
rtk cargo test --manifest-path scripts/Cargo.toml --test image_user_contract ssh
```

- [ ] **Step 3: Implement idempotent guest initialization**

`start-gascan-sshd` must:

1. Validate the environment public key as one single Ed25519 public-key record.
2. Create host/config paths without following symlinks.
3. Generate one sandbox-persistent Ed25519 host key if absent.
4. Write `authorized_keys` and `sshd_config` atomically with strict ownership
   and modes.
5. Validate with `sshd -t`.
6. `exec /usr/sbin/sshd -D -e -f <managed-config>`.

The entrypoint runs this path when enabled and otherwise runs the existing
container keepalive behavior. It never regenerates an existing valid host key.

- [ ] **Step 4: Add the guest contract**

The smoke test checks process identity, listener address, permissions,
fingerprint stability across restart, public-key login, rejection of password
and root login, allowed local forwarding, rejected remote/agent forwarding,
and absence of non-loopback port 22.

- [ ] **Step 5: Run image contracts**

```bash
rtk cargo test --manifest-path scripts/Cargo.toml --test connected_dockerfile
rtk cargo test --manifest-path scripts/Cargo.toml --test image_user_contract
rtk cargo test --manifest-path scripts/Cargo.toml --test connected_image_gate ssh
```

- [ ] **Step 6: Commit**

```bash
rtk git add images/workspace/bin/gascan-entrypoint \
  images/workspace/bin/start-gascan-sshd images/workspace/Dockerfile \
  images/workspace/tests/ssh-contract.sh scripts/run-connected-image-gate.sh \
  scripts/tests/connected_dockerfile.rs scripts/tests/image_user_contract.rs \
  scripts/tests/connected_image_gate.rs
rtk git commit -m "feat: configure guest SSH service"
```

### Task 4: Manage Host Identity and Generated OpenSSH Files

**Files:**
- Create: `crates/gascand/src/ssh.rs`
- Create: `crates/gascand/src/ssh/identity.rs`
- Create: `crates/gascand/src/ssh/config.rs`
- Modify: `crates/gascand/src/lib.rs`
- Create: `crates/gascand/tests/ssh_identity.rs`
- Create: `crates/gascand/tests/ssh_config.rs`

**Interfaces:**

```rust
pub struct HostIdentity {
    pub private_key: Utf8PathBuf,
    pub public_key: String,
    pub fingerprint: String,
}

pub struct ActiveSsh {
    pub host: IpAddr,
    pub port: u16,
    pub alias: String,
    pub host_key_fingerprint: String,
    pub client_key_fingerprint: String,
}
```

- [ ] **Step 1: Write filesystem-attack tests**

Using a temporary home, require:

- one install-wide Ed25519 client key generated via bounded `ssh-keygen`;
- private key mode `0600`, public/generated config mode `0644`, directories
  `0700`;
- owner is the current user;
- existing symlinks, hard links, FIFOs, foreign-owned files, permissive private
  keys, and malformed keys are rejected;
- interrupted writes leave the previous valid file;
- no private key bytes appear in generated config, logs, or errors.

- [ ] **Step 2: Run tests and confirm RED**

```bash
rtk cargo test -p gascand --test ssh_identity
rtk cargo test -p gascand --test ssh_config
```

- [ ] **Step 3: Implement safe state resolution**

Resolve `$XDG_CONFIG_HOME/gascan/ssh`, otherwise
`$HOME/.config/gascan/ssh`. Use descriptor-relative, no-follow creation and
atomic same-directory replacement following the daemon socket security
patterns. Generate:

```text
identity_ed25519
identity_ed25519.pub
known_hosts
config
```

The generated `Host gascan-*` stanzas use:

```text
HostName 127.0.0.1
IdentityFile <absolute managed identity>
UserKnownHostsFile <absolute managed known_hosts>
StrictHostKeyChecking yes
IdentitiesOnly yes
ForwardAgent no
```

Add `ClearAllForwardings yes` only to readiness probes, not the reusable
stanza, so VSCode local forwarding works.

- [ ] **Step 4: Run tests**

```bash
rtk cargo test -p gascand --test ssh_identity
rtk cargo test -p gascand --test ssh_config
```

- [ ] **Step 5: Commit**

```bash
rtk git add crates/gascand/src/ssh.rs crates/gascand/src/ssh/identity.rs \
  crates/gascand/src/ssh/config.rs crates/gascand/src/lib.rs \
  crates/gascand/tests/ssh_identity.rs crates/gascand/tests/ssh_config.rs
rtk git commit -m "feat: manage SSH client identity and config"
```

### Task 5: Bridge Host TCP to the Guest Exec Channel

**Files:**
- Create: `crates/gascand/src/ssh/bridge.rs`
- Create: `crates/gascand/tests/ssh_bridge.rs`
- Modify: `crates/gascan-core/src/fake_runtime.rs`

**Interfaces:**

```rust
#[async_trait]
pub trait SshGuestConnector: Send + Sync {
    async fn connect_ssh(&self, id: &SandboxId) -> Result<ExecSession, ServiceError>;
}

pub struct SshBridge<C> { /* listener/task registry */ }

impl<C: SshGuestConnector> SshBridge<C> {
    pub async fn activate(
        &self,
        id: SandboxId,
        requested_port: Option<u16>,
    ) -> Result<SocketAddr, SshError>;
    pub async fn deactivate(&self, id: &SandboxId);
}
```

- [ ] **Step 1: Write failing byte-bridge tests**

Prove:

- default activation performs one `TcpListener::bind((127.0.0.1, 0))`;
- explicit ports bind only `127.0.0.1`;
- IPv6 wildcard and non-loopback addresses are impossible through the API;
- each accepted socket opens `["/usr/bin/nc", "127.0.0.1", "22"]` with no
  TTY, environment, or initial stdin;
- arbitrary non-UTF-8 and multi-megabyte bidirectional streams are unchanged;
- TCP EOF sends `ExecInput::Close`;
- exec stdout reaches TCP, stderr is bounded for diagnostics, and exit closes
  the socket;
- duplicate activation is idempotent;
- cancellation/deactivation closes listener, sockets, and exec sessions;
- task counts return to zero after errors.

- [ ] **Step 2: Run bridge tests and confirm RED**

```bash
rtk cargo test -p gascand --test ssh_bridge
```

- [ ] **Step 3: Implement bounded bridge ownership**

Use Tokio split TCP halves and the existing bounded `ExecSession` channels.
Apply explicit connection and stderr bounds, track child tasks, cancel them
with listener shutdown, and never buffer an unbounded stream. Map bind
conflicts to a stable error that names the requested port.

- [ ] **Step 4: Run bridge and existing attach tests**

```bash
rtk cargo test -p gascand --test ssh_bridge
rtk cargo test -p gascand --test attach_bridge
rtk cargo test -p gascan-core --test backend_contract
```

- [ ] **Step 5: Commit**

```bash
rtk git add crates/gascand/src/ssh/bridge.rs crates/gascand/tests/ssh_bridge.rs \
  crates/gascan-core/src/fake_runtime.rs
rtk git commit -m "feat: bridge loopback SSH connections"
```

### Task 6: Integrate SSH With Sandbox Lifecycle and Reconciliation

**Files:**
- Modify: `crates/gascand/src/service.rs`
- Modify: `crates/gascand/src/reconcile.rs`
- Modify: `crates/gascand/src/main.rs`
- Modify: `crates/gascand/tests/lifecycle.rs`
- Modify: `crates/gascand/tests/reconcile.rs`
- Modify: `crates/gascand/tests/apply_setup.rs`

**Interfaces:**
- `SandboxService` implements `SshGuestConnector`
- `SshManager::activate` validates guest host identity, starts the bridge,
  publishes config, and returns `ActiveSsh`
- `SshManager::deactivate` removes active stanza before closing the listener

- [ ] **Step 1: Write failing lifecycle tests**

Cover:

- `up`: ensure client identity, create with public key, start guest, read and
  validate host public key/fingerprint, activate bridge, perform strict SSH
  readiness, persist resolution, then report completion;
- `down`: remove generated stanza and stop listener before stopping container;
- `destroy`: deactivate, remove sandbox resources/config volume, remove
  known-host entry, retain only the install-wide client key;
- `apply`: preserve host/client fingerprints and reactivate on a new ephemeral
  port after image replacement;
- failed activation/readiness: no completed operation, no published stanza,
  stopped listener, and rollback using existing lifecycle rules;
- disabled SSH: no identity injection, listener, config stanza, or probe;
- daemon restart: reconcile running sandboxes to fresh listeners/config;
- stopped sandboxes remain inactive;
- explicit port conflicts fail only that sandbox with actionable diagnostics.

- [ ] **Step 2: Run lifecycle tests and confirm RED**

```bash
rtk cargo test -p gascand --test lifecycle ssh
rtk cargo test -p gascand --test reconcile ssh
rtk cargo test -p gascand --test apply_setup ssh
```

- [ ] **Step 3: Wire service and manager**

Share service ownership safely with the manager, without exposing runtime
requests outside the daemon. Implement `connect_ssh` through
`require_owned_running` and the fixed `nc` argv. Make operation completion
depend on the strict readiness command:

```text
/usr/bin/ssh -F <generated-config> -o BatchMode=yes
  -o ClearAllForwardings=yes <alias> /usr/bin/true
```

Capture the guest host public key through fixed `cat`/`ssh-keygen` argv and
write a bracketed `[127.0.0.1]:<port>` known-host record before probing.

- [ ] **Step 4: Add restart reconciliation**

After resource reconciliation, activate SSH only for owned running records.
One broken sandbox records a finding but cannot prevent other listeners or the
daemon API from starting. Regenerate the complete config from validated active
state to remove stale entries.

- [ ] **Step 5: Run lifecycle and daemon suites**

```bash
rtk cargo test -p gascand --test lifecycle
rtk cargo test -p gascand --test reconcile
rtk cargo test -p gascand --test daemon_idle
rtk cargo test -p gascand --test apply_setup
```

- [ ] **Step 6: Commit**

```bash
rtk git add crates/gascand/src/service.rs crates/gascand/src/reconcile.rs \
  crates/gascand/src/main.rs crates/gascand/tests/lifecycle.rs \
  crates/gascand/tests/reconcile.rs crates/gascand/tests/apply_setup.rs
rtk git commit -m "feat: manage SSH with sandbox lifecycle"
```

### Task 7: Expose Stable SSH API and Status Contracts

**Files:**
- Modify: `proto/gascan/v1/gascan.proto`
- Modify: `crates/gascan-proto/src/lib.rs`
- Modify: `crates/gascand/src/api.rs`
- Modify: `crates/gascan/src/client.rs`
- Modify: `crates/gascand/tests/lifecycle.rs`

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
```

- [ ] **Step 1: Write failing protobuf/API tests**

Append `SshStatus ssh = 9` to `SandboxStatus`; do not reuse reserved field 7
or the image upgrade field 8 introduced by the prerequisite plan. Require
absent host/port/alias when inactive, exact loopback host when active, valid
port range, and stable fingerprint formatting. Extend list and status fixtures
and JSON golden output.

- [ ] **Step 2: Run API tests and confirm RED**

```bash
rtk cargo test -p gascand api::tests::status
rtk cargo test -p gascan client::tests::status
```

- [ ] **Step 3: Implement append-only schema and mappings**

Update proto compatibility expectations, daemon mapping, client mapping, and
advance API minor version from 2 to 3. SSH details are read-only status; no RPC
accepts an arbitrary host or command.

- [ ] **Step 4: Run protocol and API suites**

```bash
rtk cargo test -p gascan-proto
rtk cargo test -p gascand
rtk cargo test -p gascan
```

- [ ] **Step 5: Commit**

```bash
rtk git add proto/gascan/v1/gascan.proto crates/gascan-proto/src/lib.rs \
  crates/gascand/src/api.rs crates/gascan/src/client.rs \
  crates/gascand/tests/lifecycle.rs
rtk git commit -m "feat: expose sandbox SSH status"
```

### Task 8: Add `gascan ssh` and Safe Include Management

**Files:**
- Modify: `crates/gascan/src/cli.rs`
- Create: `crates/gascan/src/ssh_config.rs`
- Modify: `crates/gascan/src/main.rs`
- Modify: `crates/gascan/src/presentation.rs`
- Create: `crates/gascan/tests/ssh_cli.rs`
- Create: `crates/gascan/tests/ssh_config.rs`

**Interfaces:**

```text
gascan ssh [--] [COMMAND...]
gascan ssh-config install
gascan ssh-config remove
gascan ssh-config path
```

- [ ] **Step 1: Write failing CLI parsing and execution tests**

Require `gascan ssh` to run `/usr/bin/ssh -F <managed-config> <alias>` with
inherited stdio. `gascan ssh -- cmd arg` appends each argument unchanged and
never invokes a shell. Reject inactive/disabled SSH with actionable guidance.
Propagate the OpenSSH exit code, including signal handling.

- [ ] **Step 2: Write failing include-management tests**

`install` adds exactly one managed block:

```text
# >>> gascan managed ssh include >>>
Include ~/.config/gascan/ssh/config
# <<< gascan managed ssh include <<<
```

Require idempotent install/removal, preservation of unrelated bytes and line
endings, atomic replacement, restrictive creation mode, and rejection of
symlink/hard-link/ownership attacks. Removal deletes only an exact managed
block. `path` prints the absolute generated config path.

- [ ] **Step 3: Run CLI tests and confirm RED**

```bash
rtk cargo test -p gascan --test ssh_cli
rtk cargo test -p gascan --test ssh_config
```

- [ ] **Step 4: Implement commands and presentation**

Use status selection rules already shared by shell/status. Invoke the system
SSH binary with discrete arguments and inherited stdin/stdout/stderr. Human
status/list output shows `SSH gascan-<id> (127.0.0.1:<port>)`; JSON emits the
new structured status unchanged.

After the first successful interactive human `up`, when stdin and stderr are
TTYs, the include is absent, and the safe managed
`include-offer-v1` receipt is absent, prompt once:

```text
Add Gas Can's generated SSH hosts to ~/.ssh/config? [Y/n]
```

Yes runs the same safe installer; no leaves a concise command:
`gascan ssh-config install`. Record the receipt after either interactive
answer, so declining does not cause repeated prompts. JSON, piped, CI, and
other noninteractive modes neither prompt, change the host file, nor record the
receipt.

- [ ] **Step 5: Run CLI and presentation suites**

```bash
rtk cargo test -p gascan --test ssh_cli
rtk cargo test -p gascan --test ssh_config
rtk cargo test -p gascan presentation
```

- [ ] **Step 6: Commit**

```bash
rtk git add crates/gascan/src/cli.rs crates/gascan/src/ssh_config.rs \
  crates/gascan/src/main.rs crates/gascan/src/presentation.rs \
  crates/gascan/tests/ssh_cli.rs crates/gascan/tests/ssh_config.rs
rtk git commit -m "feat: add managed SSH CLI"
```

### Task 9: Add Diagnostics, Documentation, and Live Security Acceptance

**Files:**
- Modify: `crates/gascan-core/src/doctor.rs`
- Modify: `crates/gascand/src/main.rs`
- Modify: `crates/gascand/tests/doctor_state.rs`
- Modify: `crates/gascan-e2e/tests/apple_apply.rs`
- Modify: `crates/gascan-e2e/tests/apple_common/mod.rs`
- Modify: `README.md`
- Create: `docs/reference/manifest.md`

**Interfaces:**
- Adds concise `ssh.client`, `ssh.identity`, and `ssh.config` doctor checks
- Produces release-blocking loopback/isolation/persistence evidence

- [ ] **Step 1: Write failing doctor tests**

Check `/usr/bin/ssh` version support, managed identity validity/permissions,
and generated-config validity with `ssh -G -F`. Human doctor output stays
concise; exact paths and causes remain in failure detail and JSON.

- [ ] **Step 2: Run doctor tests and confirm RED**

```bash
rtk cargo test -p gascand --test doctor_state ssh
rtk cargo test -p gascan doctor
```

- [ ] **Step 3: Implement doctor checks and documentation**

Document defaults, `[ssh]`, generated paths, install/remove commands, direct
SSH, Remote SSH/VSCode alias usage, forwarding policy, identity lifetime,
destroy semantics, noninteractive behavior, and why offline sandboxes still
have host-local SSH without internet access.

- [ ] **Step 4: Run complete static verification**

```bash
rtk cargo fmt --all -- --check
rtk cargo clippy --workspace --all-targets --all-features -- -D warnings
rtk cargo test --workspace
rtk cargo test --manifest-path scripts/Cargo.toml
rtk git diff --check
```

- [ ] **Step 5: Run live Apple acceptance**

```bash
rtk bash ./scripts/apple-test-preflight.sh
rtk cargo test -p gascan-e2e --test apple_apply ssh -- --ignored --nocapture
```

The test must prove:

1. The host listener is loopback-only and is not an Apple published port.
2. Public-key login and `gascan ssh -- printf ...` preserve exact arguments.
3. VSCode-style local forwarding reaches a guest-loopback test service.
4. Remote and agent forwarding fail.
5. An offline sandbox has SSH but cannot reach internet or host/LAN targets.
6. A separate Apple container cannot reach the SSH listener or sandbox.
7. Host and client fingerprints persist across down/up and image replacement.
8. Daemon restart selects a fresh default port and rewrites config safely.
9. Destroy removes alias, known-host record, host key, and config-volume auth
   sentinels.
10. Explicit-port collision is actionable and cleanup leaves no test-owned
    containers, networks, volumes, listeners, or processes.

- [ ] **Step 6: Rebuild and approve the final image**

Run the connected image prefetch/build/gate after the SSH scripts land, update
the exact approved image digest, and repeat the live SSH acceptance against
that digest. Do not release code that expects SSH before the approved image
containing OpenSSH is publishable.

- [ ] **Step 7: Commit**

```bash
rtk git add crates/gascan-core/src/doctor.rs crates/gascand/src/main.rs \
  crates/gascand/tests/doctor_state.rs crates/gascan-e2e/tests/apple_apply.rs \
  crates/gascan-e2e/tests/apple_common/mod.rs README.md docs/reference/manifest.md \
  images/workspace/approved-image.txt docs/evidence/connected-workspace-image.md
rtk git commit -m "feat: verify managed SSH access"
```

## Release Order

1. Publish and approve the locked workstation image containing OpenSSH and the
   guest initialization scripts.
2. Merge daemon/CLI support and its compatible status schema.
3. Bump the product version through the repository release driver.
4. Run its preflight, tag, publish, and GitHub-release phases without bypassing
   any existing signed-release evidence.
