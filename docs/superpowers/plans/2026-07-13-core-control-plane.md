# Core Control Plane Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the backend-neutral Gas Can manifest policy, durable state machine, on-demand daemon API, and complete CLI behavior against a deterministic fake runtime.

**Architecture:** `gascan-core` contains pure policy and lifecycle orchestration around an async `RuntimeBackend`. `gascand` persists desired state and operations in SQLite and exposes a versioned Tonic API over a permission-checked Unix socket. `gascan` is a thin CLI client; tests boot the daemon with `FakeRuntime` and exercise real RPC without Apple software.

**Tech Stack:** Rust 1.85+ edition 2024, Tokio, Tonic/prost, SQLite/rusqlite, Clap, Serde/TOML, `camino`, `sha2`, `tempfile`.

## Global Constraints

- Begin only after Roadmap Gate 1 freezes shared runtime capability types.
- Do not import `gascan-apple` from `gascan-core`, `gascand`, or `gascan`.
- The daemon listens on a mode-0600 Unix socket beneath the user's runtime directory and never binds TCP.
- Only the canonical code root may appear in a bind-mount request.
- Unknown manifest keys are errors; security-sensitive defaults never degrade silently.
- Only `TERM`, `COLORTERM`, `LANG`, and `LC_*` inherit from the host by default.
- All state-changing operations are durable, idempotent, and serialized per sandbox.
- Rust production code denies unsafe code, unwraps, expects, and panics.

---

### Task 1: Define sandbox identity and strict manifest parsing

**Files:**
- Create: `crates/gascan-core/src/manifest.rs`
- Create: `crates/gascan-core/src/sandbox.rs`
- Modify: `crates/gascan-core/src/lib.rs`
- Test: `crates/gascan-core/tests/manifest.rs`
- Test: `crates/gascan-core/tests/sandbox_identity.rs`

**Interfaces:**
- Produces: `Manifest::load(root: &Utf8Path) -> Result<Manifest, ManifestError>`.
- Produces: `SandboxId::from_root(name: &str, canonical_root: &Utf8Path) -> SandboxId` and `SandboxSpec`.
- `SandboxSpec` contains exactly one `BindMount { source: canonical_root, target: "/workspace", writable: true }`.

- [ ] **Step 1: Write strict parsing and stable-identity tests**

```rust
#[test]
fn unknown_manifest_key_is_rejected() {
    let error = parse("version = 1\nnetwork = 'offline'\nssh_agent = true\n").unwrap_err();
    assert!(error.to_string().contains("unknown field `ssh_agent`"));
}

#[test]
fn canonical_path_produces_stable_noncolliding_id() {
    let first = SandboxId::from_root("code", Utf8Path::new("/Users/me/code"));
    let again = SandboxId::from_root("code", Utf8Path::new("/Users/me/code"));
    let other = SandboxId::from_root("code", Utf8Path::new("/Volumes/code"));
    assert_eq!(first, again);
    assert_ne!(first, other);
}
```

- [ ] **Step 2: Verify tests fail for missing types**

Run: `cargo test -p gascan-core --test manifest --test sandbox_identity`

Expected: FAIL because manifest and sandbox modules are undefined.

- [ ] **Step 3: Implement the schema and identity rules**

Define `Manifest` with `#[serde(deny_unknown_fields)]`, `version = 1`, `NetworkMode::{Networked, Offline}`, `UserMode::{Workspace, Root}`, resource types with validated units, `[tools]` as an ordered string map, `[ports]` as named `u16` values, Gascamp source, and optional setup path. Canonicalize with `std::fs::canonicalize`, require a directory, reject non-UTF8 paths with a typed error, slugify the name, and append the first 12 hexadecimal SHA-256 characters of the canonical path.

- [ ] **Step 4: Run manifest and identity tests**

Run: `cargo test -p gascan-core --test manifest --test sandbox_identity && cargo clippy -p gascan-core --all-targets -- -D warnings`

Expected: PASS for defaults, invalid versions, invalid resource strings, traversal setup paths, symlink canonicalization, names, and digest collisions in the fixture set.

- [ ] **Step 5: Commit manifest policy**

```bash
git add crates/gascan-core
git commit -m "feat: define sandbox manifest and identity"
```

### Task 2: Complete the backend contract and deterministic fake runtime

**Files:**
- Modify: `crates/gascan-core/src/runtime.rs`
- Create: `crates/gascan-core/src/fake_runtime.rs`
- Test: `crates/gascan-core/tests/backend_contract.rs`

**Interfaces:**
- Produces exactly nine async methods: `RuntimeBackend::{capabilities, inspect, create, start, stop, remove, exec, logs, list_resources}`. This Task 2/Task 5 joint interface revision makes creation and deletion resource-exact.
- Request/response types include `CreateRequest`, `CreateOutcome { created }`, typed full `RuntimeResource` inventory, `RemoveRequest`, `ContainerState`, `ExecRequest`, `ExecSession`, and typed `RuntimeError`.
- Produces `FakeRuntime::new(capabilities)` with deterministic failure injection and call recording.

- [ ] **Step 1: Write the reusable backend contract suite**

```rust
pub async fn backend_contract<B: RuntimeBackend>(backend: &B) {
    let id = SandboxId::test("contract");
    assert_eq!(backend.inspect(&id).await.unwrap(), None);
    let outcome = backend.create(validated_request).await.unwrap();
    assert_eq!(backend.inspect(&id).await.unwrap().unwrap().state, ContainerState::Stopped);
    backend.start(&id).await.unwrap();
    assert_eq!(backend.exec(ExecRequest::fixture(id.clone(), ["true"])).await.unwrap().exit_code(), 0);
    backend.stop(&id).await.unwrap();
    backend.remove(RemoveRequest::from_resources(outcome.created)?).await.unwrap();
    assert_eq!(backend.inspect(&id).await.unwrap(), None);
}
```

- [ ] **Step 2: Run and verify missing operations fail compilation**

Run: `cargo test -p gascan-core --test backend_contract`

Expected: FAIL listing undefined request and backend methods.

- [ ] **Step 3: Implement contract types and in-memory behavior**

Use object-safe async methods, byte-oriented exec streams, stable error codes, and ownership metadata that distinguishes GasCan-owned, foreign, and mismatched resources. `create` reports only resources created by that call; `remove` revalidates each exact identity and expected ownership. `FakeRuntime` stores state behind a Tokio mutex, models containers and volumes, rejects collisions, makes start/stop idempotent, records literal requests/outcomes, and can fail once at a named call boundary.

- [ ] **Step 4: Run contract tests under normal and injected failure modes**

Run: `cargo test -p gascan-core --test backend_contract`

Expected: PASS for happy path, idempotency, ownership filtering, binary I/O, exact exit codes, and each injected error.

- [ ] **Step 5: Commit the backend seam**

```bash
git add crates/gascan-core
git commit -m "feat: define runtime backend contract"
```

### Task 3: Implement policy compilation and fail-closed requests

**Files:**
- Create: `crates/gascan-core/src/policy.rs`
- Test: `crates/gascan-core/tests/policy.rs`

**Interfaces:**
- Consumes: `Manifest`, `SandboxSpec`, `RuntimeCapabilities`.
- Produces: `PolicyCompiler::compile(spec, capabilities) -> Result<CreateRequest, PolicyError>`.
- Produces: `filtered_host_environment(iter) -> BTreeMap<String, String>`.

- [ ] **Step 1: Write mount, offline, port, and environment rejection tests**

```rust
#[test]
fn offline_is_rejected_before_request_contains_a_mount() {
    let mut capabilities = RuntimeCapabilities::fixture();
    capabilities.offline = NetworkIsolation::Unsupported;
    let error = PolicyCompiler::compile(SandboxSpec::offline_fixture(), &capabilities).unwrap_err();
    assert_eq!(error.code(), "offline_unavailable");
}

#[test]
fn host_environment_has_a_fixed_allowlist() {
    let env = filtered_host_environment([("TERM", "xterm"), ("AWS_SECRET_ACCESS_KEY", "secret"), ("LC_ALL", "C")]);
    assert_eq!(env.keys().collect::<Vec<_>>(), vec!["LC_ALL", "TERM"]);
}
```

- [ ] **Step 2: Verify policy tests fail**

Run: `cargo test -p gascan-core --test policy`

Expected: FAIL because `PolicyCompiler` is undefined.

- [ ] **Step 3: Implement complete policy compilation**

Validate all required capabilities before constructing a request. Emit one canonical `/workspace` mount, Gas Can-owned named volumes only, loopback host bindings, immutable network mode, default/maximum resources, guest user, init, image digest, and ownership labels. Reject duplicate ports, privileged/raw options, paths outside the root, and unsupported resource controls.

- [ ] **Step 4: Run policy tests and snapshot the request shape**

Run: `cargo test -p gascan-core --test policy`

Expected: PASS; the approved JSON snapshot contains no host home, credentials, sockets, arbitrary devices, or backend flags.

- [ ] **Step 5: Commit policy compilation**

```bash
git add crates/gascan-core/src/policy.rs crates/gascan-core/tests/policy.rs
git commit -m "feat: compile fail-closed sandbox policy"
```

### Task 4: Add durable SQLite metadata and operation state

**Files:**
- Create: `crates/gascand/Cargo.toml`
- Create: `crates/gascand/src/lib.rs`
- Create: `crates/gascand/src/store.rs`
- Create: `crates/gascand/migrations/001_initial.sql`
- Test: `crates/gascand/tests/store.rs`

**Interfaces:**
- Produces: `Store::open(path)`, `put_sandbox`, `sandbox`, `list_sandboxes`, `begin_operation`, `complete_operation`, `fail_operation`, and `pending_operations`.
- Produces durable `DesiredState`, `ActualState`, `OperationKind`, `OperationStatus`, and setup/tool/image resolution records.

- [ ] **Step 1: Write crash-persistence and transition tests**

```rust
#[test]
fn pending_operation_survives_reopen() -> TestResult {
    let temp = tempfile::tempdir()?;
    let store = Store::open(temp.path().join("state.db"))?;
    let op = store.begin_operation(&SandboxRecord::fixture(), OperationKind::Create)?;
    drop(store);
    let reopened = Store::open(temp.path().join("state.db"))?;
    assert_eq!(reopened.pending_operations()?, vec![op]);
    Ok(())
}
```

- [ ] **Step 2: Verify the store test fails**

Run: `cargo test -p gascand --test store`

Expected: FAIL because the crate/store is missing.

- [ ] **Step 3: Implement migrations and transactional mutations**

Use WAL mode, foreign keys, busy timeout, explicit transactions, unique sandbox IDs/canonical roots, append-only operation events, and JSON fields only for versioned extensible details. Validate allowed lifecycle transitions in Rust before committing each transaction.

- [ ] **Step 4: Run persistence tests including kill-point simulations**

Run: `cargo test -p gascand --test store`

Expected: PASS for reopen, duplicate roots, invalid transitions, pending/failed/completed operations, concurrent readers, and schema version rejection.

- [ ] **Step 5: Commit durable state**

```bash
git add crates/gascand
git commit -m "feat: persist sandbox lifecycle state"
```

### Task 5: Build the lifecycle service and reconciliation

**Files:**
- Create: `crates/gascand/src/service.rs`
- Create: `crates/gascand/src/reconcile.rs`
- Test: `crates/gascand/tests/lifecycle.rs`
- Test: `crates/gascand/tests/reconcile.rs`

**Interfaces:**
- Produces: `SandboxService<B: RuntimeBackend>` methods `up`, `apply`, `start`, `stop`, `destroy`, `status`, `list`, and `reconcile`.
- Operations return an `OperationId` and structured `OperationEvent` stream.
- A keyed async lock serializes mutations for one sandbox while allowing unrelated sandboxes concurrently.

- [ ] **Step 1: Write rollback and reconciliation tests using `FakeRuntime`**

```rust
#[tokio::test]
async fn failed_create_preserves_existing_volumes_and_records_failure() {
    let runtime = FakeRuntime::failing_once("start");
    runtime.seed_volume("gascan-cache-code").await;
    let service = fixture_service(runtime.clone()).await;
    assert!(service.up(UpRequest::fixture()).await.is_err());
    assert!(runtime.volume_exists("gascan-cache-code").await);
    assert_eq!(service.store().latest_operation().unwrap().status, OperationStatus::Failed);
}
```

- [ ] **Step 2: Verify lifecycle tests fail**

Run: `cargo test -p gascand --test lifecycle --test reconcile`

Expected: FAIL because `SandboxService` is undefined.

- [ ] **Step 3: Implement transactional orchestration**

For `up`: validate/canonicalize, persist pending, compile policy, create only absent resources, start, provision/health-check through injected hooks, persist ready, and emit events. Roll back only `CreateOutcome.created`. Destroy inventories and removes only exact resources classified as owned for the target. Reconcile desired and actual state after restart; report unknown owned, unknown unowned, and mismatched resources but never delete unknown resources. Implement explicit `apply` change detection and non-destructive failure behavior.

- [ ] **Step 4: Run lifecycle and reconciliation tests**

Run: `cargo test -p gascand --test lifecycle --test reconcile`

Expected: PASS for idempotent up/down, stopped auto-start, missing sandbox refusal, concurrent operations, every injected crash point, unknown owned/unowned resources, and setup failure.

- [ ] **Step 5: Commit orchestration**

```bash
git add crates/gascand/src crates/gascand/tests
git commit -m "feat: orchestrate durable sandbox lifecycle"
```

### Task 6: Define the versioned Unix-socket API

**Files:**
- Create: `proto/gascan/v1/gascan.proto`
- Create: `crates/gascan-proto/Cargo.toml`
- Create: `crates/gascan-proto/build.rs`
- Create: `crates/gascan-proto/src/lib.rs`
- Test: `crates/gascan-proto/tests/api_compatibility.rs`

**Interfaces:**
- Produces Tonic service `GasCan` with unary `Handshake`, `Status`, `List`, `Doctor`; server-streaming lifecycle operations; and bidirectional `Attach`.
- Produces `ClientFrame` variants stdin/resize/signal/close and `ServerFrame` variants stdout/stderr/exit/error.
- API major version is `1`; handshake rejects different majors.

- [ ] **Step 1: Write an API descriptor compatibility test**

```rust
#[test]
fn v1_descriptor_contains_required_rpc_surface() {
    let descriptor = gascan_proto::FILE_DESCRIPTOR_SET;
    let text = descriptor_debug(descriptor).unwrap();
    for rpc in ["Handshake", "Up", "Apply", "Run", "Shell", "Down", "Destroy", "Status", "List", "Logs", "Doctor", "Attach"] {
        assert!(text.contains(rpc), "missing RPC {rpc}");
    }
}
```

- [ ] **Step 2: Verify protobuf generation is absent**

Run: `cargo test -p gascan-proto --test api_compatibility`

Expected: FAIL because the proto crate does not exist.

- [ ] **Step 3: Add explicit protobuf messages and generated bindings**

Define stable string error codes, operation IDs, timestamps, desired/actual states, capability fields, and byte payloads. Include `api_major`/`api_minor` in handshake and reserve removed field numbers. Vendor `protoc` through `protoc-bin-vendored` so clean builds do not require a system install.

- [ ] **Step 4: Run generation and compatibility tests**

Run: `cargo test -p gascan-proto --test api_compatibility && cargo doc -p gascan-proto --no-deps`

Expected: PASS and descriptor includes all required messages/RPCs.

- [ ] **Step 5: Commit API v1**

```bash
git add Cargo.toml proto crates/gascan-proto
git commit -m "feat: define Gas Can local API v1"
```

### Task 7: Serve the daemon securely and shut it down on idle

**Files:**
- Create: `crates/gascand/src/api.rs`
- Create: `crates/gascand/src/socket.rs`
- Create: `crates/gascand/src/main.rs`
- Test: `crates/gascand/tests/socket_security.rs`
- Test: `crates/gascand/tests/daemon_idle.rs`

**Interfaces:**
- Produces: `Daemon::serve(config, service)`, `SocketPaths::for_user()`, and Tonic API implementation.
- Socket directory mode is 0700 and socket mode is 0600; stale sockets are removed only after a failed connection and ownership check.

- [ ] **Step 1: Write socket-mode and idle-lifetime tests**

```rust
#[tokio::test]
async fn daemon_uses_private_unix_socket_and_exits_when_idle() -> TestResult {
    let harness = DaemonHarness::start(Duration::from_millis(50)).await?;
    assert_eq!(harness.socket_mode()?, 0o600);
    assert!(!harness.has_tcp_listener()?);
    harness.wait_for_exit().await?;
    assert!(!harness.socket_path().exists());
    Ok(())
}
```

- [ ] **Step 2: Verify daemon tests fail**

Run: `cargo test -p gascand --test socket_security --test daemon_idle`

Expected: FAIL because daemon serving is absent.

- [ ] **Step 3: Implement UDS serving, peer checks, and activity leases**

Create the runtime directory without following symlinks, verify ownership, bind the socket, chmod it, and accept Tonic connections. Each RPC/session holds an activity lease; idle timeout begins only when leases and active operations reach zero. On SIGTERM stop accepting, finish durable operations, close streams, remove the owned socket, and exit.

- [ ] **Step 4: Run daemon and adversarial socket tests**

Run: `cargo test -p gascand --test socket_security --test daemon_idle`

Expected: PASS for wrong-owner directory rejection, symlink path rejection, stale socket handling, active-session retention, clean SIGTERM, and idle exit.

- [ ] **Step 5: Commit the daemon transport**

```bash
git add crates/gascand
git commit -m "feat: serve Gas Can over a private Unix socket"
```

### Task 8: Implement the CLI and fake-backend end-to-end suite

**Files:**
- Create: `crates/gascan/Cargo.toml`
- Create: `crates/gascan/src/main.rs`
- Create: `crates/gascan/src/cli.rs`
- Create: `crates/gascan/src/client.rs`
- Create: `crates/gascan/src/terminal.rs`
- Create: `crates/gascan-e2e/Cargo.toml`
- Create: `crates/gascan-e2e/src/lib.rs`
- Test: `crates/gascan-e2e/tests/fake_backend.rs`

**Interfaces:**
- Produces CLI commands `up`, `apply`, `shell`, `run`, `down`, `destroy`, `list`, `status`, `logs`, and `doctor`.
- `Client::connect_or_start()` connects to `gascand`, starts it on absence, waits for handshake readiness, and negotiates API v1.
- Exit codes: command exit is preserved; CLI/config/runtime failures use documented non-command codes.

- [ ] **Step 1: Write a real-process CLI scenario**

```rust
#[tokio::test]
async fn complete_cli_lifecycle_uses_daemon_api() -> TestResult {
    let env = E2eEnvironment::fake().await?;
    env.gascan(["up", env.workspace()]).assert_success();
    env.gascan(["run", "--", "sh", "-c", "exit 42"]).assert_code(42);
    env.gascan(["down"]).assert_success();
    env.gascan(["status", "--json"]).assert_json_path("$.actual_state", "stopped");
    env.gascan(["destroy", "--yes"]).assert_success();
    Ok(())
}
```

- [ ] **Step 2: Verify CLI binary/test is missing**

Run: `cargo test -p gascan-e2e --test fake_backend`

Expected: FAIL because `gascan` is not built.

- [ ] **Step 3: Implement command parsing, rendering, daemon startup, and terminal bridging**

Use Clap subcommands, JSON output flags for inspection commands, exact argv separation after `--`, human progress from operation events, and bidirectional attach frames. Put the local terminal into raw mode only for TTY sessions and restore it through an RAII guard on success, error, signal, and panic unwinding. `destroy` requires a TTY confirmation or `--yes`.

- [ ] **Step 4: Run all platform-neutral gates**

Run: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo test --workspace`

Expected: PASS, including daemon kill/restart, two unrelated concurrent sandboxes, binary output, resize/signal frames, safe environment filtering, auto-start race, and API-version mismatch.

- [ ] **Step 5: Commit Roadmap Gate 3**

```bash
git add crates/gascan crates/gascan-e2e Cargo.toml
git commit -m "feat: complete fake-backend Gas Can control plane"
```
