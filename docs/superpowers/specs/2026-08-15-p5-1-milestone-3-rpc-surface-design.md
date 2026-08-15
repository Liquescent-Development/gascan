# P5.1 milestone 3 — finishing the RPC surface

**Status:** approved 2026-08-15. Supersedes the milestone-2 design's §9 line assigning only
`Exec` and `Logs` to this milestone.

**Parent design:** `docs/superpowers/specs/2026-08-10-p5-1-engine-service-and-wiring-design.md`
**Preceding milestone:** `docs/superpowers/specs/2026-08-12-p5-1-milestone-2-engine-lifecycle-design.md`
**Governing roadmap:** `docs/superpowers/plans/2026-08-04-arca-integration-roadmap.md`

Every line anchor in this document was re-derived on 2026-08-15 against Gas Can `e9468d8` and
Arca `b3ffdf5` with submodule `3f68806`. Anchors in this project drift under every task; re-derive
before editing, and re-derive again after your own edits if you cite them.

---

## 1. What this milestone is

Six pieces in two groups.

**The RPC surface**, which is what the milestone is named for:

| | |
|---|---|
| `CreateContainer` | `SandboxEngineService.swift:674`, refusing at `:677` |
| `Exec` | `SandboxEngineService.swift:1023`, refusing at `:1029` |
| `Logs` | `SandboxEngineService.swift:1033`, refusing at `:1039` |
| `ExecManager.signalExec(execID:signal:)` | does not exist; `ExecClientFrame.signal` has nothing to call |

All three RPCs refuse through `notImplemented(_:)` (`SandboxEngineService.swift:69-74`), which
returns `.unsupportedCapability`.

**Two follow-ups carried out of milestone 2's reviews**, each closing a gap that milestone's own
record states:

- **(a)** Move the shutdown wait out of the `arca-engine` executable into `ArcaEngine` as
  `runUntilQuiesced`, so milestone 2's task 17 gets a fails-before/passes-after test. Reverting
  that fix currently leaves `swift test` green, and Gas Can's live tier is the only thing that
  catches it.
- **(b)** A test that `unpackLayerToCache` actually calls `cachedLayerIsReusable`. Milestone 2's
  re-review pinned the decision at the call site but recorded that the *call* from
  `unpackLayerToCache` is still unmeasured. It needs an `Image` fixture.

**When this milestone lands, no RPC answers `unsupported_capability`.** That is what makes P5's
exit criterion — "`gascan-arca` passes conformance and existing `gascan-e2e`"
(`2026-08-04-arca-integration-roadmap.md:379`) — reachable at all.

### 1.1 Why `CreateContainer` is here, and it had no milestone until now

`CreateContainer` was listed beside `Exec` and `Logs` as a refusing RPC while only those two were
assigned to a milestone. It is settled here on three grounds, all verified 2026-08-15:

1. **`gascand` calls it on two production paths** — `crates/gascand/src/service.rs:1699`
   (`rollback_image`) and `:1778` (`replace_image`). An earlier record named a third site at
   `:4314`; that is inside `#[cfg(test)] mod storage_tests`, which opens at `:4252`, in a
   `MutableCapabilitiesRuntime` test double delegating to `FakeRuntime`. It is not a call path.
2. **It reuses machinery `Create` already has**, so it is the cheapest of the three (§3).
3. **It flips no capability flag.** The six flags are `project_mount`, `named_volumes`, `tty`,
   `signals`, `loopback_publish`, `resource_limits` (`engine.proto:117-122`); none covers it.

It goes **first**, so the milestone's cheapest piece lands before its hardest.

---

## 2. Decisions

### 2.1 This milestone is Arca-side; Gas Can contributes live tests only

**Gas Can's half is already built and tested, and this was verified rather than assumed.**
`RuntimeBackend` declares `exec` (`crates/gascan-core/src/runtime.rs:1059`) and `logs` (`:1060`).
`EngineTransport` declares both (`crates/gascan-arca/src/transport.rs:120`, `:122`) over
`ExecStream` and `LogsStream`. `ArcaBackend::exec` is implemented at
`crates/gascan-arca/src/backend.rs:227`, including the initial-stdin pump and the cancellation
channel. `crates/gascan-arca/tests/backend_streams.rs:16` and `:186` drive `logs` and `exec`
against a fake transport. `grep -rn "todo!\|unimplemented!" crates/gascan-arca/src/` returns
nothing.

**Consequence:** no Gas Can PR is on this milestone's critical path, and the consumer already
enforces part of the engine's contract. `crates/gascan-arca/tests/backend_unary.rs:740`
(`a_recreate_answered_with_the_whole_topology_is_refused`) makes `CreateOutcome::for_recreate`
reject a `CreateContainer` answer that reports volumes and a network, so an engine that rebuilds
retained resources is caught by a test that exists today.

### 2.2 `CreateContainer` verifies its retained resources rather than trusting the caller

`engine.proto:296-302` defines the request as a `CreateRequest` plus `repeated Resource retained`,
with the engine creating the container only and everything in `retained` already existing.

**AMENDED 2026-08-15, after Task 1's review measured that the original wording did not achieve its
own stated purpose.** The paragraph below said "confirm each retained resource is present and
owned". A reviewer showed that verifies the wrong list: the container mounts
`request.create.volumes` and attaches `request.create.network`, while `retained` is a separate
field, so a request with `retained: []` and populated `create.volumes` passed the guard untouched
and built the container — **the exact silent failure this section exists to prevent.** Task 1's
first implementation shipped that, and the test written to prove the guard asserted the bypass as
intended behaviour.

**The engine verifies the topology the container will actually mount.** It iterates
`request.create.volumes` plus the managed name from `request.create.network`, and requires each to
be **both held by the engine and named in `retained`**, refusing with `not_found` naming the
missing one, or `ownership_mismatch` / `foreign_resource_refused` when it is held by someone else.

This is not a contract change: the wire format is untouched, and `engine.proto` does not forbid an
engine refusing more than the minimum. It makes `retained` an assertion the caller must match rather
than the sole source of truth.

**Gas Can already enforces the same correspondence client-side** — `validate_retained_resources`
(`crates/gascan-core/src/runtime.rs:893-918`) requires every retained resource to be an expected
volume or the expected network, owned by Gas Can, matching the sandbox id, non-duplicated, and
**exactly count-equal** to the requested topology. So this guard is defence in depth against a
non-Gas-Can or buggy client, not a fix for a reachable Gas Can bug. It is still required: an engine
guard that is bypassed by under-populating a list is not a guard.

**The reason is that the alternative failure is silent.** A container attached to a volume the
engine no longer holds starts anyway and the mount is simply absent — which is the exact shape of
the `named_volumes` defect milestone 2 spent two sessions diagnosing, where three volumes were
attached, mounted somewhere unreachable, and nothing refused. Verification is a store read, and
`not_found` (`crates/gascan-arca/src/error.rs:50`) is already in the twelve codes that file accepts
at `:21-52`.

### 2.3 The log writer widens to fractional seconds; the contract does not narrow

`LogsRequest.since_unix_millis` is millisecond-resolution (`engine.proto:463-467`, the field at
`:466`).
`FileLogWriter.createLogEntry` (`Sources/ContainerBridge/LogWriter.swift:72-84`) stamps each entry
with `ISO8601DateFormatter().string(from: Date())`, and that formatter at its default options
emits no fractional seconds — so the log's resolution is one second.

**`ISO8601DateFormatter` gains `.withFractionalSeconds`.** The rejected alternative was to document
the filter as second-resolution, which leaves a `since` that silently returns up to a second of log
the caller asked to exclude. A filter that quietly over-returns is worse than one that costs a
one-line change to make honest.

`ContainerBridge` is shared with Arca's Docker surface, so this changes the timestamp format Docker
log consumers see. Fractional seconds are valid ISO 8601 and Docker's own log format carries them.

### 2.4 `Logs` reads the combined log

`ContainerLogManager` keeps three paths per container — `stdoutPath`, `stderrPath`, `combinedPath`
(`Sources/ContainerBridge/LogWriter.swift:113-117`). `Logs` reads `combinedPath`, because ordering
across the two streams is what a log consumer needs and the per-entry `"stream"` field
(`:84`) preserves which stream each line came from.

### 2.5 The JSON log format becomes load-bearing for the first time

`createLogEntry` builds its entry by string interpolation with hand-rolled escaping
(`Sources/ContainerBridge/LogWriter.swift:84`). **Nothing has ever parsed those lines back.**
`Logs` will.

This is the shape milestone 2 recorded as a trap: making a wait real made every timeout around it
load-bearing, and a second defect surfaced underneath. A guest message containing a quote, a
backslash, a newline or a control character has been harmless while the file was write-only and
becomes a parse failure the moment this ships.

**Requirement:** a round-trip test over adversarial payloads — embedded `"`, `\`, newline, tab, a
lone `{`, and non-UTF-8 bytes — writing through `FileLogWriter` and reading through the `Logs`
reader. If the escaping cannot carry a payload, the writer is fixed; the reader does not paper
over it.

### 2.6 `Exec`'s logic lives in adapters, because that is what a VM-free test can reach

`Exec` end to end needs a booted container and cannot be reached from Arca's suite —
`startExec` requires a native container instance (`Sources/ContainerBridge/ExecManager.swift:139`).
But `startExec` takes `stdin: ReaderStream?`, `stdout: Writer?`, `stderr: Writer?` (`:117-125`),
so the engine must supply:

- a `Writer` that emits `ExecServerFrame.stdout` / `.stderr`, and
- a `ReaderStream` fed by the client's `stdin` frames.

**Those adapters are pure value transformations with no VM in them, and Arca's own suite drives
them.** This is milestone 2's most-repeated lesson applied at design time rather than at review:
put the logic where a test can get at it, and say plainly which repository can prove what.

### 2.7 Two `Exec` hazards are design requirements, not implementation details

**Serialize the response stream.** stdout and stderr are two independent `Writer`s feeding one
`GRPCAsyncResponseStreamWriter`. Concurrent sends must go through a single actor. An interleaved
or corrupted frame reads as a flake, which is the most expensive class of defect this project has.

**Refuse unknown signal numbers.** `ExecClientFrame.signal` is a raw `int32` from the wire
(`engine.proto:437`, documented at `:436`); `LinuxProcess.kill` takes a typed `Signal`
(`containerization/Sources/Containerization/LinuxProcess.swift:315`). An unmapped number is
`invalid_state` naming the number, never a coercion to a default.

---

## 3. Per-RPC behaviour

### 3.1 `CreateContainer`

`create(request:)` (`SandboxEngineService.swift:324-418`) runs three phases: the volume loop
(`:335-349`), the network branch (`:351-391`), then `containerManager.createContainer(...)`
(`:393-409`). **`CreateContainer` is the third phase alone**, preceded by §2.2's retained check.

The container phase is extracted into one method both RPCs call. **DRY here is not tidiness.**
`createSpec`'s doc comment (`:423-440`) records that a review mutation replaced the single line
deciding the image reference with `references.first ?? …` and the whole suite stayed green, which
would have made every sandbox record a tag and every `Inspect` answer `invalid_output`. Two
independent container-build paths would let exactly that drift back in on one of them.

Reused unchanged: `createSpec` (`:446`, `package func`, already VM-free and already tested),
`createFailed` (`:461`), `createCatching` (`:481`) with its `resource_conflict` /
`command_failed` distinction.

The response is `CreateResponse`. On success `Created.created` carries **the container alone** —
§2.1's consumer test already refuses anything more.

### 3.2 `Exec`

`ExecSession.swift` holds the state machine, per parent design §3.1.

1. The first client frame must be `ExecStart`; exactly one may appear per stream
   (`engine.proto:412-421`, the rule stated at `:408-411`). Any other first frame is a protocol
   error.
2. Resolve `sandbox_id` to a container through the §4 naming rule, refusing an unlabelled or
   foreign container the way `Inspect` does.
3. `createExec` (`ExecManager.swift:47`) with argv, environment and `tty`.
4. `startExec` (`:117`) with the §2.6 adapters.
5. Client frames thereafter: `stdin` → the process, `resize` → `resizeExec` (`:255`),
   `signal` → `signalExec` (§3.4), `close` → close stdin.
6. On process exit, send `Exit{code, signal}` and end the stream cleanly.
7. **A mid-exec client reset is cancellation** (parent design §2.4): kill the guest process, reap
   the exec instance, emit nothing.

**With `tty` set, there is no stderr stream.** `startExec` sets `processConfig.terminal` from the
request at `ExecManager.swift:153` and then sets stderr only when that is false (`:173`, `:175`),
because a TTY merges stderr into stdout. The engine must
not expect stderr frames in that mode — and that merge is what §5.2 uses as proof a TTY was really
allocated.

### 3.3 `Logs`

Server-streaming. Read `combinedPath` (§2.4), filter by `since_unix_millis` against each entry's
`time` field, emit ordered `LogsChunk.data` frames, end the stream cleanly. An absent `since`
means from the beginning (`engine.proto:465-466`).

**There is no follow mode and none is to be added.** `engine.proto:474-476` states the reason: a
follow mode is the first step back toward a general container API, and this contract has a size
budget for exactly that reason.

Chunking is by size, not by log entry — the message is "one logical buffer, chunked; the consumer
concatenates data frames in order" (`engine.proto:470`). A consumer must not need to know where
the engine split.

### 3.4 `signalExec`

One new method on `ExecManager`, over an API that already exists: it holds
`ExecInfo.process: LinuxProcess?`, and `LinuxProcess.kill(_ signal: Signal)` is at
`containerization/Sources/Containerization/LinuxProcess.swift:315` (verified 2026-08-15 at
submodule `3f68806`).

**It is called out separately because `ContainerBridge` is shared with Arca's Docker surface**, so
this is a change with a second consumer. Per milestone 2's standing rule for every `ContainerBridge`
change, it takes no defaulted parameter.

---

## 4. Identity and ownership

Unchanged from milestone 2. `Exec` and `Logs` both take a `sandbox_id` and both must refuse a
container that is unlabelled or owned by someone else, using the same rule and the same error codes
`Inspect` uses. Neither may reach `ExecManager` or the log directory before that refusal.

The refusal must run **before** any resolution that could name the wrong container. Milestone 2
recorded this exact ordering trap for `start`: `startContainer` resolves the name and only then
guards, so a hex-prefix match happens first and by the time anything refuses, the container is
already the wrong one (`SandboxEngineService.swift:704-708`).

---

## 5. Testing

### 5.1 Arca

VM-free, in `ArcaEngineTests`:

- The `Exec` adapters of §2.6 — frame emission, stdin feeding, and the serialization of §2.7.
- The `ExecStart`-must-be-first state machine, including every wrong first frame.
- Unknown signal numbers refused as `invalid_state`.
- The §2.5 log round-trip over adversarial payloads.
- `since_unix_millis` filtering, including the fractional-second boundary §2.3 creates.
- `CreateContainer`'s retained check (§2.2) and its container-alone response (§3.1).
- **(a)** `runUntilQuiesced` moved into `ArcaEngine`, with the fails-before/passes-after test
  milestone 2's task 17 lacks.
- **(b)** the `unpackLayerToCache` → `cachedLayerIsReusable` call, with its `Image` fixture.

### 5.2 Gas Can live tier

Everything needing a booted container. **The fixtures are one call each to affordances milestone 2
already built**: `layout_running(base, destination, tag, command)`
(`crates/gascan-arca/tests/live/common/mod.rs:710`) writes a one-image OCI layout running any
command, `report_section` (`:512`) parses a guest's text answer, and `read_from_loopback` (`:530`)
reads a published port.

- **`Logs`** — an image whose `Cmd` prints known text and exits; the test asserts the text comes
  back, in order, and that `since_unix_millis` excludes what it should.
- **`Exec`** — an image that stays alive; the test execs into it and asserts stdout, stderr and the
  exit code arrive separately.
- **`tty`** — the same, with `tty` set, asserting stderr arrives **merged into stdout**. Per §3.2
  that merge happens only when the process really is a terminal, so it is proof rather than a
  restatement.
- **`signals`** — signal a live guest process and read the number back in `Exit.signal`.
- **`CreateContainer`** — recreate a sandbox's container against retained volumes and network, and
  assert the volumes still hold data written before the recreate. Data survival is what
  distinguishes reuse from a rebuild that happened to succeed.
- `crates/gascan-arca/tests/live/read_rpcs.rs:125` asserts `CreateContainer` errors; its count of
  unimplemented methods drops to zero and the test is retired or inverted.

**These stay `#[ignore]`d.** `scripts/build-arca-engine.sh` builds the pin, and the pin bump is
milestone 4's with its signed tag, so CI cannot run them this milestone. "Run the tier at least
once" means a local run against a branch build, recorded with its command and output.

### 5.3 The capability flags are the exit gate

`tty` and `signals` are the two flags still `false` (`engine.proto:119-120`), correctly, because
nothing drives them. **Neither flips until its §5.2 test passes**, under the rule milestone 2 states
and has now applied four times: a flag whose machinery is unproved is a claim with no instrument.

`CreateContainer` flips no flag (§1.1), so its only instrument is its live test.

### 5.4 Every guard must be proved capable of failing

Standing requirement, carried from milestone 2 §7.3 and from the rule that milestone's Task 11 cost
four fix rounds to establish: before a claim goes into a commit message, a source comment or a
report, ask what mutation would falsify it and whether a test already fails under that mutation. If
none does, write the test or write the weaker claim.

Mutate the **production default**, not the seam, and the **call site**, not only the function — both
of which this project has shipped past before, most recently in milestone 2's re-review where
`LayerCacheRoleTests` pinned the predicate while `if true || cachedRole == .overlayLayer` at the
call site left `swift test` at 155 passing.

---

## 6. Sequencing

| | Piece | Why here |
|---|---|---|
| 1 | `CreateContainer` | Cheapest; reuses `Create`'s reviewed machinery; unblocks P5's exit criterion |
| 2 | **(a)** `runUntilQuiesced` | Independent of everything else; closes a stated gap |
| 3 | **(b)** the `unpackLayerToCache` call test | Same |
| 4 | `signalExec` | `Exec` cannot handle a `signal` frame without it |
| 5 | `Logs` | Simpler stream than `Exec`; forces §2.3 and §2.5 before `Exec` depends on neither |
| 6 | `Exec`, then the `tty` and `signals` flips | Hardest; needs 4; its flags need 6's live tests |

2 and 3 are independent of the rest and of each other. **Do not split them across concurrent agents
by file** — milestone 2's record is explicit that the split must be on the resource a measurement
depends on, and every piece here shares one Arca working tree.

---

## 7. Out of scope

- **(c) the host telling the guest how many layers it attached** — milestone 4. It needs a
  `containerization` submodule change, a `make vminit-rebuild`, and a guest-side measurement;
  milestone 4's pin bump already forces a submodule decision. Until then the guest's `exit(1)` on a
  writable device with no layers can refuse a legitimate `FROM scratch` boot, which milestone 2's
  re-review recorded and deliberately left.
- **Daemon wiring, `BackendSelection::Arca`, the launchd plist, packaging, `gascan doctor`, the
  offline proof, and the pin bump** — milestone 4.
- **The two contract defects** — the proto permitting offline-plus-ports with no stated winner, and
  `AckResponse` being unable to express a partial `Remove`. Both are contract changes and belong to
  milestone 4's design pass.
- **P5.3 conformance, U5, P6's network model**, and the duplicated `sandbox_id`-claim rule between
  `gascan-arca/src/translate.rs` and `gascan-apple/src/inspect.rs`.
- **D7's narrowed retry** — its own PR by maintainer ruling, not folded into unrelated work.
- **Arca's own CI.** Still none; Gas Can's `engine` job remains the only automated thing that
  exercises Arca.

---

## 8. Documents this work must correct

- `docs/status/START-HERE.md` — the refusing-RPC anchors `:636`, `:988`, `:998` are stale
  (§1 carries the current ones); the "`gascand` calls it in three places" count is two (§1.1); and
  `CreateContainer` now has a milestone.
- The milestone-2 design's §9, whose out-of-scope line assigns only `Exec` and `Logs` to
  milestone 3.
- `docs/superpowers/plans/2026-08-04-arca-integration-roadmap.md`, if its P5.1 milestone outline
  names milestone 3's contents.
