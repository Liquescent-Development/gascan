# Apple Backend and Lifecycle Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the production Apple `container` backend and prove the complete Gas Can lifecycle on a supported Mac.

**Architecture:** `AppleBackend<R: CommandRunner>` translates backend-neutral requests into the exact structured command forms approved by the feasibility report. `gascand` selects it in production while tests reuse the core backend contract and a live harness with uniquely owned resources.

**Tech Stack:** Rust 1.85+ edition 2024, Tokio, Apple `container` 1.x, Serde structured output, Tonic daemon API, existing Gas Can core contracts.

## Global Constraints

- Begin only after Roadmap Gates 2 and 3 pass.
- Implement only command forms documented in the approved Apple feasibility report.
- Never invoke a shell or parse human table output.
- Every created runtime resource carries Gas Can ownership metadata and a stable sandbox ID.
- Destructive operations act only on resources whose ownership and expected identity both match.
- Offline mode must fail before mount request construction when capability proof is unavailable.
- Live tests use temporary code roots and serial execution; they never touch the operator's real workspaces.
- Rust production code denies unsafe code, unwraps, expects, and panics.

---

### Task 1: Translate image, mount, volume, resource, and network requests

**Files:**
- Create: `crates/gascan-apple/src/translate.rs`
- Modify: `crates/gascan-apple/src/lib.rs`
- Test: `crates/gascan-apple/tests/translate.rs`
- Test fixtures: `crates/gascan-apple/tests/fixtures/*.json`

**Interfaces:**
- Consumes: `CreateRequest` from `gascan_core::runtime`.
- Produces: `AppleCommandBuilder::pull(image)`, `create(request)`, and `inspect(id)` returning literal `CommandSpec` values.
- Translation accepts only requests already approved by `PolicyCompiler` and still revalidates ownership-sensitive invariants.

- [ ] **Step 1: Write exact argument-vector tests**

```rust
#[test]
fn create_uses_one_workspace_mount_loopback_ports_and_owned_volumes() {
    let request = CreateRequest::fixture(SandboxId::test("code"));
    let spec = AppleCommandBuilder::create(&request).unwrap();
    assert_eq!(spec.program, "container");
    assert!(spec.args.windows(2).any(|p| p == ["--mount", "source=/tmp/code,target=/workspace"]));
    assert!(spec.args.iter().any(|a| a == "127.0.0.1:3000:3000"));
    assert!(!spec.args.join(" ").contains("/Users/tester"));
}
```

- [ ] **Step 2: Run translation tests and observe missing builder**

Run: `cargo test -p gascan-apple --test translate`

Expected: FAIL because `AppleCommandBuilder` is undefined.

- [ ] **Step 3: Implement deterministic translation**

Follow the feasibility report exactly for image references, create/run distinction, labels/annotations, init, user, bind mount, named volumes, CPU/memory/disk/process controls, offline/networked mode, and loopback ports. Sort maps before generating repeated arguments. Reject extra bind mounts, non-loopback host addresses, missing image digests, unknown networks, and unowned names with typed errors.

- [ ] **Step 4: Run translation and fixture tests**

Run: `cargo test -p gascan-apple --test translate --test probe`

Expected: PASS; snapshots contain literal argv arrays and no shell quoting.

- [ ] **Step 5: Commit request translation**

```bash
git add crates/gascan-apple
git commit -m "feat: translate Gas Can policy to Apple runtime"
```

### Task 2: Implement inspection and owned-resource discovery

**Files:**
- Create: `crates/gascan-apple/src/inspect.rs`
- Test: `crates/gascan-apple/tests/fixtures/container-running-1.0.json`
- Test: `crates/gascan-apple/tests/fixtures/container-stopped-1.0.json`
- Test: `crates/gascan-apple/tests/fixtures/container-list-mixed-1.0.json`
- Test: `crates/gascan-apple/tests/inspect.rs`

**Interfaces:**
- Produces: `AppleInspector<R>::inspect(&SandboxId) -> Result<Option<RuntimeSandbox>, RuntimeError>`.
- Produces: `AppleInspector<R>::list_resources() -> Result<Vec<RuntimeResource>, RuntimeError>` with owned, foreign, and mismatched classifications and fresh opaque process-local removal proofs. Resources are not deserializable; removal must re-inventory and revalidate the proof against current Apple state.
- Unknown fields are tolerated; absent/invalid required identity and state fields are errors.

- [ ] **Step 1: Write fixture tests for state and ownership**

```rust
#[tokio::test]
async fn mixed_list_returns_only_valid_gascan_owned_resources() {
    let inspector = inspector_with_fixture("container-list-mixed-1.0.json");
    let resources = inspector.list_resources().await.unwrap();
    assert_eq!(owned.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["code-a1b2c3"]);
}
```

- [ ] **Step 2: Verify parser tests fail**

Run: `cargo test -p gascan-apple --test inspect`

Expected: FAIL because inspector types are absent.

- [ ] **Step 3: Implement private Apple DTOs and explicit domain mapping**

Parse versioned JSON fixtures into private Serde DTOs. Map Apple states to `Creating`, `Running`, `Stopped`, or typed `UnknownActualState`; validate both ownership label and sandbox digest annotation. Treat a CLI not-found result as `Ok(None)` only for the exact documented error code.

- [ ] **Step 4: Run parser and malformed-fixture tests**

Run: `cargo test -p gascan-apple --test inspect`

Expected: PASS for running/stopped/missing/mixed lists, unknown fields, malformed IDs, absent labels, and unsupported states.

- [ ] **Step 5: Commit structured inspection**

```bash
git add crates/gascan-apple/src/inspect.rs crates/gascan-apple/tests
git commit -m "feat: inspect Apple runtime state safely"
```

### Task 3: Implement lifecycle mutations and ownership-safe cleanup

**Files:**
- Create: `crates/gascan-apple/src/backend.rs`
- Modify: `crates/gascan-apple/src/lib.rs`
- Test: `crates/gascan-apple/tests/backend_fake_runner.rs`
- Test: `crates/gascan-apple/tests/live/backend_contract.rs`

**Interfaces:**
- Produces: `AppleBackend<R>` implementing all non-attach `RuntimeBackend` methods.
- Consumes: command builder, inspector, runner, and attachment implementation.

- [ ] **Step 1: Apply the shared backend contract to AppleBackend with a scripted runner**

```rust
#[tokio::test]
async fn apple_backend_satisfies_runtime_contract() {
    let runner = StatefulAppleRunner::new();
    let backend = AppleBackend::new(runner);
    gascan_core::runtime::tests::backend_contract(&backend).await;
}

#[tokio::test]
async fn remove_refuses_identity_mismatch() {
    let backend = backend_reporting_wrong_digest();
    let error = backend.remove(exact_remove_request).await.unwrap_err();
    assert_eq!(error.code(), "ownership_mismatch");
}
```

- [ ] **Step 2: Verify the backend trait is not implemented**

Run: `cargo test -p gascan-apple --test backend_fake_runner`

Expected: FAIL at `AppleBackend: RuntimeBackend`.

- [ ] **Step 3: Implement pull/create/start/stop/remove/log/list operations**

Run one command at a time through `CommandRunner`, parse structured results, make start/stop idempotent after inspection, and inspect immediately before remove. Remove child resources only when their recorded owner and sandbox identity match. Map documented transient daemon/runtime errors separately from permanent policy/compatibility errors.

- [ ] **Step 4: Run scripted and live contract tests**

Run: `cargo test -p gascan-apple --test backend_fake_runner && cargo test -p gascan-apple --test live backend_contract -- --ignored --test-threads=1`

Expected: PASS; live cleanup leaves no current-prefix resources.

- [ ] **Step 5: Commit lifecycle implementation**

```bash
git add crates/gascan-apple
git commit -m "feat: implement Apple runtime lifecycle backend"
```

### Task 4: Connect byte-stream attachment to daemon sessions

**Files:**
- Modify: `crates/gascan-apple/src/attach.rs`
- Modify: `crates/gascan-apple/src/backend.rs`
- Modify: `crates/gascand/src/api.rs`
- Test: `crates/gascan-apple/tests/live/attach_backend.rs`
- Test: `crates/gascand/tests/attach_bridge.rs`

**Interfaces:**
- Consumes: `ClientFrame`, `ServerFrame`, and `ExecSession` from Plans 1-2.
- Produces: cancellation-safe bridge that sends exactly one terminal `Exit` or `Error` frame.

- [ ] **Step 1: Write an RPC-to-runtime bridge test**

```rust
#[tokio::test]
async fn bridge_preserves_binary_streams_and_exact_exit() -> TestResult {
    let runtime = FakeRuntime::exec_script([AttachOutput::Stdout(vec![0, 255]), AttachOutput::Exit(42)]);
    let mut client = attach_through_daemon(runtime).await?;
    assert_eq!(client.next().await?, ServerFrame::Stdout(vec![0, 255]));
    assert_eq!(client.next().await?, ServerFrame::Exit(42));
    assert!(client.next().await.is_none());
    Ok(())
}
```

- [ ] **Step 2: Verify bridge cancellation/exit tests fail**

Run: `cargo test -p gascand --test attach_bridge`

Expected: FAIL because the API does not yet delegate real attach sessions.

- [ ] **Step 3: Implement bidirectional bridging**

Forward stdin, resize, the verified signal support matrix, and close independently from stdout/stderr/exit. For pinned Apple 1.1.0, translate TTY SIGINT to byte `0x03` and promptly reject non-TTY SIGINT and all other signals as unsupported. On client disconnect close input and cancel the guest process according to documented semantics. On daemon shutdown use the documented backend lifecycle path, wait a bounded grace period, and return a typed error if forced termination is necessary.

- [ ] **Step 4: Run fake and live attach tests**

Run: `cargo test -p gascand --test attach_bridge && cargo test -p gascan-apple --test live attach_backend -- --ignored --test-threads=1`

Expected: PASS for binary data, backpressure, resize, TTY SIGINT, prompt typed rejection of unsupported signals, disconnect, daemon shutdown, and exact exit 42 for a process that starts.

- [ ] **Step 5: Commit attachment integration**

```bash
git add crates/gascan-apple crates/gascand
git commit -m "feat: bridge Apple sessions through gascand"
```

### Task 5: Select the production backend and implement doctor

**Files:**
- Modify: `crates/gascand/src/main.rs`
- Modify: `crates/gascand/src/service.rs`
- Modify: `crates/gascan/src/cli.rs`
- Create: `crates/gascan-core/src/doctor.rs`
- Test: `crates/gascand/tests/backend_selection.rs`
- Test: `crates/gascan-e2e/tests/doctor.rs`

**Interfaces:**
- Produces: production `gascand` with `AppleBackend<ProcessRunner>`; tests select fake backend only through an explicit test-only flag/environment guarded by debug assertions.
- Produces: `DoctorReport { checks: Vec<DoctorCheck> }` with stable check IDs and remedies.

- [ ] **Step 1: Write doctor output tests**

```rust
#[test]
fn doctor_reports_offline_capability_as_release_blocker() {
    let report = doctor_with(NetworkIsolation::Unsupported);
    let check = report.check("runtime.offline").unwrap();
    assert_eq!(check.status, DoctorStatus::Fail);
    assert!(check.remedy.contains("supported Apple container"));
}
```

- [ ] **Step 2: Verify doctor and selection tests fail**

Run: `cargo test -p gascand --test backend_selection && cargo test -p gascan-e2e --test doctor`

Expected: FAIL because production selection and doctor report are absent.

- [ ] **Step 3: Implement preflight checks and backend construction**

Check `aarch64`, macOS major >= 26, CLI presence/version, Apple service response, kernel readiness, supported structured schema, free state/image disk, workspace accessibility, and every mandatory capability. Render human and JSON forms. Ensure offline unavailability and unsupported CLI versions fail before `PolicyCompiler` receives a root.

- [ ] **Step 4: Run doctor tests and a live doctor**

Run: `cargo test -p gascand --test backend_selection && cargo test -p gascan-e2e --test doctor && cargo run -p gascan -- doctor --json`

Expected: tests PASS; live output is valid JSON with all supported-host checks passing.

- [ ] **Step 5: Commit production selection**

```bash
git add crates/gascan-core crates/gascand crates/gascan crates/gascan-e2e/tests/doctor.rs
git commit -m "feat: select and diagnose Apple runtime"
```

### Task 6: Exercise complete real CLI lifecycle and reconciliation

**Files:**
- Create: `crates/gascan-e2e/tests/apple_lifecycle.rs`
- Create: `crates/gascan-e2e/tests/apple_recovery.rs`
- Create: `scripts/run-apple-e2e.sh`
- Modify: `crates/gascand/src/reconcile.rs` only for failures exposed by tests

**Interfaces:**
- Consumes: installed/current-workspace `gascan`, `gascand`, supported Apple runtime, and temporary host roots.
- Produces: a serial real-runtime suite and sanitized test transcript suitable for Roadmap Gate 4 evidence.

- [ ] **Step 1: Write the full ignored scenario**

```rust
#[tokio::test]
#[ignore = "requires supported Apple runtime"]
async fn cli_lifecycle_survives_daemon_and_host_state_changes() -> TestResult {
    let env = AppleE2e::new().await?;
    env.gascan(["up", env.root()]).success();
    env.gascan(["run", "--", "sh", "-c", "exit 42"]).code(42);
    env.kill_daemon().await?;
    env.gascan(["status", "--json"]).json("$.actual_state", "running");
    env.gascan(["down"]).success();
    env.gascan(["shell", "--command", "id -u"]).stdout("1000\n");
    env.gascan(["destroy", "--yes"]).success();
    env.assert_no_owned_resources().await
}
```

- [ ] **Step 2: Run the new suite to expose integration gaps**

Run: `./scripts/run-apple-e2e.sh apple_lifecycle`

Expected: initial FAIL at the first incomplete real-runtime path, with cleanup still succeeding.

- [ ] **Step 3: Fix only observed contract mismatches**

Use typed adapter changes or reconciliation corrections; do not leak Apple DTOs upward or weaken policy. Add a fixture/unit regression before each production fix discovered by live execution.

- [ ] **Step 4: Run lifecycle, recovery, and global Rust gates**

Run: `./scripts/run-apple-e2e.sh && cargo fmt --all -- --check && cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo test --workspace`

Expected: PASS for create, idempotent up, run, shell, TTY, exit, signal, down/start, daemon kill, stale metadata, an injected no-op apply hook, destroy, and owned cleanup.

- [ ] **Step 5: Commit Roadmap Gate 4 evidence**

```bash
git add crates scripts/run-apple-e2e.sh docs/evidence
git commit -m "test: pass Apple lifecycle release gate"
```
