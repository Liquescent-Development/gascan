# P5.1 Milestone 2 — Engine State Ownership and Sandbox Lifecycle

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the Arca engine a private state root it fully owns, so `initialize()` can run safely, and build the sandbox lifecycle — create, start, stop, inspect, remove — on top of it.

**Architecture:** The engine stops sharing any mutable state with `ArcaDaemon` or with Apple's containerization framework. Mutable state moves under `~/Library/Application Support/dev.gascan/engine/`; the kernel and the vminit OCI layout become explicit read-only inputs. With a private root, `ContainerManager.initialize()`'s crash-recovery write becomes correct rather than destructive, so the engine initializes fully and refuses to start when an input is missing.

**Tech Stack:** Swift 6.x (Arca, `ContainerBridge` / `ArcaEngine` / `arca-engine`, XCTest), Rust (Gas Can, `crates/gascan-arca` live tier), gRPC over a Unix socket.

**Design:** `docs/superpowers/specs/2026-08-12-p5-1-milestone-2-engine-lifecycle-design.md`
**Parent design:** `docs/superpowers/specs/2026-08-10-p5-1-engine-service-and-wiring-design.md`

## Global Constraints

- **Signing is inverted between the repos.** Gas Can commits with `env -u SSH_AUTH_SOCK git commit`; **Arca needs the 1Password agent and breaks with that flag**. Never `--no-gpg-sign`, never a lightweight tag. Verify `%G?` is `G` after every commit. If Arca signing fails, **stop and ask** — 1Password answers `ssh-add -l` without approval but refuses to sign without a human at the keyboard.
- **Never commit to `main`.** Work on a branch; code reaches `main` only via PR. Merge commits only, never squash.
- **No co-author trailer and no AI-tool mention** in any commit message.
- **Rust:** prefix every cargo command with `env -u RUSTUP_TOOLCHAIN` (`RUSTUP_TOOLCHAIN=1.95.0` is exported and overrides `rust-toolchain.toml`). Use `--no-fail-fast`. `cargo clippy --fix` is prohibited.
- **Never run the workspace suite while any other cargo runs**, including other repos' sessions on this machine. Run it only after `pgrep -fl "cargo test"` comes back empty, and record that output. Concurrent suites produce dozens of unrelated failures. `-- --test-threads=N` makes it worse, not better.
- **`ls` is aliased** to something that rejects trailing-slash paths — use `find` or `git ls-files`.
- **No API takes a default** for the new path parameters. A default is how a caller silently keeps the old behaviour after the reason for it has gone.
- **No escaping Swift `throw` from a provider method.** An uncaught throw becomes a gRPC status, which `engine.proto:52-58` reserves for transport faults. Every RPC catches everything and maps to an `EngineError`.
- **Engine error vocabulary is fixed by the client** — the twelve codes at `crates/gascan-arca/src/error.rs:20-55`. `resource` holds the resource name, `message` holds prose; they are never transposed. Every response sets its `oneof`.
- **Every guard ships only after being seen to fail.** Revert the guard alone, confirm the test goes red, restore it, and record the observed failure. A check that passes for a reason you cannot name is red.

## Where the tests live, and why

Gas Can's engine job runs **only** `swift test --filter ArcaEngineTests` (`scripts/build-arca-engine.sh:199-200`, guarded by a listing check at `:195-198`). That job is the only automated thing that ever exercises Arca — Arca has no CI of its own (`.github/workflows` does not exist there, 2026-08-12).

**So every test this plan adds goes in `Tests/ArcaEngineTests/`, including the `ContainerBridge` ones.** A `ContainerBridgeTests` target would not be run by the gate and would rot unnoticed — exactly the decay the parent design §8.1 identified. The properties under test are engine requirements, so the placement is honest as well as practical.

## File Structure

**Arca — modified**

| File | Responsibility after this milestone |
|---|---|
| `Sources/ContainerBridge/ContainerManager.swift` | takes an image-store root and a layer-cache path; `listContainers` takes `includeInternal` |
| `Sources/ContainerBridge/NetworkManager.swift` | `listNetworks()` propagates backend failures instead of swallowing them |
| `Sources/ArcaDaemon/ArcaDaemon.swift` | passes its paths explicitly at the two construction sites |
| `Sources/DockerAPI/Handlers/*.swift` | four `listContainers` sites and two `listNetworks` sites updated |
| `Sources/arca-engine/ArcaEngineCommand.swift` | three path options, input validation, vminit load, full `initialize()` |
| `Sources/ArcaEngine/SandboxEngineService.swift` | `Inspect`, `ListResources`, `PrepareImage`, `Create`, `Start`, `Stop`, `Remove` |

**Arca — created**

| File | Responsibility |
|---|---|
| `Sources/ArcaEngine/EngineStartup.swift` | input validation, vminit load, digest-keyed `initfs.ext4` — testable without a VM |
| `Tests/ArcaEngineTests/ContainerBridgePathsTests.swift` | the isolation properties of Task 1 |
| `Tests/ArcaEngineTests/ListFilterTests.swift` | Tasks 2 and 3 |
| `Tests/ArcaEngineTests/EngineStartupTests.swift` | Tasks 4 and 5 |

**Gas Can — modified**: `crates/gascan-arca/tests/live/`, `tests/ci/expected-ignored-tests.txt`.

---

## Landing 1 — the `ContainerBridge` changes

Arca only. Separable, reviewable, and green on its own.

### Task 1: `ContainerManager` takes its storage roots explicitly

**Files:**
- Modify: `~/code/arca/Sources/ContainerBridge/ContainerManager.swift` (init at `:171`, `initialize()` at `:213`, `:240`, `:247`)
- Modify: `~/code/arca/Sources/ArcaDaemon/ArcaDaemon.swift:163`
- Modify: `~/code/arca/Sources/arca-engine/ArcaEngineCommand.swift:84`
- Modify: `~/code/arca/Tests/ArcaEngineTests/TestSupport.swift:28`
- Test: `~/code/arca/Tests/ArcaEngineTests/ContainerBridgePathsTests.swift` (create)

**Interfaces:**
- Produces: `ContainerManager.init(imageManager:kernelPath:imageStoreRoot:layerCachePath:stateStore:logger:)` where `imageStoreRoot: URL` and `layerCachePath: URL`; `ContainerManager.imageStoreRoot: URL` and `ContainerManager.layerCachePath: URL` as `public let`, so a test can assert on the resolved paths.

**Why both parameters in one task:** they are two fields on one initializer, both about path isolation, and every call site changes once. A reviewer would accept or reject them together.

- [ ] **Step 1: Write the failing test**

Create `~/code/arca/Tests/ArcaEngineTests/ContainerBridgePathsTests.swift`:

```swift
import ContainerBridge
import Foundation
import Logging
import XCTest

final class ContainerBridgePathsTests: XCTestCase {
    private func temporaryRoot() -> URL {
        URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("arca-paths-\(UUID().uuidString)")
    }

    /// The engine must not share Apple's containerization image store, because
    /// `initfs.ext4` is derived from it -- Containerization's ContainerManager
    /// builds `imageStore.path/initfs.ext4`. Sharing that store is what forces
    /// ArcaDaemon to delete the file on every start; a private root removes the
    /// need for any coordination.
    func testTheImageStoreRootIsTheOneItWasGiven() throws {
        let logger = Logger(label: "paths-tests")
        let root = temporaryRoot()
        let stateStore = try StateStore(
            path: root.appendingPathComponent("state.db").path,
            logger: logger
        )
        let imageStoreRoot = root.appendingPathComponent("images")
        let manager = ContainerManager(
            imageManager: try ImageManager(logger: logger, imageStorePath: imageStoreRoot),
            kernelPath: root.appendingPathComponent("vmlinux").path,
            imageStoreRoot: imageStoreRoot,
            layerCachePath: root.appendingPathComponent("layers"),
            stateStore: stateStore,
            logger: logger
        )

        XCTAssertEqual(manager.imageStoreRoot, imageStoreRoot)
        XCTAssertFalse(
            manager.imageStoreRoot.path.contains("com.apple.containerization"),
            "the engine's image store must not resolve into Apple's shared store"
        )
    }

    /// `~/.arca/layers` was hardcoded, so a dev.gascan-rooted engine would still
    /// write its layer cache into Arca's tree.
    func testTheLayerCacheIsTheOneItWasGiven() throws {
        let logger = Logger(label: "paths-tests")
        let root = temporaryRoot()
        let stateStore = try StateStore(
            path: root.appendingPathComponent("state.db").path,
            logger: logger
        )
        let layerCachePath = root.appendingPathComponent("layers")
        let manager = ContainerManager(
            imageManager: try ImageManager(
                logger: logger,
                imageStorePath: root.appendingPathComponent("images")
            ),
            kernelPath: root.appendingPathComponent("vmlinux").path,
            imageStoreRoot: root.appendingPathComponent("images"),
            layerCachePath: layerCachePath,
            stateStore: stateStore,
            logger: logger
        )

        XCTAssertEqual(manager.layerCachePath, layerCachePath)
        XCTAssertFalse(
            manager.layerCachePath.path.hasSuffix(".arca/layers"),
            "the engine's layer cache must not resolve into Arca's tree"
        )
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cd ~/code/arca && swift test --filter ContainerBridgePathsTests 2>&1 | tail -20
```

Expected: FAIL to compile — `extra arguments 'imageStoreRoot', 'layerCachePath'` and `value of type 'ContainerManager' has no member 'imageStoreRoot'`.

- [ ] **Step 3: Add the two stored properties and initializer parameters**

In `Sources/ContainerBridge/ContainerManager.swift`, beside the existing stored properties:

```swift
    /// Root of the Containerization ImageStore this manager uses. Held as a
    /// `let` and not derived, because `initfs.ext4` lives inside it: sharing
    /// this root is what makes two products fight over one initfs.
    public let imageStoreRoot: URL

    /// Directory holding the OverlayFS layer cache. Was hardcoded to
    /// ~/.arca/layers, which meant every consumer wrote into Arca's tree
    /// regardless of the state root it was given.
    public let layerCachePath: URL
```

Replace the initializer at `:171` with:

```swift
    public init(
        imageManager: ImageManager,
        kernelPath: String,
        imageStoreRoot: URL,
        layerCachePath: URL,
        stateStore: StateStore,
        logger: Logger
    ) {
        self.imageManager = imageManager
        self.kernelPath = kernelPath
        self.imageStoreRoot = imageStoreRoot
        self.layerCachePath = layerCachePath
        self.stateStore = stateStore
        self.logger = logger
        self.logManager = ContainerLogManager(logger: logger)
    }
```

Neither parameter takes a default. A default would let a caller keep the shared path after the reason for it has gone.

- [ ] **Step 4: Use them in `initialize()`**

At `:240`, pass the root through to Containerization — its initializer at `containerization/Sources/Containerization/ContainerManager.swift:128-166` accepts `root: URL?` and builds `ImageStore(path: root)`:

```swift
        nativeManager = try await Containerization.ContainerManager(
            kernel: kernel,
            initfsReference: "arca-vminit:latest",
            root: imageStoreRoot,
            network: try Containerization.VmnetNetwork()
        )
```

At `:247`, replace the hardcoded path:

```swift
        overlayUnpacker = OverlayFSUnpacker(
            layerCachePath: layerCachePath,
            stateStore: stateStore,
            logger: logger
        )
```

- [ ] **Step 5: Update all three call sites explicitly**

`Sources/ArcaDaemon/ArcaDaemon.swift:163` — Arca keeps its own paths, stated rather than defaulted:

```swift
        let containerManager = ContainerManager(
            imageManager: imageManager,
            kernelPath: config.kernelPath,
            // ImageStore.default.path rather than a re-derivation of Apple's
            // path: re-deriving it would silently diverge the day Apple changes
            // it, and a hand-rolled `urls(for:in:)[0]` traps on an empty array
            // where ImageStore.defaultRoot() throws.
            imageStoreRoot: ImageStore.default.path,
            layerCachePath: URL(
                fileURLWithPath: NSString(string: "~/.arca/layers").expandingTildeInPath
            ),
            stateStore: stateStore,
            logger: logger
        )
```

`Sources/arca-engine/ArcaEngineCommand.swift:84` — the engine roots both under its state root:

```swift
        let containerManager = ContainerManager(
            imageManager: imageManager,
            kernelPath: kernelPath,
            imageStoreRoot: root.appendingPathComponent("images"),
            layerCachePath: root.appendingPathComponent("layers"),
            stateStore: stateStore,
            logger: logger
        )
```

**SUPERSEDED 2026-08-12 by review, maintainer ruled the reviewer governs.** The two blocks
below duplicate the engine's path derivation between production and test support, and that
duplication is itself the defect: a test built on a hand-copy exercises a replica of the
wiring rather than the wiring, so changing the engine's root leaves the suite green. Derive
both paths in **one shared function** — `enginePaths(stateRoot:)` — called by
`ArcaEngineCommand` and `TestSupport` alike, and assert on that function. Later tasks must
not reintroduce the copy.

`Tests/ArcaEngineTests/TestSupport.swift:28` — same shape, against the throwaway root already built there:

```swift
        let containerManager = ContainerManager(
            imageManager: imageManager,
            kernelPath: root.appendingPathComponent("vmlinux").path,
            imageStoreRoot: root.appendingPathComponent("images"),
            layerCachePath: root.appendingPathComponent("layers"),
            stateStore: stateStore,
            logger: logger
        )
```

- [ ] **Step 6: Run the tests to verify they pass**

```bash
cd ~/code/arca && swift test --filter ArcaEngineTests 2>&1 | tail -20
```

Expected: PASS, with the count risen from 30 by exactly 2. Confirm the `Executed N tests` line; a bare filter name that matches nothing exits 0 having run nothing.

- [ ] **Step 7: Prove the guard can fail**

Revert **only** the `initialize()` change at `:240` — drop `root: imageStoreRoot` so it falls back to `ImageStore.default` — and confirm nothing goes red, because the test asserts on the stored property rather than on Containerization's resolved store. **This is the instrument being narrower than the claim.** Fix it by asserting the property is what `initialize()` actually passes: extract the root selection into a `public func containerizationRoot() -> URL` on `ContainerManager`, call that at `:240`, and assert on it in the test. Re-run, revert again, confirm **red**, restore, and record both observations.

- [ ] **Step 8: Commit** (Arca — needs the 1Password agent; do **not** use `env -u SSH_AUTH_SOCK`)

```bash
cd ~/code/arca
git add Sources/ContainerBridge/ContainerManager.swift Sources/ArcaDaemon/ArcaDaemon.swift \
        Sources/arca-engine/ArcaEngineCommand.swift Tests/ArcaEngineTests/
git commit -m "feat(bridge): let each consumer own its image store and layer cache

initfs.ext4 is not a fixed path -- Containerization derives it as
imageStore.path/initfs.ext4 (ContainerManager.swift:102, :146) and its
initializer takes root:. Passing no root put every consumer on
ImageStore.default, which is why ArcaDaemon deletes that file on each start to
force regeneration. A consumer given its own root needs no such coordination.

The layer cache was hardcoded to ~/.arca/layers, so a consumer with its own
state root still wrote into Arca's tree.

Neither parameter takes a default. A default is how a call site keeps a shared
path after the reason for sharing it has gone."
git log --format='%h %G? %s' -1
```

Expected: `%G?` reports `G`.

---

### Task 2: `listContainers` stops hiding internal containers by accident

**Files:**
- Modify: `~/code/arca/Sources/ContainerBridge/ContainerManager.swift:523`, `:531`, `:556`
- Modify: `~/code/arca/Sources/DockerAPI/Handlers/ContainerHandlers.swift:72`, `:1567`
- Modify: `~/code/arca/Sources/DockerAPI/Handlers/ImageHandlers.swift:390`
- Modify: `~/code/arca/Sources/DockerAPI/Handlers/SystemHandlers.swift:70`
- Test: `~/code/arca/Tests/ArcaEngineTests/ListFilterTests.swift` (create)

**Interfaces:**
- Produces: `ContainerManager.listContainers(all:filters:includeInternal:) async throws -> [ContainerSummary]` with `includeInternal: Bool` and no default; `ContainerManager.internalContainersRequested(in filters: [String: [String]]) -> Bool` as a `public static func` so the Docker path keeps its filter-derived behaviour without duplicating the rule.

**Deviation from the design, stated rather than silent.** Design §3 says "Docker surface passes `false`". That is right for three of the four call sites but wrong for `ContainerHandlers.swift:72`, which serves `docker ps` and today turns internal containers **on** when the caller passes `--filter label=com.arca.internal`. Passing `false` there would change Docker behaviour. That site passes the derived value instead; the derivation moves to a named static so the rule exists once.

- [ ] **Step 1: Write the failing test**

Create `~/code/arca/Tests/ArcaEngineTests/ListFilterTests.swift`:

```swift
import ContainerBridge
import XCTest

final class ListFilterTests: XCTestCase {
    /// gascan's drift and leak detection reads ListResources. A container that
    /// exists but is not listed is a leak the consumer can never see, so the
    /// engine asks for everything explicitly.
    func testInternalContainersAreRequestedByAnExplicitFlagNotASubstring() {
        XCTAssertFalse(ContainerManager.internalContainersRequested(in: [:]))
        XCTAssertFalse(
            ContainerManager.internalContainersRequested(in: ["label": ["com.example.other"]])
        )
        XCTAssertTrue(
            ContainerManager.internalContainersRequested(in: ["label": ["com.arca.internal"]])
        )
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cd ~/code/arca && swift test --filter ListFilterTests 2>&1 | tail -20
```

Expected: FAIL to compile — `type 'ContainerManager' has no member 'internalContainersRequested'`.

- [ ] **Step 3: Add the static and the parameter**

In `Sources/ContainerBridge/ContainerManager.swift`, above `listContainers`:

```swift
    /// Docker's rule for asking to see internal containers: a `label` filter
    /// mentioning `com.arca.internal`. Named and lifted out because the engine
    /// does not use it -- the engine asks with `includeInternal:` directly --
    /// and a rule that exists in two places diverges.
    public static func internalContainersRequested(in filters: [String: [String]]) -> Bool {
        filters["label"]?.contains { $0.contains("com.arca.internal") } ?? false
    }
```

Change the signature at `:523` and delete the derivation at `:531`:

```swift
    public func listContainers(
        all: Bool = false,
        filters: [String: [String]] = [:],
        includeInternal: Bool
    ) async throws -> [ContainerSummary] {
```

At `:556` the filter now reads the parameter:

```swift
            if !includeInternal && info.labels["com.arca.internal"] == "true" {
                return nil
            }
```

- [ ] **Step 4: Update the four Docker call sites**

`ContainerHandlers.swift:72` keeps Docker's filter-derived behaviour:

```swift
            var containers = try await containerManager.listContainers(
                all: all,
                filters: filters,
                includeInternal: ContainerManager.internalContainersRequested(in: filters)
            )
```

`ContainerHandlers.swift:1567`, `ImageHandlers.swift:390`, `SystemHandlers.swift:70` each add `includeInternal: false`, preserving today's behaviour exactly. For `SystemHandlers.swift:70` keep the existing `(try? …) ?? []` shape unchanged apart from the new argument.

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cd ~/code/arca && swift test --filter ArcaEngineTests 2>&1 | tail -20
```

Expected: PASS, count up by 1 from Task 1's total.

- [ ] **Step 6: Prove the guard can fail**

Revert **only** the `:556` change back to the derived `showInternal`, re-run, and confirm the `includeInternal: true` behaviour is now unobservable through the static test alone — then add the behavioural assertion this reveals is missing: a `ContainerManager` seeded with one container labelled `com.arca.internal=true`, listed with `includeInternal: true` and then `false`, asserting the container appears and then does not. Confirm **red** with the guard reverted, restore, record both.

- [ ] **Step 7: Commit** (Arca — 1Password agent)

```bash
cd ~/code/arca
git add Sources/ContainerBridge/ContainerManager.swift Sources/DockerAPI/Handlers/ Tests/ArcaEngineTests/
git commit -m "feat(bridge): ask for internal containers explicitly

listContainers hid every container labelled com.arca.internal=true unless a
label filter happened to mention that string. The engine's contract is 'every
resource the engine holds, labelled or not', so it must be able to ask
directly: a flag cannot be satisfied by accident, and a substring match on a
filter value can.

Docker's filter-derived rule is preserved for docker ps and lifted into a named
static, so the rule exists in one place rather than two."
git log --format='%h %G? %s' -1
```

---

### Task 3: `listNetworks` reports a backend failure instead of a clean list

**Files:**
- Modify: `~/code/arca/Sources/ContainerBridge/NetworkManager.swift:545`, `:553`, `:600`
- Modify: `~/code/arca/Sources/DockerAPI/Handlers/NetworkHandlers.swift:42-44`, `:521`
- Test: `~/code/arca/Tests/ArcaEngineTests/ListFilterTests.swift` (extend)

**Interfaces:**
- Produces: `NetworkManager.listNetworks() async throws -> [NetworkMetadata]`.

- [ ] **Step 1: Change the signature and propagate**

At `:545`:

```swift
    /// Throws rather than returning a short list. A WireGuard-backend failure
    /// swallowed by `try?` turns a real failure into a confident report of a
    /// clean host, which is the report that hides a leak. gascan maps a thrown
    /// failure to `command_io`; it has no way to see a silently short list.
    public func listNetworks() async throws -> [NetworkMetadata] {
```

At `:553` the `try?` becomes a `try`:

```swift
        if let backend = wireGuardBackend {
            networks.append(contentsOf: try await backend.listNetworks())
        }
```

- [ ] **Step 2: Update the three call sites**

`NetworkManager.swift:600` becomes `let allNetworks = try await listNetworks()`; its enclosing function gains `throws` if it does not already have it — check with `swift build` and follow the compiler.

`NetworkHandlers.swift:44` and `:521` become `try await networkManager.listNetworks()`. **Delete the comment at `:42`** ("networkManager.listNetworks() doesn't throw - it returns an array directly"), which is now false; a stale comment asserting the opposite of the code is worse than none.

- [ ] **Step 3: Build to find every caller the compiler knows about**

```bash
cd ~/code/arca && swift build 2>&1 | tail -30
```

Expected: clean. Any remaining error names a call site missing `try` — fix it rather than re-adding `try?`.

- [ ] **Step 4: Run the tests**

```bash
cd ~/code/arca && swift test --filter ArcaEngineTests 2>&1 | tail -20
```

Expected: PASS at Task 2's count.

- [ ] **Step 5: Prove the guard can fail**

This one has no unit-level seam yet: the backends are populated only by `initialize()`. **Do not ship it unproven.** Write the failure injection now — a `NetworkManager` whose `wireGuardBackend` is a stub that throws — assert `listNetworks()` throws rather than returning `[]`, then revert `:553` to `try?` and confirm the test goes **red** (it returns an empty array). Restore and record. If injecting the stub requires a seam that does not exist, add the seam; do not settle for asserting the signature.

- [ ] **Step 6: Commit** (Arca — 1Password agent)

```bash
cd ~/code/arca
git add Sources/ContainerBridge/NetworkManager.swift Sources/DockerAPI/Handlers/NetworkHandlers.swift Tests/ArcaEngineTests/
git commit -m "fix(bridge): let a network backend failure surface

listNetworks swallowed a WireGuard-backend failure with try?, dropping every
bridge network and returning success. That is latent while both backends are
nil and becomes live the moment initialize() runs -- a real failure rendered as
a clean host, which is the answer that hides a leak rather than reporting one."
git log --format='%h %G? %s' -1
```

---

### Task 3b: The remaining `try?` swallows on the same backend

**Added 2026-08-12 by maintainer ruling**, after Task 3's review found two more swallows of the
identical shape. Task 3 fixed `listNetworks`; these are its twins, and one is worse than the
original because it gates a **deletion** rather than a listing.

**Files:**
- Modify: `~/code/arca/Sources/ContainerBridge/NetworkManager.swift` — `getNetworkAttachments` and `getContainerNetworks`
- Modify: `~/code/arca/Sources/DockerAPI/Handlers/NetworkHandlers.swift` — the prune path
- Test: `~/code/arca/Tests/ArcaEngineTests/ListFilterTests.swift` (extend)

**Re-derive every line number before editing.** Task 3 shifted this file; the review read
`getNetworkAttachments` around `:571-577` and `getContainerNetworks` around `:625` at `8b3e16f`,
while the implementer's earlier report cited `:530-534` and `:578` pre-change. Locate them with
`grep -n 'func getNetworkAttachments\|func getContainerNetworks' Sources/ContainerBridge/NetworkManager.swift`.

**Order matters — take the prune gate first.**

1. **`getNetworkAttachments`** swallows with `try?` and returns `[:]`, which is indistinguishable
   from "nothing attached". `NetworkHandlers.swift`'s prune path reads it as the "skip networks
   with active containers" gate. Failure: a transient store failure on `getNetworkContainers`
   while `listNetworks` succeeds → `docker network prune` **deletes an in-use network** and
   reports success. Task 3 narrowed this — a *total* backend failure now makes `listNetworks`
   throw and prune bail — so only partial failure remains, which is the harder case to notice.
2. **`getContainerNetworks`** swallows the same way and feeds `getWireGuardClient`, which reads
   an empty result as "not attached to any WireGuard network".

**Widening `BridgeNetworkLister` is a decision, not a drive-by.** Task 3's seam covers listing
only. If these need injection, decide deliberately whether the protocol grows or a second seam is
better, and say which in the report. Prefer `package` visibility over `public`, per the ruling on
`loadPersistedState()`.

**Prove each guard can fail**, separately, the way Task 3 did: induce the backend failure, assert
the call throws rather than returning an empty collection, revert that guard alone, confirm red,
restore. A test covering only one of the two methods leaves the other unproven.

**For the prune gate specifically, assert on the deletion**, not only on the thrown error: a test
that proves `getNetworkAttachments` throws does not prove prune declines to delete. The property
that matters is that an in-use network survives a partial store failure.

---

### Task 3c: Make the gate actually run the DockerAPI-side tests

**Added 2026-08-12 by maintainer ruling.** Task 3b's prune-deletion test — the one guarding a
bug that deletes an in-use network — lives in `Tests/ArcaTests/` and **nothing ever runs it.**

The bind is structural and the placement is correct: `NetworkHandlers` is in `DockerAPI`,
`ArcaEngineTests` depends only on `ArcaEngine`, and Gas Can's `tests/release/engine-targets-check.sh`
exists to assert the engine never reaches `DockerAPI`. The test cannot move into `ArcaEngineTests`
without breaking the property this milestone protects.

**Two changes are needed, and the second alone is not enough.** VERIFIED 2026-08-12: the suite
uses `import Testing` (swift-testing), and `swift test list --disable-swift-testing` — the gate's
own invocation at `scripts/build-arca-engine.sh:190-191` — returns **zero** matches for it. So
widening the filter by itself yields a gate that looks broader and still runs nothing new.

1. **Arca — convert `Tests/ArcaTests/NetworkPruneGateTests.swift` to XCTest.** The rest of the
   suite is XCTest and the gate's contract is built on `swift test list` identifiers under
   `--disable-swift-testing`. Preserve both tests exactly, including the control
   ("an unused network *is* pruned") that stops the survival assertion passing vacuously.
   Confirm afterwards that `swift test list --disable-swift-testing | grep NetworkPruneGate`
   returns a match — that is the check that this task did anything at all.
2. **Gas Can — widen `scripts/build-arca-engine.sh`.** The filter at `:199-200` becomes a regex
   covering `ArcaEngineTests` and the named DockerAPI-side suite. Filter the *named suite*, not
   all of `ArcaTests`: that target holds integration tests needing a live daemon and VMs, which
   must not enter the gate.

**The listing guard is the delicate part.** `:195-198` exists because `swift test --filter` exits
0 when it matches nothing — the guard fails the build when the listing has no `ArcaEngineTests`
entry. Widening the filter without widening the guard reintroduces exactly the hole the guard was
written to close, for the new suite. **Assert both suites are present, independently**, so the
gate fails if either disappears.

**Prove the guard can fail, both halves separately.** Rename each suite in a scratch checkout in
turn and confirm the script exits non-zero naming the missing one. A guard that fires only for the
suite it was originally written for is the same defect in a new place.

**Convert all of `NetworkPruneGateTests`, including the third test Task 3b's fix round adds.**
That round is closing a hole the reviewer measured: deleting `if !attachments.isEmpty { continue }`
from `NetworkHandlers.swift:606` leaves every test green, because both original prune tests pin
"the read *failed* → don't delete" and neither pins "the read *succeeded and was non-empty* →
don't delete" — which is what the gate exists for. **Converting before that lands would enshrine
a suite that still misses the gate's normal path.** Sequence this task after Task 3b closes.

Gas Can commits use `env -u SSH_AUTH_SOCK git commit`; Arca commits use the plain form. This task
touches **both repositories**, so the signing rule flips partway through it.

---

## Landing 2 — engine startup

### Task 4: The three path options, validated before anything is constructed

**Files:**
- Create: `~/code/arca/Sources/ArcaEngine/EngineStartup.swift`
- Modify: `~/code/arca/Sources/arca-engine/ArcaEngineCommand.swift`
- Test: `~/code/arca/Tests/ArcaEngineTests/EngineStartupTests.swift` (create)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces:
  ```swift
  public struct EngineInputs: Sendable {
      public let stateRoot: URL
      public let kernelPath: URL
      public let vminitLayout: URL
      public init(stateRoot: URL, kernelPath: URL, vminitLayout: URL)
  }

  public enum EngineStartupError: Error, CustomStringConvertible {
      case missingInput(name: String, path: String)
      case unreadableInput(name: String, path: String, cause: String)
      case unexpectedVminitReference(expected: String, actual: String)
  }

  public func validateEngineInputs(_ inputs: EngineInputs) throws
  ```
  `--kernel-path` and `--vminit-layout` as required `@Option`s on `ArcaEngineCommand`, joining the existing required `--state-root`.

- [ ] **Step 1: Write the failing test**

Create `~/code/arca/Tests/ArcaEngineTests/EngineStartupTests.swift`:

```swift
import Foundation
import XCTest
@testable import ArcaEngine

final class EngineStartupTests: XCTestCase {
    private func temporaryRoot() -> URL {
        let root = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("arca-startup-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(
            at: root, withIntermediateDirectories: true
        )
        return root
    }

    /// A missing kernel is a refusal to start, not a degraded engine. An engine
    /// that starts and answers unsupported_capability for everything that
    /// matters is the state the C1 review finding was raised against.
    func testAMissingKernelRefusesAndNamesThePathTried() throws {
        let root = temporaryRoot()
        let kernelPath = root.appendingPathComponent("vmlinux")
        let inputs = EngineInputs(
            stateRoot: root,
            kernelPath: kernelPath,
            vminitLayout: root.appendingPathComponent("vminit")
        )

        XCTAssertThrowsError(try validateEngineInputs(inputs)) { error in
            guard let startupError = error as? EngineStartupError,
                  case .missingInput(let name, let path) = startupError else {
                return XCTFail("expected missingInput, got \(error)")
            }
            XCTAssertEqual(name, "--kernel-path")
            XCTAssertEqual(path, kernelPath.path)
        }
    }

    /// The vminit layout must be a directory holding an OCI layout, not merely
    /// a path that exists.
    func testAVminitLayoutWithoutAnOCIMarkerIsRefused() throws {
        let root = temporaryRoot()
        let kernelPath = root.appendingPathComponent("vmlinux")
        FileManager.default.createFile(atPath: kernelPath.path, contents: Data("k".utf8))
        let layout = root.appendingPathComponent("vminit")
        try FileManager.default.createDirectory(at: layout, withIntermediateDirectories: true)

        XCTAssertThrowsError(
            try validateEngineInputs(
                EngineInputs(stateRoot: root, kernelPath: kernelPath, vminitLayout: layout)
            )
        )
    }

    /// All three present and well-formed is the only case that proceeds.
    func testCompleteInputsValidate() throws {
        let root = temporaryRoot()
        let kernelPath = root.appendingPathComponent("vmlinux")
        FileManager.default.createFile(atPath: kernelPath.path, contents: Data("k".utf8))
        let layout = root.appendingPathComponent("vminit")
        try FileManager.default.createDirectory(at: layout, withIntermediateDirectories: true)
        FileManager.default.createFile(
            atPath: layout.appendingPathComponent("oci-layout").path,
            contents: Data(#"{"imageLayoutVersion":"1.0.0"}"#.utf8)
        )
        FileManager.default.createFile(
            atPath: layout.appendingPathComponent("index.json").path,
            contents: Data(#"{"schemaVersion":2,"manifests":[]}"#.utf8)
        )

        XCTAssertNoThrow(
            try validateEngineInputs(
                EngineInputs(stateRoot: root, kernelPath: kernelPath, vminitLayout: layout)
            )
        )
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cd ~/code/arca && swift test --filter EngineStartupTests 2>&1 | tail -20
```

Expected: FAIL to compile — `cannot find 'EngineInputs' in scope`.

- [ ] **Step 3: Write `EngineStartup.swift`**

```swift
import Foundation

/// The engine's inputs, split by mutability.
///
/// `stateRoot` is mutable and private to this engine: state.db, images/,
/// volumes/, layers/. Sharing it with a live ArcaDaemon is the hazard the C1
/// review finding named -- ContainerManager's restore loop marks persisted
/// "running" containers exited 137 and writes that back.
///
/// `kernelPath` and `vminitLayout` are read-only inputs. A file two processes
/// read is safe to share; a state root is not. They are separate options so
/// that the CLI cannot express the confusion.
public struct EngineInputs: Sendable {
    public let stateRoot: URL
    public let kernelPath: URL
    public let vminitLayout: URL

    public init(stateRoot: URL, kernelPath: URL, vminitLayout: URL) {
        self.stateRoot = stateRoot
        self.kernelPath = kernelPath
        self.vminitLayout = vminitLayout
    }
}

public enum EngineStartupError: Error, CustomStringConvertible {
    case missingInput(name: String, path: String)
    case unreadableInput(name: String, path: String, cause: String)
    case unexpectedVminitReference(expected: String, actual: String)

    public var description: String {
        switch self {
        case .missingInput(let name, let path):
            return "\(name) names nothing that exists: \(path)"
        case .unreadableInput(let name, let path, let cause):
            return "\(name) is unusable at \(path): \(cause)"
        case .unexpectedVminitReference(let expected, let actual):
            return "the vminit layout holds \(actual), not \(expected)"
        }
    }
}

/// Refuses before any manager is constructed, so a bad input costs a clear
/// error rather than a partially-initialised engine.
public func validateEngineInputs(_ inputs: EngineInputs) throws {
    let fileManager = FileManager.default

    var isDirectory: ObjCBool = false
    guard fileManager.fileExists(atPath: inputs.kernelPath.path, isDirectory: &isDirectory) else {
        throw EngineStartupError.missingInput(
            name: "--kernel-path", path: inputs.kernelPath.path
        )
    }
    guard !isDirectory.boolValue else {
        throw EngineStartupError.unreadableInput(
            name: "--kernel-path", path: inputs.kernelPath.path,
            cause: "is a directory, not a kernel image"
        )
    }

    isDirectory = false
    guard fileManager.fileExists(atPath: inputs.vminitLayout.path, isDirectory: &isDirectory) else {
        throw EngineStartupError.missingInput(
            name: "--vminit-layout", path: inputs.vminitLayout.path
        )
    }
    guard isDirectory.boolValue else {
        throw EngineStartupError.unreadableInput(
            name: "--vminit-layout", path: inputs.vminitLayout.path,
            cause: "is a file, not an OCI layout directory"
        )
    }

    // An OCI layout is identified by these two files. Checking only that the
    // directory exists would accept an empty directory and fail later, inside
    // ImageManager, with a message about the wrong thing.
    for marker in ["oci-layout", "index.json"] {
        let path = inputs.vminitLayout.appendingPathComponent(marker).path
        guard fileManager.fileExists(atPath: path) else {
            throw EngineStartupError.unreadableInput(
                name: "--vminit-layout", path: inputs.vminitLayout.path,
                cause: "is not an OCI layout: no \(marker)"
            )
        }
    }
}
```

- [ ] **Step 4: Add the two options to the command**

In `Sources/arca-engine/ArcaEngineCommand.swift`, beside the existing `--state-root` at `:19-20`:

```swift
    @Option(name: .customLong("kernel-path"), help: "Path of the Linux kernel image to boot sandboxes with.")
    var kernelPath: String

    @Option(name: .customLong("vminit-layout"), help: "Directory holding the arca-vminit OCI layout.")
    var vminitLayout: String
```

Both required, neither defaulted, and **neither falls back to `~/.arca`**. Early in `run()`, before any manager is constructed:

```swift
        let inputs = EngineInputs(
            stateRoot: URL(fileURLWithPath: stateRoot),
            kernelPath: URL(fileURLWithPath: kernelPath),
            vminitLayout: URL(fileURLWithPath: vminitLayout)
        )
        try validateEngineInputs(inputs)
```

Replace `root.appendingPathComponent("vmlinux").path` at `:86` and `:91` with `inputs.kernelPath.path`.

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cd ~/code/arca && swift test --filter ArcaEngineTests 2>&1 | tail -20
```

Expected: PASS, count up by 3 from Task 3's total.

- [ ] **Step 6: Prove the guard can fail**

Revert **only** the `--kernel-path` existence check (the first `guard` in `validateEngineInputs`), run `swift test --filter EngineStartupTests`, and confirm `testAMissingKernelRefusesAndNamesThePathTried` goes **red**. Restore and record. Repeat for the `oci-layout` marker loop against `testAVminitLayoutWithoutAnOCIMarkerIsRefused`.

- [ ] **Step 7: Commit** (Arca — 1Password agent)

```bash
cd ~/code/arca
git add Sources/ArcaEngine/EngineStartup.swift Sources/arca-engine/ArcaEngineCommand.swift Tests/ArcaEngineTests/EngineStartupTests.swift
git commit -m "feat(engine): take the kernel and vminit as inputs, and refuse without them

Mutable state and read-only inputs are now separate options. --state-root is
private to this engine; --kernel-path and --vminit-layout are files it reads
and may share. Neither has a default and neither falls back to ~/.arca: a
default is how a process silently ends up pointed at another product's state.

A missing or malformed input refuses to start and names which one and the path
tried, before any manager is constructed. An engine that starts and cannot act
is the state the C1 finding was raised against."
git log --format='%h %G? %s' -1
```

---

### Task 5: vminit loads into the engine's own store, and `initfs.ext4` is keyed to it

**Files:**
- Modify: `~/code/arca/Sources/ArcaEngine/EngineStartup.swift`
- Modify: `~/code/arca/Sources/arca-engine/ArcaEngineCommand.swift`
- Test: `~/code/arca/Tests/ArcaEngineTests/EngineStartupTests.swift` (extend)

**Interfaces:**
- Consumes: `EngineInputs`, `EngineStartupError` from Task 4.
- Produces:
  ```swift
  public func loadVminit(
      from layout: URL,
      into imageManager: ImageManager,
      stateRoot: URL,
      logger: Logger
  ) async throws -> String   // returns the loaded image digest
  ```
  and `public func recordedVminitDigest(stateRoot: URL) -> String?`, `public func recordVminitDigest(_ digest: String, stateRoot: URL) throws`, backed by `<stateRoot>/vminit-digest`.

**Verify before implementing.** Two readings below are a best reading, not verified:
- **That `loadFromOCILayout` returns an image exposing a stable digest string.** Confirm with `grep -n 'public var digest\|public let digest\|var descriptor' ~/code/arca/containerization/Sources/Containerization/Image/Image.swift`. If the digest is reachable only through `descriptor.digest`, use that and adjust the signature.
- **That `arca-vminit:latest` is the reference the layout carries.** `ArcaDaemon.swift:117` says the OCI layout is tagged via an index annotation. Confirm against the real layout with `head -c 400 ~/.arca/vminit/index.json`.

- [ ] **Step 1: Verify both readings above and record the output**

Run both commands and paste their output into the task notes before writing code. If either differs from the reading, adjust the signature and say so in the commit message.

- [ ] **Step 2: Write the failing test**

Extend `EngineStartupTests.swift`:

```swift
    /// The digest is recorded so that initfs.ext4 is regenerated when vminit
    /// changes and only then. Unconditional deletion would rebuild a ~178MB
    /// image on every start; sharing Apple's path -- which is what ArcaDaemon
    /// deletes -- would destroy a live daemon's initfs.
    func testAnUnrecordedDigestReadsAsAbsentAndRoundTrips() throws {
        let root = temporaryRoot()
        XCTAssertNil(recordedVminitDigest(stateRoot: root))

        try recordVminitDigest("sha256:abc123", stateRoot: root)
        XCTAssertEqual(recordedVminitDigest(stateRoot: root), "sha256:abc123")

        try recordVminitDigest("sha256:def456", stateRoot: root)
        XCTAssertEqual(recordedVminitDigest(stateRoot: root), "sha256:def456")
    }

    /// The record lives under the state root, never beside Apple's shared
    /// initfs.ext4.
    func testTheDigestRecordLivesUnderTheStateRoot() throws {
        let root = temporaryRoot()
        try recordVminitDigest("sha256:abc123", stateRoot: root)

        let recorded = root.appendingPathComponent("vminit-digest")
        XCTAssertTrue(FileManager.default.fileExists(atPath: recorded.path))
        XCTAssertFalse(recorded.path.contains("com.apple.containerization"))
    }
```

- [ ] **Step 3: Run the test to verify it fails**

```bash
cd ~/code/arca && swift test --filter EngineStartupTests 2>&1 | tail -20
```

Expected: FAIL to compile — `cannot find 'recordedVminitDigest' in scope`.

- [ ] **Step 4: Implement the digest record and the load**

Append to `EngineStartup.swift`:

```swift
import ContainerBridge
import Logging

private let vminitDigestFile = "vminit-digest"
private let expectedVminitReference = "arca-vminit:latest"

public func recordedVminitDigest(stateRoot: URL) -> String? {
    try? String(
        contentsOf: stateRoot.appendingPathComponent(vminitDigestFile), encoding: .utf8
    ).trimmingCharacters(in: .whitespacesAndNewlines)
}

public func recordVminitDigest(_ digest: String, stateRoot: URL) throws {
    try FileManager.default.createDirectory(
        at: stateRoot, withIntermediateDirectories: true
    )
    try digest.write(
        to: stateRoot.appendingPathComponent(vminitDigestFile),
        atomically: true,
        encoding: .utf8
    )
}

/// Loads the vminit layout into the engine's OWN image store and returns its
/// digest.
///
/// ArcaDaemon logs and continues when the loaded reference is unexpected
/// (ArcaDaemon.swift:131). This engine refuses: booting sandboxes on an
/// unknown init image is not a warning-level condition.
public func loadVminit(
    from layout: URL,
    into imageManager: ImageManager,
    stateRoot: URL,
    logger: Logger
) async throws -> String {
    let loaded = try await imageManager.loadFromOCILayout(directory: layout)
    guard let image = loaded.first(where: { $0.reference == expectedVminitReference }) else {
        throw EngineStartupError.unexpectedVminitReference(
            expected: expectedVminitReference,
            actual: loaded.map(\.reference).joined(separator: ", ")
        )
    }

    let digest = image.descriptor.digest   // adjust per Step 1's verification
    if recordedVminitDigest(stateRoot: stateRoot) == digest {
        logger.debug("vminit unchanged; keeping the existing initfs")
    } else {
        let initfs = stateRoot.appendingPathComponent("images/initfs.ext4")
        try? FileManager.default.removeItem(at: initfs)
        logger.info("vminit changed; initfs will be regenerated", metadata: [
            "digest": "\(digest)"
        ])
        try recordVminitDigest(digest, stateRoot: stateRoot)
    }
    return digest
}
```

**Carried in from Task 4's review:** `EngineStartupError.unexpectedVminitReference` is the one
error case naming neither an option nor a path — it renders as `"the vminit layout holds X, not Y"`.
It is unused until this task, where it becomes user-visible for the first time. Give it the
`--vminit-layout` option name and the layout path, matching the other two cases: a refusal that
does not say what to fix is a worse failure than a crash.

- [ ] **Step 5: Call it from `run()` before any manager is constructed**

In `ArcaEngineCommand.swift`, after `validateEngineInputs` and after `imageManager` is built but before `ContainerManager` is constructed:

```swift
        _ = try await loadVminit(
            from: inputs.vminitLayout,
            into: imageManager,
            stateRoot: inputs.stateRoot,
            logger: logger
        )
```

Then **delete the long comment block at `:38-73`** explaining why `initialize()` is not called — it documents a decision this milestone reverses — and replace it with a short note that the engine owns its state root, citing the design.

**Also carried into this task by Task 1's review (Minor 3):** `ArcaDaemon.swift:97-98` still hand-derives `~/Library/Application Support/com.apple.containerization/initfs.ext4` in order to delete it — the exact re-derivation Task 1's new comment says the file avoids, sitting 70 lines above it. Task 1 left it because its behaviour had to stay fixed. Bind it to the resolved store instead, so the daemon names its image store once rather than three times by three mechanisms.

- [ ] **Step 6: Run the tests to verify they pass**

```bash
cd ~/code/arca && swift test --filter ArcaEngineTests 2>&1 | tail -20
```

Expected: PASS, count up by 2 from Task 4's total.

- [ ] **Step 7: Prove the guard can fail**

Revert **only** the digest comparison — always regenerate — and confirm `testAnUnrecordedDigestReadsAsAbsentAndRoundTrips` still passes, which shows it does not cover the branch. Add the covering test: record digest A, call the regeneration decision with digest A (expect keep) and with digest B (expect delete). Revert again, confirm **red**, restore, record.

- [ ] **Step 8: Commit** (Arca — 1Password agent)

```bash
cd ~/code/arca
git add Sources/ArcaEngine/EngineStartup.swift Sources/arca-engine/ArcaEngineCommand.swift Tests/ArcaEngineTests/EngineStartupTests.swift
git commit -m "feat(engine): load vminit into the engine's own store, keyed by digest

The engine loads its init image into <state-root>/images rather than the shared
store, so initfs.ext4 -- which Containerization derives from the store path --
is the engine's own. ArcaDaemon deletes the shared copy on every start; this
engine never touches that path and needs no coordination with it.

Regeneration is keyed on the loaded vminit digest, recorded under the state
root. Unconditional deletion would rebuild a ~178MB image every start for no
correctness gain, since initBlock reuses an existing file.

An unexpected reference is a refusal, not a warning. ArcaDaemon logs and
continues there; booting sandboxes on an unknown init image is not a
warning-level condition."
git log --format='%h %G? %s' -1
```

---

### Task 6: `initialize()` runs, and the engine serves only after it succeeds

**Files:**
- Modify: `~/code/arca/Sources/arca-engine/ArcaEngineCommand.swift`
- Test: `~/code/arca/Tests/ArcaEngineTests/EngineStartupTests.swift` (extend)

**Interfaces:**
- Consumes: everything from Tasks 1, 4, 5.
- Produces: an engine process that has called `initialize()` on `ContainerManager`, `VolumeManager` and `NetworkManager` before binding the socket.

**Verify before implementing.** This is the first task that needs a real VM host, and it is where a wrong reading costs the most:
- **`Containerization.VmnetNetwork()` may require entitlements or elevated privileges.** Milestone 1 never constructed one. Confirm by running the engine against a temp root with real inputs and reading the failure, if any.
- **`NetworkManager.initialize()` ends in `createDefaultNetworks()`, which creates a vmnet `host` network** (`NetworkManager.swift:95`, `:120-132`). Against a private state root that is the engine's own network to create — but confirm it does not collide with a `host` network a live `ArcaDaemon` already created at the vmnet layer, which is *not* state-root-scoped. **If it collides, stop and raise it**: it is the one hazard the private root may not cover, and the answer is a design decision, not a workaround.

- [ ] **Step 1: Run the engine by hand against real inputs and record what happens**

```bash
cd ~/code/arca && swift build --product arca-engine 2>&1 | tail -5
root=$(mktemp -d)/engine
.build/debug/arca-engine \
  --socket-path "$root.sock" \
  --state-root "$root" \
  --kernel-path ~/.arca/vmlinux \
  --vminit-layout ~/.arca/vminit \
  --log-level debug
```

Record the full output. If it fails, that failure is the task's real content — do not work around it, diagnose it. If `VmnetNetwork()` or `createDefaultNetworks()` is the failure, stop and raise it per the note above.

- [ ] **Step 2: Hoist the inline manager construction into named bindings**

`ArcaEngineCommand.swift:98` and `:103` construct `VolumeManager` and `NetworkManager` inline inside the `SandboxEngineService(...)` argument list, so there is nothing to call `initialize()` on. Bind them first:

```swift
        let volumeManager = VolumeManager(
            volumesBasePath: inputs.stateRoot.appendingPathComponent("volumes").path,
            stateStore: stateStore,
            logger: logger
        )
        let networkManager = NetworkManager(
            config: config,
            stateStore: stateStore,
            containerManager: containerManager,
            logger: logger
        )
```

then pass the bindings to `SandboxEngineService(...)` rather than fresh expressions.

**Two deferred minors from Task 1's review land here, because this step rewrites the exact
wiring they describe:**

- **Minor A — the parameter assignment is still duplicated.** `EnginePaths` unified the
  *derivation*, but which derived path goes to which constructor argument is spelt twice:
  `ArcaEngineCommand.swift:83-90` and `TestSupport.swift:49-58`. Swapping
  `imageStoreRoot: paths.layerCache` in the command alone leaves all 34 tests green,
  because the test drives a parallel wiring over a shared derivation. **Hoist into one
  factory that returns the built managers**, called by both the command and `TestSupport`
  — that is this step's real deliverable, not merely local `let` bindings.
- **Minor B — the path assertions do not separate the paths.** Setting
  `layerCache = stateRoot/"images"` passes all four tests: `ContainerBridgePathsTests.swift:36,50`
  check only `hasPrefix(root.path)` and `:64` checks `hasPrefix(root.path + "/")`. The image
  store and the layer cache would then be the same directory — a live failure mode, the
  OverlayFS layer cache writing into the content store. **Assert the six `EnginePaths`
  values are pairwise distinct** (two lines; the type is `Equatable`). Do **not** fix this
  by restating the derivation in the test — that reintroduces the tautology Task 1's review
  removed. Also add the missing trailing slash at `:36,50`.

- [ ] **Step 3: Add the initialize calls, ordered**

In `run()`, after the vminit load and before `EngineServer.start`:

```swift
        // Order matters and mirrors ArcaDaemon: the vminit image must be in the
        // store before ContainerManager.initialize() asks for it, and
        // NetworkManager needs a ContainerManager to resolve containers.
        try await volumeManager.initialize()
        try await containerManager.initialize()
        try await networkManager.initialize()
```

Bind the socket only after all three return. A failure here propagates out of `run()` and the process exits non-zero, which is the fail-fast behaviour §2.3 requires.

- [ ] **Step 4: Verify the engine starts and the socket appears**

Re-run Step 1's command. Expected: `engine listening`, a socket at `$root.sock` with mode `srw-------`, and `<state-root>/images/initfs.ext4` present. Confirm **no** file was created or modified under `~/.arca` or `~/Library/Application Support/com.apple.containerization`:

```bash
find ~/.arca -newermt '-5 minutes' -print
find ~/Library/Application\ Support/com.apple.containerization -newermt '-5 minutes' -print
```

Expected: both empty. **This is the milestone's central claim; if either prints a path, the isolation is not real** and the task is not done.

- [ ] **Step 5: Run the full Arca suite**

```bash
cd ~/code/arca && swift test --filter ArcaEngineTests 2>&1 | tail -20
```

Expected: PASS at Task 5's count.

- [ ] **Step 6: Commit** (Arca — 1Password agent)

```bash
cd ~/code/arca
git add Sources/arca-engine/ArcaEngineCommand.swift Tests/ArcaEngineTests/
git commit -m "feat(engine): initialize the managers before serving

With a state root the engine owns, initialize() is safe: the restore loop's
crash-recovery write marks containers this engine's own StateStore recorded as
running, which died with the previous engine process. That write was only ever
destructive against a root shared with a live ArcaDaemon.

The socket binds only after all three managers initialize, so a client never
reaches an engine that cannot act."
git log --format='%h %G? %s' -1
```

---

### Task 6b: Sign `arca-engine`, because unsigned it cannot start a container

**Added 2026-08-13 by maintainer ruling**, after Task 6's Step 1 measured it.

`ContainerManager.initialize()` constructs `Containerization.VmnetNetwork()`, which requires the
`com.apple.security.virtualization` entitlement. **Nothing signs `arca-engine`.** Arca's
`Makefile` `codesign:` target covers `$(BINARY)` and `$(TEST_HELPER)` only, and `arca-engine`
appears nowhere in that file (VERIFIED: `grep -n 'arca-engine' Makefile` returns nothing).

Unentitled, the failure is loud but misleading: `vmnet_return_t(rawValue: 1002)`, which the SDK
header (`vmnet.framework/Versions/A/Headers/vmnet.h:144`) labels `VMNET_MEM_FAILURE`. It is not a
memory failure. The process exits and never creates a socket. Signed ad-hoc with the existing
`Arca.entitlements`, the same binary initializes all three managers and serves.

**This reaches further than Arca.** `scripts/build-arca-engine.sh` builds `--product arca-engine`
and ships that binary to the live tier, unsigned — so the engine Gas Can hands its own tests
cannot start a container.

**Both repositories, ad-hoc for now:**

1. **Arca** — add `arca-engine` to the `codesign:` target alongside the existing two binaries, so
   a developer building it by hand gets a working engine instead of a misleading
   `VMNET_MEM_FAILURE` to debug.
2. **Gas Can** — `scripts/build-arca-engine.sh` signs the binary it produces, with
   `Arca.entitlements` taken **from the verified checkout** it already has. Ad-hoc (`--sign -`)
   needs no certificate, so it works on a hosted runner. Sign after the build and **before** the
   binary path is printed, so no caller can receive an unsigned path.

**Real Developer ID signing is milestone 4's**, with the rest of packaging. Ad-hoc is sufficient
for the gate and the live tier and is not sufficient for a shipped `.pkg`; say so in the script's
comment so milestone 4 does not mistake this for done.

**Prove it fails.** A test or contract case must fail when the signing step is removed —
otherwise this is another instrument that cannot fail. The honest check is behavioural: an
unsigned engine exits non-zero without creating its socket, so assert on that rather than on
`codesign -d` output, which proves the command ran and not that it worked.

---

## Landings 3-5

These tasks are named, scoped and ordered, but their steps are **not** written out in bite-sized detail, and that is deliberate rather than an omission. Landing 2's Task 6 is the first time anything in this project constructs a `VmnetNetwork` or runs `initialize()` against a private root. What it finds decides the shape of everything below — most sharply whether the vmnet `host` network collides with a live `ArcaDaemon`'s.

`START-HERE` records the rule this follows: nine blocks of milestone 1's plan were wrong, every one was marked "a best reading, not verified" with the command to confirm it, and every one surfaced as a directed correction rather than a fix round. **Writing detailed TDD steps for `Create` before Task 6 has run would be writing fiction.**

**Expand each landing into full steps immediately before starting it**, using the findings recorded in Task 6.

### What Task 6 actually found, and what it changes below

Task 6 ran and its review reproduced every measurement. The private-root model **holds**: the
isolation probes came back empty, and the vmnet `host` network does not collide — two concurrent
engines on separate state roots took distinct auto-allocated subnets, because the `host` name is a
row in each engine's own `state.db` and `VmnetNetworkBackend`'s `isDefault` guard reads a
per-instance dictionary, not a host-wide namespace. **Landings 3-5 proceed as designed**, with
four corrections:

1. **Landing 3 — Tasks 7 and 8 must seed state through `loadPersistedState()`, not a stub.** It is
   the only VM-free writer of `ContainerManager.containers`, it needs no entitlement, and using it
   exercises the real restore path rather than a replica.
2. **Landing 4 — Task 11 gains a new first step: cross-wire the managers.** The engine never calls
   `containerManager.setNetworkManager(_:)` or `setVolumeManager(_:)`; `ArcaDaemon` does, at
   `ArcaDaemon.swift:236` and `:262`. Without them `ContainerManager.swift:1741` throws
   `volumeManagerNotAvailable` for any container with anonymous volumes, `:2427` skips network
   auto-attachment, and — worst — `:4129` `cleanupVolumesForContainer` **returns silently** when
   `volumeManager` is nil, so `Remove` leaks volumes while reporting success. Task 6's fix round
   closes this, but Task 11 must not assume it: verify the wiring before writing `Create`.

   **A fourth setter of the same class is still unwired: `setPortMapManager`.** The daemon sets it
   (`ArcaDaemon.swift:278`); the engine does not. `ContainerManager.swift:2482` guards
   `if let portMapManager` before `publishPorts`, and `:2730`/`:3058` do the same before
   `unpublishPorts` — so an unwired engine **starts a container with published ports, publishes
   nothing, and reports success.** Pre-existing rather than introduced by Task 6, and outside that
   task's finding, but it lands on work already scheduled: Task 7 maps real ports, Task 11 has
   ports publishing on loopback, and `loopback_publish` is one of Task 14's capability flips.
   Wire it in Task 11 beside the other two, and test it the same way — drive `Start` and assert a
   port is **actually published**, not that the setter was called.
3. **Task 14 — `named_volumes` cannot be flipped until that wiring exists.** A flag whose
   machinery is unwired is a claim with no instrument, which §5 already forbids.
4. **Landing 5 reorders — signing (Task 6b) lands BEFORE the live tier.** No live test runs against
   an unentitled binary: unsigned, `initialize()` dies at `VmnetNetwork()` and the engine never
   creates a socket.

### The daemon-vs-engine divergence audit, closed rather than incidental

Three findings of one class had surfaced by accident, so the enumeration was finished deliberately.
`grep -rn "public func set[A-Z]" Sources/ContainerBridge/*.swift` returns **eight** collaborator
setters in the whole module — six on `ContainerManager`, one each on `ImageManager` and
`NetworkManager`; `VolumeManager` and `ExecManager` have none, taking everything by initializer.
**The daemon calls seven, the engine calls two, and one is called by neither.** Six divergences,
three silent, one to fix.

**Fix first — `setPortMapManager`, already routed to Task 11.** `ContainerManager.swift:2482` is a
three-way `if let` chain with **no `else`**, so `Start` completes and reports success having
published nothing; `:2730` and `:3058` guard teardown the same way. `networkManager` is now set,
so this is the only missing conjunct. It hits Task 11 and Task 12, and Task 7 as well — `Inspect`
would report bindings that were never published, which is worse than reporting none.

**Silent but CORRECT, do not "fix" — `setHealthChecker` and `setEventEmitter`.** Both are invisible
when unset, and `engine.proto:38-48` lists eleven RPCs with no health or events method and no
health field anywhere. There is no engine answer a null collaborator makes wrong, and wiring an
`EventManager` would build toward a surface `tests/release/engine-targets-check.sh` requires the
engine not to have. Health becomes a watch item only if a later milestone reports it through
`Inspect`.

**TWO THINGS THE ENGINE MUST CONTINUE NOT TO DO.** Recorded because "mirror the daemon" is the
obvious way to close the item above, and it would import both:

1. **`applyRestartPolicies()` calls `startContainer` and boots VMs.** In an engine it would
   **resurrect sandboxes the consumer believes stopped, before the socket binds** — the consumer
   would never see the transition, and reconcile would meet running containers it did not start.
2. **The daemon's deletion of Apple's `initfs.ext4` and vminit image** is precisely the destructive
   shared-store behaviour the private root exists to avoid.

**No consequence:** `imageManager.initialize()` is a no-op — two log lines and a comment saying
"ImageStore is already initialized in init()".

**Two non-divergences, recorded so nobody chases them:** `setSharedVmnetNetwork` writes a property
with **no reader anywhere** in `ContainerBridge` (dead code), and the engine's wire-after-initialize
ordering is safe — none of the five collaborator names appears anywhere in `loadPersistedState()`
(`ContainerManager.swift:316-490`), so `portMapManager` can be set beside the other two.

**One unpinned observation, recorded because Landing 5 restarts engines in a loop.** In 13 engine
shutdowns the reviewer saw `ERROR: Cannot schedule tasks on an EventLoop that has already shut
down` **once**, under a concurrent pair receiving SIGTERM (exit was still 0). Six further
shutdowns, four with `SWIFTNIO_STRICT=1`, were clean. `serve()`'s own docstring
(`ArcaEngineCommand.swift:116-136`) claims this fixed, and NIO upgrades it to a forced crash under
strict mode. Not this milestone's doing — `serve()` is untouched — but a 1-in-13 shutdown fault is
a flaky live tier waiting to happen, and it should be chased before Task 13 rather than after.

### Landing 3 — `Inspect` and `ListResources` (EXPANDED 2026-08-13, from Task 6's findings)

Anchors re-derived at Arca `db6bedc`. Re-derive again before editing — this file has moved under
every task so far.

| What | Where, at `db6bedc` |
|---|---|
| `inspect` returning `notImplemented` | `SandboxEngineService.swift:130-131` |
| `listResources` returning `notImplemented` | `:268-269` |
| The stale comment read as spec | `:245-267` |
| `convertPortBindingsToMappings` — **`private`** | `ContainerManager.swift:944` |
| Inspect's entry point | `ContainerManager.getContainer(id:) async throws -> Container?` `:808` |
| Volumes | `VolumeManager.listVolumes(filters:) async throws` `:245` |
| Digest split, status map | `EngineTranslation.swift:27`, `:40-47` |

**Seed through `loadPersistedState()`, never a stub.** It is `package func`, VM-free, needs no
entitlement, and is the only writer of `ContainerManager.containers` reachable without a kernel.
`Tests/ArcaEngineTests/CrashRecoveryTests.swift` is the worked example. A stub would exercise a
replica of the wiring — the defect Task 1's review removed and Task 3's review re-found.

**Delete the comment at `:245-267` in whichever task lands first.** It states in the present tense
that `listContainers` drops internal containers and that both defects "must be fixed in
`ContainerBridge` before this method reports anything". Both were fixed in Tasks 2 and 3. Whoever
writes `ListResources` reads that block as their specification and will either re-fix a fixed thing
or miss that they must now pass `includeInternal: true` explicitly.

#### Task 7 — `Inspect`

**Three arms** — sandbox, `Absent`, error — because "it is not there" and "I could not tell" demand
opposite behaviour from a reconciler (`engine.proto:354-357`).

**Order is the whole fix for review I5.** Read the owner labels **first**. The removed body ran the
digest check first, so a container created from a tag under a colliding name made
`imageDigest(fromReference:)` return nil and the engine answered `invalid_output` — the code
`crates/gascan-arca/src/error.rs` reserves for "the engine sent me something I cannot interpret".
That tells the consumer the engine is broken when the truth is a foreign resource, and
`foreign_resource_refused` / `ownership_mismatch` exist for exactly this.

**Ports are review I4.** `convertPortBindingsToMappings` is `private`; widen it to `package` — not
`public`, per the `loadPersistedState()` ruling. `crates/gascan-arca/src/translate.rs:436-437`
reads an empty list as "publishes nothing", not "unknown", so a hardcoded `[]` is a fabricated
value every port-drift comparison then runs against.

**The trap:** `setPortMapManager` is still unwired (see the divergence audit above). Until Task 11
wires it, a container's *stored* `hostConfig.portBindings` can name bindings that were never
published. `Inspect` must report what the store holds — that is what drift detection compares —
but the test must not be written in a way that implies publication was verified.

Tests, each proved failable by reverting that guard alone:
- a sandbox with owner labels and a digest reference round-trips, ports included;
- a container with **no** gascan labels is refused as foreign, **not** as `invalid_output`;
- a container created from a **tag** under a colliding name takes the same foreign arm — this is
  the ordering test, and it must fail if the digest check moves back in front;
- an absent id answers `Absent`, and that arm is distinguishable from the error arm;
- a sandbox publishing 8080→80 reports one mapping, not zero.

#### Task 8 — `ListResources`

Containers via `listContainers(all: true, includeInternal: true)`, volumes via
`VolumeManager.listVolumes()`, networks via `NetworkManager.listNetworks()` — which **throws** now,
so a backend failure surfaces as `command_io` rather than a short list.

**Unlabelled and internal resources are reported, never filtered.** `engine.proto:387-391` is
"every resource the engine holds, labelled or not", because the consumer's drift and leak detection
depends on seeing them. Filtering engine-side breaks it silently, which is why Task 2 added an
explicit flag rather than a substring-matched filter.

Tests:
- a container labelled `com.arca.internal=true` **appears**;
- an unlabelled container appears;
- a volume and a network appear, seeded through `StateStore` and read back through the real path;
- a `listNetworks()` backend failure surfaces as an error arm, not an empty list — reuse the
  `BridgeNetworkLister` seam Task 3 built, and mutate the **production default** to prove it;
- an engine holding nothing answers with an empty list rather than an error, and that assertion is
  distinguishable from the failure above.

**The eighth-finding watch for this landing:** every test here asserts on a *collection*. The
recurring defect has twice been an assertion that could not tell "held one back" from "held
everything back" — `isEmpty` in Task 2, and the one-sided prune pair in Task 3b. **Assert
equality on the whole expected set**, not membership or non-emptiness, and seed at least two
resources so a filter that drops everything is distinguishable from one that drops the right thing.

### Landing 3 — original outline, superseded by the expansion above

**Task 7 — `Inspect`.** Replace the `notImplemented` body at `SandboxEngineService.swift`. Three arms: sandbox, `Absent`, error. Two constraints from the adversarial review, both of which the obvious implementation reintroduces:
- **Read the owner labels BEFORE the image-digest check** (review I5). A container under a colliding name created from a tag makes `imageDigest(fromReference:)` (`EngineTranslation.swift:27`) return nil; answering `invalid_output` there tells the consumer the engine is broken when the truth is a foreign resource. `foreign_resource_refused` and `ownership_mismatch` exist for this.
- **Map real ports** (review I4). `ContainerManager.convertPortBindingsToMappings` (`:881`) already does it for the restore path; it is `private` today and must be exposed. `crates/gascan-arca/src/translate.rs:436-437` reads an empty list as "publishes nothing", not "unknown".

**Task 8 — `ListResources`.** Containers via `listContainers(all: true, includeInternal: true)` (Task 2), volumes via `VolumeManager.listVolumes()`, networks via `NetworkManager.listNetworks()` (Task 3, now throwing). Unlabelled and internal resources are reported, never filtered.

### Landing 4 — image ingress and lifecycle

**Task 9 — `arca-engine image load --oci-layout <dir>`.** Over `ImageManager.loadFromOCILayout`, for workspace images. Distinct from `--vminit-layout`, which Task 5 already handles at startup; do not merge them.

**Task 10 — `PrepareImage`.** Hold-or-fail. Looks the digest up, materialises the rootfs, fails when absent. **Never pulls** — a fallback to `ImageManager.pullImage` would put registry credentials back inside the component the policy boundary exists to constrain.

**Task 11 — `Create`.** Volumes → network → container, in that order, because whatever succeeded before a failure must be reported in `CreateFailed.created` (`engine.proto:279-286`) or it leaks with nothing knowing to look for it. Offline means no network attachment. Container name **equals the sandbox id** (parent design §4). Ports publish on loopback.

**Task 12 — `Start`, `Stop`, `Remove`.** `Remove` refuses any resource whose stored labels differ from the caller's.

### Landing 5 — Gas Can live tier and capability flips

**Task 13 — live tier.** `crates/gascan-arca/tests/live/` gains create → start → inspect → stop → remove over a real socket, a partial-failure case asserting `CreateFailed.created`, and `Inspect` reporting real ports. Every test `#[ignore]`d with a reason and registered in `tests/ci/expected-ignored-tests.txt`, or `scripts/ci-check-ignored-tests.sh` fails in either direction.

**Task 14 — capability flips.** `project_mount`, `named_volumes`, `loopback_publish`, `resource_limits` become true. `tty` and `signals` stay false (milestone 3); `offline` stays `ISOLATION_UNVERIFIED` (milestone 4). **A flag flips only when a live test drives the capability it names** — a flag set ahead of its test is a claim with no instrument.

**Task 15 — the workspace suite, run alone.**

```bash
pgrep -fl "cargo test"   # must be empty; record the output
cd ~/code/gascan && env -u RUSTUP_TOOLCHAIN cargo test --workspace --no-fail-fast
```

Account for every increment against a per-target table derived from `running N tests` lines. Sum only `test result:` lines reporting `0 filtered out`. A green figure you cannot account for is not a pass.

---

## Out of scope

- **`Exec`, `Logs`, and `ExecManager.signalExec`** — milestone 3.
- **Daemon wiring, `BackendSelection::Arca`, the launchd plist, installer changes, `gascan doctor` surfacing engine facts, and the offline proof** — milestone 4. Task 4 makes the engine's *refusal message* clear; **surfacing it through `doctor` is milestone 4's**, since doctor needs the daemon wiring that milestone owns.
- **Which mechanism ships the 27 MB kernel and 163 MB vminit** — milestone 4, constrained by design §2.6 to the `--kernel-path` / `--vminit-layout` seam.
- **P5.3 conformance, U5, P6's network model**, and the duplicated `sandbox_id`-claim rule.
