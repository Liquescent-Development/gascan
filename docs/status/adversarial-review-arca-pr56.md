# Adversarial review — Arca PR #56 (`feat/sandbox-engine`, head `f5fde96`)

Reviewed against merge-base `e68ac5c`. Working tree `/Users/kiener/code/arca`, plus the
consumer at `/Users/kiener/code/gascan`.

**Counts: 1 Critical, 6 Important, 8 Minor.**

Everything below was reached by reading code outside the diff (`ContainerBridge`,
`swift-nio`, gascan's `crates/gascan-arca`) and by running the engine binary. What I ran is
listed at the end, including the claims I attacked and could not break.

---

## Critical

### C1. `Inspect` and `ListResources` are structurally incapable of reporting anything. Both always answer "nothing here", for the whole life of the process.

`Sources/arca-engine/ArcaEngineCommand.swift:38-58` constructs `ContainerManager`,
`VolumeManager`, `NetworkManager` and never calls `initialize()` on any of them. That is
deliberate and commented. What the comment does not say is how total the consequence is:

- `ContainerManager.containers` is written in exactly two places —
  `Sources/ContainerBridge/ContainerManager.swift:382` (inside `initialize()`, the
  StateStore restore loop) and `:1883` (inside `createContainer`). `initialize()` is never
  called, and `Create`/`CreateContainer` answer `unsupported_capability`
  (`SandboxEngineService.swift:139,166`), so `containers` is **permanently empty**.
- `VolumeManager.volumes` is written at `VolumeManager.swift:495` (inside
  `loadVolumesFromDatabase()`, called only from `initialize()`) and `:228` (`createVolume`).
  Also permanently empty. `listVolumes()` (`:246`) returns `Array(volumes.values)`.
- `NetworkManager.listNetworks()` (`NetworkManager.swift:545-566`) reads `vmnetBackend`,
  `wireGuardBackend` and `networkDrivers`. All three are populated only by
  `NetworkManager.initialize()` (`:46-88`). Both backends are `nil`, so it returns `[]`.

So `SandboxEngineService.listResources` (`:241-260`) always returns an empty
`ResourceList`, and `inspect` (`:90-97`) always takes the `.success(nil)` → `absent` arm.
Neither is an error path; both are confident, well-formed "I hold nothing" answers.

**Proven live.** I copied the user's real `~/.arca/state.db` (1 container row, 2 network
rows — `sqlite3 … "select count(*) from containers"` → 1, `networks` → 2) into a temp state
root, started `.build/release/arca-engine` against it, and drove `ListResources` through
gascan's own `ArcaBackend` over gRPC:

```
SEEDED LIST_RESOURCES = Ok([])
```

**Concrete failure scenario.** Point `arca-engine --state-root` at `~/.arca` (the natural
choice — it is where the kernel, the images and the state live, and where `ArcaDaemon` puts
them). A sandbox container exists and is running. gascan's reconciler calls `Inspect`; the
engine answers `Absent`, which per `engine.proto:353-357` means "it is not there" — the arm
whose whole purpose is to be distinguishable from "I could not tell". The reconciler creates
a second sandbox with the same name. Separately, `ListResources` is what gascan's drift and
leak detection reads; it reports a clean host while the host holds containers, volumes and
networks.

**Why this is Critical rather than Important.** The contract text for `ListResources`
(`engine.proto:387-391`) is "Every resource the engine holds, labelled or not", and the
service's own doc comment (`SandboxEngineService.swift:234-239`) says "Unlabelled resources
are reported, never filtered … hiding it here would defeat that silently". The
implementation cannot report a resource under any input. That is a check that cannot fail
while claiming it can. The PR body's "Three methods are real — `Capabilities`, `Inspect`,
`ListResources` — implemented over `ContainerBridge`" is true of the wiring and false of the
behaviour; the source comment at `ArcaEngineCommand.swift:51-56` discloses only the
container half ("a restarted engine reports zero containers"), understates it as a
restart-only condition, and says nothing about volumes or networks.

**It also makes three of the 27 tests vacuous.** See I6.

**Fix.** Either (a) call `initialize()` on the three managers, gating the VM-requiring parts
behind an option, or (b) if that genuinely cannot be done this milestone, make `Inspect` and
`ListResources` answer `unsupported_capability` like the other eight rather than answer a
falsehood — an engine that says "I cannot tell you" is safe, one that says "nothing exists"
is not. Do not leave (c), the current state. If (b), the PR body and
`docs`/handoff must stop calling these three methods implemented.

---

## Important

### I1. The stale-socket guard cannot distinguish a stale socket from a live one. A second engine silently steals the path from a running first.

`Sources/ArcaEngine/EngineServer.swift:41-47` `lstat`s the path, checks only `S_IFSOCK`, and
unlinks. There is no probe of whether anything is listening. The doc comment (`:11-17`)
frames the only hazard as a mistyped path pointing at a regular file.

**Proven live.** Two engines, same socket path, different state roots:

```
engine1 pid=31892 socket inode=247552053
engine2 pid=32153 socket inode=247552074   <- rebound, no error, no warning
engine1 still alive? 31892 SN
engine2 still alive? 32153 SN
```

Engine 1 remains running, listening on an unlinked inode that nothing can dial again. It
holds its `StateStore` handle and (in later milestones) live VMs, forever, silently. Every
new client reaches engine 2.

**Fix.** Before unlinking, `connect()` to the path; `ECONNREFUSED` means stale, success
means live — refuse to start and say which pid holds it (or take an exclusive `flock` on a
sibling lockfile, which also closes the TOCTOU between `lstat` and `bind`).

### I2. `arca-engine` has no shutdown path at all. Claim 6's "shuts down cleanly" is not implemented.

`ArcaEngineCommand.swift:93-100` creates a `MultiThreadedEventLoopGroup`, starts the server,
and awaits `server.onClose`. Nothing ever calls `server.close()`; there is no `SIGTERM`/
`SIGINT` handler; `group.syncShutdownGracefully()` is never called; the socket file is never
unlinked. The only way this process ends is by being killed, which runs no cleanup.

**Proven live.** `kill -TERM` on a running engine: process gone, and

```
socket still present? srw------- /tmp/adv56/open/engine.sock
```

The leftover socket is then what I1's blind unlink removes — the two defects are load-bearing
for each other, which is presumably why neither was noticed.

**Fix.** Install a signal source for `SIGTERM`/`SIGINT` that calls `server.initiateGracefulShutdown()`
/ `close()`, unlink the socket in a `defer`, and shut the group down.

### I3. `ListResources` omits resources the engine holds, in two distinct ways.

`SandboxEngineService.swift:243` calls `listContainers(all: true)` with no filters.
`ContainerManager.swift:531-533`:

```swift
let showInternal = filters["label"]?.contains(where: { $0.contains("com.arca.internal") }) ?? false
```

and `:556-558` drops every container labelled `com.arca.internal=true`. With no filters
passed, `showInternal` is false, so internal containers are invisible to the engine's
`ListResources`. Separately, `NetworkManager.listNetworks()` at `NetworkManager.swift:552-556`
swallows a WireGuard-backend failure with `try?`, silently dropping every bridge network
rather than reporting an error.

Both contradict `engine.proto:387-391` and the method's own comment. In gascan's terms: a
resource that exists but is not listed is a leak the consumer can never see, and the
WireGuard case turns a real failure into a false "clean".

**Fix.** Pass `filters: ["label": ["com.arca.internal"]]` (or add a bridge API that does not
filter), and change `listNetworks` to propagate the backend error so `engineErrorCatching`
turns it into a `command_io` outcome. This one is latent behind C1 today; fixing C1 without
fixing this yields a silently-incomplete list, which is worse than an empty one.

### I4. `Inspect` hardcodes `sandbox.ports = []` while the data is available.

`SandboxEngineService.swift:117`. `Container.hostConfig.portBindings` and
`networkSettings.ports` carry the real mappings (`Types.swift:78,80`). gascan reads this
field as truth: `crates/gascan-arca/src/translate.rs:436-437` builds `RuntimeSandbox::observed`
with `runtime_ports(&sandbox.ports)`, and an empty list is a valid answer meaning "publishes
nothing", not "unknown". So a sandbox that publishes 8080→80 is reported as publishing
nothing, and any port-drift comparison is a comparison against a fabricated value. This is
not an unimplemented field with an error attached — it is a wrong answer with no marker.

**Fix.** Map `container.hostConfig.portBindings` into `PortMapping`, or, if this milestone
will not, answer `unsupported_capability` from `Inspect` rather than emit a field known to be
false.

### I5. `Inspect` refuses a present-but-foreign sandbox as `invalid_output` instead of letting the consumer judge it.

`SandboxEngineService.swift:100-107`: the digest check runs *before* the owner-label read at
`:114`. A container that exists under a colliding name and was created from a tag (`ubuntu:latest`)
makes `imageDigest(fromReference:)` return nil, and the engine answers
`invalid_output: "container image ubuntu:latest is not an exact digest reference"`.

`invalid_output` is the code gascan reserves for "the engine sent me something I cannot
interpret" (`crates/gascan-arca/src/error.rs:30-33`, and the catch-all at `:55`). So the
consumer cannot distinguish "the engine is broken" from "a container with your sandbox's name
exists and belongs to someone else" — the exact judgment `engine.proto:143-148` says stays
with the consumer. `foreign_resource_refused` and `ownership_mismatch` exist in the vocabulary
for this and are never emitted anywhere in the engine.

**Fix.** Read the owner labels first. If the container carries no gascan labels, this is a
foreign resource and should be reported as such (or reported as a `Sandbox` without owner, which
gascan already rejects deliberately at `translate.rs:411-414`) — not as malformed engine output.
Ordering is the whole fix.

### I6. Three of the 27 tests assert nothing that could fail, and one would pass inverted.

- `Tests/ArcaEngineTests/InspectTests.swift:44-51` (`testAnAbsentSandboxIsAnAnswerRatherThanAnError`)
  and `Tests/ArcaEngineTests/ListResourcesTests.swift:14-22`
  (`testAnEmptyEngineListsNoResourcesRatherThanFailing`) drive a service whose backing state
  is unconditionally empty (C1). They would pass identically if `inspect` were
  `{ .with { $0.absent = .init() } }` and `listResources` were `{ .with { $0.resources = .init() } }`
  — with `ContainerBridge` deleted from both. Neither test can distinguish the implementation
  from that constant. The `absent` arm's *interesting* case — a sandbox that is present —
  has no test at any tier, and cannot have one in this build.
- `Tests/ArcaEngineTests/SandboxEngineServiceTests.swift:31-42`
  (`testEveryUnimplementedResponseSetsItsOutcome`) asserts only `XCTAssertNotNil(outcome)`.
  It passes if every one of those five methods answered `ok` — the precise inversion of what
  the method name and the surrounding prose claim.
- gascan's live tier has the same shape: `crates/gascan-arca/tests/live/read_rpcs.rs:38-62`
  asserts absent-and-empty against a fresh state root, which is the one condition under which
  the answer is right for the wrong reason.

**Fix.** For the third, assert `case .error(let e)` and `e.code == "unsupported_capability"`
for each of the five. For the first two, there is no honest local fix until C1 is resolved;
the tests should be marked as covering only the empty case, and a seeded-state test added
once state is loaded.

---

## Minor

1. **`imageDigest` accepts non-ASCII digits as hex.** `EngineTranslation.swift:32`:
   `hex.allSatisfy({ $0.isNumber || ("a"..."f").contains($0) })`. `Character.isNumber` is true
   for any Unicode numeric character. Compiled and ran the predicate: 64 × U+0663 (ARABIC-INDIC
   DIGIT THREE) → `passes: true`; 64 × U+FF10 (FULLWIDTH DIGIT ZERO) → `true`. The doc comment
   at `:23-25` says "bare and lowercase, exactly 64 characters". Use
   `$0.isHexDigit && $0.isLowercase`, or an ASCII byte check.
2. **Ownership is read from two different fields.** `Inspect` reads
   `container.config.labels` (`SandboxEngineService.swift:114`); `ListResources` reads
   `ContainerSummary.labels` (`:246`), which is `ContainerInfo.labels`. Both write paths
   (`ContainerManager.swift:1834+1863`, `:363+366`) set them from the same source today, so
   they agree — but nothing enforces that, and a divergence would make one RPC report a
   sandbox owned and the other report it unowned. Pick one accessor.
3. **`--log-level` fails silently.** `ArcaEngineCommand.swift:27`:
   `Logger.Level(rawValue: logLevel) ?? .info`. A typo (`--log-level debgu`) yields info-level
   logging with no diagnostic. Validate and exit non-zero.
4. **`EngineServer.start` leaks a listening server on the chmod failure path.**
   `EngineServer.swift:34-37`: if `setAttributes` throws, `server` is already accepting and is
   never closed. Wrap in `do { } catch { try? await server.close().get(); throw error }`.
5. **`createSocketParentDirectory`'s 0700 does not cover intermediates.**
   `ArcaEngineCommand.swift:107-115` passes `withIntermediateDirectories: true` with
   `attributes:`; Foundation applies those attributes to the final component only, so
   intermediates get `0777 & ~umask`. Also, the guard at `:109` returns early for an existing
   directory — I confirmed live that the engine binds happily inside a pre-existing `drwxrwxrwx`
   directory (`srw------- engine.sock` inside `drwxrwxrwx open/`). That is defensible, but the
   security comment at `EngineServer.swift:19-24` states the 0700 parent as "the real control"
   without the "when we created it" qualifier, and in a world-writable parent any local user can
   unlink the socket and bind their own in its place — impersonating the engine to gascan. Either
   verify the existing directory's mode and ownership and refuse otherwise, or weaken the comment.
6. **`containerResourceName` can report a 12-hex short id as a resource name.**
   `EngineTranslation.swift:53-57` falls back to `id`; but for an unnamed container
   `ContainerManager.swift:725` already synthesises `"/" + dockerID.prefix(12)`, so the fallback
   never fires and the reported name is a truncated id that matches no sandbox id. Harmless today,
   confusing later.
7. **`testContainerResourceNameStripsOnlyOneLeadingSlash`** (`ListResourcesTests.swift:78-80`)
   enshrines `"//x"` → `"/x"` as correct. `ContainerManager.swift:725` unconditionally prepends
   `/` to `info.name`, and nothing in `ContainerBridge` strips a leading slash a caller supplied,
   so `//name` is reachable and `/name` fails gascan's identity comparison. The test documents the
   bug rather than the behaviour.
8. **`execManager` and `imageManager` are stored on the service and never read**
   (`SandboxEngineService.swift:27-28`). They widen the constructor and the test fixture for no
   current purpose.

---

## Claims I attacked and could not break

- **No Swift error escapes a provider method (claim 2).** Walked all eleven. The nine unary
  methods have non-`throws` bodies behind `async throws` protocol shims that only `await`; no
  `try`, no force-unwrap, no subscripting, no integer conversion in any of them.
  `engineErrorCatching` (`EngineErrors.swift:57-64`) catches `Error`, which includes
  `CancellationError`. The only `try` in the whole target is
  `try await responseStream.send(...)` in `exec` (`:219`) and `logs` (`:229`) — a write failure,
  which is a transport fault by the contract's own reading. I attacked the most plausible way to
  turn that into a status: `Exec` sends its error frame and returns without draining the request
  stream, so a client mid-send should see a closed stream. I wrote a live test (in a throwaway
  `git worktree`, since I may not touch either working tree) driving gascan's real `ArcaBackend`
  with 64 KiB of initial stdin, 5 runs: every run delivered
  `Err(UnsupportedCapability { capability: "Exec is not implemented by this engine build" })`.
  Logs likewise delivered `unsupported_capability`. gascan's writer task does have a losing
  branch (`crates/gascan-arca/src/backend.rs:246-255` reports `command_io: "the engine closed the
  stream"`), but the reader wins deterministically at this size. I could not produce a status.
- **The twelve-code table (claim 3).** Every `EngineError` in the target is built through
  `engineError(_:resource:message:)` with an `EngineErrorCode` case; there are exactly three call
  sites (`SandboxEngineService.swift:44,60,102`) plus the catch at `EngineErrors.swift:62`. Only
  `unsupported_capability`, `invalid_output` and `command_io` are ever emitted. `injected_failure`
  and `unsupported_version` appear nowhere. Cross-checked the enum against gascan's match arms at
  `crates/gascan-arca/src/error.rs:20-55`: exact set match. The one mis-meaning I found is I5.
- **`Capabilities` claims nothing (claim 4).** Verified by unit test and by the live tier against
  the real binary: all six flags false, `offline == Unverified`, `contract_minor == 0`.
- **No `DockerAPI` / `ArcaDaemon` edge (claim 5).** Verified independently of the PR's own script
  by walking `swift package describe --type json` myself:
  `ArcaEngine → {ArcaIP, ContainerBridge, SandboxEngineProto}`, `arca-engine → {ArcaEngine} ∪ that`.
  No `DockerAPI`, no `ArcaDaemon`, at any depth. `tests/release/engine-targets-check.sh` in gascan
  does root at both targets and does check the roots exist before believing the closures, as its
  comment claims.
- **Socket permissions (claim 6, first half).** `srw-------` confirmed on a live engine.
- **`sun_path` overflow.** swift-nio `SocketAddress.init(unixDomainSocketPath:)` throws
  `unixDomainSocketPathTooLong` (`.build/checkouts/swift-nio/…/SocketAddresses.swift:351-354`)
  rather than trapping. A 110-byte path made the engine print
  `Error: unixDomainSocketPathTooLong` and exit. No crash path here.
- **Sandbox-id → short-hex-ID collision.** `SandboxIdentity.swift:19-20` claims safety because a
  sandbox id always contains a hyphen. Verified: gascan's `validate_sandbox_id`
  (`crates/gascan-core/src/sandbox.rs:168-198`) requires `<slug>-<12 lowercase hex>`, so a hyphen
  is structurally guaranteed, and `ContainerManager.resolveContainerID`'s hex-prefix branch
  (`:1931-1948`, which returns an arbitrary sorted-first match on ambiguity) is unreachable for a
  valid sandbox id. The claim holds.
- **Concurrency.** `ArcaEngine` has no mutable state anywhere: `SandboxEngineService` holds only
  `let`s, all five managers are `actor`s, `EngineServer` and `SandboxIdentity` are stateless. The
  package is `swift-tools-version: 6.2` with no `swiftLanguageMode` override, so Swift 6 strict
  concurrency is checked by the compiler and the build is clean. I found no `nonisolated` escape
  and no one-call-at-a-time assumption.
- **27 tests (claim 7).** Ran `swift test --filter ArcaEngineTests` against the warm `.build`:
  `Executed 27 tests, with 0 failures`. Also ran gascan's live tier against
  `.build/release/arca-engine`: 6 passed, 0 failed. They pass — see I6 for what three of them
  are worth.

## What I ran

- `swift test --filter ArcaEngineTests` (warm `.build`) → 27 passed.
- `swift package describe --type json`, closures computed by script.
- `.build/release/arca-engine` directly: over-long path, world-writable parent, double-bind,
  `SIGTERM`.
- `cargo test -p gascan-arca --test live -- --ignored` with
  `GASCAN_ARCA_ENGINE_BIN=/Users/kiener/code/arca/.build/release/arca-engine` → 6 passed.
- Two additional live tests of my own (exec-with-stdin, and a seeded state root), written in a
  temporary `git worktree` of gascan with `CARGO_TARGET_DIR` pointed at the existing target dir.
  The worktree has been removed; neither repository's working tree, index, HEAD or branch was
  touched.
- A standalone `swiftc` program to test the `isNumber` predicate.
