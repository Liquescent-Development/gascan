# Apple Runtime Feasibility Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prove that Apple `container` can enforce every runtime property required by Gas Can and freeze reusable backend contracts and fixtures.

**Architecture:** A small Rust `gascan-apple` crate executes Apple commands through an injected runner and converts structured output into backend-neutral capabilities. Ignored integration tests perform destructive experiments only on uniquely labeled Gas Can test resources and write a versioned feasibility report.

**Tech Stack:** Rust 1.85+ edition 2024, Tokio process APIs, Serde/JSON, `container` 1.x, `cargo-nextest` optional, shell security probes.

## Global Constraints

- Run live tests only on Apple silicon with macOS 26+ and Apple `container` 1.x.
- Never pass commands through a shell; use executable plus argument arrays.
- Live resources use the prefix `gascan-feas-<process-id>-` and ownership label `dev.gascan.test=true`.
- Never mount the operator's real code or home directory during feasibility tests; use a temporary directory.
- A live test may delete only resources it created and recorded in its test context.
- Offline networking, TTY/signal propagation, bind-mount boundaries, and cleanup are mandatory release capabilities.
- Rust production code denies unsafe code, unwraps, expects, and panics.

---

### Task 1: Scaffold the Rust workspace and shared capability vocabulary

**Files:**
- Create: `Cargo.toml`
- Create: `rust-toolchain.toml`
- Create: `crates/gascan-core/Cargo.toml`
- Create: `crates/gascan-core/src/lib.rs`
- Create: `crates/gascan-core/src/runtime.rs`
- Test: `crates/gascan-core/tests/runtime_capabilities.rs`

**Interfaces:**
- Produces: `RuntimeCapabilities`, `RuntimeVersion`, `NetworkIsolation`, and `RuntimeError` in `gascan_core::runtime`.
- Plan 2 defines `RuntimeBackend` after consuming these frozen capability meanings; Plan 3 implements it without changing them.

- [ ] **Step 1: Write the failing capability serialization test**

```rust
use gascan_core::runtime::{NetworkIsolation, RuntimeCapabilities, RuntimeVersion};

#[test]
fn capabilities_round_trip_without_backend_fields() {
    let value = RuntimeCapabilities {
        version: RuntimeVersion::new(1, 0, 0),
        bind_mounts: true,
        named_volumes: true,
        tty: true,
        signals: true,
        loopback_publish: true,
        resource_limits: true,
        offline: NetworkIsolation::Proven,
    };
    let json = serde_json::to_string(&value).unwrap();
    assert!(!json.contains("apple"));
    assert_eq!(serde_json::from_str::<RuntimeCapabilities>(&json).unwrap(), value);
}
```

- [ ] **Step 2: Run the test and verify the missing crate failure**

Run: `cargo test -p gascan-core --test runtime_capabilities`

Expected: FAIL because `gascan-core` or `gascan_core::runtime` does not exist.

- [ ] **Step 3: Add the workspace and minimal capability types**

```rust
#![forbid(unsafe_code)]

pub mod runtime;
```

```rust
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RuntimeVersion { pub major: u64, pub minor: u64, pub patch: u64 }

impl RuntimeVersion {
    pub const fn new(major: u64, minor: u64, patch: u64) -> Self { Self { major, minor, patch } }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkIsolation { Proven, Unsupported, Unverified }

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RuntimeCapabilities {
    pub version: RuntimeVersion,
    pub bind_mounts: bool,
    pub named_volumes: bool,
    pub tty: bool,
    pub signals: bool,
    pub loopback_publish: bool,
    pub resource_limits: bool,
    pub offline: NetworkIsolation,
}
```

Add workspace package metadata with `license = "AGPL-3.0-only"`, lint configuration matching the global constraints, and `serde`/`serde_json` dependencies.

- [ ] **Step 4: Run formatting and the capability test**

Run: `cargo fmt --all -- --check && cargo test -p gascan-core --test runtime_capabilities`

Expected: PASS, 1 test.

- [ ] **Step 5: Commit the shared vocabulary**

```bash
git add Cargo.toml rust-toolchain.toml crates/gascan-core
git commit -m "chore: scaffold Gas Can runtime contracts"
```

### Task 2: Implement an injectable Apple command runner

**Files:**
- Create: `crates/gascan-apple/Cargo.toml`
- Create: `crates/gascan-apple/src/lib.rs`
- Create: `crates/gascan-apple/src/command.rs`
- Test: `crates/gascan-apple/tests/command_runner.rs`

**Interfaces:**
- Consumes: `RuntimeError` from `gascan_core::runtime`.
- Produces: `CommandSpec { program, args, stdin }`, `CommandOutput { status, stdout, stderr }`, and async trait `CommandRunner::run(&self, spec) -> Result<CommandOutput, RuntimeError>`.
- Produces: `ProcessRunner`, the only production implementation allowed to spawn Apple commands.

- [ ] **Step 1: Write a fake-runner contract test**

```rust
#[tokio::test]
async fn command_spec_keeps_arguments_literal() {
    let runner = RecordingRunner::returning(0, br#"{}"#, b"");
    let spec = CommandSpec::new("container", ["inspect", "name; touch /tmp/nope", "--format", "json"]);
    let output = runner.run(spec).await.unwrap();
    assert_eq!(output.status, 0);
    assert_eq!(runner.calls()[0].args[1], "name; touch /tmp/nope");
}
```

- [ ] **Step 2: Run and observe the unresolved interface failure**

Run: `cargo test -p gascan-apple --test command_runner`

Expected: FAIL because `CommandSpec` and `CommandRunner` are undefined.

- [ ] **Step 3: Implement the runner with Tokio**

```rust
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandSpec { pub program: String, pub args: Vec<String>, pub stdin: Vec<u8> }

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandOutput { pub status: i32, pub stdout: Vec<u8>, pub stderr: Vec<u8> }

#[async_trait::async_trait]
pub trait CommandRunner: Send + Sync {
    async fn run(&self, spec: CommandSpec) -> Result<CommandOutput, RuntimeError>;
}
```

Implement `ProcessRunner` using `tokio::process::Command`, `.args(&spec.args)`, piped stdio, and `wait_with_output`. Map spawn, stdin, signal termination, and non-UTF8 diagnostics into typed `RuntimeError` variants without logging environment variables.

- [ ] **Step 4: Run runner tests and Clippy**

Run: `cargo test -p gascan-apple --test command_runner && cargo clippy -p gascan-apple --all-targets -- -D warnings`

Expected: PASS and no warnings.

- [ ] **Step 5: Commit the command boundary**

```bash
git add Cargo.toml crates/gascan-apple
git commit -m "feat: add injectable Apple command runner"
```

### Task 3: Probe and validate Apple runtime versions

**Files:**
- Create: `crates/gascan-apple/src/probe.rs`
- Create: `crates/gascan-apple/tests/fixtures/system-version-1.0.0.json`
- Create: `crates/gascan-apple/tests/fixtures/system-version-unsupported.json`
- Test: `crates/gascan-apple/tests/probe.rs`

**Interfaces:**
- Consumes: `CommandRunner` and shared capability types.
- Produces: `AppleProbe<R>::version() -> Result<RuntimeVersion, RuntimeError>` and `AppleProbe<R>::base_capabilities() -> Result<RuntimeCapabilities, RuntimeError>`.
- Supported range: major version exactly `1`; later majors fail until fixtures and live tests approve them.

- [ ] **Step 1: Add fixture-driven tests**

```rust
#[tokio::test]
async fn accepts_supported_major_and_rejects_future_major() {
    let supported = probe_with_fixture("system-version-1.0.0.json").await;
    assert_eq!(supported.unwrap().version, RuntimeVersion::new(1, 0, 0));
    let future = probe_with_fixture("system-version-unsupported.json").await;
    assert!(matches!(future, Err(RuntimeError::UnsupportedVersion { .. })));
}
```

- [ ] **Step 2: Verify the parser test fails**

Run: `cargo test -p gascan-apple --test probe`

Expected: FAIL because `AppleProbe` is undefined.

- [ ] **Step 3: Implement strict structured parsing**

Define a private Serde struct matching the captured `container system version` JSON. Reject missing semantic-version fields and trailing schema substitutions. Initialize unproven live capabilities as `false` or `NetworkIsolation::Unverified`; tests in later tasks are the only place that promotes them.

- [ ] **Step 4: Verify fixtures and public API docs**

Run: `cargo test -p gascan-apple --test probe && cargo doc -p gascan-apple --no-deps`

Expected: PASS; documentation builds without warnings.

- [ ] **Step 5: Commit and declare Roadmap Gate 1 ready for review**

```bash
git add crates/gascan-apple crates/gascan-core
git commit -m "feat: probe Apple container capabilities"
```

### Task 4: Prove lifecycle, mounts, volumes, resources, and ports

**Files:**
- Create: `crates/gascan-apple/tests/live/common/mod.rs`
- Create: `crates/gascan-apple/tests/live.rs`
- Create: `crates/gascan-apple/tests/live/lifecycle.rs`
- Create: `crates/gascan-apple/tests/live/storage.rs`
- Create: `crates/gascan-apple/tests/live/resources.rs`
- Create: `scripts/apple-test-preflight.sh`

**Interfaces:**
- Consumes: `ProcessRunner`, Apple `container` 1.x, temporary directories.
- Produces: `LiveContext`, which records every created container, volume, network, and temporary path and deletes only those records in `Drop`/explicit async cleanup.

- [ ] **Step 1: Write ignored live tests before command forms**

```rust
#[tokio::test]
#[ignore = "requires Apple silicon macOS 26+ with container service"]
async fn bind_mount_is_exact_and_named_volume_persists() -> Result<(), TestError> {
    let ctx = LiveContext::new("storage").await?;
    ctx.write_host("visible.txt", "host").await?;
    ctx.run_workspace("printf changed > /workspace/visible.txt").await?;
    assert_eq!(ctx.read_host("visible.txt").await?, "changed");
    assert!(ctx.exec("test ! -e /workspace/../forbidden").await?.success());
    ctx.write_cache("sentinel", "persisted").await?;
    ctx.recreate_container().await?;
    assert_eq!(ctx.read_cache("sentinel").await?, "persisted");
    ctx.cleanup().await
}
```

- [ ] **Step 2: Run preflight and one ignored test to record the baseline failure**

Run: `./scripts/apple-test-preflight.sh && cargo test -p gascan-apple --test live -- --ignored --test-threads=1 bind_mount_is_exact_and_named_volume_persists`

Expected: test FAILS at the first unimplemented live helper, while preflight prints the exact macOS, architecture, and Apple CLI versions.

- [ ] **Step 3: Implement live helpers with proven CLI arguments**

Use `container run --name <owned-name> --mount source=<temp>,target=/workspace --volume <owned-volume>:/opt/gascan ...`, structured `inspect`, explicit CPU/memory arguments, and loopback-only `--publish 127.0.0.1:<host>:<guest>`. Add tests for idempotent stop/start, inspect, resource observation inside the guest, loopback access, and rejection of non-loopback publishing by Gas Can command construction.

- [ ] **Step 4: Run the complete storage/resource suite serially**

Run: `cargo test -p gascan-apple --test live -- --ignored --test-threads=1`

Expected: PASS; `container list --all --format json` contains no IDs with the current test prefix after cleanup.

- [ ] **Step 5: Commit the proven command forms**

```bash
git add crates/gascan-apple/tests/live scripts/apple-test-preflight.sh
git commit -m "test: prove Apple lifecycle and storage capabilities"
```

### Task 5: Prove TTY, signal, and exact exit behavior through the Apple attach helper

**Files:**
- Create: `crates/gascan-apple/src/attach.rs`
- Create: `crates/gascan-apple/src/helper_protocol.rs`
- Create: `helpers/apple-attach/Package.swift`
- Create: `helpers/apple-attach/Sources/GasCanAppleAttach/main.swift`
- Create: `helpers/apple-attach/Sources/GasCanAppleAttach/Protocol.swift`
- Create: `helpers/apple-attach/Tests/GasCanAppleAttachTests/ProtocolTests.swift`
- Create: `scripts/build-apple-attach-helper.sh`
- Create: `crates/gascan-apple/tests/live/attach.rs`
- Test: `crates/gascan-apple/tests/attach_protocol.rs`

**Interfaces:**
- Produces: `AttachInput::{Stdin(Vec<u8>), Resize { rows, cols }, Signal(i32), Close}` and `AttachOutput::{Stdout(Vec<u8>), Stderr(Vec<u8>), Exit(i32)}`.
- Produces: `AppleAttach::exec(container, argv, tty) -> AttachSession`; Plan 3 uses this interface for daemon streaming.
- Produces: `gascan-apple-attach`, a single-session helper using a versioned newline-delimited JSON protocol with base64 byte payloads. The helper retains the Apple guest process identity and is the only component allowed to call `ClientProcess.resize`, `ClientProcess.kill`, and `ClientProcess.wait`.
- Requires: the helper dependency is pinned to Apple `container` 1.1.0, and its protocol version is validated before the guest process starts.

- [ ] **Step 1: Write protocol and ignored live tests**

```rust
#[tokio::test]
#[ignore = "requires supported Apple runtime"]
async fn attached_process_reports_resize_signal_and_exit() -> Result<(), TestError> {
    let mut session = live_workspace().await?.attach(["sh", "-c", "trap 'exit 42' TERM; stty size; sleep 30"], true).await?;
    session.send(AttachInput::Resize { rows: 41, cols: 113 }).await?;
    assert!(session.read_until(b"41 113").await?.is_some());
    session.send(AttachInput::Signal(libc::SIGTERM)).await?;
    assert_eq!(session.exit().await?, 42);
    Ok(())
}
```

- [ ] **Step 2: Verify unit serialization passes only after types exist and live behavior fails**

Run: `cargo test -p gascan-apple --test attach_protocol && cargo test -p gascan-apple --test live attached_process_reports_resize_signal_and_exit -- --ignored --test-threads=1`

Expected: initial FAIL for missing attach implementation.

- [ ] **Step 3: Implement the scoped Swift attach helper and Rust bridge**

Build the helper against Apple `container` 1.1.0's public `ContainerAPIClient`, pinned exactly in `Package.swift`. It accepts one `start` frame, creates one guest process with private pipes, and then accepts only stdin, resize, signal, and close frames for that process. It emits stdout/stderr bytes, typed errors, and exactly one exit frame from `ClientProcess.wait()`. It must not expose lifecycle, image, registry, mount, network, or arbitrary XPC operations. The Rust bridge spawns the helper with literal argv, validates protocol version and signals, applies backpressure, owns helper cleanup, and never infers guest state or exit status from helper process text/status. Cross-platform Rust tests use a fake helper; Swift and live integration tests run only on a supported Mac.

- [ ] **Step 4: Run attach tests**

Run: `swift test --package-path helpers/apple-attach && cargo test -p gascan-apple --test attach_protocol && ./scripts/build-apple-attach-helper.sh && cargo test -p gascan-apple --test live attach -- --ignored --test-threads=1`

Expected: PASS for binary I/O, resize, SIGINT/SIGTERM, disconnect, and exit codes 0, 42, and 127.

- [ ] **Step 5: Commit the attachment proof**

```bash
git add helpers/apple-attach scripts/build-apple-attach-helper.sh crates/gascan-apple/src/attach.rs crates/gascan-apple/src/helper_protocol.rs crates/gascan-apple/tests
git commit -m "feat: prove Apple process attachment semantics"
```

### Task 6: Prove offline isolation and publish the feasibility report

**Files:**
- Create: `crates/gascan-apple/tests/live/network.rs`
- Create: `tests/fixtures/network/host-server.rs`
- Create: `docs/feasibility/apple-container-template.md`
- Create at execution: `docs/feasibility/apple-container-report.md`
- Modify: `crates/gascan-apple/src/probe.rs`

**Interfaces:**
- Consumes: the live context and Apple network commands proven here.
- Produces: `NetworkIsolation::Proven` only after all offline probes pass.
- Produces: a feasibility report listing command forms and observed evidence for every `RuntimeCapabilities` field.

- [ ] **Step 1: Write the adversarial offline test**

```rust
#[tokio::test]
#[ignore = "requires supported Apple runtime"]
async fn offline_workspace_cannot_reach_external_or_host_networks() -> Result<(), TestError> {
    let ctx = LiveContext::offline("network").await?;
    for target in ["https://example.com", "http://192.0.2.1", ctx.host_probe_url()] {
        assert!(!ctx.curl(target).await?.success(), "offline target unexpectedly reachable: {target}");
    }
    assert!(ctx.exec("test -d /workspace && test -n \"$(ip link show lo)\"").await?.success());
    ctx.cleanup().await
}
```

- [ ] **Step 2: Run it before marking offline capability proven**

Run: `cargo test -p gascan-apple --test live -- --ignored --test-threads=1 offline_workspace_cannot_reach_external_or_host_networks`

Expected: FAIL until the adapter uses an Apple-supported no-network configuration that survives guest-root attempts to add routes/interfaces.

- [ ] **Step 3: Implement fail-closed capability detection**

Use the exact no-network mechanism proven by the live experiment. If it is absent or ambiguous, return `NetworkIsolation::Unsupported`; never substitute DNS blocking. Add a negative fixture proving that `up --offline` would be rejected before a mount is constructed.

- [ ] **Step 4: Run all checks and write the report from observed results**

Run: `cargo test --workspace && cargo test -p gascan-apple --test live -- --ignored --test-threads=1 && cargo clippy --workspace --all-targets -- -D warnings`

Expected: PASS. The report has no unresolved sections, documents the tested OS/CLI/image digest, and maps every capability to a passing test name. If offline or another mandatory capability fails, record `BLOCKED` and stop before Plans 3-4 integration.

- [ ] **Step 5: Commit the Gate 2 evidence**

```bash
git add crates/gascan-apple tests/fixtures/network docs/feasibility
git commit -m "docs: record Apple runtime feasibility"
```
