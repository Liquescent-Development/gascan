# P5.1 Milestone 1 — Engine skeleton and the first live answers

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up an `arca-engine` executable serving all eleven `SandboxEngine` methods — three of them real — and a Gas Can live-test tier that dials it, answering the `connect` error paths and the placeholder-authority claim that `START-HERE:83-95` records as unverified.

**Architecture:** A new `ArcaEngine` library target in Arca conforms to the generated `Arca_Engine_V1_SandboxEngineAsyncProvider` over `ContainerBridge`, and an `arca-engine` executable binds it to a Unix socket. `Capabilities`, `Inspect` and `ListResources` are implemented; the other eight return a stated `EngineError` until later milestones. Gas Can gains `crates/gascan-arca/tests/live/`, which spawns the built binary on a temporary socket and drives `ArcaBackend<ChannelTransport>` against it.

**Tech Stack:** Swift 6.2 / SwiftPM, grpc-swift 1.23 (v1 API, `Arca_Engine_V1_SandboxEngineAsyncProvider`), swift-argument-parser, NIO; Rust 2024 edition, tonic, tokio.

**Design:** `docs/superpowers/specs/2026-08-10-p5-1-engine-service-and-wiring-design.md`. Read §3, §4, §6 and §8 before starting.

## Global Constraints

Every task's requirements implicitly include this section.

- **Repositories.** Arca work is in `~/code/arca`; Gas Can work is in `~/code/gascan`. They are separate git repositories with separate branches and separate PRs.
- **Never commit to `main`** in either repository. Work on a branch; land via PR; **merge only — never squash, never rebase.**
- **Commit with `env -u SSH_AUTH_SOCK git commit`.** `user.signingkey` is a file path (`~/.ssh/gascan-signing`), so no agent is needed. **Never `--no-gpg-sign`.** Verify `git log --format='%G?' -1` prints `G`. No co-author trailer and no mention of any AI tool in any commit message.
- **`RUSTUP_TOOLCHAIN=1.95.0` is exported and overrides `rust-toolchain.toml`.** Prefix every cargo command with `env -u RUSTUP_TOOLCHAIN`. Use `--no-fail-fast`. Confirm the `running N tests` line — a mistyped test name silently runs zero and exits 0. `cargo clippy --fix` is prohibited in this repository.
- **`ls` is aliased to something that rejects trailing-slash paths.** Use `find` or `git ls-files`.
- **`ArcaEngine` must not depend on `DockerAPI` or `ArcaDaemon`.** Task 11 makes this checkable; do not add the edge to make something compile.
- **`EngineError.code` must be one of exactly twelve values**, because `crates/gascan-arca/src/error.rs:20-55` accepts no others and maps anything else to `invalid_output`: `command_io`, `command_failed`, `invalid_output`, `helper_error`, `unsupported_capability`, `ownership_mismatch`, `foreign_resource_refused`, `invalid_resource_identity`, `resource_conflict`, `not_found`, `invalid_state`, `unknown_actual_state`. **`injected_failure` and `unsupported_version` are explicitly not an engine's to raise** (`error.rs:8-11`).
- **No Swift error may escape a provider method.** An uncaught `throw` becomes a gRPC status, and status codes are reserved for transport faults (`engine.proto:52-58`). Every method catches everything and answers in its response `oneof`.
- **`Capabilities` reports what this build actually implements.** Each milestone flips only the flags it has earned. A capability that is true before the code exists is the "instrument narrower than the claim" defect this project keeps paying for (`START-HERE:128-135`).
- **Every live test is `#[ignore]`d** with a reason naming its requirements, and listed in `tests/ci/expected-ignored-tests.txt`, or `scripts/ci-check-ignored-tests.sh` fails in either direction.
- **Never run the Gas Can workspace suite while any other cargo process is running.** Confirm `pgrep -fl "cargo test"` is empty first. Concurrent suites against one target directory produce spurious failures across the `gascand`/`gascan-e2e` binaries.

## File Structure

**Arca (`~/code/arca`), all new unless noted:**

| File | Responsibility |
|---|---|
| `Package.swift` (modify) | Declare `ArcaEngine`, `arca-engine`, `ArcaEngineTests`. |
| `Sources/ArcaEngine/EngineErrors.swift` | The twelve-code table and the catch-all that keeps Swift errors from escaping. |
| `Sources/ArcaEngine/SandboxEngineService.swift` | The eleven provider methods. Translate in, call ContainerBridge, translate out. |
| `Sources/ArcaEngine/SandboxIdentity.swift` | The naming and owner-label rules of design §4. |
| `Sources/ArcaEngine/EngineTranslation.swift` | Wire ⇄ ContainerBridge value mapping. |
| `Sources/ArcaEngine/EngineServer.swift` | Binding the provider to a Unix socket, and shutdown. |
| `Sources/arca-engine/ArcaEngineCommand.swift` | The executable: arguments, signal handling. |
| `Tests/ArcaEngineTests/*.swift` | Unit tests per source file above. |

**Gas Can (`~/code/gascan`):**

| File | Responsibility |
|---|---|
| `scripts/build-arca-engine.sh` (modify) | Build the engine product; print checkout path and binary path. |
| `crates/gascan-arca/tests/live.rs` | Module wiring for the live tier, mirroring `crates/gascan-apple/tests/live.rs`. |
| `crates/gascan-arca/tests/live/common/mod.rs` | Spawning the engine on a temporary socket, and tearing it down. |
| `crates/gascan-arca/tests/live/connect.rs` | `ChannelTransport::connect` error paths and the placeholder authority. |
| `crates/gascan-arca/tests/live/read_rpcs.rs` | `Capabilities`, `Inspect`-absent and `ListResources` against a real engine. |
| `crates/gascan-arca/Cargo.toml` (modify) | Dev-dependencies the live tier needs. |
| `tests/ci/expected-ignored-tests.txt` (modify) | The ignored-test baseline. |
| `tests/release/engine-targets-contract.sh` | Assert `ArcaEngine` does not reach `DockerAPI`. |

---

### Task 1: The error table

**Files:**
- Modify: `~/code/arca/Package.swift`
- Create: `~/code/arca/Sources/ArcaEngine/EngineErrors.swift`
- Test: `~/code/arca/Tests/ArcaEngineTests/EngineErrorsTests.swift`

**Interfaces:**
- Consumes: `Arca_Engine_V1_EngineError` from `SandboxEngineProto`.
- Produces: `enum EngineErrorCode: String, CaseIterable` with the twelve cases; `func engineError(_ code: EngineErrorCode, resource: String = "", message: String) -> Arca_Engine_V1_EngineError`; `func engineErrorCatching<T>(_ code: EngineErrorCode, resource: String, _ body: () async throws -> T) async -> Result<T, Arca_Engine_V1_EngineError>`.

- [ ] **Step 1: Declare the targets**

In `~/code/arca/Package.swift`, add these three entries to `targets`, after the `SandboxEngineProto` entry:

```swift
        // Gas Can's sandbox engine. Deliberately does NOT depend on DockerAPI or
        // ArcaDaemon: Gas Can builds only the targets it ships, and that absent
        // edge is asserted by gascan's tests/release/engine-targets-contract.sh.
        .target(
            name: "ArcaEngine",
            dependencies: [
                "SandboxEngineProto",
                "ContainerBridge",
                .product(name: "GRPC", package: "grpc-swift"),
                .product(name: "Logging", package: "swift-log"),
            ]
        ),

        .executableTarget(
            name: "arca-engine",
            dependencies: [
                "ArcaEngine",
                .product(name: "ArgumentParser", package: "swift-argument-parser"),
                .product(name: "Logging", package: "swift-log"),
            ]
        ),

        .testTarget(
            name: "ArcaEngineTests",
            dependencies: ["ArcaEngine"]
        ),
```

- [ ] **Step 2: Write the failing test**

Create `~/code/arca/Tests/ArcaEngineTests/EngineErrorsTests.swift`:

```swift
import XCTest
@testable import ArcaEngine

final class EngineErrorsTests: XCTestCase {
    /// The consumer accepts exactly these twelve and maps anything else to
    /// invalid_output, so the engine's vocabulary is not the engine's to widen.
    /// See gascan crates/gascan-arca/src/error.rs:20-55.
    func testCodeVocabularyIsExactlyTheTwelveTheConsumerAccepts() {
        XCTAssertEqual(
            Set(EngineErrorCode.allCases.map(\.rawValue)),
            [
                "command_io", "command_failed", "invalid_output", "helper_error",
                "unsupported_capability", "ownership_mismatch", "foreign_resource_refused",
                "invalid_resource_identity", "resource_conflict", "not_found",
                "invalid_state", "unknown_actual_state",
            ]
        )
    }

    /// gascan asserts the exact rendered string per code because a
    /// resource<->message transposition is invisible to a code check
    /// (crates/gascan-arca/src/error.rs:137-207). Placement is the assertion.
    func testResourceAndMessageLandInTheirOwnFields() {
        let error = engineError(.invalidState, resource: "code-a1b2c3d4e5f6", message: "not running")
        XCTAssertEqual(error.code, "invalid_state")
        XCTAssertEqual(error.resource, "code-a1b2c3d4e5f6")
        XCTAssertEqual(error.message, "not running")
    }

    func testCatchingConvertsAThrownErrorRatherThanLettingItEscape() async {
        struct Boom: Error {}
        let result = await engineErrorCatching(.commandIo, resource: "vol-a") {
            throw Boom()
        }
        guard case .failure(let error) = result else {
            return XCTFail("a thrown error must become an EngineError, not a success")
        }
        XCTAssertEqual(error.code, "command_io")
        XCTAssertEqual(error.resource, "vol-a")
        XCTAssertTrue(error.message.contains("Boom"), "must name the underlying error: \(error.message)")
    }

    func testCatchingPassesSuccessThrough() async {
        let result = await engineErrorCatching(.commandIo, resource: "") { 41 + 1 }
        guard case .success(let value) = result else {
            return XCTFail("a non-throwing body must succeed")
        }
        XCTAssertEqual(value, 42)
    }
}
```

- [ ] **Step 3: Run the test to verify it fails**

```bash
cd ~/code/arca && swift test --filter ArcaEngineTests
```

Expected: FAIL to build, with `no such module 'ArcaEngine'` or `cannot find 'EngineErrorCode' in scope`.

- [ ] **Step 4: Write the implementation**

Create `~/code/arca/Sources/ArcaEngine/EngineErrors.swift`:

```swift
import SandboxEngineProto

/// The engine's entire error vocabulary.
///
/// NOT open to extension. Gas Can maps these with a table rather than a
/// judgment, "so a new engine failure mode cannot quietly become an existing
/// one" (proto/arca/engine/v1/engine.proto:62-65), and its table
/// (crates/gascan-arca/src/error.rs:20-55) accepts exactly these twelve --
/// anything else arrives as invalid_output naming the offender. Two further
/// codes are not an engine's to raise: injected_failure belongs to Gas Can's
/// fake runtime, and unsupported_version is the consumer's own refusal.
public enum EngineErrorCode: String, CaseIterable, Sendable {
    case commandIo = "command_io"
    case commandFailed = "command_failed"
    case invalidOutput = "invalid_output"
    case helperError = "helper_error"
    case unsupportedCapability = "unsupported_capability"
    case ownershipMismatch = "ownership_mismatch"
    case foreignResourceRefused = "foreign_resource_refused"
    case invalidResourceIdentity = "invalid_resource_identity"
    case resourceConflict = "resource_conflict"
    case notFound = "not_found"
    case invalidState = "invalid_state"
    case unknownActualState = "unknown_actual_state"
}

/// `resource` names the thing the failure is about and is empty when it is not
/// about one; `message` is prose and is never parsed. They are not
/// interchangeable: two codes carry both fields, so a transposition survives
/// every assertion weaker than an exact string comparison.
public func engineError(
    _ code: EngineErrorCode,
    resource: String = "",
    message: String
) -> Arca_Engine_V1_EngineError {
    var error = Arca_Engine_V1_EngineError()
    error.code = code.rawValue
    error.resource = resource
    error.message = message
    return error
}

/// Runs `body`, converting any thrown error into an `EngineError`.
///
/// This exists because an uncaught throw in a grpc-swift provider method becomes
/// a gRPC status, and status codes are reserved for transport faults and carry
/// no engine semantics (engine.proto:52-58). A status where an outcome belongs
/// is a contract violation, not an error path.
public func engineErrorCatching<T>(
    _ code: EngineErrorCode,
    resource: String = "",
    _ body: () async throws -> T
) async -> Result<T, Arca_Engine_V1_EngineError> {
    do {
        return .success(try await body())
    } catch {
        return .failure(engineError(code, resource: resource, message: "\(error)"))
    }
}
```

- [ ] **Step 5: Run the test to verify it passes**

```bash
cd ~/code/arca && swift test --filter ArcaEngineTests
```

Expected: PASS, 4 tests.

- [ ] **Step 6: Commit**

```bash
cd ~/code/arca
git add Package.swift Sources/ArcaEngine/EngineErrors.swift Tests/ArcaEngineTests/EngineErrorsTests.swift
env -u SSH_AUTH_SOCK git commit -m "feat(engine): add the ArcaEngine target and its fixed error vocabulary

The consumer's table accepts exactly twelve codes and maps anything else
to invalid_output, so the vocabulary is not the engine's to widen. The
catching helper exists because an uncaught throw becomes a gRPC status,
and status codes are reserved for transport faults."
git log --format='%h %G? %s' -1
```

Expected: `%G?` prints `G`.

---

### Task 2: The service skeleton, with every method answering

**Files:**
- Create: `~/code/arca/Sources/ArcaEngine/SandboxEngineService.swift`
- Test: `~/code/arca/Tests/ArcaEngineTests/SandboxEngineServiceTests.swift`

**Interfaces:**
- Consumes: `EngineErrorCode`, `engineError` from Task 1.
- Produces: `public final class SandboxEngineService: Arca_Engine_V1_SandboxEngineAsyncProvider`, with `public init(containerManager: ContainerManager, volumeManager: VolumeManager, networkManager: NetworkManager, imageManager: ImageManager, execManager: ExecManager, logger: Logger)`. Later tasks replace individual method bodies.

Every method returns a response rather than throwing. The eight not yet implemented answer `unsupported_capability` naming the RPC — chosen because it is in the accepted twelve and because it is what the condition actually is: this build does not support that operation.

- [ ] **Step 1: Write the failing test**

Create `~/code/arca/Tests/ArcaEngineTests/SandboxEngineServiceTests.swift`:

```swift
import GRPC
import SandboxEngineProto
import XCTest
@testable import ArcaEngine

final class SandboxEngineServiceTests: XCTestCase {
    /// An unimplemented method must still ANSWER. Returning a gRPC status
    /// instead would be a transport fault by the contract's reading, and the
    /// consumer would report an unreachable engine rather than an unsupported
    /// operation.
    func testUnimplementedMethodsAnswerWithUnsupportedCapabilityNamingTheRpc() async throws {
        let service = SandboxEngineService.forTesting()
        let response = try await service.start(
            request: Arca_Engine_V1_StartRequest.with { $0.sandboxID = "web-a1b2c3d4e5f6" },
            context: GRPCAsyncServerCallContext.forTesting()
        )
        guard case .error(let error) = response.outcome else {
            return XCTFail("an unimplemented method must answer with an error outcome")
        }
        XCTAssertEqual(error.code, "unsupported_capability")
        XCTAssertTrue(error.message.contains("Start"), "must name the RPC: \(error.message)")
    }

    /// Every response type sets its oneof. An unset outcome is representable in
    /// proto3 and reaches the consumer as invalid_output
    /// (crates/gascan-arca/src/translate.rs:291-293).
    func testEveryUnimplementedResponseSetsItsOutcome() async throws {
        let service = SandboxEngineService.forTesting()
        let context = GRPCAsyncServerCallContext.forTesting()
        XCTAssertNotNil(try await service.stop(request: .init(), context: context).outcome)
        XCTAssertNotNil(try await service.remove(request: .init(), context: context).outcome)
        XCTAssertNotNil(try await service.create(request: .init(), context: context).outcome)
        XCTAssertNotNil(try await service.createContainer(request: .init(), context: context).outcome)
        XCTAssertNotNil(try await service.prepareImage(request: .init(), context: context).outcome)
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cd ~/code/arca && swift test --filter SandboxEngineServiceTests
```

Expected: FAIL with `cannot find 'SandboxEngineService' in scope`.

- [ ] **Step 3: Write the implementation**

Create `~/code/arca/Sources/ArcaEngine/SandboxEngineService.swift`. The three implemented methods are filled in by Tasks 4-6; here they answer `unsupported_capability` like the rest.

```swift
import ContainerBridge
import GRPC
import Logging
import SandboxEngineProto

/// Arca's implementation of the published sandbox-engine contract.
///
/// Each method is a thin seam: translate in, call ContainerBridge, translate
/// out. Business logic belongs in ContainerBridge and mapping belongs in
/// EngineTranslation, so that this file stays readable as a list of the
/// contract's eleven methods.
public final class SandboxEngineService: Arca_Engine_V1_SandboxEngineAsyncProvider {
    public let interceptors: Arca_Engine_V1_SandboxEngineServerInterceptorFactoryProtocol? = nil

    let containerManager: ContainerManager
    let volumeManager: VolumeManager
    let networkManager: NetworkManager
    let imageManager: ImageManager
    let execManager: ExecManager
    let logger: Logger

    public init(
        containerManager: ContainerManager,
        volumeManager: VolumeManager,
        networkManager: NetworkManager,
        imageManager: ImageManager,
        execManager: ExecManager,
        logger: Logger
    ) {
        self.containerManager = containerManager
        self.volumeManager = volumeManager
        self.networkManager = networkManager
        self.imageManager = imageManager
        self.execManager = execManager
        self.logger = logger
    }

    /// The stated answer for an operation this build does not implement.
    ///
    /// unsupported_capability rather than a gRPC status: a status would tell the
    /// consumer the engine is unreachable, which is a different and more
    /// alarming fact than "this build cannot do that".
    static func notImplemented(_ rpc: String) -> Arca_Engine_V1_EngineError {
        engineError(
            .unsupportedCapability,
            message: "\(rpc) is not implemented by this engine build"
        )
    }

    public func capabilities(
        request: Arca_Engine_V1_CapabilitiesRequest,
        context: GRPCAsyncServerCallContext
    ) async throws -> Arca_Engine_V1_CapabilitiesResponse {
        Arca_Engine_V1_CapabilitiesResponse.with { $0.error = Self.notImplemented("Capabilities") }
    }

    public func inspect(
        request: Arca_Engine_V1_InspectRequest,
        context: GRPCAsyncServerCallContext
    ) async throws -> Arca_Engine_V1_InspectResponse {
        Arca_Engine_V1_InspectResponse.with { $0.error = Self.notImplemented("Inspect") }
    }

    public func create(
        request: Arca_Engine_V1_CreateRequest,
        context: GRPCAsyncServerCallContext
    ) async throws -> Arca_Engine_V1_CreateResponse {
        Arca_Engine_V1_CreateResponse.with {
            $0.failed = Arca_Engine_V1_CreateFailed.with { $0.error = Self.notImplemented("Create") }
        }
    }

    public func prepareImage(
        request: Arca_Engine_V1_PrepareImageRequest,
        context: GRPCAsyncServerCallContext
    ) async throws -> Arca_Engine_V1_PrepareImageResponse {
        Arca_Engine_V1_PrepareImageResponse.with { $0.error = Self.notImplemented("PrepareImage") }
    }

    public func createContainer(
        request: Arca_Engine_V1_CreateContainerRequest,
        context: GRPCAsyncServerCallContext
    ) async throws -> Arca_Engine_V1_CreateResponse {
        Arca_Engine_V1_CreateResponse.with {
            $0.failed = Arca_Engine_V1_CreateFailed.with {
                $0.error = Self.notImplemented("CreateContainer")
            }
        }
    }

    public func start(
        request: Arca_Engine_V1_StartRequest,
        context: GRPCAsyncServerCallContext
    ) async throws -> Arca_Engine_V1_AckResponse {
        Arca_Engine_V1_AckResponse.with { $0.error = Self.notImplemented("Start") }
    }

    public func stop(
        request: Arca_Engine_V1_StopRequest,
        context: GRPCAsyncServerCallContext
    ) async throws -> Arca_Engine_V1_AckResponse {
        Arca_Engine_V1_AckResponse.with { $0.error = Self.notImplemented("Stop") }
    }

    public func remove(
        request: Arca_Engine_V1_RemoveRequest,
        context: GRPCAsyncServerCallContext
    ) async throws -> Arca_Engine_V1_AckResponse {
        Arca_Engine_V1_AckResponse.with { $0.error = Self.notImplemented("Remove") }
    }

    public func exec(
        requestStream: GRPCAsyncRequestStream<Arca_Engine_V1_ExecClientFrame>,
        responseStream: GRPCAsyncResponseStreamWriter<Arca_Engine_V1_ExecServerFrame>,
        context: GRPCAsyncServerCallContext
    ) async throws {
        try await responseStream.send(
            Arca_Engine_V1_ExecServerFrame.with { $0.error = Self.notImplemented("Exec") }
        )
    }

    public func logs(
        request: Arca_Engine_V1_LogsRequest,
        responseStream: GRPCAsyncResponseStreamWriter<Arca_Engine_V1_LogsChunk>,
        context: GRPCAsyncServerCallContext
    ) async throws {
        try await responseStream.send(
            Arca_Engine_V1_LogsChunk.with { $0.error = Self.notImplemented("Logs") }
        )
    }

    public func listResources(
        request: Arca_Engine_V1_ListResourcesRequest,
        context: GRPCAsyncServerCallContext
    ) async throws -> Arca_Engine_V1_ListResourcesResponse {
        Arca_Engine_V1_ListResourcesResponse.with { $0.error = Self.notImplemented("ListResources") }
    }
}
```

- [ ] **Step 4: Add the test constructors**

The test calls `SandboxEngineService.forTesting()` and `GRPCAsyncServerCallContext.forTesting()`, neither of which exists. Create `~/code/arca/Tests/ArcaEngineTests/TestSupport.swift`:

```swift
import ContainerBridge
import GRPC
import Logging
import NIOCore
import NIOPosix
@testable import ArcaEngine

extension SandboxEngineService {
    /// A service over real ContainerBridge managers against a throwaway state
    /// root. Nothing in Tasks 1-6's tests starts a VM; these managers exist
    /// because the service holds them, not because the tests drive them.
    static func forTesting() -> SandboxEngineService {
        let logger = Logger(label: "arca-engine-tests")
        let root = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("arca-engine-tests-\(UUID().uuidString)")
        let containerManager = ContainerManager(stateRoot: root, logger: logger)
        return SandboxEngineService(
            containerManager: containerManager,
            volumeManager: VolumeManager(stateRoot: root, logger: logger),
            networkManager: NetworkManager(stateRoot: root, logger: logger),
            imageManager: ImageManager(stateRoot: root, logger: logger),
            execManager: ExecManager(containerManager: containerManager, logger: logger),
            logger: logger
        )
    }
}

extension GRPCAsyncServerCallContext {
    static func forTesting() -> GRPCAsyncServerCallContext {
        GRPCAsyncServerCallContext(
            headers: [:],
            logger: Logger(label: "arca-engine-tests"),
            contextProvider: .userProvided
        )
    }
}
```

**The `ContainerManager`, `VolumeManager`, `NetworkManager` and `ImageManager` initialiser signatures above are a best reading of `Sources/ContainerBridge/*.swift`, not a verified fact.** Before writing this file, read the four `public init` declarations and use the real ones:

```bash
cd ~/code/arca && grep -n "public init" -A 12 \
  Sources/ContainerBridge/ContainerManager.swift \
  Sources/ContainerBridge/VolumeManager.swift \
  Sources/ContainerBridge/NetworkManager.swift \
  Sources/ContainerBridge/ImageManager.swift
```

Likewise confirm `GRPCAsyncServerCallContext`'s test-constructible initialiser:

```bash
cd ~/code/arca && grep -rn "public init" -A 8 \
  .build/checkouts/grpc-swift/Sources/GRPC/ServerCallContexts/GRPCAsyncServerCallContext.swift
```

If no public initialiser exists, change the two tests to call the method bodies through a helper that takes no context, and record the deviation in the task report.

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cd ~/code/arca && swift test --filter ArcaEngineTests
```

Expected: PASS, 6 tests.

- [ ] **Step 6: Commit**

```bash
cd ~/code/arca
git add Sources/ArcaEngine/SandboxEngineService.swift Tests/ArcaEngineTests/
env -u SSH_AUTH_SOCK git commit -m "feat(engine): implement the eleven-method service surface

Every method answers rather than throwing. The eight this build does not
implement report unsupported_capability naming the RPC, because a gRPC
status would tell the consumer the engine is unreachable, which is a
different and more alarming fact."
git log --format='%h %G? %s' -1
```

---

### Task 3: Identity and owner labels

**Files:**
- Create: `~/code/arca/Sources/ArcaEngine/SandboxIdentity.swift`
- Test: `~/code/arca/Tests/ArcaEngineTests/SandboxIdentityTests.swift`

**Interfaces:**
- Produces: `enum SandboxIdentity` with `static let managedByLabelKey = "dev.gascan.managed-by"`, `static let sandboxIdLabelKey = "dev.gascan.sandbox-id"`, `static func labels(from owner: Arca_Engine_V1_OwnerLabels) -> [String: String]`, `static func owner(from labels: [String: String]) -> Arca_Engine_V1_OwnerLabels?`, `static func containerName(forSandboxId id: String) -> String`.

- [ ] **Step 1: Confirm the label keys Gas Can uses**

```bash
cd ~/code/gascan && grep -n "MANAGED_BY_LABEL\|SANDBOX_ID_LABEL\|dev.gascan" crates/gascan-core/src/runtime.rs | head
```

Use the exact strings printed. `crates/gascan-core/src/runtime.rs:56-57` names `MANAGED_BY_LABEL` as `dev.gascan.managed-by`; find the sandbox-id key the same way and use what the command prints, not what this plan guesses.

- [ ] **Step 2: Write the failing test**

Create `~/code/arca/Tests/ArcaEngineTests/SandboxIdentityTests.swift`:

```swift
import SandboxEngineProto
import XCTest
@testable import ArcaEngine

final class SandboxIdentityTests: XCTestCase {
    /// The container's name IS the sandbox id. gascan builds the expected
    /// identity as request.id.to_string() and validates created resources
    /// against it (crates/gascan-core/src/runtime.rs:829-832), so any other
    /// name fails every create client-side.
    func testContainerNameIsTheSandboxIdVerbatim() {
        XCTAssertEqual(
            SandboxIdentity.containerName(forSandboxId: "web-a1b2c3d4e5f6"),
            "web-a1b2c3d4e5f6"
        )
    }

    /// Labels are stored verbatim and never interpreted (engine.proto:144-148).
    /// Round-tripping is the whole contract.
    func testOwnerLabelsRoundTripUnchanged() {
        var owner = Arca_Engine_V1_OwnerLabels()
        owner.managedBy = "gascan"
        owner.sandboxID = "web-a1b2c3d4e5f6"

        let recovered = SandboxIdentity.owner(from: SandboxIdentity.labels(from: owner))

        XCTAssertEqual(recovered?.managedBy, "gascan")
        XCTAssertEqual(recovered?.sandboxID, "web-a1b2c3d4e5f6")
    }

    /// A resource the engine holds no labels for is how a consumer sees one it
    /// does not own (engine.proto:169-173). Absent, not empty: an OwnerLabels
    /// with two empty strings would claim managed_by "" and defeat gascan's
    /// ownership classifier.
    func testUnlabelledResourcesHaveNoOwnerRatherThanAnEmptyOne() {
        XCTAssertNil(SandboxIdentity.owner(from: [:]))
        XCTAssertNil(SandboxIdentity.owner(from: ["com.example.other": "x"]))
    }

    /// A half-labelled resource is not ours and must not be reported as though
    /// it were partially ours.
    func testAPartiallyLabelledResourceHasNoOwner() {
        XCTAssertNil(SandboxIdentity.owner(from: [SandboxIdentity.managedByLabelKey: "gascan"]))
        XCTAssertNil(SandboxIdentity.owner(from: [SandboxIdentity.sandboxIdLabelKey: "web-a1b2c3d4e5f6"]))
    }
}
```

- [ ] **Step 3: Run the test to verify it fails**

```bash
cd ~/code/arca && swift test --filter SandboxIdentityTests
```

Expected: FAIL with `cannot find 'SandboxIdentity' in scope`.

- [ ] **Step 4: Write the implementation**

Create `~/code/arca/Sources/ArcaEngine/SandboxIdentity.swift`:

```swift
import SandboxEngineProto

/// The naming and labelling rules, in one place because Gas Can validates them
/// exactly and a divergence fails every create rather than degrading.
public enum SandboxIdentity {
    public static let managedByLabelKey = "dev.gascan.managed-by"
    public static let sandboxIdLabelKey = "dev.gascan.sandbox-id"

    /// The container's name is the sandbox id, unchanged.
    ///
    /// gascan's validator builds the expected container identity as the
    /// request's id (crates/gascan-core/src/runtime.rs:829-832). A prefix, a
    /// suffix, or a normalisation makes every create fail client-side.
    ///
    /// Safe because ContainerBridge applies no container-name grammar
    /// validation, and because a sandbox id always contains a hyphen -- which
    /// keeps resolveContainerID from reading it as a hex short ID
    /// (Sources/ContainerBridge/ContainerManager.swift:1930-1934).
    public static func containerName(forSandboxId id: String) -> String { id }

    /// Stored verbatim, echoed back, never interpreted. Deciding whether a
    /// labelled resource is yours is the consumer's judgment
    /// (engine.proto:144-148).
    public static func labels(from owner: Arca_Engine_V1_OwnerLabels) -> [String: String] {
        [managedByLabelKey: owner.managedBy, sandboxIdLabelKey: owner.sandboxID]
    }

    /// nil when the engine holds no labels for a resource, which is how a
    /// consumer sees one it does not own. Both keys are required: a half
    /// labelled resource would otherwise be reported claiming an empty
    /// managed_by, which gascan's classifier would have to interpret.
    public static func owner(from labels: [String: String]) -> Arca_Engine_V1_OwnerLabels? {
        guard let managedBy = labels[managedByLabelKey],
              let sandboxId = labels[sandboxIdLabelKey]
        else { return nil }
        var owner = Arca_Engine_V1_OwnerLabels()
        owner.managedBy = managedBy
        owner.sandboxID = sandboxId
        return owner
    }
}
```

- [ ] **Step 5: Run the test to verify it passes**

```bash
cd ~/code/arca && swift test --filter SandboxIdentityTests
```

Expected: PASS, 4 tests.

- [ ] **Step 6: Commit**

```bash
cd ~/code/arca
git add Sources/ArcaEngine/SandboxIdentity.swift Tests/ArcaEngineTests/SandboxIdentityTests.swift
env -u SSH_AUTH_SOCK git commit -m "feat(engine): state the identity and owner-label rules once

The container's name is the sandbox id verbatim, because gascan validates
created resources against exactly that. An unlabelled resource reports no
owner rather than an empty one, so a half-labelled resource cannot claim
an empty managed_by that the consumer's classifier would have to read."
git log --format='%h %G? %s' -1
```

---

### Task 4: Capabilities

**Files:**
- Modify: `~/code/arca/Sources/ArcaEngine/SandboxEngineService.swift` (the `capabilities` method)
- Create: `~/code/arca/Sources/ArcaEngine/EngineTranslation.swift`
- Test: `~/code/arca/Tests/ArcaEngineTests/CapabilitiesTests.swift`

**Interfaces:**
- Produces: `func engineVersion(from string: String) -> Arca_Engine_V1_Version?` in `EngineTranslation.swift`; `SandboxEngineService.capabilities` returning a populated `Capabilities`.

**Every capability flag reports what this build implements.** Milestone 1 implements no create and no exec, so every feature flag is `false` and `offline` is `ISOLATION_UNVERIFIED`. Milestones 2-4 flip them as they earn them; Milestone 4 adds the test asserting they are all true. A flag that is true before its code exists is the exact defect `START-HERE:128-135` catalogues.

- [ ] **Step 1: Write the failing test**

Create `~/code/arca/Tests/ArcaEngineTests/CapabilitiesTests.swift`:

```swift
import GRPC
import SandboxEngineProto
import XCTest
@testable import ArcaEngine

final class CapabilitiesTests: XCTestCase {
    func testVersionParsesTheLeadingSemverAndIgnoresAPrerelease() {
        let version = engineVersion(from: "0.2.4-alpha")
        XCTAssertEqual(version?.major, 0)
        XCTAssertEqual(version?.minor, 2)
        XCTAssertEqual(version?.patch, 4)
    }

    func testVersionRejectsAnythingItCannotReadRatherThanGuessing() {
        XCTAssertNil(engineVersion(from: ""))
        XCTAssertNil(engineVersion(from: "0.2"))
        XCTAssertNil(engineVersion(from: "v0.2.4"))
        XCTAssertNil(engineVersion(from: "0.2.x"))
    }

    /// This build implements no create and no exec, so it claims nothing. A
    /// capability that is true before its code exists is how a consumer is
    /// induced to send a request the engine cannot honour.
    func testThisBuildClaimsOnlyWhatItImplements() async throws {
        let response = try await SandboxEngineService.forTesting()
            .capabilities(request: .init(), context: .forTesting())

        guard case .capabilities(let capabilities) = response.outcome else {
            return XCTFail("Capabilities must answer with capabilities")
        }
        XCTAssertFalse(capabilities.projectMount)
        XCTAssertFalse(capabilities.namedVolumes)
        XCTAssertFalse(capabilities.tty)
        XCTAssertFalse(capabilities.signals)
        XCTAssertFalse(capabilities.loopbackPublish)
        XCTAssertFalse(capabilities.resourceLimits)
        XCTAssertEqual(capabilities.offline, .unverified)
        XCTAssertEqual(capabilities.contractMinor, 0)
        XCTAssertEqual(capabilities.engineVersion.minor, 2)
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cd ~/code/arca && swift test --filter CapabilitiesTests
```

Expected: FAIL — `cannot find 'engineVersion' in scope`, and the capabilities assertion failing because the method still returns `unsupported_capability`.

- [ ] **Step 3: Write the version translation**

Create `~/code/arca/Sources/ArcaEngine/EngineTranslation.swift`:

```swift
import SandboxEngineProto

/// Reads the leading `major.minor.patch` of a version string.
///
/// Returns nil rather than guessing: Gas Can refuses to drive an engine version
/// it does not recognise (contract §9), and a version invented from an
/// unparseable string would defeat that refusal by making every engine look
/// recognisable.
public func engineVersion(from string: String) -> Arca_Engine_V1_Version? {
    let core = string.split(separator: "-", maxSplits: 1).first.map(String.init) ?? string
    let parts = core.split(separator: ".", omittingEmptySubsequences: false)
    guard parts.count == 3 else { return nil }
    guard let major = UInt32(parts[0]), let minor = UInt32(parts[1]), let patch = UInt32(parts[2])
    else { return nil }
    var version = Arca_Engine_V1_Version()
    version.major = major
    version.minor = minor
    version.patch = patch
    return version
}
```

- [ ] **Step 4: Replace the `capabilities` method**

In `~/code/arca/Sources/ArcaEngine/SandboxEngineService.swift`, replace the `capabilities` method body with:

```swift
    public func capabilities(
        request: Arca_Engine_V1_CapabilitiesRequest,
        context: GRPCAsyncServerCallContext
    ) async throws -> Arca_Engine_V1_CapabilitiesResponse {
        guard let version = engineVersion(from: ArcaVersion.version) else {
            return Arca_Engine_V1_CapabilitiesResponse.with {
                $0.error = engineError(
                    .invalidOutput,
                    message: "engine version \(ArcaVersion.version) is not a readable semantic version"
                )
            }
        }
        return Arca_Engine_V1_CapabilitiesResponse.with { response in
            response.capabilities = Arca_Engine_V1_Capabilities.with { capabilities in
                capabilities.engineVersion = version
                capabilities.contractMinor = 0
                // Each flag is flipped by the milestone that implements it. See
                // the plan's Task 4 note: a capability that is true before its
                // code exists induces requests the engine cannot honour.
                capabilities.projectMount = false
                capabilities.namedVolumes = false
                capabilities.tty = false
                capabilities.signals = false
                capabilities.loopbackPublish = false
                capabilities.resourceLimits = false
                capabilities.offline = .unverified
            }
        }
    }
```

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cd ~/code/arca && swift test --filter ArcaEngineTests
```

Expected: PASS. `SandboxEngineServiceTests` still passes because it asserts on `start`, not `capabilities`.

- [ ] **Step 6: Commit**

```bash
cd ~/code/arca
git add Sources/ArcaEngine/EngineTranslation.swift Sources/ArcaEngine/SandboxEngineService.swift Tests/ArcaEngineTests/CapabilitiesTests.swift
env -u SSH_AUTH_SOCK git commit -m "feat(engine): report capabilities, claiming only what this build implements

Every feature flag is false and offline is UNVERIFIED, because this build
creates nothing and execs nothing. Later milestones flip each flag as they
earn it. An unparseable engine version is refused rather than guessed, so
gascan's version refusal cannot be defeated by an invented version."
git log --format='%h %G? %s' -1
```

---

### Task 5: Inspect

**Files:**
- Modify: `~/code/arca/Sources/ArcaEngine/SandboxEngineService.swift` (the `inspect` method)
- Modify: `~/code/arca/Sources/ArcaEngine/EngineTranslation.swift`
- Test: `~/code/arca/Tests/ArcaEngineTests/InspectTests.swift`

**Interfaces:**
- Produces: `func imageDigest(fromReference reference: String) -> Arca_Engine_V1_ImageDigest?` and `func sandboxState(fromStatus status: String) -> Arca_Engine_V1_SandboxState` in `EngineTranslation.swift`.

- [ ] **Step 1: Read what ContainerBridge reports for a container**

```bash
cd ~/code/arca && grep -n "public func getContainer" -A 25 Sources/ContainerBridge/ContainerManager.swift
cd ~/code/arca && grep -n "struct ContainerInfo\|let status\|let image" Sources/ContainerBridge/Types.swift | head -20
```

Use the real property names in Step 3. The status strings ContainerBridge uses are what `sandboxState` must map; list them with:

```bash
cd ~/code/arca && grep -rn 'updateContainerStatus(id:.*status: "' Sources/ContainerBridge/ | sed 's/.*status: //' | sort -u
```

- [ ] **Step 2: Write the failing test**

Create `~/code/arca/Tests/ArcaEngineTests/InspectTests.swift`:

```swift
import GRPC
import SandboxEngineProto
import XCTest
@testable import ArcaEngine

final class InspectTests: XCTestCase {
    /// gascan reassembles the canonical reference and asserts it is one, which
    /// is what lets the daemon compare one observation against another by exact
    /// string (crates/gascan-arca/src/translate.rs:333-336). A tag, or a digest
    /// in another form, breaks reconciliation rather than looking untidy.
    func testImageDigestSplitsRepositoryFromBareLowercaseHex() {
        let digest = imageDigest(
            fromReference: "ghcr.io/liquescent-development/gascan/workspace@sha256:"
                + String(repeating: "a", count: 64)
        )
        XCTAssertEqual(digest?.repository, "ghcr.io/liquescent-development/gascan/workspace")
        XCTAssertEqual(digest?.sha256Hex, String(repeating: "a", count: 64))
    }

    func testImageDigestRefusesAnythingThatIsNotAnExactDigestReference() {
        XCTAssertNil(imageDigest(fromReference: "ubuntu:latest"))
        XCTAssertNil(imageDigest(fromReference: "ubuntu"))
        XCTAssertNil(imageDigest(fromReference: "ubuntu@sha256:abc"))
        XCTAssertNil(imageDigest(fromReference: "ubuntu@sha512:" + String(repeating: "a", count: 64)))
        XCTAssertNil(imageDigest(fromReference: "ubuntu@sha256:" + String(repeating: "A", count: 64)))
    }

    /// Three arms, not two. "It is not there" and "I could not tell" demand
    /// opposite behaviour from a reconciler (engine.proto:354-357).
    func testAnAbsentSandboxIsAnAnswerRatherThanAnError() async throws {
        let response = try await SandboxEngineService.forTesting().inspect(
            request: Arca_Engine_V1_InspectRequest.with { $0.sandboxID = "absent-a1b2c3d4e5f6" },
            context: .forTesting()
        )
        guard case .absent = response.outcome else {
            return XCTFail("an unknown sandbox must be Absent, not an error: \(String(describing: response.outcome))")
        }
    }
}
```

- [ ] **Step 3: Run the test to verify it fails**

```bash
cd ~/code/arca && swift test --filter InspectTests
```

Expected: FAIL with `cannot find 'imageDigest' in scope`.

- [ ] **Step 4: Add the translation helpers**

Append to `~/code/arca/Sources/ArcaEngine/EngineTranslation.swift`:

```swift
/// Splits an exact digest reference into the two fields the contract carries.
///
/// A tag is not representable on the wire, so a reference that is not an exact
/// digest has nothing to map to and is refused. The hex is bare and lowercase,
/// exactly 64 characters, with no "sha256:" prefix (engine.proto:179-185).
public func imageDigest(fromReference reference: String) -> Arca_Engine_V1_ImageDigest? {
    guard let separator = reference.range(of: "@sha256:", options: .backwards) else { return nil }
    let repository = String(reference[reference.startIndex..<separator.lowerBound])
    let hex = String(reference[separator.upperBound...])
    guard !repository.isEmpty, hex.count == 64,
          hex.allSatisfy({ $0.isNumber || ("a"..."f").contains($0) })
    else { return nil }
    var digest = Arca_Engine_V1_ImageDigest()
    digest.repository = repository
    digest.sha256Hex = hex
    return digest
}

/// Maps ContainerBridge's status string onto the contract's three states.
///
/// UNSPECIFIED for anything unrecognised rather than a guess: reporting a
/// running sandbox as stopped would have a reconciler destroy live work.
public func sandboxState(fromStatus status: String) -> Arca_Engine_V1_SandboxState {
    switch status {
    case "created": return .creating
    case "running": return .running
    case "exited", "stopped", "dead": return .stopped
    default: return .unspecified
    }
}
```

**Replace the `case` labels above with the exact strings Step 1's last command printed.** If a status appears that none of the three arms fits, map it to `.unspecified` and record it in the task report rather than inventing a mapping.

- [ ] **Step 5: Replace the `inspect` method**

In `~/code/arca/Sources/ArcaEngine/SandboxEngineService.swift`:

```swift
    public func inspect(
        request: Arca_Engine_V1_InspectRequest,
        context: GRPCAsyncServerCallContext
    ) async throws -> Arca_Engine_V1_InspectResponse {
        let name = SandboxIdentity.containerName(forSandboxId: request.sandboxID)
        let found = await engineErrorCatching(.commandIo, resource: name) {
            try await self.containerManager.getContainer(id: name)
        }
        switch found {
        case .failure(let error):
            return Arca_Engine_V1_InspectResponse.with { $0.error = error }
        case .success(nil):
            return Arca_Engine_V1_InspectResponse.with { $0.absent = Arca_Engine_V1_Absent() }
        case .success(.some(let container)):
            guard let digest = imageDigest(fromReference: container.config.image) else {
                return Arca_Engine_V1_InspectResponse.with {
                    $0.error = engineError(
                        .invalidOutput,
                        resource: name,
                        message: "container image \(container.config.image) is not an exact digest reference"
                    )
                }
            }
            return Arca_Engine_V1_InspectResponse.with { response in
                response.sandbox = Arca_Engine_V1_Sandbox.with { sandbox in
                    sandbox.sandboxID = request.sandboxID
                    sandbox.image = digest
                    sandbox.state = sandboxState(fromStatus: container.status)
                    if let owner = SandboxIdentity.owner(from: container.config.labels) {
                        sandbox.owner = owner
                    }
                    sandbox.ports = []
                }
            }
        }
    }
```

**`container.config.image`, `container.status` and `container.config.labels` are a best reading, not verified.** Use the property names Step 1 printed. `sandbox.ports` is deliberately empty in this milestone: no create path publishes ports yet, and Milestone 2 fills it when it can be tested end to end.

- [ ] **Step 6: Run the tests to verify they pass**

```bash
cd ~/code/arca && swift test --filter ArcaEngineTests
```

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
cd ~/code/arca
git add Sources/ArcaEngine/ Tests/ArcaEngineTests/InspectTests.swift
env -u SSH_AUTH_SOCK git commit -m "feat(engine): implement Inspect with an absent arm and exact digests

An unknown sandbox is Absent, never an error: a reconciler must be able to
tell 'it is not there' from 'I could not tell'. A container image that is
not an exact digest reference is refused rather than reported as a tag,
because gascan compares observations by exact string."
git log --format='%h %G? %s' -1
```

---

### Task 6: ListResources

**Files:**
- Modify: `~/code/arca/Sources/ArcaEngine/SandboxEngineService.swift` (the `listResources` method)
- Test: `~/code/arca/Tests/ArcaEngineTests/ListResourcesTests.swift`

**Interfaces:**
- Consumes: `SandboxIdentity.owner(from:)` from Task 3.
- Produces: `SandboxEngineService.listResources` returning containers, volumes and networks.

- [ ] **Step 1: Write the failing test**

Create `~/code/arca/Tests/ArcaEngineTests/ListResourcesTests.swift`:

```swift
import GRPC
import SandboxEngineProto
import XCTest
@testable import ArcaEngine

final class ListResourcesTests: XCTestCase {
    /// On an empty engine this is an empty list, not an error. A reconciler
    /// reads "nothing exists" from this and must not see a failure instead.
    func testAnEmptyEngineListsNoResourcesRatherThanFailing() async throws {
        let response = try await SandboxEngineService.forTesting()
            .listResources(request: .init(), context: .forTesting())

        guard case .resources(let list) = response.outcome else {
            return XCTFail("ListResources must answer with a list: \(String(describing: response.outcome))")
        }
        XCTAssertTrue(list.resources.isEmpty)
    }

    /// Unlabelled resources are NOT filtered out. gascan's drift detection
    /// depends on seeing them, and hiding them engine-side would break it
    /// silently (engine.proto:389-391). Asserted here on the mapping helper,
    /// and again against a real engine in the live tier.
    func testAnUnlabelledResourceMapsToOneWithNoOwner() {
        let resource = resourceMessage(kind: .volume, name: "someone-elses-volume", labels: [:])
        XCTAssertEqual(resource.identity.name, "someone-elses-volume")
        XCTAssertEqual(resource.identity.kind, .volume)
        XCTAssertFalse(resource.hasOwner)
    }

    func testALabelledResourceCarriesItsOwnerBack() {
        let resource = resourceMessage(
            kind: .container,
            name: "web-a1b2c3d4e5f6",
            labels: [
                SandboxIdentity.managedByLabelKey: "gascan",
                SandboxIdentity.sandboxIdLabelKey: "web-a1b2c3d4e5f6",
            ]
        )
        XCTAssertTrue(resource.hasOwner)
        XCTAssertEqual(resource.owner.managedBy, "gascan")
        XCTAssertEqual(resource.owner.sandboxID, "web-a1b2c3d4e5f6")
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cd ~/code/arca && swift test --filter ListResourcesTests
```

Expected: FAIL with `cannot find 'resourceMessage' in scope`.

- [ ] **Step 3: Add the resource mapper**

Append to `~/code/arca/Sources/ArcaEngine/EngineTranslation.swift`:

```swift
/// One resource on the way out.
///
/// `owner` stays unset when the engine holds no labels for the resource, which
/// is how a consumer sees one it does not own (engine.proto:169-173).
public func resourceMessage(
    kind: Arca_Engine_V1_ResourceKind,
    name: String,
    labels: [String: String]
) -> Arca_Engine_V1_Resource {
    Arca_Engine_V1_Resource.with { resource in
        resource.identity = Arca_Engine_V1_ResourceIdentity.with {
            $0.kind = kind
            $0.name = name
        }
        if let owner = SandboxIdentity.owner(from: labels) {
            resource.owner = owner
        }
    }
}
```

- [ ] **Step 4: Replace the `listResources` method**

In `~/code/arca/Sources/ArcaEngine/SandboxEngineService.swift`:

```swift
    public func listResources(
        request: Arca_Engine_V1_ListResourcesRequest,
        context: GRPCAsyncServerCallContext
    ) async throws -> Arca_Engine_V1_ListResourcesResponse {
        let collected = await engineErrorCatching(.commandIo) {
            var resources: [Arca_Engine_V1_Resource] = []
            for container in try await self.containerManager.listContainers(all: true) {
                resources.append(
                    resourceMessage(kind: .container, name: container.name, labels: container.labels)
                )
            }
            for volume in try await self.volumeManager.listVolumes() {
                resources.append(
                    resourceMessage(kind: .volume, name: volume.name, labels: volume.labels)
                )
            }
            for network in await self.networkManager.listNetworks() {
                resources.append(
                    resourceMessage(kind: .network, name: network.name, labels: network.labels)
                )
            }
            return resources
        }
        switch collected {
        case .failure(let error):
            return Arca_Engine_V1_ListResourcesResponse.with { $0.error = error }
        case .success(let resources):
            return Arca_Engine_V1_ListResourcesResponse.with { response in
                response.resources = Arca_Engine_V1_ResourceList.with { $0.resources = resources }
            }
        }
    }
```

**The three listing calls and their result properties are a best reading.** Confirm each before writing:

```bash
cd ~/code/arca && grep -n "public func listContainers\|public func listVolumes\|public func listNetworks" -A 6 \
  Sources/ContainerBridge/ContainerManager.swift \
  Sources/ContainerBridge/VolumeManager.swift \
  Sources/ContainerBridge/NetworkManager.swift
```

`NetworkMetadata` may carry no `labels` property. If it does not, pass `[:]` and record it — a network Gas Can created will still be recognised by name in Milestone 2, and the gap belongs in that milestone's create path, not invented here.

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cd ~/code/arca && swift test --filter ArcaEngineTests
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
cd ~/code/arca
git add Sources/ArcaEngine/ Tests/ArcaEngineTests/ListResourcesTests.swift
env -u SSH_AUTH_SOCK git commit -m "feat(engine): list every resource the engine holds, labelled or not

Unlabelled resources are reported rather than filtered. gascan's drift
detection depends on seeing them, and hiding them engine-side would break
it silently while looking tidier."
git log --format='%h %G? %s' -1
```

---

### Task 7: The server and the executable

**Files:**
- Create: `~/code/arca/Sources/ArcaEngine/EngineServer.swift`
- Create: `~/code/arca/Sources/arca-engine/ArcaEngineCommand.swift`
- Test: `~/code/arca/Tests/ArcaEngineTests/EngineServerTests.swift`

**Interfaces:**
- Produces: `public struct EngineServer` with `public static func start(socketPath: String, service: SandboxEngineService, group: EventLoopGroup) async throws -> Server`; the `arca-engine` executable accepting `--socket-path <path>`.

- [ ] **Step 1: Write the failing test**

Create `~/code/arca/Tests/ArcaEngineTests/EngineServerTests.swift`:

```swift
import GRPC
import NIOCore
import NIOPosix
import XCTest
@testable import ArcaEngine

final class EngineServerTests: XCTestCase {
    /// The socket carries the engine's whole authority, so it must not be
    /// reachable by another user on a shared machine.
    func testTheSocketIsCreatedOwnerOnly() async throws {
        let group = MultiThreadedEventLoopGroup(numberOfThreads: 1)
        defer { try? group.syncShutdownGracefully() }
        let path = NSTemporaryDirectory() + "arca-engine-test-\(UUID().uuidString).sock"

        let server = try await EngineServer.start(
            socketPath: path,
            service: .forTesting(),
            group: group
        )
        defer { try? server.close().wait() }

        let mode = try FileManager.default.attributesOfItem(atPath: path)[.posixPermissions] as? NSNumber
        XCTAssertEqual(mode?.uint16Value ?? 0 & 0o777, 0o600)
    }

    /// A stale socket file from a killed engine must not make the next start
    /// fail. Removing it is safe only because the caller owns the path.
    func testAStaleSocketFileDoesNotBlockStartup() async throws {
        let group = MultiThreadedEventLoopGroup(numberOfThreads: 1)
        defer { try? group.syncShutdownGracefully() }
        let path = NSTemporaryDirectory() + "arca-engine-test-\(UUID().uuidString).sock"
        FileManager.default.createFile(atPath: path, contents: Data())

        let server = try await EngineServer.start(
            socketPath: path,
            service: .forTesting(),
            group: group
        )
        try server.close().wait()
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cd ~/code/arca && swift test --filter EngineServerTests
```

Expected: FAIL with `cannot find 'EngineServer' in scope`.

- [ ] **Step 3: Write the server**

Create `~/code/arca/Sources/ArcaEngine/EngineServer.swift`:

```swift
import Foundation
import GRPC
import NIOCore
import NIOPosix

/// Binds the sandbox-engine service to a Unix domain socket.
public struct EngineServer {
    /// Starts the engine on `socketPath`.
    ///
    /// A stale socket left by a killed engine is removed first: bind fails with
    /// EADDRINUSE against a file whose listener is gone, and an engine that
    /// cannot restart after a crash is worse than one that reclaims its own
    /// path. Only a socket is removed -- refusing to unlink a regular file
    /// keeps a mistyped path from destroying data.
    ///
    /// The mode is set to 0600 before the listener accepts, because the socket
    /// carries the engine's entire authority.
    public static func start(
        socketPath: String,
        service: SandboxEngineService,
        group: EventLoopGroup
    ) async throws -> Server {
        try removeStaleSocket(at: socketPath)
        let server = try await Server.insecure(group: group)
            .withServiceProviders([service])
            .bind(unixDomainSocketPath: socketPath)
            .get()
        try FileManager.default.setAttributes(
            [.posixPermissions: NSNumber(value: Int16(0o600))],
            ofItemAtPath: socketPath
        )
        return server
    }

    private static func removeStaleSocket(at path: String) throws {
        var status = stat()
        guard lstat(path, &status) == 0 else { return }
        guard (status.st_mode & S_IFMT) == S_IFSOCK else {
            throw EngineServerError.pathIsNotASocket(path)
        }
        try FileManager.default.removeItem(atPath: path)
    }
}

public enum EngineServerError: Error, CustomStringConvertible {
    case pathIsNotASocket(String)

    public var description: String {
        switch self {
        case .pathIsNotASocket(let path):
            return "refusing to replace \(path): it exists and is not a socket"
        }
    }
}
```

- [ ] **Step 4: Write the executable**

Create `~/code/arca/Sources/arca-engine/ArcaEngineCommand.swift`:

```swift
import ArcaEngine
import ArgumentParser
import ContainerBridge
import Foundation
import Logging
import NIOPosix

@main
struct ArcaEngineCommand: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "arca-engine",
        abstract: "Serves the arca.engine.v1 sandbox-engine contract over a Unix socket."
    )

    @Option(name: .customLong("socket-path"), help: "Path of the Unix socket to serve on.")
    var socketPath: String

    @Option(name: .customLong("state-root"), help: "Directory holding engine state.")
    var stateRoot: String

    @Option(name: .customLong("log-level"), help: "trace, debug, info, notice, warning, error.")
    var logLevel: String = "info"

    func run() async throws {
        var logger = Logger(label: "arca-engine")
        logger.logLevel = Logger.Level(rawValue: logLevel) ?? .info

        let root = URL(fileURLWithPath: stateRoot)
        let containerManager = ContainerManager(stateRoot: root, logger: logger)
        let service = SandboxEngineService(
            containerManager: containerManager,
            volumeManager: VolumeManager(stateRoot: root, logger: logger),
            networkManager: NetworkManager(stateRoot: root, logger: logger),
            imageManager: ImageManager(stateRoot: root, logger: logger),
            execManager: ExecManager(containerManager: containerManager, logger: logger),
            logger: logger
        )

        let group = MultiThreadedEventLoopGroup(numberOfThreads: System.coreCount)
        let server = try await EngineServer.start(
            socketPath: socketPath,
            service: service,
            group: group
        )
        logger.info("engine listening", metadata: ["socket": "\(socketPath)"])
        try await server.onClose.get()
    }
}
```

Use the same real `init` signatures Task 2 Step 4 established. If `ContainerManager` requires `initialize()` before use, call it here after construction and record that in the task report.

- [ ] **Step 5: Run the tests and build the executable**

```bash
cd ~/code/arca && swift test --filter ArcaEngineTests && swift build --product arca-engine
```

Expected: tests PASS; build succeeds and prints nothing on success.

- [ ] **Step 6: Confirm it actually serves**

```bash
cd ~/code/arca
sock=$(mktemp -d)/engine.sock
state=$(mktemp -d)
swift run arca-engine --socket-path "$sock" --state-root "$state" &
enginepid=$!
sleep 3
find "$sock" -type s && echo "SOCKET PRESENT"
kill "$enginepid"
```

Expected: `SOCKET PRESENT`. If the socket is absent, read the engine's output before changing anything — a missing socket here is a real failure, not a timing artefact to be slept around.

- [ ] **Step 7: Commit**

```bash
cd ~/code/arca
git add Sources/ArcaEngine/EngineServer.swift Sources/arca-engine/ Tests/ArcaEngineTests/EngineServerTests.swift
env -u SSH_AUTH_SOCK git commit -m "feat(engine): serve the contract over a Unix socket

The socket is 0600 because it carries the engine's whole authority. A
stale socket from a killed engine is reclaimed, but only when it is
actually a socket -- refusing to unlink a regular file keeps a mistyped
path from destroying data."
git log --format='%h %G? %s' -1
```

---

### Task 8: Gas Can builds the engine product

**Files:**
- Modify: `~/code/gascan/scripts/build-arca-engine.sh:101-112`

**Interfaces:**
- Produces: the script prints two lines — the checkout path, then the absolute path of the built `arca-engine` binary. Task 9 consumes the second line.

The script currently builds `--target ContainerBridge --target SandboxEngineProto` and prints only the checkout path (`:109-112`). Its comment at `:101-102` says this is the line that changes when P5.1 lands an executable.

- [ ] **Step 1: Confirm the current behaviour**

```bash
cd ~/code/gascan && sed -n '100,113p' scripts/build-arca-engine.sh
```

Expected: the `swift build --target ...` invocation and a single `printf '%s\n' "$checkout"`.

- [ ] **Step 2: Replace the build and output**

In `~/code/gascan/scripts/build-arca-engine.sh`, replace lines 101-112 with:

```bash
# The engine product, plus SandboxEngineProto so the generated server half is
# proven to build rather than merely proven to have been emitted --
# crates/gascan-engine-proto generates a client from the same revision, so
# without this the pinned server end would be the only one nothing compiled.
#
# ContainerBridge is no longer named: arca-engine reaches it transitively, and
# naming it separately would hide the day that edge disappears.
swift build --package-path "$checkout" --configuration release \
  --product arca-engine --target SandboxEngineProto >&2

# Arca has no CI, so nothing else ever runs the engine's own tests and they
# would rot unnoticed. This is a clean checkout of the signed tag, which makes
# it the right place: it proves the pinned engine passes its own suite rather
# than proving a developer's working tree did.
swift test --package-path "$checkout" --filter ArcaEngineTests >&2

binary=$checkout/.build/release/arca-engine
[[ -x $binary ]] || {
  printf 'engine build produced no executable at %s\n' "$binary" >&2
  exit 70
}

printf '%s\n%s\n' "$checkout" "$binary"
```

- [ ] **Step 3: Verify the script passes shellcheck**

```bash
cd ~/code/gascan && shellcheck scripts/build-arca-engine.sh
```

Expected: no output, exit 0.

- [ ] **Step 4: Run it against a local checkout**

Because the committed pin still names the pre-engine tag, point the script at the working Arca tree with a temporary pin. Create a signed tag on the Arca branch first — the script verifies the tag against `engine/allowed-signers` and asserts it resolves to the pinned revision (`:64-78`), and that gate is not to be weakened.

```bash
cd ~/code/arca
env -u SSH_AUTH_SOCK git tag -s gascan-engine-dev -m "P5.1 milestone 1 development tag"
revision=$(git rev-parse HEAD)

cd ~/code/gascan
mkdir -p .artifacts
cat > .artifacts/arca-dev-pin.json <<JSON
{
  "schema": 1,
  "name": "arca",
  "url": "file://$HOME/code/arca",
  "tag": "gascan-engine-dev",
  "revision": "$revision"
}
JSON
GASCAN_ARCA_PIN_FILE=$PWD/.artifacts/arca-dev-pin.json ./scripts/build-arca-engine.sh
```

`.artifacts/` is gitignored (`.gitignore:3`), so the development pin never lands in a
commit. **The committed `engine/arca-pin.json` is not touched by this milestone** — it moves
once, in Milestone 4, to the single signed tag that ships. Tasks 9 and 10 re-derive the
binary path from this same file, so keep it.

Re-run this whole step after every Arca commit: the tag must move to the new revision
(`git tag -f -s`), and the pin's `revision` must be updated to match, or the script's
tag-target assertion (`:74-78`) will reject it — correctly.

Expected: two lines on stdout — a checkout path, then a path ending `/.build/release/arca-engine`. Confirm the second is executable with `find <path> -perm -u+x`.

If the tag signature does not verify, check that `git config --local user.signingkey` in `~/code/arca` names a key whose public half is in `~/code/gascan/engine/allowed-signers`. **Do not bypass the verification** — a build that skips it is not the build Gas Can ships.

- [ ] **Step 5: Commit**

```bash
cd ~/code/gascan
git add scripts/build-arca-engine.sh
env -u SSH_AUTH_SOCK git commit -m "build: build the engine product and report its binary path

The pin build has never produced an executable because none existed. It
does now, and the live tier needs to find it. ContainerBridge stops being
named because arca-engine reaches it transitively, so naming it would hide
the day that edge disappears."
git log --format='%h %G? %s' -1
```

---

### Task 9: The live tier, and the connect answers

**Files:**
- Create: `~/code/gascan/crates/gascan-arca/tests/live.rs`
- Create: `~/code/gascan/crates/gascan-arca/tests/live/common/mod.rs`
- Create: `~/code/gascan/crates/gascan-arca/tests/live/connect.rs`
- Modify: `~/code/gascan/crates/gascan-arca/Cargo.toml`

**Interfaces:**
- Consumes: the engine binary path from Task 8, supplied to tests as `GASCAN_ARCA_ENGINE_BIN`.
- Produces: `common::LiveEngine` with `async fn start() -> LiveEngine`, `fn socket(&self) -> &Utf8Path`, and `async fn transport(&self) -> ChannelTransport`; `Drop` kills the child.

This is the tier that answers two of the claims in design §9. It requires a built engine, so every test is `#[ignore]`d.

- [ ] **Step 1: Add the dev-dependencies**

In `~/code/gascan/crates/gascan-arca/Cargo.toml`, replace the `[dev-dependencies]` block with:

```toml
[dev-dependencies]
camino.workspace = true
tempfile = "3"
tokio = { workspace = true, features = ["macros", "process", "rt", "rt-multi-thread", "sync", "time"] }
```

- [ ] **Step 2: Write the module wiring**

Create `~/code/gascan/crates/gascan-arca/tests/live.rs`, mirroring `crates/gascan-apple/tests/live.rs`:

```rust
#[path = "live/common/mod.rs"]
mod common;
#[path = "live/connect.rs"]
mod connect;
```

- [ ] **Step 3: Write the harness**

Create `~/code/gascan/crates/gascan-arca/tests/live/common/mod.rs`:

```rust
use camino::{Utf8Path, Utf8PathBuf};
use gascan_arca::ChannelTransport;
use std::time::Duration;

/// An engine process on a temporary socket, killed when the test ends.
///
/// The live tier drives the engine directly rather than through `gascand`.
/// It kills streams, resets mid-exec, and kills the engine under an open
/// call, and a supervisor whose job is to react to exactly those events
/// would be fighting the tests. Supervision is exercised by `gascan-e2e`.
pub struct LiveEngine {
    child: tokio::process::Child,
    socket: Utf8PathBuf,
    _root: tempfile::TempDir,
}

impl LiveEngine {
    /// Starts the engine named by `GASCAN_ARCA_ENGINE_BIN`.
    ///
    /// Panics with a directive message when the variable is absent, because a
    /// live test that silently skips is a live test nobody notices has stopped
    /// running.
    pub async fn start() -> Self {
        let binary = std::env::var("GASCAN_ARCA_ENGINE_BIN").expect(
            "GASCAN_ARCA_ENGINE_BIN must name a built arca-engine; \
             run scripts/build-arca-engine.sh and use its second output line",
        );
        let root = tempfile::tempdir().unwrap();
        let path = Utf8Path::from_path(root.path()).unwrap().to_owned();
        let socket = path.join("engine.sock");
        let state = path.join("state");
        std::fs::create_dir_all(&state).unwrap();

        let child = tokio::process::Command::new(&binary)
            .arg("--socket-path")
            .arg(socket.as_str())
            .arg("--state-root")
            .arg(state.as_str())
            .kill_on_drop(true)
            .spawn()
            .unwrap_or_else(|error| panic!("could not spawn {binary}: {error}"));

        let engine = Self {
            child,
            socket,
            _root: root,
        };
        engine.await_socket().await;
        engine
    }

    /// Waits for the socket to appear, then for a connection to succeed.
    ///
    /// Both halves are needed: the file appears before the listener accepts,
    /// so waiting only for the file races the bind. Bounded, because a hang
    /// here is a failure to report rather than a condition to wait out.
    async fn await_socket(&self) {
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            if self.socket.exists()
                && ChannelTransport::connect(self.socket.as_std_path().to_owned())
                    .await
                    .is_ok()
            {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "engine did not accept a connection on {} within 30s",
                self.socket
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    pub async fn transport(&self) -> ChannelTransport {
        ChannelTransport::connect(self.socket.as_std_path().to_owned())
            .await
            .expect("connecting to a started engine must succeed")
    }

    pub async fn kill(mut self) {
        self.child.kill().await.unwrap();
    }
}
```

- [ ] **Step 4: Write the connect tests**

Create `~/code/gascan/crates/gascan-arca/tests/live/connect.rs`:

```rust
use crate::common::LiveEngine;
use gascan_arca::ChannelTransport;

/// START-HERE recorded every error path through `connect` as unverified,
/// because no socket was ever dialed. These are those paths.
#[tokio::test]
#[ignore = "requires a built arca-engine named by GASCAN_ARCA_ENGINE_BIN"]
async fn connect_reports_a_missing_socket_by_naming_the_path() {
    let root = tempfile::tempdir().unwrap();
    let missing = root.path().join("absent.sock");

    let error = ChannelTransport::connect(missing.clone())
        .await
        .expect_err("connecting to a path with no socket must fail");
    let rendered = error.to_string();

    assert!(
        rendered.contains(missing.to_str().unwrap()),
        "must name the path it dialed: {rendered}"
    );
    assert!(
        rendered.contains("No such file or directory"),
        "must carry the io cause through the source chain rather than the \
         opaque 'transport error': {rendered}"
    );
}

#[tokio::test]
#[ignore = "requires a built arca-engine named by GASCAN_ARCA_ENGINE_BIN"]
async fn connect_distinguishes_a_path_that_is_not_a_socket() {
    let root = tempfile::tempdir().unwrap();
    let regular = root.path().join("not-a-socket");
    std::fs::write(&regular, b"regular file").unwrap();

    let error = ChannelTransport::connect(regular)
        .await
        .expect_err("connecting to a regular file must fail");

    assert!(
        !error.to_string().contains("No such file or directory"),
        "a present non-socket must not report as absent: {error}"
    );
}

/// The client dials with the placeholder authority `http://[::]:50051`, which
/// the connector ignores. Whether a real server accepts it was unverified.
#[tokio::test]
#[ignore = "requires a built arca-engine named by GASCAN_ARCA_ENGINE_BIN"]
async fn a_real_engine_accepts_the_placeholder_authority() {
    let engine = LiveEngine::start().await;
    let transport = engine.transport().await;

    let response = gascan_arca::EngineTransport::capabilities(
        &transport,
        gascan_engine_proto::v1::CapabilitiesRequest {},
    )
    .await
    .expect("a real engine must answer a request carrying the placeholder authority");

    assert!(
        response.outcome.is_some(),
        "the engine answered but set no outcome"
    );
}

/// An engine that dies under an open connection must surface as a transport
/// failure, not as a hang.
#[tokio::test]
#[ignore = "requires a built arca-engine named by GASCAN_ARCA_ENGINE_BIN"]
async fn a_call_against_a_killed_engine_fails_rather_than_hanging() {
    let engine = LiveEngine::start().await;
    let transport = engine.transport().await;
    engine.kill().await;

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        gascan_arca::EngineTransport::capabilities(
            &transport,
            gascan_engine_proto::v1::CapabilitiesRequest {},
        ),
    )
    .await
    .expect("a call against a dead engine must not hang");

    assert!(result.is_err(), "a dead engine must not answer successfully");
}
```

`gascan_engine_proto` is not currently a dev-dependency of `gascan-arca` — it is a regular dependency (`crates/gascan-arca/Cargo.toml`), so it is already in scope for tests. `EngineTransport` is exported from `gascan_arca` (`crates/gascan-arca/src/lib.rs:20`).

- [ ] **Step 5: Run the tests, ignored, to prove they compile**

```bash
cd ~/code/gascan && env -u RUSTUP_TOOLCHAIN cargo test -p gascan-arca --test live --no-fail-fast
```

Expected: compiles; `running 4 tests` with all 4 reported `ignored`.

- [ ] **Step 6: Run them for real**

```bash
cd ~/code/gascan
binary=$(GASCAN_ARCA_PIN_FILE=$PWD/.artifacts/arca-dev-pin.json ./scripts/build-arca-engine.sh | tail -1)
find "$binary" -perm -u+x || { echo "no executable engine at $binary"; exit 1; }
GASCAN_ARCA_ENGINE_BIN=$binary env -u RUSTUP_TOOLCHAIN \
  cargo test -p gascan-arca --test live --no-fail-fast -- --ignored
```

`.artifacts/arca-dev-pin.json` is the development pin Task 8 Step 4 wrote. If it is absent,
go back and run that step — it is what points the build at your Arca working tree.

Expected: `running 4 tests`, 4 passed. **Confirm the `running 4 tests` line** — a filter typo runs zero and exits 0.

If `connect_reports_a_missing_socket_by_naming_the_path` fails on the "No such file or directory" assertion, that is a real finding about `source_chain` (`crates/gascan-arca/src/channel.rs:62-78`) and belongs in the task report, not in a weakened assertion.

- [ ] **Step 7: Commit**

```bash
cd ~/code/gascan
git add crates/gascan-arca/tests/live.rs crates/gascan-arca/tests/live/ crates/gascan-arca/Cargo.toml
env -u SSH_AUTH_SOCK git commit -m "test: dial a real engine, and answer the connect claims

Every error path through connect was unverified because no socket had ever
been dialed, and whether a real server accepts the placeholder authority
was unknown. Both now have answers produced by a running engine."
git log --format='%h %G? %s' -1
```

---

### Task 10: Live coverage of the three implemented RPCs

**Files:**
- Create: `~/code/gascan/crates/gascan-arca/tests/live/read_rpcs.rs`
- Modify: `~/code/gascan/crates/gascan-arca/tests/live.rs`
- Modify: `~/code/gascan/tests/ci/expected-ignored-tests.txt`

**Interfaces:**
- Consumes: `common::LiveEngine` from Task 9.

- [ ] **Step 1: Add the module**

Append to `~/code/gascan/crates/gascan-arca/tests/live.rs`:

```rust
#[path = "live/read_rpcs.rs"]
mod read_rpcs;
```

- [ ] **Step 2: Write the tests**

Create `~/code/gascan/crates/gascan-arca/tests/live/read_rpcs.rs`:

```rust
use crate::common::LiveEngine;
use gascan_arca::ArcaBackend;
use gascan_core::runtime::{NetworkIsolation, RuntimeBackend};
use gascan_core::sandbox::SandboxId;

/// The backend over a real engine, not a fake. Everything below goes through
/// ChannelTransport and the real gRPC stack.
async fn backend(engine: &LiveEngine) -> ArcaBackend<gascan_arca::ChannelTransport> {
    ArcaBackend::new(engine.transport().await)
}

#[tokio::test]
#[ignore = "requires a built arca-engine named by GASCAN_ARCA_ENGINE_BIN"]
async fn capabilities_report_only_what_this_engine_build_implements() {
    let engine = LiveEngine::start().await;
    let capabilities = backend(&engine).await.capabilities().await.unwrap();

    // Milestone 1 creates nothing and execs nothing, so it claims nothing.
    // Milestone 4 replaces this test with one asserting every flag is true.
    assert!(!capabilities.bind_mounts);
    assert!(!capabilities.named_volumes);
    assert!(!capabilities.tty);
    assert!(!capabilities.signals);
    assert!(!capabilities.loopback_publish);
    assert!(!capabilities.resource_limits);
    assert_eq!(capabilities.offline, NetworkIsolation::Unverified);
}

/// Three arms, and this is the one a reconciler depends on most: an absent
/// sandbox must be Ok(None), never an error.
#[tokio::test]
#[ignore = "requires a built arca-engine named by GASCAN_ARCA_ENGINE_BIN"]
async fn inspecting_an_unknown_sandbox_answers_absent_rather_than_failing() {
    let engine = LiveEngine::start().await;
    let id = SandboxId::test("never-created");

    let observed = backend(&engine).await.inspect(&id).await;

    assert!(
        matches!(observed, Ok(None)),
        "an unknown sandbox must be Ok(None): {observed:?}"
    );
}

#[tokio::test]
#[ignore = "requires a built arca-engine named by GASCAN_ARCA_ENGINE_BIN"]
async fn listing_an_empty_engine_returns_an_empty_list_rather_than_an_error() {
    let engine = LiveEngine::start().await;

    let resources = backend(&engine).await.list_resources().await.unwrap();

    assert!(resources.is_empty(), "a fresh engine holds nothing: {resources:?}");
}

/// The eight unimplemented methods must ANSWER. A gRPC status would reach the
/// consumer as an unreachable engine, which is a different fact from "this
/// build cannot do that", and would send a reconciler down the wrong path.
#[tokio::test]
#[ignore = "requires a built arca-engine named by GASCAN_ARCA_ENGINE_BIN"]
async fn an_unimplemented_method_reports_unsupported_capability_not_a_transport_fault() {
    let engine = LiveEngine::start().await;
    let id = SandboxId::test("never-created");

    let error = backend(&engine)
        .await
        .start(&id)
        .await
        .expect_err("Start is not implemented in this milestone");

    assert_eq!(
        error.code(),
        "unsupported_capability",
        "an unimplemented method must answer in its outcome, not as a status: {error}"
    );
}
```

- [ ] **Step 3: Run them**

```bash
cd ~/code/gascan
binary=$(GASCAN_ARCA_PIN_FILE=$PWD/.artifacts/arca-dev-pin.json ./scripts/build-arca-engine.sh | tail -1)
GASCAN_ARCA_ENGINE_BIN=$binary env -u RUSTUP_TOOLCHAIN \
  cargo test -p gascan-arca --test live --no-fail-fast -- --ignored
```

Expected: `running 8 tests`, 8 passed.

- [ ] **Step 4: Update the ignored-test baseline**

```bash
cd ~/code/gascan && ./scripts/ci-check-ignored-tests.sh
```

Expected: FAIL, listing the eight new ignored tests. Add exactly those names to `tests/ci/expected-ignored-tests.txt`, keeping the file's existing sort order, then re-run:

```bash
cd ~/code/gascan && ./scripts/ci-check-ignored-tests.sh
```

Expected: exit 0.

- [ ] **Step 5: Run the workspace suite alone**

```bash
cd ~/code/gascan && pgrep -fl "cargo test"
```

Expected: no output. Only then:

```bash
cd ~/code/gascan && env -u RUSTUP_TOOLCHAIN cargo test --workspace --no-fail-fast 2>&1 | tail -40
```

Expected: rc=0. Count only `test result:` lines reporting `0 filtered out`; the total should be 1433 plus the tests this milestone added, and you must be able to say which task added each one.

- [ ] **Step 6: Commit**

```bash
cd ~/code/gascan
git add crates/gascan-arca/tests/ tests/ci/expected-ignored-tests.txt
env -u SSH_AUTH_SOCK git commit -m "test: cover the three implemented RPCs against a real engine

Also pins that an unimplemented method answers in its outcome rather than
as a gRPC status: a status would reach the consumer as an unreachable
engine, which would send a reconciler down a different path entirely."
git log --format='%h %G? %s' -1
```

---

### Task 11: The engine must not reach DockerAPI

**Files:**
- Create: `~/code/gascan/tests/release/engine-targets-contract.sh`

**Interfaces:**
- Consumes: the checkout path from `scripts/build-arca-engine.sh`'s first output line.

Design §3.1 makes the absent `DockerAPI` edge the load-bearing property of the modular build. Nothing enforces it, and "we did not add that dependency" is exactly the kind of claim that decays silently.

- [ ] **Step 1: Confirm what the manifest describes**

```bash
cd ~/code/arca && swift package describe --type json | jq -r '.targets[] | select(.name=="ArcaEngine") | .target_dependencies[]?, .product_dependencies[]?'
```

Expected: `SandboxEngineProto`, `ContainerBridge`, `GRPC`, `Logging`. **Not** `DockerAPI` or `ArcaDaemon`.

- [ ] **Step 2: Write the contract test**

Create `~/code/gascan/tests/release/engine-targets-contract.sh`:

```sh
#!/bin/sh
# The engine must not reach Arca's Docker surface, transitively or directly.
#
# This is the property that makes "Gas Can builds only the targets it ships"
# checkable rather than aspirational. Contract §2 states why: a Docker-shaped
# API on the engine socket is a policy-bypass surface sitting beside the policy
# gate. An edge added to make something compile would forfeit that silently.
set -eu

checkout=${1:?usage: engine-targets-contract.sh <arca-checkout>}

for command in swift jq; do
  command -v "$command" >/dev/null || {
    printf 'required command is unavailable: %s\n' "$command" >&2
    exit 69
  }
done

describe=$(swift package describe --package-path "$checkout" --type json)

# Walk ArcaEngine's transitive target closure and fail on either forbidden name.
forbidden=$(printf '%s' "$describe" | jq -r '
  [.targets[] | {name: .name, deps: (.target_dependencies // [])}] as $targets
  | def closure($frontier; $seen):
      if ($frontier | length) == 0 then $seen
      else ($frontier[0]) as $name
        | ($targets[] | select(.name == $name) | .deps) as $deps
        | closure(($frontier[1:] + [$deps[]? | select(. as $d | $seen | index($d) | not)]);
                  ($seen + [$name]))
      end;
    closure(["ArcaEngine"]; [])
  | map(select(. == "DockerAPI" or . == "ArcaDaemon"))
  | unique
  | join(" ")
')

if [ -n "$forbidden" ]; then
  printf 'ArcaEngine reaches forbidden target(s): %s\n' "$forbidden" >&2
  exit 1
fi

printf 'ArcaEngine reaches neither DockerAPI nor ArcaDaemon\n'
```

- [ ] **Step 3: Make it executable and lint it**

```bash
cd ~/code/gascan && chmod +x tests/release/engine-targets-contract.sh && shellcheck tests/release/engine-targets-contract.sh
```

Expected: no shellcheck output, exit 0.

- [ ] **Step 4: Run it, and prove it can fail**

```bash
cd ~/code/gascan && ./tests/release/engine-targets-contract.sh ~/code/arca
```

Expected: `ArcaEngine reaches neither DockerAPI nor ArcaDaemon`, exit 0.

Now prove the test is capable of failing — a contract test that cannot fail is the defect `START-HERE:128-135` names. Temporarily add `"DockerAPI"` to `ArcaEngine`'s dependencies in `~/code/arca/Package.swift`, re-run, and confirm it exits 1 naming `DockerAPI`. **Then revert that edit with a targeted edit — never `git checkout <path>`**, which discards other in-flight work in a shared tree.

```bash
cd ~/code/arca && git diff --stat
```

Expected after reverting: no output. Re-run the contract test and confirm it passes again.

- [ ] **Step 5: Commit**

```bash
cd ~/code/gascan
git add tests/release/engine-targets-contract.sh
env -u SSH_AUTH_SOCK git commit -m "test: assert the engine reaches neither DockerAPI nor ArcaDaemon

The absent edge is what makes 'build only the targets we ship' checkable
instead of aspirational, and an edge added to make something compile would
forfeit it silently. Verified capable of failing by adding the dependency
and watching the test reject it."
git log --format='%h %G? %s' -1
```

---

## Milestone outline — what follows this plan

Each becomes its own plan, written when its predecessor lands so that nothing is guessed.

### Milestone 2 — Image ingress and lifecycle

`arca-engine image load --oci-layout <dir>` over `ImageManager.loadFromOCILayout`; `PrepareImage` as hold-or-fail; `Create` running volumes → network → container with partial-failure evidence in `CreateFailed.created`; `Start`, `Stop`, `Remove` with the label-mismatch refusal. `Capabilities` flips `project_mount`, `named_volumes`, `loopback_publish` and `resource_limits`. Live tier gains create/remove coverage, including a partial-failure case and `Inspect` reporting ports. **Ends with:** a sandbox that can be created, started, stopped and removed through the real client.

### Milestone 3 — Exec and Logs

`ExecManager.signalExec(execID:signal:)` added to `ContainerBridge`; the `Exec` bidi state machine with the first-frame rule; `Logs` streaming with `since_unix_millis`. `Capabilities` flips `tty` and `signals`. Live tier answers the remaining claims: framing frame for frame, `LogsChunk` ordering and clean end-of-stream, and cancellation on reset — asserted on engine-observable state (guest process gone, exec instance reaped) and proved capable of failing by mutation. **Ends with:** the RST_STREAM question answered, and the full backend surface exercised.

### Milestone 4 — Wiring, packaging, and the offline proof

`BackendSelection::Arca` with `GASCAN_ARCA_BACKEND` and `GASCAN_ENGINE_SOCKET`; `gascand` dialing, validating the socket's owning uid, and refusing an unrecognised `engine_version`; doctor facts reported as "the sandbox engine"; `gascan daemon status` including engine health and `gascan daemon restart --engine`; the launchd plist and `install.sh`/`uninstall.sh`/`verify-package.sh` changes; the `gascan-e2e` daemon-on-engine pass including a restart proving reconcile adopts a surviving sandbox. The offline-proof exercise and its `docs/evidence/` artifact, after which `Capabilities.offline` returns `PROVEN` for that revision and a test asserts every capability flag is true. Finally one signed Arca tag, one `engine/arca-pin.json` bump, and the documentation corrections in design §10. **Ends with:** `gascand` on the Arca backend, end to end.
