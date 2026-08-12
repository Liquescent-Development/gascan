# P5.1 — Engine service and daemon wiring

Date: 2026-08-10
Status: Design, approved in conversation; not yet planned or implemented
Scope: Arca's `SandboxEngine` service and executable, and Gas Can's consumption of it.

Companion documents:

- Contract: `docs/superpowers/specs/2026-08-04-sandbox-engine-contract.md`
- Proto design: `docs/superpowers/specs/2026-08-07-arca-engine-proto-design.md`
- Backend design: `docs/superpowers/specs/2026-08-08-gascan-arca-backend-design.md`
- Roadmap: `docs/superpowers/plans/2026-08-04-arca-integration-roadmap.md`

---

## 1. What P5.1 is

Two documents name P5.1 differently, and the difference is a whole deliverable.

- `docs/superpowers/plans/2026-08-04-arca-integration-roadmap.md:374` — "Implement the
  engine service in Swift against existing ContainerBridge machinery."
- `docs/status/arca-integration-handoff.md:885` — "P5.1 is 'implement the engine
  **service**'".
- `docs/status/START-HERE.md:64` — "P5.1 — wire the backend to the daemon."

**P5.1 is both, and this document supersedes the narrower readings.** The Swift engine
service does not exist: at Arca `e68ac5c`, `Sources/SandboxEngineProto/` holds exactly two
files, `Generated/arca/engine/v1/engine.grpc.swift` and `engine.pb.swift` (VERIFIED, `find
Sources/SandboxEngineProto -type f`), and `Package.swift:84-87` states the position
deliberately: "Nothing depends on this target yet. That is deliberate — P3's exit is
'proto exists, both sides generate, nothing implements it yet'." Arca's only products are
the `Arca` and `ArcaTestHelper` executables.

So the wiring half cannot answer any of the claims `START-HERE:83-95` records as
unverified, because there is nothing on the other end of the socket to dial. Producing the
engine is a precondition for the wiring being worth anything.

**Exit:** `gascand`, running on `ArcaBackend<ChannelTransport>` against a real engine,
creates an offline sandbox, execs into it, reads its logs, and removes it; and every claim
in §9 has an answer recorded against a command that produced it.

## 2. Decisions

Each was taken deliberately; the rationale is recorded because the reasoning is what a
later change needs, not the conclusion.

### 2.1 `Capabilities.offline` reports `ISOLATION_PROVEN`, earned by observation

Gas Can's default network mode is `Offline` (`crates/gascan-core/src/manifest.rs:189-193`),
and
`crates/gascan-core/src/policy.rs:417-427` rejects an offline manifest with
`PolicyError::OfflineUnavailable` unless the backend reports `NetworkIsolation::Proven`. An
engine answering anything else cannot create a default sandbox at all, which would make the
wiring undemonstrable.

`PROVEN` is earned the way `gascan-apple` earns it: by live observation, recorded as
evidence, pinned to an exact revision — `crates/gascan-apple/src/probe.rs:224-228` reports
`Proven` only for the certified release, and the proof itself is an out-of-band signed-off
matrix referenced through `gate2_evidence` (`crates/gascand/src/main.rs:787-795`). Offline
means **no network attachment at all** — no vmnet, no WireGuard — and the proof observes
that from inside a running sandbox (§8.4).

Not chosen: reporting `ISOLATION_UNVERIFIED` until P6.2 can dump a live guest ruleset. It
is closer to the proto's literal wording (`proto/arca/engine/v1/engine.proto:95-97` in
Arca) but leaves the default path uncreatable. Also not chosen: implementing guest-enforced
packet filtering now, which pulls an undesigned phase into this one.

### 2.2 Image ingress is an engine subcommand, off the gRPC surface

`PrepareImage` materialises content the engine already holds and fails when it is absent;
it is never a fetch (`engine.proto:308-313`, contract §4 and §11). Nothing else in the
eleven RPCs puts content in.

The engine executable therefore grows a non-RPC subcommand — `arca-engine image load
--oci-layout <dir>` — over `ImageManager.loadFromOCILayout(directory:)`
(`Sources/ContainerBridge/ImageManager.swift:46`). The published surface stays at eleven
RPCs, so "the protocol is the policy" holds literally: a compromised guest has no frame it
can send that touches images.

Not chosen: a twelfth `LoadImage` RPC. It is defensible — streaming bytes the consumer
already holds still cannot name a registry — but it grows the contract at the seam the
proto explicitly warns about. Also not chosen: letting `PrepareImage` fall back to
`ImageManager.pullImage`, which would put registry credentials and Keychain access back
inside the component the policy boundary exists to constrain.

**U5 is not resolved by this.** How a shipped `.pkg` gets the workspace image into a user's
engine remains P5.4's question (`roadmap:499-506`). This decision only gives P5.1 a way to
get an image in far enough to run a container.

### 2.3 The engine is a launchd job; `gascand` dials it

`gascand` connects to a configured socket. It does not spawn or supervise the engine.

Supervision was considered and rejected on a structural defect: after `gascand` is
`SIGKILL`ed its engine child survives holding the socket, so the next `gascand` spawns a
second engine that cannot bind. Recovery therefore *requires* dialing an existing engine —
supervision does not replace the dialing case, it adds a second one plus a fallback branch
to choose between them.

launchd reaps a reparented process but never terminates one, and it has no `BindsTo=`
equivalent, so it cannot couple two jobs' lifetimes. Making the engine its own job sidesteps
both: one job, one socket, launchd owning the process throughout.

A surviving engine is a feature rather than a leak. `run_daemon` calls `service.reconcile()`
before serving (`crates/gascand/src/main.rs:483`), and `ReconcileFinding`
(`crates/gascand/src/reconcile.rs:5-24`) already distinguishes `UnknownOwned`,
`MissingOwned`, `OwnershipMismatch` and `UnknownUnowned` — so a restarted daemon adopts
running sandboxes by their owner labels. An agent's long-running sandbox surviving a
control-plane restart is the better failure mode for this product.

Not chosen: launchd socket activation. Its real benefit is letting the engine exit when
idle, and the engine must not exit while sandboxes are running, so v1 could barely use it.
What remains is on-demand start instead of start-at-login, which does not justify putting an
unverified mechanism — whether grpc-swift 1.23 can serve on a launchd-provided listener
descriptor — on the critical path. Recorded as a possible later refinement.

### 2.4 A mid-exec client reset means cancellation, not error

The engine kills the guest process and reaps the exec instance. It emits no `EngineError`,
and it leaves nothing running.

Dropping `ExecSession` is ordinary client behaviour in `gascan-core`, and `FakeRuntime`
models it as cancellation; treating it as a fault would make normal teardown
indistinguishable from a real failure. This is a behavioural commitment, so it belongs in
the contract document and not only in code — a later engine change could otherwise break it
silently.

`START-HERE:83-89` recorded this as something P5.1 would *discover*. Because Gas Can writes
the server, it is a decision instead.

### 2.5 One install, and Gas Can owns the whole control surface

The `.pkg` installs `gascan`, `gascand`, the engine binary and the launchd plist, and loads
the agent. `packaging/macos/install.sh:33` currently aborts unless the user has separately
installed Apple `container` 1.1.0; bundling the engine removes that requirement rather than
adding a second one.

The engine binary keeps the name `arca-engine`. Hiding the name is not a goal; **separate
management is what must not exist.** So:

- `gascan doctor` reports engine facts as "the sandbox engine", the way it reports Apple
  runtime facts today.
- `gascan daemon status` includes engine health.
- `gascan daemon restart --engine` restarts the engine and states that running sandboxes
  die with it. Plain `gascan daemon restart` restarts `gascand` only — sandboxes surviving
  it is the adoption property §2.3 rests on. The two are separate flags rather than one
  command because their blast radius differs by an order of magnitude.
- `packaging/macos/uninstall.sh` unloads the agent and removes the plist.

`launchctl` appears in no documentation, no error message, and no recovery instruction.

## 3. Architecture

### 3.1 Arca

Two new targets in `Package.swift`.

`ArcaEngine` (library) conforms to `Arca_Engine_V1_SandboxEngineAsyncProvider`
(`Sources/SandboxEngineProto/Generated/arca/engine/v1/engine.grpc.swift:168`) and depends on
`SandboxEngineProto` and `ContainerBridge` — **and not on `DockerAPI` or `ArcaDaemon`**.

That absent edge is the load-bearing part. It makes "build only what we need" checkable by
`swift package describe --type json` rather than aspirational, and today the only path to a
shippable Arca binary runs `Arca → ArcaDaemon → DockerAPI` (`roadmap:341-343`). Arca keeps
its Docker surface for its own purposes; the engine does not link it.

`arca-engine` (executable) is the shell: socket path argument, the `image load` subcommand,
server bootstrap, signal handling.

`ArcaEngine` is split by concern rather than into one service file:

| File | Responsibility |
|---|---|
| `SandboxEngineService.swift` | The eleven methods. Each: translate in, call ContainerBridge, translate out. No business logic. |
| `EngineTranslation.swift` | Wire ⇄ ContainerBridge types — labels, resources, image digests, ports, mounts, limits, user, init. |
| `SandboxIdentity.swift` | The naming rule of §4, in one place, because Gas Can validates it exactly. |
| `ExecSession.swift` | The bidi state machine: first frame `Start`, then stdin/resize/signal/close; reset means cancellation. |
| `EngineErrors.swift` | The stable code table of §6. |

One change to `ContainerBridge` is required: `ExecManager` exposes `resizeExec` and
`deleteExec` but no signal path, so `ExecClientFrame.signal` has nothing to call. It holds
`ExecInfo.process: LinuxProcess?`, and `LinuxProcess.kill(_ signal: Signal)` exists
(`containerization/Sources/Containerization/LinuxProcess.swift:315`), so this is one new
`signalExec(execID:signal:)` method over an existing API. It is called out because
`ContainerBridge` is shared with the Docker surface.

### 3.2 Gas Can

- `crates/gascand` gains `BackendSelection::Arca`, engine socket configuration, and
  construction of `ArcaBackend<ChannelTransport>`; `crates/gascand/Cargo.toml` gains
  `gascan-arca`. No such dependency edge exists today (VERIFIED, `grep -rn "RuntimeBackend"
  crates/` reports no `gascand` reference to `gascan_arca`).
- Selection is by explicit environment variable — `GASCAN_ARCA_BACKEND` to select it,
  `GASCAN_ENGINE_SOCKET` for the socket path, following the existing
  `TEST_FAKE_BACKEND_ENV` shape (`crates/gascand/src/lib.rs:5`). **`Apple` remains the
  default**, because
  contract §8.3-4 requires `gascan-arca` to pass conformance and existing `gascan-e2e`
  coverage before it is eligible to become default, and conformance is P5.3.
- `scripts/build-arca-engine.sh` builds the engine **product** rather than only targets —
  its own comment at `:101-102` states that this is the line that changes when P5.1 lands an
  executable — and additionally reports the built binary's path, since today it prints only
  the checkout path (`:112`).
- New `crates/gascan-arca/tests/live/`, mirroring `crates/gascan-apple/tests/live/`.
- New launchd plist and installer changes under `packaging/macos/`.
- `engine/arca-pin.json` moves to the new signed tag.

## 4. Identity and naming

Derived from Gas Can's validators, not invented. An engine that names things differently
fails validation client-side, so this is a hard interface.

- The container resource's name **equals the sandbox id**.
  `crates/gascan-core/src/runtime.rs:829-832` builds the expected container identity as
  `request.id.to_string()`, and `validate_created_resources` enforces it.
- Volumes carry exactly the requested names; the network carries
  `request.network().managed_name()` (`crates/gascan-core/src/runtime.rs:883-891`).
- Owner labels are echoed with `managed_by == "gascan"`
  (`crates/gascan-core/src/runtime.rs:55`) and `sandbox_id` equal to the request's id,
  stored verbatim and never interpreted (`engine.proto:144-148`).

**RESOLVED 2026-08-10 — no constraint.** A `SandboxId` is `<slug>-<12 lowercase hex>`,
the slug being lowercase ASCII alphanumerics with single interior hyphens, no leading or
trailing hyphen and no `--` (`crates/gascan-core/src/sandbox.rs:168-198`). ContainerBridge
applies no container-name grammar validation: `createContainer(name: String?)` takes the
string as given (`Sources/ContainerBridge/ContainerManager.swift:1514-1516`), and the only
two `CharacterSet` uses in that file are unrelated — a CPU-set parser at `:1615` and hex
short-ID prefix matching at `:1933` (VERIFIED, `grep -n "CharacterSet\|NSRegular\|regex"`
reports exactly those two).

The hyphen is load-bearing and must stay. `resolveContainerID` treats any 4-63 character
all-hex string as a short container ID (`ContainerManager.swift:1930-1934`); `-` is not in
that character set, so a sandbox id cannot be misread as one. A future id format without a
hyphen would collide silently.

## 5. Per-RPC behaviour

**Create** runs volumes → network → container. The ordering is what makes partial-failure
evidence honest: whatever succeeded before a failure is reported in `CreateFailed.created`,
because losing it leaks resources nothing later knows to look for
(`engine.proto:279-286`). Offline means no network attachment. Ports publish on loopback;
no bind address is on the wire, and `gascan-arca` restores loopback client-side
(`crates/gascan-arca/src/translate.rs:350-352`).

**Inspect** returns the sandbox, `Absent`, or an error — three arms, because "it is not
there" and "I could not tell" demand opposite behaviour from a reconciler
(`engine.proto:354-357`). The image digest must round-trip deterministically:
`crates/gascan-arca/src/translate.rs:333-336` reassembles the canonical reference and
asserts it is one, "which is what lets the daemon compare one observation against another by
exact string". Reporting a tag, or a digest in a different form than it was given, breaks
reconciliation. Container status maps to `CREATING`/`RUNNING`/`STOPPED`.

**ListResources** returns containers, volumes and networks **including unlabelled ones**.
Filtering them engine-side would break Gas Can's drift detection silently
(`engine.proto:389-391`).

**Remove** takes exact identities and refuses any resource whose stored labels differ from
the caller's (`engine.proto:380-384`).

**PrepareImage** looks the digest up, materialises the rootfs, and fails when absent. It
never pulls.

**Exec.** The first frame must be `ExecStart`; any other first frame is a protocol error.
Then `stdin` to the process, `resize` to `ExecManager.resizeExec`, `close` closes stdin, and
`signal` reaches the guest process via the new `signalExec`. Server side, stdout and stderr
stream until `Exit`. A mid-exec client reset is cancellation (§2.4): kill the guest process,
reap the exec instance, emit nothing.

**Logs** streams ordered `data` chunks filtered by `since_unix_millis`, then ends the stream
cleanly. There is no follow mode and none is to be added (`engine.proto:473-476`).

## 6. Error handling

**The engine's error vocabulary is fixed by the client.**
`crates/gascan-arca/src/error.rs:20-55` accepts exactly twelve codes — `command_io`,
`command_failed`, `invalid_output`, `helper_error`, `unsupported_capability`,
`ownership_mismatch`, `foreign_resource_refused`, `invalid_resource_identity`,
`resource_conflict`, `not_found`, `invalid_state`, `unknown_actual_state` — and maps
anything else to `invalid_output` naming the offender. `injected_failure` and
`unsupported_version` are explicitly not an engine's to raise (`error.rs:8-11`, asserted at
`error.rs:111-121`). The Swift table is a subset of those twelve.

**A thrown Swift error is a contract violation, not an error path.** gRPC status codes are
reserved for transport faults and carry no engine semantics (`engine.proto:52-58`), but an
uncaught `throw` in a grpc-swift provider method becomes exactly that. Every method catches
everything and maps to an `EngineError`; only genuine transport faults surface as statuses.

**`resource` and `message` are not interchangeable.** `error.rs:137-207` asserts the full
rendered string for every code precisely because a `resource`↔`message` transposition is
invisible to a code check — `resource_conflict` and `invalid_state` carry both fields, so a
swap passes a `contains` assertion on both sides. The resource name goes in `resource`,
prose in `message`.

**Every response sets its `oneof`.** An unset outcome is `invalid_output` client-side
(`crates/gascan-arca/src/translate.rs:291-293`).

Daemon side: an unreachable engine at startup is a hard failure naming the socket; an
unrecognised `engine_version` is a refusal (contract §9); an engine that dies mid-call fails
that call and appears as `MissingOwned` on the next reconcile, never as a silent reconnect
implying state survived. `gascand` validates the socket's owning uid before dialing.

## 7. Sequencing

Thin end-to-end spine first, with a single landing at the end.

1. Arca: `ArcaEngine` + `arca-engine` serving all eleven methods, with
   `Capabilities`/`Inspect`/`ListResources` real and the rest returning a stated
   `EngineError`.
2. Gas Can: live-tier harness spawns it; drive `ArcaBackend<ChannelTransport>`. Answers the
   `connect` error paths and the placeholder-authority claim immediately.
3. Arca: `PrepareImage`, the `image load` subcommand, and `Create`/`Start`/`Stop`/`Remove`.
4. Arca: `Exec` and `Logs`. Answers RST_STREAM, frame-for-frame framing, and `LogsChunk`
   ordering.
5. Gas Can: `BackendSelection::Arca`, daemon wiring, launchd plist, installer changes,
   `gascan-e2e` run.
6. The offline proof exercise and its evidence artifact; `Capabilities.offline` returns
   `PROVEN` for that revision.
7. One signed Arca tag, one `engine/arca-pin.json` bump, documentation corrections (§10).

Iteration uses a local `file://` pin — `scripts/build-arca-engine.sh:5` honours
`GASCAN_ARCA_PIN_FILE` and `:27` accepts `file://` URLs — so the feedback loop does not
require pushing tags. Exactly one signed tag and one pin bump land, verified against
`engine/allowed-signers`.

Sequence 1-2 exists so that a wrong assumption about the wire costs a day rather than the
whole effort. `START-HERE:128-135` records that the recurring defect in this project is an
instrument narrower than the claim it appears to support.

## 8. Testing

### 8.1 Arca Swift tests

Translation in both directions, the identity rule of §4, the error table of §6 including the
no-escaping-throw property, and the exec state machine.

**Gap, stated rather than assumed away:** Arca has no CI (P2.3 is unstarted), and
`scripts/build-arca-engine.sh:109-110` runs `swift build`, never `swift test`. These tests
would rot unnoticed. The live tier's build step therefore runs `swift test` for the engine
target, so the Swift half cannot silently decay.

### 8.2 `crates/gascan-arca/tests/live/`

Spawns the engine directly on a temporary socket and drives
`ArcaBackend<ChannelTransport>`. It bypasses the daemon deliberately: it kills streams,
resets mid-exec, and kills the engine under an open call, and it is the tier that answers
§9.

- every `ChannelTransport::connect` error path — missing socket, a path that is not a
  socket, a socket refusing connections;
- that a real server accepts the placeholder authority `http://[::]:50051` over a UDS;
- `Exec` accepted frame for frame — start, stdin, resize, signal, close, and
  stdout/stderr/exit back;
- `LogsChunk` ordering and clean end-of-stream across a log larger than one chunk;
- **cancellation on reset**: drop `ExecSession` mid-exec and assert the guest process is
  gone and the exec instance reaped.

That last assertion is on engine-observable state, not the client's own view, and its
ability to fail is proved by mutation rather than by reading. `START-HERE:132-134` records a
drop-cancellation test whose every exit was cancellation-independent, so no mutation of the
wiring could fail it.

### 8.3 `gascan-e2e`

One daemon-on-engine pass — `gascan up`, exec, logs, `gascan down` — plus a `gascand`
restart proving reconcile adopts a surviving sandbox, which is the property §2.3 rests on.
This tier is also where engine dialing, the version refusal, and the launchd job are
exercised as a whole.

### 8.4 The offline proof

A recorded observation from inside a running offline sandbox establishing that it has no
egress, pinned to the exact Arca revision, stored under `docs/evidence/`.
`Capabilities.offline` returns `PROVEN` only for that revision, mirroring the certified
release gate at `crates/gascan-apple/src/probe.rs:224-228`.

### 8.5 Baseline hygiene

Every live test is `#[ignore]`d with a reason naming its requirements, following
`crates/gascan-apple/tests/live/backend_contract.rs:15`, and added to
`tests/ci/expected-ignored-tests.txt` or `scripts/ci-check-ignored-tests.sh` fails in either
direction.

## 9. The claims this answers

`START-HERE`'s **"What P5.1 will discover"** section records these as unverified — the
section as it stood at `9665107`, before this branch rewrote that file. Cited by section
name and not by line range on purpose: the range this once carried (`:83-95`) already
drifted onto unrelated text when `START-HERE` was rewritten in `ba15c51`, which is the
exact defect §10 exists to complain about.

Each gets an answer produced by a command, recorded with that command. Milestone 1
answered the last two rows; `docs/status/arca-integration-handoff.md` carries those
answers with their anchors, because this table's own scaffolding is disposable and that
document is not.

| Claim | Answered by |
|---|---|
| Whether a mid-exec reset is cancellation or error | §2.4 decides it; §8.2 proves the engine honours it |
| Exec teardown being engine-paced | §8.2, against a real engine half |
| That Arca accepts this client's `Exec` framing frame for frame | §8.2 |
| `LogsChunk` ordering and end-of-stream | §8.2 |
| That a real server ignores the placeholder authority | §8.2 |
| Every error path through `connect` | §8.2 |

## 10. Documents this work must correct

- Contract §8.1 states "`Sources/DockerAPI/` is deleted early rather than retained"
  (`2026-08-04-sandbox-engine-contract.md:190-193`), and roadmap **P4** is "Docker removal
  in Arca". Arca retains its Docker capability; Gas Can builds and ships only the targets it
  needs. Both statements need correcting to the modular-consumption model.
- The contract gains §2.4's cancellation semantics as a behavioural commitment.
- **The contract's line anchors have drifted and mislead.** §2 cites
  `crates/gascan-core/src/manifest.rs:128` for the `NetworkMode` default, which is now
  `ports()`; the default is at `:189-193` (VERIFIED, `grep -n "enum NetworkMode" -A 12`).
  §5 cites `runtime.rs:717` for `RuntimeBackend`, which is now `:1047` (VERIFIED, `grep -n
  "RuntimeBackend" crates/gascan-core/src/runtime.rs`). Refresh them, or drop line numbers
  where the symbol name alone locates the thing.
- `docs/status/START-HERE.md` and `docs/status/arca-integration-handoff.md` need P5.1
  restated per §1.

## 11. Out of scope

- **P5.3 conformance.** Extracting the suite from `fake_runtime.rs` and running it against
  the arca backend remains P5.3. This design does not pre-empt it, and until it lands the
  Arca backend is not eligible to be the default (contract §8.3-4).
- **U5.** How a shipped `.pkg` gets the workspace image into a user's engine stays P5.4
  (§2.2).
- **P6 network model.** Egress policy, peer channels, and guest-enforced packet filtering
  are untouched. `Capabilities` fields 10-19 stay reserved.
- **The `sandbox_id`-claim rule duplicated between `gascan-arca/src/translate.rs` and
  `gascan-apple/src/inspect.rs`**, which `START-HERE:72-80` books to P5.3.
- **Socket activation** (§2.3) and **engine idle-exit**.
- **`SIGKILL` of the whole machine.** If launchd is not running, nothing here helps.
