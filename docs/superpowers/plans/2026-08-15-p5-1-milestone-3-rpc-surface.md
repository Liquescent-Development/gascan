# P5.1 Milestone 3 — Finishing the RPC Surface Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make every RPC in `arca.engine.v1.SandboxEngine` answer for real, so that no method returns `unsupported_capability` and P5's exit criterion becomes reachable.

**Architecture:** All production code is Swift in `~/code/arca`. Gas Can's half is already implemented and tested (design §2.1), so Gas Can's only contribution is `#[ignore]`d live tests in `crates/gascan-arca/tests/live/`. `CreateContainer` reuses `Create`'s reviewed machinery; `Exec` puts its logic in two adapters that Arca's VM-free suite can drive; `Logs` reads the container's existing JSON-lines log.

**Tech Stack:** Swift 6, grpc-swift 2, XCTest, `ContainerBridge` (Arca's own), the `containerization` submodule, Rust/`cargo` for the live tier.

**Design:** `docs/superpowers/specs/2026-08-15-p5-1-milestone-3-rpc-surface-design.md` (approved 2026-08-15)

## Global Constraints

- **Never commit to `main`.** All work reaches `main` through a pull request.
- **Signing is inverted between the repositories.** Gas Can: `env -u SSH_AUTH_SOCK git commit` (its `user.signingkey` is a file path). **Arca: the key is in 1Password, so `env -u SSH_AUTH_SOCK` breaks every commit with `unable to sign` — commit normally.** Never `--no-gpg-sign`. Verify `%G?` is `G`. No co-author trailer and no AI-tool mention in any commit message.
- **`RUSTUP_TOOLCHAIN=1.95.0` is exported** and overrides `rust-toolchain.toml`. Prefix every cargo command with `env -u RUSTUP_TOOLCHAIN`. Use `--no-fail-fast`. `cargo clippy --fix` is prohibited.
- **Never run the workspace suite while any other cargo is running.** Check `pgrep -fl "cargo test"` is empty first and record the output. Run alone it takes ~93 seconds.
- **Re-derive every line anchor immediately before editing.** Anchors in `SandboxEngineService.swift`, `ContainerManager.swift` and `NetworkManager.swift` moved under every task of milestone 2. Every anchor in this plan was derived 2026-08-15 against Gas Can `e9468d8` and Arca `b3ffdf5` with submodule `3f68806`; assume it has drifted.
- **No `ContainerBridge` parameter takes a default.** A default is how a caller silently keeps the old behaviour after the reason for it has gone.
- **Before writing a claim into a commit message, a source comment or a report, ask what mutation would falsify it and whether a test already fails under that mutation.** If none does, write the test or write the weaker claim.
- **Mutate the production default, not the seam; mutate the call site, not only the function.** A test that drives an injected stub proves the stub.
- **The engine must be ad-hoc signed or it never creates a socket.** After any `swift build --product arca-engine`:
  ```bash
  codesign --force --sign - --options runtime --timestamp \
    --entitlements Arca.entitlements .build/arm64-apple-macosx/debug/arca-engine
  ```
- **If the engine dies with `vmnet_return_t(rawValue: 1001)`, force-quit the `InternetSharing` process** by PID (Activity Monitor, or `kill <pid>`). **Never `pkill -f`.** `launchctl kickstart` does not work under SIP.
- **The live tier's four environment variables**, none defaulted:
  ```bash
  export GASCAN_ARCA_ENGINE_BIN=~/code/arca/.build/arm64-apple-macosx/debug/arca-engine
  export GASCAN_ARCA_KERNEL_PATH=$HOME/.arca/vmlinux
  export GASCAN_ARCA_VMINIT_LAYOUT=$HOME/.arca/vminit
  export GASCAN_ARCA_BASE_OCI_LAYOUT=/tmp/alpine-oci
  ```
- **Live tests stay `#[ignore]`d.** `scripts/build-arca-engine.sh` builds the pin; the pin bump is milestone 4's. "Run the tier at least once" means a local run against a branch build, recorded with its command and output.
- **Baseline to hold or account for:** `swift test --filter ArcaEngineTests` = `Executed 160 tests, with 0 failures`; `swift test --filter ArcaTests.NetworkPruneGateTests` = 3 tests, 0 failures; `env -u RUSTUP_TOOLCHAIN cargo test --workspace --no-fail-fast` = 1436 passed / 0 failed / 36 ignored across 74 targets. **A green figure you cannot account for is not a pass.**

---

## File Structure

**Arca — `~/code/arca`**

| File | Responsibility |
|---|---|
| `Sources/ArcaEngine/SandboxEngineService.swift` | The eleven RPC methods. Translate in, call `ContainerBridge`, translate out. Modified by tasks 1, 5, 6. |
| `Sources/ArcaEngine/EngineServer.swift` | Server lifecycle. Gains `runUntilQuiesced` in task 2. |
| `Sources/arca-engine/ArcaEngineCommand.swift` | The executable. Loses the shutdown wait to task 2. |
| `Sources/ArcaEngine/ExecSession.swift` | **New, task 6.** The bidi state machine and its two stream adapters. |
| `Sources/ArcaEngine/LogReader.swift` | **New, task 5.** Parses the JSON-lines container log and filters by timestamp. |
| `Sources/ContainerBridge/ExecManager.swift` | Gains `signalExec` in task 4. |
| `Sources/ContainerBridge/LogWriter.swift` | Gains fractional-second timestamps in task 5. |
| `containerization/Sources/Containerization/Image/Unpacker/OverlayFSUnpacker.swift` | Read-only in task 3; the test drives its public entry point. |

**Gas Can — `~/code/gascan`**

| File | Responsibility |
|---|---|
| `crates/gascan-arca/tests/live/recreate.rs` | **New, task 1.** `CreateContainer` against a real engine. |
| `crates/gascan-arca/tests/live/logs.rs` | **New, task 5.** |
| `crates/gascan-arca/tests/live/exec.rs` | **New, task 6.** Including the `tty` and `signals` proofs. |
| `crates/gascan-arca/tests/live/read_rpcs.rs` | Modified by task 6; its unimplemented-method count reaches zero. |

---

## A NOTE ON THIS PLAN'S SHAPE — READ IT BEFORE TASK 1

**Landing 1 (tasks 1-3) is expanded to the step. Landings 2 and 3 (tasks 4-6) carry their files, interfaces, tests and acceptance criteria but not their step-by-step code, and that is deliberate.**

Milestone 2's own record is the reason, and it is evidence rather than preference: that plan's landings 3, 4 and 5 were each expanded *after* the task preceding them ran, "so they reflect what the machine actually does rather than what the code appeared to say" — and where it did guess ahead, **nine blocks of its Swift and shell were wrong**, every one marked as a guess and every one surfacing as a correction. The worst would have been silent.

`Exec`'s adapters sit on `Writer` and `ReaderStream` protocols and on grpc-swift's streaming API at this repository's pinned version. Writing exact Swift for them before `signalExec` and `Logs` have been built against the same machinery is how this project produced those nine blocks.

**Expand task 4 after task 3 lands, and tasks 5-6 after task 4 lands.** Do not expand them from reading alone.

---

# LANDING 1 — `CreateContainer`, and the two carried follow-ups

## Task 1: `CreateContainer` creates the container and only the container

**Files:**
- Modify: `~/code/arca/Sources/ArcaEngine/SandboxEngineService.swift` — `create(request:)` at `:324-418`, the `createContainer` stub at `:674-687`
- Test: `~/code/arca/Tests/ArcaEngineTests/CreateTests.swift`
- Test: `~/code/gascan/crates/gascan-arca/tests/live/recreate.rs` (create), `~/code/gascan/crates/gascan-arca/tests/live/main.rs` or the live target's module list (modify, to declare `mod recreate;`)

**Interfaces:**
- Consumes: `createSpec(for:)` (`:446`, `package func`, returns `Result<SandboxContainerSpec, Arca_Engine_V1_EngineError>`); `createFailed(_:_:)` (`:461`); `createCatching(resource:_:)` (`:481`); `resourceMessage(kind:name:labels:)` (`EngineTranslation.swift:254`); `volumeManager.inspectVolume(name:)` (`VolumeManager.swift:297`, throws); `networkManager.getNetworkByName(name:)` (`NetworkManager.swift:696`, returns `NetworkMetadata?`)
- Produces: `private func buildContainer(spec:created:) async -> Arca_Engine_V1_CreateResponse` — the container phase, called by both `create(request:)` and `createContainer(request:)`.

- [ ] **Step 1: Re-derive the anchors**

```bash
cd ~/code/arca
grep -n "func create(request:\|func createContainer(request:\|func createSpec\|private static func createFailed\|private static func createCatching" \
  Sources/ArcaEngine/SandboxEngineService.swift
```

Expected: six lines. Record them; the numbers below are from 2026-08-15 and drift under every task.

- [ ] **Step 2: Write the failing test for the retained check**

Append to `Tests/ArcaEngineTests/CreateTests.swift`:

```swift
/// A recreate whose retained volume the engine does not hold is refused
/// before anything is built.
///
/// **The failure this prevents is silent.** A container attached to a volume
/// the engine no longer holds starts anyway and the mount is simply absent --
/// which is the exact shape of the named-volume defect of 2026-08-14, where
/// three volumes were attached, mounted somewhere unreachable, and nothing
/// refused. `not_found` naming the volume is the loud form of the same state.
func testCreateContainerRefusesARetainedResourceTheEngineDoesNotHold() async throws {
    let service = SandboxEngineService.forTesting()
    var request = Arca_Engine_V1_CreateContainerRequest()
    request.create = Self.validCreateRequest(sandboxID: "gascan-retained-missing")
    request.retained = [
        resourceMessage(kind: .volume, name: "a-volume-nothing-holds", labels: [:])
    ]

    let response = await service.createContainer(request: request)

    guard case .failed(let failure) = response.outcome else {
        return XCTFail("a retained resource the engine does not hold must be refused")
    }
    XCTAssertEqual(failure.error.code, "not_found")
    XCTAssertEqual(
        failure.error.resource,
        "a-volume-nothing-holds",
        "the resource field names the offender; prose goes in message"
    )
    XCTAssertTrue(
        failure.created.isEmpty,
        "the refusal runs before anything is built, so there is nothing to report"
    )
}
```

If `Self.validCreateRequest(sandboxID:)` does not exist in `CreateTests.swift`, use whatever that file's existing tests use to build a valid `CreateRequest` — find it with `grep -n "func .*[Cc]reateRequest" Tests/ArcaEngineTests/CreateTests.swift` — and match it exactly rather than writing a second builder.

- [ ] **Step 3: Run it to verify it fails**

```bash
cd ~/code/arca
swift test --filter ArcaEngineTests.CreateTests/testCreateContainerRefusesARetainedResourceTheEngineDoesNotHold
```

Expected: FAIL. The stub at `:674-687` returns `unsupported_capability`, so `failure.error.code` is `unsupported_capability`, not `not_found`.

- [ ] **Step 4: Extract the container phase from `create(request:)`**

`create(request:)` currently ends with the container build inline at `:393-417`. Replace those lines with a call, and add the extracted method beneath `create`:

```swift
/// The container phase of a create, shared by `Create` and `CreateContainer`.
///
/// **Extracted rather than duplicated, and the reason is a mutation that
/// survived.** `createSpec`'s comment records that a review replaced the one
/// line deciding the image reference with `references.first ?? …` and the whole
/// suite stayed green -- every sandbox would have recorded a tag and every
/// `Inspect` would have answered `invalid_output`. Two independent container
/// build paths would let exactly that drift back in on one of them.
private func buildContainer(
    spec: SandboxContainerSpec,
    created: [Arca_Engine_V1_Resource]
) async -> Arca_Engine_V1_CreateResponse {
    var created = created
    let container = await Self.createCatching(resource: spec.name) {
        try await self.containerManager.createContainer(
            image: spec.image,
            name: spec.name,
            entrypoint: nil,
            command: nil,
            env: spec.env,
            workingDir: nil,
            labels: spec.labels,
            networkMode: spec.networkMode,
            binds: spec.binds,
            portBindings: spec.portBindings,
            memory: spec.memory,
            nanoCpus: spec.nanoCpus,
            user: spec.user
        )
    }
    if case .failure(let error) = container {
        return Self.createFailed(created, error)
    }
    created.append(resourceMessage(kind: .container, name: spec.name, labels: spec.labels))

    return Arca_Engine_V1_CreateResponse.with { response in
        response.created = Arca_Engine_V1_Created.with { $0.created = created }
    }
}
```

And in `create(request:)`, replace `:393-417` with:

```swift
    return await buildContainer(spec: spec, created: created)
```

- [ ] **Step 5: Run the whole suite to prove the extraction changed nothing**

```bash
cd ~/code/arca
swift test --filter ArcaEngineTests
```

Expected: `Executed 160 tests, with 1 failure` — the 160 baseline holds and the only failure is the new test from step 2, which still fails because `createContainer` is untouched. **If any other test moved, stop:** the extraction was supposed to be behaviour-preserving and something else changed.

- [ ] **Step 6: Implement `createContainer`**

Replace the stub at `:674-687`:

```swift
/// See the note on the `create(request:)` overload above.
///
/// **The container only.** `engine.proto:296-302` states it: everything named
/// in `retained` already exists and is reused, so this creates no volume and no
/// network. Gas Can already enforces the other half --
/// `CreateOutcome::for_recreate` refuses an answer carrying the whole topology
/// (`crates/gascan-arca/tests/backend_unary.rs:740`) -- so an engine that
/// rebuilt a retained resource would be caught there rather than here.
func createContainer(
    request: Arca_Engine_V1_CreateContainerRequest
) async -> Arca_Engine_V1_CreateResponse {
    if let missing = await firstRetainedResourceNotHeld(request.retained) {
        return Self.createFailed([], missing)
    }

    let spec: SandboxContainerSpec
    switch await createSpec(for: request.create) {
    case .failure(let error):
        return Self.createFailed([], error)
    case .success(let translated):
        spec = translated
    }

    return await buildContainer(spec: spec, created: [])
}

/// The first retained resource this engine does not hold, or nil.
///
/// A store read, not a guess. Containers are not checked: the container is what
/// this RPC builds, so one appearing in `retained` is the caller's error and
/// `createContainer` will refuse it as a name conflict with a better message
/// than this could give.
private func firstRetainedResourceNotHeld(
    _ retained: [Arca_Engine_V1_Resource]
) async -> Arca_Engine_V1_EngineError? {
    for resource in retained {
        let name = resource.identity.name
        switch resource.identity.kind {
        case .volume:
            do {
                _ = try volumeManager.inspectVolume(name: name)
            } catch {
                return engineError(
                    .notFound,
                    resource: name,
                    message: "this engine holds no volume named \(name)"
                )
            }
        case .network:
            if await networkManager.getNetworkByName(name: name) == nil {
                return engineError(
                    .notFound,
                    resource: name,
                    message: "this engine holds no network named \(name)"
                )
            }
        default:
            continue
        }
    }
    return nil
}
```

- [ ] **Step 7: Run the tests to verify they pass**

```bash
cd ~/code/arca
swift test --filter ArcaEngineTests
```

Expected: `Executed 161 tests, with 0 failures` — the 160 baseline plus step 2's test. **Account for the increment; a green figure you cannot account for is not a pass.**

- [ ] **Step 8: Prove the retained check is load-bearing**

Comment out the `firstRetainedResourceNotHeld` call in `createContainer` and re-run:

```bash
cd ~/code/arca
swift test --filter ArcaEngineTests 2>&1 | tail -5
```

Expected: **1 failure**, the step-2 test. Restore the line and confirm 161 pass again. Record both numbers — this is the mutation that says the guard is real, and a guard nothing can falsify does not ship.

- [ ] **Step 9: Write the live test**

Create `~/code/gascan/crates/gascan-arca/tests/live/recreate.rs`:

```rust
//! `CreateContainer` against a real engine.
//!
//! **Data survival is the assertion, not a successful return.** A recreate that
//! quietly rebuilt its volumes would also return `Ok` and would also report one
//! container; what distinguishes reuse from a rebuild that happened to succeed
//! is that what was written before the recreate is still there after it. An
//! assertion on the response alone would pass against the defect.

use crate::common::{
    LiveEngine, answering, await_state, base_oci_layout, layout_running_with_directories,
    policy_request_from_manifest, read_from_loopback, report_section, reserve_port,
};
use camino::{Utf8Path, Utf8PathBuf};
use gascan_core::runtime::{
    ContainerState, RecreateRequest, RemoveRequest, ResourceKind, RetainedResources,
};
use std::time::Duration;

const TAG: &str = "recreate:latest";

/// Appends a line to a file on a managed volume, then serves the whole file.
///
/// Append rather than overwrite: after the recreate the file must hold BOTH
/// boots' lines, which distinguishes "the volume survived" from "the volume was
/// rebuilt and the second boot rewrote it" -- an overwrite makes those two
/// outcomes identical.
fn appending_and_reporting(destination: &Utf8Path, port: u16) -> Utf8PathBuf {
    let script = format!(
        "date +%s%N >> /home/workspace/.local/boots; {}",
        answering(port, "sh -c 'echo ---boots---; cat /home/workspace/.local/boots'")
    );
    layout_running_with_directories(
        &base_oci_layout(),
        destination,
        TAG,
        &["sh", "-c", &script],
        &["/home/workspace/.local"],
    )
}

#[tokio::test]
#[ignore = "needs a real engine, kernel and VM; see the live tier's four environment variables"]
async fn a_recreate_reuses_its_retained_volumes_rather_than_rebuilding_them() {
    let port = reserve_port();
    let images = tempfile::tempdir().expect("a temporary layout root");
    let layout =
        appending_and_reporting(Utf8Path::from_path(images.path()).expect("a utf-8 path"), port);
    let engine = LiveEngine::start_with_images(&[&layout]).await;
    let backend = backend(&engine).await;

    let (_root, request) =
        policy_request_from_manifest("recreating", &engine.image(TAG), MANIFEST);
    backend
        .prepare_image(request.image())
        .await
        .expect("the store holds the image the request names");

    let created = backend
        .create(request.clone())
        .await
        .expect("create against a seeded store must succeed");
    backend.start(request.id()).await.expect("start must boot");
    await_state(&backend, &request, ContainerState::Running, Duration::from_secs(180)).await;

    let first = read_from_loopback(port, Duration::from_secs(120)).await;
    let before = report_section(&first, "boots");
    assert_eq!(before.len(), 1, "the first boot writes one line: {first}");

    // Remove the container ALONE. Everything else is what `retained` names.
    let container: Vec<_> = created
        .created()
        .iter()
        .filter(|resource| resource.identity().kind() == ResourceKind::Container)
        .cloned()
        .collect();
    assert_eq!(container.len(), 1, "create reports exactly one container");

    backend.stop(request.id()).await.expect("stop must answer");
    await_state(&backend, &request, ContainerState::Stopped, Duration::from_secs(120)).await;
    backend
        .remove(RemoveRequest::from_resources(container).expect("gascan-owned"))
        .await
        .expect("removing the container alone must succeed");

    let retained_resources: Vec<_> = created
        .created()
        .iter()
        .filter(|resource| resource.identity().kind() != ResourceKind::Container)
        .cloned()
        .collect();
    let retained = RetainedResources::new(&request, retained_resources)
        .expect("the retained set matches the requested topology exactly");
    let recreate = RecreateRequest::new(request.clone(), retained).expect("a recreate request");

    let rebuilt = backend
        .create_container(recreate)
        .await
        .expect("CreateContainer must rebuild the container against retained resources");
    assert_eq!(
        rebuilt.created().len(),
        1,
        "a recreate rebuilds the container alone: {:?}",
        rebuilt.created()
    );

    backend.start(request.id()).await.expect("the rebuilt container must start");
    await_state(&backend, &request, ContainerState::Running, Duration::from_secs(180)).await;

    let second = read_from_loopback(port, Duration::from_secs(120)).await;
    let after = report_section(&second, "boots");
    assert_eq!(
        after.len(),
        2,
        "the retained volume must still hold the first boot's line; \
         a rebuilt volume would show only the second: {second}"
    );
}
```

**Match `MANIFEST`, `backend()` and `reserve_port` against `lifecycle.rs`, `mounts.rs` and `ports.rs` as they exist when this task runs** — every live module declares its own `MANIFEST` const and a local `backend()` helper, and the port-reservation helper's exact name must be taken from `ports.rs` rather than from this plan. Copy, do not reinvent: a second helper with the same job is how two tests drift apart.

- [ ] **Step 10: Run the live test**

```bash
cd ~/code/arca && swift build --product arca-engine && codesign --force --sign - \
  --options runtime --timestamp --entitlements Arca.entitlements \
  .build/arm64-apple-macosx/debug/arca-engine
cd ~/code/gascan && env -u RUSTUP_TOOLCHAIN cargo test -p gascan-arca --test live \
  -- --ignored --test-threads=1 recreate::
```

Expected: `1 passed; 0 failed`. If the engine dies with `vmnet_return_t(rawValue: 1001)`, force-quit `InternetSharing` by PID and retry.

- [ ] **Step 11: Commit**

Arca (commit normally — its key is in 1Password):

```bash
cd ~/code/arca
git add Sources/ArcaEngine/SandboxEngineService.swift Tests/ArcaEngineTests/CreateTests.swift
git commit
```

Gas Can (`env -u SSH_AUTH_SOCK` — its key is a file path):

```bash
cd ~/code/gascan
git add crates/gascan-arca/tests/live/recreate.rs
env -u SSH_AUTH_SOCK git commit
```

Both messages must record the mutation from step 8 with its two numbers. Verify `git log -1 --format='%h %G? %s'` shows `G` in both.

---

## Task 2: The shutdown wait moves into `ArcaEngine` as `runUntilQuiesced`

**Files:**
- Modify: `~/code/arca/Sources/ArcaEngine/EngineServer.swift`
- Modify: `~/code/arca/Sources/arca-engine/ArcaEngineCommand.swift` — `serve(service:group:logger:)` at `:218-343`
- Test: `~/code/arca/Tests/ArcaEngineTests/EngineServerTests.swift`

**Interfaces:**
- Consumes: `EngineServer.start(socketPath:service:group:)` (`EngineServer.swift:63`); `EngineServer.onClose` (`:26`); `ShutdownRequests` (already moved into `ArcaEngine` by milestone 2's re-review)
- Produces: `public func runUntilQuiesced(...) async throws` on `EngineServer` — the wait `ArcaEngineCommand.serve` performs today, in a type a test can construct without an entitlement.

**Why this task exists.** `serve()` currently awaits `quiesced.futureResult` (`ArcaEngineCommand.swift:341`), and the comment at `:291-294` states the gap plainly: changing that line back to `engine.onClose` — the pre-fix defect exactly — **leaves `swift test` at 157 passing**, and Gas Can's live tier is the only thing that catches it. `serve()` is private and `run()` reaches `networkManager.initialize()`, which constructs a real `VmnetNetwork`, so a test of that function needs an entitlement and a host vmnet. Moving the wait into `ArcaEngine` removes that.

**Acceptance:** the mutation described at `ArcaEngineCommand.swift:291-294` — awaiting `onClose` instead of the drain promise — **fails a test in `ArcaEngineTests`**, where today it leaves the suite green.

- [ ] **Step 1: Read the current wait in full**

```bash
cd ~/code/arca
sed -n '218,343p' Sources/arca-engine/ArcaEngineCommand.swift
```

The comment block from `:230` to `:294` carries every measurement behind this code — three workload rates, the interleaved comparison, and the third-binary mutation that showed passing the promise is a consequence and not the fix. **That reasoning moves with the code.** Do not summarise it away; a comment describing behaviour is a claim, and this project has shipped false ones in exactly this file.

- [ ] **Step 2: Write the failing test**

Append to `Tests/ArcaEngineTests/EngineServerTests.swift`, following the shape of the reviewer's probe that `ShutdownObserverTests` already uses — a single-threaded loop, `EngineServer.start` and `SandboxEngineService.forTesting()`:

```swift
/// `runUntilQuiesced` returns when the ACCEPTED connections have drained, not
/// when the listening socket closes.
///
/// **This is the test the executable could not have.** `ArcaEngineCommand.serve`
/// is private and `run()` constructs a real `VmnetNetwork`, so proving this
/// there needs an entitlement and a host vmnet. The mutation that matters --
/// awaiting `onClose` instead of the drain promise, which is the pre-fix
/// behaviour exactly -- left `swift test` green before this existed, and Gas
/// Can's live tier was the only thing that caught it.
///
/// **The accept race is SETUP, not assertion, and it fails safe.** An
/// unaccepted connection lets the drain complete immediately, so the pending
/// assertion goes red rather than falsely green. The raw peer is silent for the
/// reason `ShutdownObserverTests` records: grpc-swift closes a connection whose
/// protocol it has finished negotiating when quiescing sends its GOAWAY, and
/// closing is exactly what must not happen while this test looks.
func testRunUntilQuiescedWaitsForAcceptedConnectionsNotTheListener() async throws {
    let path = testSocketPath()
    let engine = try await EngineServer.start(
        socketPath: path, service: .forTesting(), group: group)
    let peer = try connectRawSocket(to: path)
    try await Task.sleep(nanoseconds: 300_000_000)

    let returned = NIOLockedValueBox(false)
    let asked = ShutdownRequests()
    let waiting = Task {
        try await engine.runUntilQuiesced(logger: Logger(label: "test"), asked: asked)
        returned.withLockedValue { $0 = true }
    }

    XCTAssertTrue(asked.recordAndReportFirst())
    engine.server.initiateGracefulShutdown(promise: nil)
    try await Task.sleep(nanoseconds: 500_000_000)

    XCTAssertTrue(
        try engine.onClose.wait() == (),
        "the listener must have closed, or this test asserts nothing at all"
    )
    XCTAssertFalse(
        returned.withLockedValue { $0 },
        """
        runUntilQuiesced returned while an accepted connection was still open. \
        That is the pre-fix behaviour: it waited on the LISTENING socket, which \
        ServerQuiescingHelper closes synchronously, and shut the event-loop \
        group down under live channels.
        """
    )

    close(peer)
    try await waiting.value
    XCTAssertTrue(
        returned.withLockedValue { $0 },
        "once the peer is gone the drain completes and the wait must return"
    )
}
```

**Two things to reconcile against the code as it then is.** `runUntilQuiesced`'s exact signature is yours to choose in step 4 — the call above assumes `(logger:asked:)`; make the test match whatever you write, and record the signature in this task's Interfaces block. And the `onClose.wait()` assertion is the "did the listener actually close" control that `ShutdownObserverTests` gets from its `ran` box; if `wait()` is awkward on this NIO version, take that file's `installObserver` approach instead. `testSocketPath()`, `connectRawSocket(to:)` and the `NIOLockedValueBox` import already exist in `ShutdownObserverTests.swift` — **move them to a shared helper rather than copying them**, since a duplicated fixture is how two tests drift apart.

- [ ] **Step 3: Run it to verify it fails**

```bash
cd ~/code/arca
swift test --filter ArcaEngineTests.EngineServerTests
```

Expected: FAIL — `runUntilQuiesced` does not exist yet, so this is a compile error naming it.

- [ ] **Step 4: Add `runUntilQuiesced` to `EngineServer`**

Move the promise creation, the signal-handler installation, the `onClose` guard and the two awaits out of `ArcaEngineCommand.serve` and into a public method on `EngineServer`. The executable keeps only: start the server, log `engine listening`, call `runUntilQuiesced`. **The whole comment block from `ArcaEngineCommand.swift:230-294` moves with the code it explains.**

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cd ~/code/arca
swift test --filter ArcaEngineTests
```

Expected: 161 from task 1, plus this test. Account for the increment.

- [ ] **Step 6: Prove the mutation now fails**

Change the await inside `runUntilQuiesced` back to `engine.onClose` and re-run:

```bash
cd ~/code/arca
swift test --filter ArcaEngineTests 2>&1 | tail -5
```

Expected: **the step-2 test fails.** This is the whole point of the task — before it, this mutation left the suite green. Restore and confirm.

- [ ] **Step 7: Run the live shutdown tier, which must be unchanged**

```bash
cd ~/code/arca && swift build --product arca-engine && codesign --force --sign - \
  --options runtime --timestamp --entitlements Arca.entitlements \
  .build/arm64-apple-macosx/debug/arca-engine
cd ~/code/gascan && env -u RUSTUP_TOOLCHAIN cargo test -p gascan-arca --test live \
  -- --ignored --test-threads=1 shutdown::
```

Expected: 3 passed, 0 failed — **0/440, 0/96, 0/32**, 568 engines. This is a refactor; the rates must not move.

- [ ] **Step 8: Commit**

```bash
cd ~/code/arca
git add Sources/ArcaEngine/EngineServer.swift Sources/arca-engine/ArcaEngineCommand.swift \
  Tests/ArcaEngineTests/EngineServerTests.swift
git commit
```

The message records step 6's before/after: the mutation left `swift test` green and now fails a named test.

---

## Task 3: A test that `unpackLayerToCache` calls `cachedLayerIsReusable`

**Files:**
- Test: `~/code/arca/Tests/ArcaEngineTests/LayerCacheRoleTests.swift` (modify)
- Read-only: `~/code/arca/containerization/Sources/Containerization/Image/Unpacker/OverlayFSUnpacker.swift` — `unpackLayerToCache` at `:217`, calling `cachedLayerExists` (`:246`), `cachedLayerIsReusable` (`:247`), `discardCachedLayer` (`:261`)

**Interfaces:**
- Consumes: `OverlayFSUnpacker.cachedLayerIsReusable(at:)` (`:182`), `cachedLayerExists(at:)` (`:162`), `discardCachedLayer(at:)` (`:194`); `OCILayoutFixture` (`Tests/ArcaEngineTests/OCILayoutFixture.swift`), which writes a real OCI layout — `oci-layout`, `index.json`, and `blobs/sha256/<hex>`
- Produces: nothing. This task adds only a test.

**Why this task exists.** Milestone 2's re-review closed the call-site gap for the *decision* but recorded that the call *from* `unpackLayerToCache` is still unmeasured. `unpackLayerToCache` is `private` (`:217`), so the test must go through the unpacker's public entry point with a real `Image` — which is why this needs a fixture and why it was deferred.

**Acceptance:** seeding the cache with an **unlabelled** `{cache}/{digest}/layer.ext4` and unpacking an image over it leaves the entry **labelled**. Mutating `unpackLayerToCache` to skip the reusability check — the pre-fix behaviour — must fail this test.

- [ ] **Step 1: Read the call site and the existing fixture**

```bash
cd ~/code/arca
sed -n '217,270p' containerization/Sources/Containerization/Image/Unpacker/OverlayFSUnpacker.swift
sed -n '1,60p' Tests/ArcaEngineTests/OCILayoutFixture.swift
sed -n '155,175p' Tests/ArcaEngineTests/LayerCacheRoleTests.swift
```

`LayerCacheRoleTests.cachedLayer(in:digest:label:)` at `:157` already builds `{cache}/{digest}/layer.ext4` with a chosen label. That is the seeding half; what is missing is an `Image` to unpack over it.

- [ ] **Step 2: Write the failing test**

Append to `LayerCacheRoleTests.swift`:

```swift
/// `unpackLayerToCache` CONSULTS the reusability check, which the tests above
/// do not prove.
///
/// **The three tests above pin the predicate and the two below it pin the
/// decision; none of them pins the CALL.** That is the gap milestone 2's
/// re-review recorded and deliberately left, because closing it needs a real
/// `Image` rather than a bare path. `unpackLayerToCache` is `private`
/// (`OverlayFSUnpacker.swift:217`), so this drives it through `unpack`, which
/// is the only way in.
///
/// The assertion is on the entry's LABEL after the unpack, not on a call
/// count: a stale entry is a perfectly valid ext4 filesystem and the only
/// thing wrong with it is an absence, so "the label is now there" is the same
/// statement as "the check ran and the reformat followed".
func testUnpackingOverAStaleCacheEntryRelabelsItRatherThanReusingIt() async throws {
    let cache = scratch.appendingPathComponent("layers")
    let layout = try OCILayoutFixture.write(
        at: scratch.appendingPathComponent("layout"),
        reference: "stale-cache-probe:latest",
        payload: "the layer this test unpacks"
    )
    let image = try await loadImage(from: layout, reference: "stale-cache-probe:latest")
    let digest = try await image.manifest(for: .current).layers[0].digest

    // Seed the cache the way a pre-label engine left it: correct layout,
    // correct filename, valid ext4, no role label.
    let seeded = try cachedLayer(in: cache, digest: digest, label: nil)
    XCTAssertNil(
        ArcaBlockDeviceRole.role(ofImageAt: seeded),
        "the fixture must start unlabelled or this test asserts nothing"
    )

    let unpacker = OverlayFSUnpacker(layerCachePath: cache)
    _ = try await unpacker.unpack(
        image,
        for: .current,
        at: scratch.appendingPathComponent("container")
    )

    XCTAssertEqual(
        ArcaBlockDeviceRole.role(ofImageAt: seeded),
        .overlayLayer,
        """
        the unpacker reused a stale cache entry unexamined. The guest's \
        classifier drops an unlabelled device with 'is not an Arca role, \
        leaving it alone', so the rootfs is built from a subset of its image \
        -- or from none of it, with Start still succeeding.
        """
    )
}
```

**Two things to resolve against the code as it then is.** `loadImage(from:reference:)` is whatever `ImageLoadTests.swift` already uses to turn an `OCILayoutFixture` layout into an `Image` — find it with `grep -n "func loadImage\|loadFromOCILayout" Tests/ArcaEngineTests/ImageLoadTests.swift` and reuse it rather than writing a second loader. And `.current` is a placeholder for however this codebase spells the arm64 Linux `Platform`; take it from `OverlayFSUnpacker`'s existing call sites in `Sources/ContainerBridge/OverlayFS/OverlayFSUnpacker.swift:32`.

Note that `OverlayFSUnpacker.init` takes `recorder: (any LayerCacheRecorder)? = nil` (`:54`). That seam records layers into `StateStore` and is **not** an observer of the reusability decision — do not mistake it for one and do not assert through it.

- [ ] **Step 3: Run it to verify it fails**

Run it against a build with the reusability check bypassed at the call site (`if true || …` at `:247`, which is the pre-fix behaviour exactly):

```bash
cd ~/code/arca
swift test --filter ArcaEngineTests.LayerCacheRoleTests
```

Expected: FAIL — the stale entry is reused unexamined and stays unlabelled.

- [ ] **Step 4: Restore the call site and run again**

```bash
cd ~/code/arca
swift test --filter ArcaEngineTests
```

Expected: PASS, and the count is task 2's plus one. **Steps 3 and 4 are the fails-before/passes-after pair; both numbers go in the commit message.**

- [ ] **Step 5: Commit**

```bash
cd ~/code/arca
git add Tests/ArcaEngineTests/LayerCacheRoleTests.swift
git commit
```

---

## Landing 1 checkpoint — run before expanding task 4

- [ ] `swift test --filter ArcaEngineTests` — account for every test added against the 160 baseline
- [ ] `swift test --filter ArcaTests.NetworkPruneGateTests` — 3 tests, 0 failures
- [ ] `pgrep -fl "cargo test"` empty and recorded, then `env -u RUSTUP_TOOLCHAIN cargo test --workspace --no-fail-fast` — account for every delta against 1436 / 0 / 36 across 74 targets
- [ ] `cargo fmt --all --check`, `cargo clippy --workspace --all-targets`, `scripts/ci-check-ignored-tests.sh`
- [ ] The live tier `-- --ignored --test-threads=1` in full, with its command and output recorded
- [ ] Both working trees clean; both branches pushed

---

# LANDING 2 — `signalExec` and `Logs`

**Expand these after task 3 lands, against the code as it then is.**

## Task 4: `ExecManager.signalExec(execID:signal:)`

**EXPANDED 2026-08-16, after Task 3 landed, as this plan requires. Every fact below was verified against the tree at that point; re-derive the anchors before editing.**

**Deliberately expanded to requirements rather than to step-level code, and that is an evidence-backed choice.** Task 1's brief carried full Swift and three of its details were wrong — a test fixture that did not exist, `VolumeManager` typed as a plain object when it is an `actor`, and six anchors that were five. Tasks 2 and 3 received requirement-level briefs and produced stronger work, because the implementer re-derived from the code rather than transcribing the controller's guesses. **Write the code from the source, not from this document.**

**Files:** modify `~/code/arca/Sources/ContainerBridge/ExecManager.swift`; test under `~/code/arca/Tests/ArcaTests/` (verified: `ArcaTests` imports `ContainerBridge`, e.g. `NetworkPruneGateTests.swift`).

**Interfaces:**
- Consumes: `ExecManager.execInstances: [String: ExecInfo]` (private, `:39`); `ExecInfo.process: LinuxProcess?` (`:18`); `LinuxProcess.kill(_ signal: Signal) async throws` (`containerization/Sources/Containerization/LinuxProcess.swift:315`); `ExecManagerError` (`:359`, cases `execNotFound`, `execAlreadyRunning`, `containerNotFound`, `containerNotRunning`, `invalidCommand`, `startFailed`).
- Produces: `public func signalExec(execID: String, signal: Signal) async throws`.

**`Signal` is verified and it decides the design.** `containerization/Sources/Containerization/Signal.swift:28` — `public struct Signal: RawRepresentable, Hashable, Sendable` with `public let rawValue: Int32`. **Its `init(rawValue:)` at `:31` is NOT failable**, so `Signal(rawValue: 999)` constructs happily and validation cannot come from the initializer. The repository's own validating path is `init(_ name: String, from map: [String: Int32] = Signal.linux)` (`:35`), which **throws `SignalError.invalidSignal` for a number absent from the map** — 73 named signals are defined. **Use the repository's validation rather than inventing a second one.**

**Requirements:**

1. **No defaulted parameter** (global constraint).
2. **An unknown exec id throws `ExecManagerError.execNotFound`**, matching `resizeExec`'s shape at `:256-258`.
3. **A signal for an exec whose process has not started MUST NOT be silently ignored — and this is where `signalExec` deliberately departs from `resizeExec`.** `resizeExec` returns silently in that case (`:270-273`), which is correct for a resize: a window size that arrives early is genuinely unimportant. **A signal that goes nowhere while the caller is told nothing is precisely this project's recurring defect** — the same shape as an engine that publishes no ports and reports success, and as a guest mount that is silently absent. Throw. `containerNotRunning` is the closest existing case; adding one is acceptable if it reads better, but do not add a case that duplicates an existing meaning.
4. **`ContainerBridge` is shared with Arca's Docker surface, so this change has a second consumer.** Check `Sources/DockerAPI/` before changing any existing signature.

**Testing, and the split is honest rather than convenient.** `createExec` populates `ExecInfo` with `process` still nil; `startExec` sets it and requires a native container (`:139`). So:
- **VM-free, in `ArcaTests`:** an unknown exec id throws `execNotFound`; an exec that exists but has not started throws rather than returning silently (requirement 3); an out-of-range signal number is refused.
- **Not reachable VM-free:** that a signal actually reaches a guest process. That belongs to Task 6's live `exec.rs`, and **the `signals` capability flag does not flip until it passes there.**

**Acceptance:** deleting the not-started guard must fail a named test. A `signalExec` that silently returns when `process` is nil is the defect requirement 3 exists to prevent, and it must not be able to ship green.

## Task 5: `Logs`, and the two things it makes load-bearing

**Files:** create `~/code/arca/Sources/ArcaEngine/LogReader.swift`; modify `Sources/ContainerBridge/LogWriter.swift` and `Sources/ArcaEngine/SandboxEngineService.swift` (`logs` at `:1033`); create `~/code/gascan/crates/gascan-arca/tests/live/logs.rs`.

**Requirements, all from design §2.3-2.5:**

1. **`ISO8601DateFormatter` gains `.withFractionalSeconds`** at `LogWriter.swift:73`. The contract's field is milliseconds (`engine.proto:466`) and the default formatter emits whole seconds, so the filter cannot be honoured as written. The writer widens; the contract does not narrow.
2. **`Logs` reads `combinedPath`** (`LogWriter.swift:113-117`), because ordering across stdout and stderr is what a log consumer needs and the per-entry `"stream"` field preserves which is which.
3. **A round-trip test over adversarial payloads is required.** `createLogEntry` builds its JSON by string interpolation with hand-rolled escaping (`:84`) and **nothing has ever parsed those lines back.** `Logs` will. Drive embedded `"`, `\`, newline, tab, a lone `{`, and non-UTF-8 bytes through `FileLogWriter` and back through `LogReader`. **If a payload cannot survive, fix the writer — the reader does not paper over it.**
4. **No follow mode**, and none is to be added (`engine.proto:474-476`).
5. Chunk by size, not by log entry (`engine.proto:470`).
6. Refuse an unlabelled or foreign container before reading anything, using the same rule and codes `Inspect` uses (design §4).

**Live test:** an image built with `common::layout_running` whose `Cmd` prints known text and exits; assert the text returns in order, and that `since_unix_millis` excludes what it should — including across the fractional-second boundary requirement 1 creates.

---

# LANDING 3 — `Exec`, and the two capability flags

**Expand after task 4 lands.**

## Task 6: `Exec`, then `tty` and `signals`

**Files:** create `~/code/arca/Sources/ArcaEngine/ExecSession.swift`; modify `Sources/ArcaEngine/SandboxEngineService.swift` (`exec` at `:1023`) and `Sources/ArcaEngine/CapabilitiesTests`' subject; create `~/code/gascan/crates/gascan-arca/tests/live/exec.rs`; modify `crates/gascan-arca/tests/live/read_rpcs.rs` (`:125`).

**Requirements, all from design §2.6-2.7 and §3.2:**

1. **The state machine.** First frame must be `ExecStart`, exactly one per stream (`engine.proto:412-421`); any other first frame is a protocol error. Then `stdin`→process, `resize`→`resizeExec` (`ExecManager.swift:255`), `signal`→`signalExec` (task 4), `close`→close stdin. On exit send `Exit{code, signal}` and end cleanly. **A mid-exec client reset is cancellation:** kill the guest process, reap the exec instance, emit nothing.
2. **The two adapters are the testable seam.** `startExec` takes `stdin: ReaderStream?`, `stdout: Writer?`, `stderr: Writer?` (`ExecManager.swift:117-125`). The engine supplies a `Writer` emitting `ExecServerFrame.stdout`/`.stderr` and a `ReaderStream` fed by client `stdin` frames. **These are pure value transformations and Arca's VM-free suite drives them** — `Exec` end to end cannot be reached without a booted container (`ExecManager.swift:139`).
3. **Serialize the response stream.** Two independent `Writer`s feed one `GRPCAsyncResponseStreamWriter`; concurrent sends go through a single actor. An interleaved frame reads as a flake.
4. **Refuse unknown signal numbers** as `invalid_state` naming the number (`engine.proto:437`). Never coerce to a default.
5. **With `tty` set there is no stderr stream.** `startExec` sets `processConfig.terminal` at `:153` and sets stderr only when that is false (`:173`, `:175`).
6. Refuse an unlabelled or foreign container before reaching `ExecManager` (design §4).

**The capability flips are the milestone's exit gate.** `tty` (`engine.proto:119`) is earned by a live test asserting stderr arrives **merged into stdout** — which happens only when the process really is a terminal, so the merge is proof rather than restatement. `signals` (`:120`) is earned by a live test that signals a live guest process and reads the number back in `Exit.signal`. **Neither flips until its test passes.**

**`read_rpcs.rs`'s unimplemented-method count reaches zero**, and that test is retired or inverted. It asserts its own count precisely so this would fire.

---

## Milestone acceptance

- [ ] No RPC returns `unsupported_capability`. Verify by driving all eleven from the live tier.
- [ ] `tty` and `signals` are `true`, each with a live test that fails without it.
- [ ] Every guard added by this milestone has been shown to fail under a mutation, with the before/after numbers in its commit message.
- [ ] The full baseline suite in the Landing 1 checkpoint, re-run and accounted for.
- [ ] **The three documents design §8 names are corrected:** `docs/status/START-HERE.md` (the three refusing RPCs are gone and every anchor re-derived), the milestone-2 design's §9 out-of-scope line assigning only `Exec` and `Logs` to milestone 3, and `docs/superpowers/plans/2026-08-04-arca-integration-roadmap.md` if its P5.1 outline names milestone 3's contents.
- [ ] Two pull requests — Arca first is **not** required this time, but **if the `containerization` submodule moves for any reason, it must be pushed before Arca's PR merges** or every clone breaks at `git submodule update --init --recursive`.
