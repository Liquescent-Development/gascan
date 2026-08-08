# `gascan-arca` — design

Date: 2026-08-08
Status: Draft for review
Roadmap step: **P5.2**, `docs/superpowers/plans/2026-08-04-arca-integration-roadmap.md`
Depends on: **P3**, complete — proto published, both sides generate, nothing implements it

Type mapping: `docs/superpowers/specs/2026-08-07-arca-engine-proto-design.md` §9.
Build machinery: `docs/superpowers/specs/2026-08-07-arca-engine-codegen-design.md`.
Governing contract: `docs/superpowers/specs/2026-08-04-sandbox-engine-contract.md`.
Wire contract: `arca/proto/arca/engine/v1/engine.proto` at the pinned revision.

The artifact is one new crate, `crates/gascan-arca`, plus a small extraction in
`gascan-core` that §3 justifies. Arca owns the wire protocol; Gas Can owns the
behavioural specification, and this document is the Gas Can side of that split.

Every claim below is marked **VERIFIED** with the command, file:line or exit code
that establishes it, or **PLAN** if nothing has been run yet. Rules ship bare;
events do not.

## 1. Scope

P5.2 implements `RuntimeBackend` over the generated client, behind a transport
seam so that tests need no live engine — the roadmap's own wording for this step.
It does **not** extract the conformance suite (P5.3), wire backend selection in
`gascand`, or resolve U5 (P5.4).

**P5.1 does not exist**, so the `tonic` arm has no live counterpart. That is
stated as a known gap rather than papered over with a Rust test double: the
kickoff's standing prohibition is that "the first thing to implement a Rust
server would be a test double that made a wrong client look correct," and this
design honours it. The design's answer is that the mapping is exercised against a
fake transport (§7, all PLAN) and the `tonic` arm is kept thin enough that what
goes untested is almost entirely `tonic`'s own code.

### Four decisions taken with the maintainer before drafting

| # | Decision | Where it lands |
|---|---|---|
| 1 | Ship the mapping **and** a real `tonic` transport, with the no-live-counterpart gap stated | §2, §8 |
| 2 | The ownership judgment is **extracted into `gascan-core`** and shared, not reimplemented per backend | §3 |
| 3 | An unacceptable `EngineError` code — unknown, or illegitimate from an engine — is **rejected as `InvalidOutput`** naming the code | §5 |
| 4 | The seam is an **`EngineTransport` trait over wire types**, not a `tonic::GrpcService` generic and not a flattened-streaming variant | §2 |

## 2. Placement, registration, and the seam

### 2.1 Layout

`crates/gascan-arca`, a sibling of `gascan-apple` whose layout it copies:

| File | Contents |
|---|---|
| `src/lib.rs` | module wiring and re-exports, mirroring `gascan-apple/src/lib.rs` |
| `src/transport.rs` | `EngineTransport`, `TransportError`, `ExecStream`, `LogsStream` |
| `src/backend.rs` | `ArcaBackend<T>` and its `RuntimeBackend` impl |
| `src/translate.rs` | pure wire↔core mapping; no I/O, no transport |
| `src/error.rs` | the `EngineError` code table and its rejection path |
| `src/channel.rs` | `ChannelTransport`, the `tonic` implementation |

Dependencies: `gascan-core`, `gascan-engine-proto`, `async-trait`, `thiserror`,
`tokio`, `tonic`, `prost`. Dev: `tokio` with `macros`/`rt`/`time`, matching
`gascan-apple/Cargo.toml`.

**No `camino`**, unlike `gascan-apple`: nothing inbound constructs a `Utf8PathBuf`,
because `RuntimeSandbox` carries no mounts (`runtime.rs:253-261`), and the
outbound path only reads the paths `gascan-core` hands it.

### 2.2 Registration is one line

**VERIFIED 2026-08-08.** Adding the crate touches only `Cargo.toml`'s `members`:

| Consumer | Check | Result |
|---|---|---|
| `scripts/ci-classify-paths.sh:40` | read | matches `crates/*` as a glob, so a new crate needs no entry |
| `tests/release/source-input-contract.sh:12` | read | seeds a generic `crates/lib.rs`, not a per-crate list |
| every script and workflow | `grep -rn gascan-apple scripts/ tests/ .github/workflows/` | all hits concern the attach **helper binary**, none enumerate crates |

This check exists because P3.2's one genuinely dangerous consumer — the path
classifier — was invisible to CI. It was re-run rather than assumed.

### 2.3 Lints

`[lints] workspace = true` and nothing more, matching `gascan-apple`.
**VERIFIED:** the workspace table forbids `unsafe_code` and nothing else
(`Cargo.toml:39-40`); the `clippy::panic`/`unwrap_used`/`expect_used` denials are
per-crate (`gascan/src/lib.rs:2`) and are therefore **not** inherited here.
Adding them would diverge from the sibling backend crate, so this crate does not.

### 2.4 `EngineTransport`

```rust
#[async_trait]
pub trait EngineTransport: Send + Sync {
    async fn capabilities(&self, request: v1::CapabilitiesRequest)
        -> Result<v1::CapabilitiesResponse, TransportError>;
    async fn inspect(&self, request: v1::InspectRequest)
        -> Result<v1::InspectResponse, TransportError>;
    // … seven more unary methods, each taking and returning the generated types verbatim …
    async fn exec(&self, start: v1::ExecStart) -> Result<ExecStream, TransportError>;
    async fn logs(&self, request: v1::LogsRequest) -> Result<LogsStream, TransportError>;
}
```

Nine unary methods pass the generated types through untouched. The seam is
deliberately in **wire types**, not core types: a seam in core types would put
the mapping below the fake, and the mapping is the part with the bugs.

**`exec` takes an `ExecStart`, not a first `ExecClientFrame`.** The contract
requires exactly one `ExecStart` and requires it first (`engine.proto:409-411`).
Passing the payload rather than the frame means the type enforces that and no
implementation of this trait can violate it. This is the contract's own governing
rule — the protocol is the policy — applied one layer down to our own seam.

**`TransportError` carries transport faults only.** The contract reserves gRPC
status codes for exactly that and puts all engine semantics in the response body
(`engine.proto:54-58`), so engine meaning never arrives as a `TransportError`. It
maps to `RuntimeError::CommandIo { operation: <rpc>, message }`, whose `code()` is
`"command_io"` — already the code `gascand/src/api.rs:2375` expects on a broken
exec stream.

`ExecStream` and `LogsStream` are concrete structs over `tokio::sync::mpsc`, so
`ArcaBackend<T>` carries no stream generics and a fake is a pair of channels.

### 2.5 Why this seam and not the alternatives

`ArcaBackend<T: EngineTransport>` mirrors `AppleBackend<R: CommandRunner>`
(`gascan-apple/src/backend.rs:24-46`) — the same shape, in the same position, for
the same reason. Two alternatives were considered and rejected:

- **Generic over `tonic::client::GrpcService`.** Highest fidelity, since the
  tested path would be the real path. Rejected because every fake must then be
  built at the HTTP body and `Status` layer, and the tests would assert against
  `tonic`'s behaviour as much as ours.
- **The same trait with streaming flattened** to `Vec<LogsChunk>` and a collected
  exec transcript. Rejected because it discards the property streaming was chosen
  for — a log larger than the default message limit must not fail as a size error
  (`engine.proto:470-473`) — and because a collected transcript cannot satisfy the
  live `ExecSession` the trait returns.

The cost of the chosen seam, stated rather than hidden: a `tonic`-specific defect
can hide beneath it, and P5.1's integration work is what will surface it.

## 3. The extraction in `gascan-core`

Three things the mapping needs are private or duplicated today. This is the only
part of P5.2 that reaches outside the new crate, and each item is here because
`gascan-arca` cannot do its job correctly without it — not as opportunistic
cleanup.

### 3.1 The per-kind ownership classifier

`ResourceOwnership` is **absent from the wire on purpose**: the engine stores
labels verbatim, echoes them back, and never interprets them, because deciding
whether a labelled resource is yours is a policy question and the policy boundary
exists to keep it out of the engine (`engine.proto:143-153`). So the consumer
classifies — and if `gascan-arca` classifies differently from `gascan-apple`, the
two backends disagree about what may be deleted, since
`RemoveRequest::from_resources` admits only `GasCanOwned` (`runtime.rs:951-958`).

**VERIFIED: there are already two classifiers, and they are not duplicates.** Both
were read in full; neither was inferred from the other.

| Site | Rule | Correct for |
|---|---|---|
| `gascan-apple/src/backend.rs:650` | `managed_by == "gascan"` with a sandbox id present; **name ignored** | volumes and networks, which are not named by a sandbox id |
| `gascan-apple/src/inspect.rs:257` | additionally requires `id.as_str() == name` | containers, which **are** named by their sandbox id |

The difference is load-bearing, so the shared function keeps it. **VERIFIED** that
each existing site only ever sees the kinds its rule is right for: `inventory`
classifies volumes and networks, taking containers from `AppleInspector` instead
(`backend.rs:54-111`), and `classify_inventory_ownership` is the container path
(`inspect.rs:257`). So one per-kind function reproduces both behaviours exactly
rather than approximately.

**The two sites also differ on an *unparseable* label, and the shared function
must not absorb that.** VERIFIED: `backend.rs:74-81` parses the label first and
turns a parse failure into an `invalid_output` **error**, failing the whole
listing; `inspect.rs:268` maps the same failure to `Mismatched` and carries on.
The extraction therefore shares **the rule, not the error policy**:

```rust
pub enum SandboxLabel<'a> { Absent, Unparseable, Parsed(&'a SandboxId) }

pub fn classify_resource_ownership(
    kind: ResourceKind,
    name: &str,
    managed_by: Option<&str>,
    sandbox: SandboxLabel<'_>,
) -> ResourceOwnership
```

A total function over three explicit label states, with no label map in the
signature — `gascan-apple`'s sites hold a `BTreeMap` and `gascan-arca` holds a
typed `OwnerLabels`. `Unparseable` maps to `Mismatched`, which is `inspect.rs`'s
existing behaviour; `backend.rs` keeps its earlier hard failure and so never
passes `Unparseable`. Each site's error policy stays its own, and neither
behaviour changes.

`gascan-arca` treats an unparseable label the way `inspect.rs` does — as
`Mismatched` rather than a failed call — because `ListResources` deliberately
returns every resource the engine holds, labelled or not, so that drift detection
can see them (`engine.proto:389-391`). One malformed foreign label must not blind
the consumer to the rest of the inventory.

**The parity evidence is `gascan-apple`'s existing tests passing unchanged** —
including `mixed_list_classifies_owned_foreign_and_mismatched_resources`
(`gascan-apple/tests/inspect.rs:134-162`), which pins all three outcomes, and
`foreign_container_names_do_not_have_to_be_valid_sandbox_ids` (`:164-174`). If the
extraction changes a rule, those fail. PLAN until run.

### 3.2 `immutable_image_identity`

`runtime.rs:647` already computes exactly the tag-stripped `(repository, digest)`
pair that `ImageDigest` needs, and it is private. Publishing it rather than
re-deriving the split is not merely DRY: it makes `gascan-arca`'s canonicalisation
**provably the same one** `same_immutable_image` compares by, which is what §4.3
depends on.

### 3.3 `MANAGED_BY` and the label keys

Enumerated, not sampled. **VERIFIED 2026-08-08** by
`grep -rn 'MANAGED_BY[A-Z_]* *: *&str'`: the string `"gascan"` exists as **four**
private constants — `gascan-core/src/policy.rs:22`,
`gascan-apple/src/backend.rs:20`, `gascan-apple/src/inspect.rs:19`,
`gascan-apple/src/translate.rs:15` — and the label key `dev.gascan.managed-by` as
**two** (`backend.rs:21`, `inspect.rs:17`). `gascan-core` publishes one of each and
`gascan-apple`'s copies migrate.

**Deliberate scope boundary.** `gascand/src/service.rs` also holds bare `"gascan"`
literals. **VERIFIED 2026-08-08:** `grep -c '"gascan"' crates/gascand/src/service.rs`
reports **14**, at `:778, :808, :1007, :1305, :1668, :1954, :1999, :2417, :2517,
:2651, :2998, :3075, :3167, :3247` — every one a `managed_by` comparison
(`grep -c 'managed_by [!=]= "gascan"'` also reports 14). They are **not** in scope.
Nothing in this work needs them changed, and editing `service.rs` for cosmetics is
how a session acquires a timing flake it then has to chase.

~~"ten bare literals"~~ **corrected 2026-08-08 before commit.** The first count
came from a `grep | head -10`, which truncated silently and returned exactly the
limit — the shape that looks like a complete answer. It is left visible because it
is this project's recurring failure in miniature: **an aggregate described after
examining part of it.** The fix is the one the kickoff prescribes — count with
`grep -c`, never with a truncating pipe.

## 4. Mapping

§9 of the P3.1 design is the mapping table and is not restated. This section
records only what §9 does not say, and each entry below was checked against the
code rather than inferred from the table.

### 4.1 The sealed request is read, never reconstructed

`CreateRequest` has `pub(crate)` fields, derives `Serialize` but **not**
`Deserialize`, and carries `compile_fail` doctests against every construction path
(`runtime.rs:54-143`). `gascan-arca` is the sender, so it only ever reads through
the accessors at `runtime.rs:199-243`. **The seal is untouched by this work.**

### 4.2 Inbound goes through `gascan-core`'s validating constructors

This is the design's most important property and it required no new code.

| Wire | Constructor | What it already enforces |
|---|---|---|
| `Created` | `CreateOutcome::new(&request, created)` (`runtime.rs:721-727`) | via `validate_created_resources`: every resource is in the request's topology, is `GasCanOwned`, carries the request's sandbox id, is not a duplicate, and the container and any managed network are present |
| `CreateFailed` | `CreateFailure::from_created_evidence(&request, created, source)` (`runtime.rs:767-799`) | **filters** rather than errors — the correct behaviour on a failure path, because losing partial-create evidence leaks resources nothing later knows to look for |
| `Resource` | `RuntimeResource::discovered` (`runtime.rs:503-514`) | mints a fresh process-local `RemovalProof` |
| `Sandbox` | `RuntimeSandbox::observed` (`runtime.rs:264-278`) | — |

So the boundary check against a buggy or lying engine is **existing, tested code
reached by calling the constructor**, not new hand-rolled validation. A hostile
`Created` naming a resource outside the request cannot become a `CreateOutcome`.

### 4.3 The four §9 gaps, checked

**VERIFIED 2026-08-08.**

| Gap | Check | Result |
|---|---|---|
| Does dropping `RuntimePort.host_address` lose anything? | `grep -n host_address crates/gascan-core/src/policy.rs` | No. All three construction sites are `IpAddr::V4(Ipv4Addr::LOCALHOST)` (`policy.rs:163,451,462`), so synthesising `LOCALHOST` inbound round-trips exactly |
| Can a read-only or multi-entry bind mount reach a backend and break singular `ProjectMount`? | read `policy.rs:388-398` | No. `validate_spec` requires exactly one mount, source `== canonical_root()`, target `/workspace`, and `is_writable()`. §9's "length other than 1 is a boundary failure" is a defensive check on an unreachable path, and stays as one |
| Does `ImageDigest` dropping a tag break an equality check? | traced `service.rs:1949`, `:1697`, `:2716` | `expected_previous` is always a prior **observation** (`:2716`), so the exact-string comparison at `:1949` is observation-to-observation and holds under any **deterministic** canonicalisation. `same_immutable_image` is tag-insensitive, VERIFIED by `gascan-core/tests/image_identity.rs:11-12`. The rollback path re-requests an observed string through `RecreateRequest::for_image` (`:1697`), which requires `immutable_image_reference` to accept it — `repository@sha256:<64 hex>` does |
| Must `RemovalProof` be stable across calls, as `AppleBackend`'s `observations` cache makes it? | `grep -rn RuntimeResource crates/gascand/src/service.rs`; read `gascan-apple/tests/inspect.rs:152-161` | No. No consumer compares resources across calls, and `AppleInspector` deliberately mints **fresh** proofs each call, asserted by `"each inventory has fresh removal proofs"`. The cache is an `AppleBackend` internal, not a trait obligation, so `gascan-arca` does not build one |

### 4.4 `Remove`'s cardinality mismatch

§9 records `RemoveRequest → RemoveRequest: Identities plus OwnerLabels`, which
glosses over a real mismatch. Core's `RemoveRequest` holds resources that each
carry their **own** `sandbox_id`; the wire carries **one** `OwnerLabels` for the
whole call (`engine.proto:379-385`).

**VERIFIED 2026-08-08:** `grep -c 'RemoveRequest::from_resources'
crates/gascand/src/service.rs` reports **6** sites, and all six yield a single
sandbox's resources — counted, then each one read:

| Site | Source | Single-sandbox because |
|---|---|---|
| `:2540`, `:3190` | `list_resources()` | both filter `resource.sandbox_id() == Some(id)` before building the request |
| `:1678`, `:1767` | one container | one resource |
| `:1394` | `failure.created()` | `from_created_evidence` keeps only resources whose `sandbox_id` is the request's (`runtime.rs:794`) |
| `:2008` | `outcome.created()` | `CreateOutcome::new` enforces the same (`runtime.rs:911`) |

But **nothing enforces it**: `RemoveRequest::from_resources` checks only that the
list is non-empty and every resource is `GasCanOwned` (`runtime.rs:944-959`). So
`gascan-arca` derives the single `OwnerLabels` from the resources and **refuses a
mixed-sandbox request** as a boundary failure. Sending the first resource's labels
and hoping would be a silent wrong answer of exactly the kind the engine's
label-refusal rule exists to prevent.

### 4.5 Where the mapping refuses rather than coerces

Each is a boundary failure that names what it saw:

- a `bind_mounts` length other than 1, or a non-writable mount
- a `host_port` or `guest_port` outside `u16` (the wire is `uint32`) — refused,
  never truncated
- a `host_port` or `guest_port` of **0**, or a duplicated `host_port` — see §4.6
- an image that fails `immutable_image_reference`, on either direction
- `ISOLATION_UNSPECIFIED`, `SANDBOX_STATE_UNSPECIFIED`, `USER_UNSPECIFIED`, or
  `RESOURCE_KIND_UNSPECIFIED` arriving from the engine
- a `Sandbox` with absent or unparseable `OwnerLabels`, mirroring
  `inspect.rs:281-292`, which already requires both labels on a container
- a mixed-sandbox `RemoveRequest`, per §4.4

`contract_minor` (`engine.proto:110-115`) is read and dropped, because this client
populates no additive fields yet and `RuntimeCapabilities` has no home for it.
Recorded so a later session knows it was considered rather than missed.

### 4.6 Parity of refusals, not only of mappings

§9 is a table of type correspondences and says nothing about what the existing
backend *rejects*. Reading `gascan-apple`'s port handling changes this design.
**VERIFIED** at `gascan-apple/src/inspect.rs:161,173,179-182`, with the cases
pinned by `inspect_rejects_untrusted_published_port_shapes_and_values`
(`tests/inspect.rs:104-130`), the Apple backend refuses four published-port
shapes. Against the wire:

| Apple refuses | On the wire | `gascan-arca` |
|---|---|---|
| `host_address != 127.0.0.1` (`:161`) | **inexpressible** — `PortMapping` has no address field | nothing to refuse |
| `count != 1` (`:173`) | **inexpressible** — no count field | nothing to refuse |
| `host_port == 0` or `container_port == 0` (`:173`) | expressible | **must refuse** |
| duplicated `(host_address, host_port)` (`:179-182`) | expressible as a duplicated `host_port` | **must refuse** |

Two of the four refusals are unnecessary because the contract cannot express the
thing being refused — the "protocol is the policy" rule (`engine.proto:9-11`)
paying out in a measurable way. The other two are behaviour the existing backend
has and this one must match, or the same malformed engine answer would be caught
on Apple and accepted on Arca.

**This is the general obligation**, not a note about ports: where the Apple
backend refuses something the wire can still express, `gascan-arca` refuses it
too. P5.3's conformance suite is what will make that obligation systematic rather
than a matter of having read the right file.

## 5. The error table

**A table is mandatory, not a preference.** The code string is load-bearing to the
user-visible surface: `gascan/src/cli.rs:250-252` rewrites the message a user sees
when the stable code is `resource_conflict`. **VERIFIED 2026-08-08:**
`grep -c '\.code()' crates/gascand/src/service.rs` reports **26** call sites, of
which **9** stamp the code directly into a telemetry `reason` field
(`grep -c 'reason: .*\.code()'`). Collapsing engine failures into one variant
would silently change CLI output and flatten that telemetry.

**A dynamic code cannot flow through.** `RuntimeError::code()` is
`pub const fn code(&self) -> &'static str` (`runtime.rs:1056`), so every accepted
code must land on a known variant.

Twelve codes are accepted, filling what the wire cannot carry from the RPC name,
`None`, or `message`:

| Wire `code` | `RuntimeError` |
|---|---|
| `command_io` | `CommandIo { operation: <rpc>, message }` |
| `command_failed` | `CommandFailed { operation: <rpc>, exit_code: None, stderr: message }` |
| `invalid_output` | `InvalidOutput { operation: <rpc>, message }` |
| `helper_error` | `HelperError { operation: <rpc>, code, message }` |
| `unsupported_capability` | `UnsupportedCapability { capability: message }` |
| `ownership_mismatch` | `OwnershipMismatch { resource }` |
| `foreign_resource_refused` | `ForeignResourceRefused { resource }` |
| `invalid_resource_identity` | `InvalidResourceIdentity { name: resource }` |
| `resource_conflict` | `Conflict { resource, message }` |
| `not_found` | `NotFound { resource }` |
| `invalid_state` | `InvalidState { resource, message }` |
| `unknown_actual_state` | `UnknownActualState { resource, state: message }` |

Two of the fourteen are **rejected as illegitimate from an engine**:
`injected_failure` belongs to `FakeRuntime`, and `unsupported_version` is the
*consumer's* refusal to drive an engine and carries a `found: RuntimeVersion` the
wire never sends. An unknown code is rejected too. All three become
`InvalidOutput { operation: <rpc>, message: "engine returned unacceptable error
code `X`: …" }`, which cannot alias a known code and names the offender so the
next session can read it. This follows the contract's own instruction that a
consumer maps the code "with a table, not a judgment, so a new engine failure mode
cannot quietly become an existing one" (`engine.proto:62-65`).

**An empty `resource` on a resource-scoped code passes through verbatim.** The
contract says `resource` is empty when the failure is not about one
(`engine.proto:66-67`), and failing the whole call over an empty diagnostic field
would replace a real, readable engine error with a confusing protocol one. That is
the D7 lesson in miniature: never convert a diagnosable failure into a less
diagnosable one.

## 6. Exec and logs

### 6.1 Exec

Structurally mirrors `AppleBackend::exec` (`gascan-apple/src/backend.rs:517-602`),
which was read in full rather than paraphrased: one spawned pump task, a
three-way `tokio::select!` over cancellation, consumer input, and server frames;
terminal on `Exit`, on a server error frame, or on a failed delivery; returning
`ExecSession::live_cancellable` so that dropping the session cancels the guest
work (`runtime.rs:381-391`, `:415-419`).

Two parities with the Apple backend, both deliberate:

- the request's `stdin` buffer is sent as the first stdin frame **only when
  non-empty** (`backend.rs:533`)
- **no `Close` is auto-sent.** The consumer sends `ExecInput::Close` when it means
  to, exactly as it does for the Apple backend

Mapping notes: `argv: Vec<String>` widens losslessly into `repeated bytes`
(`engine.proto:414-417`); `Resize` is `uint32` on the wire and `u32` in core, so
unlike the Apple path (`backend.rs:556-561`, which must fit `u16`) **no range
refusal is needed**; and `Exit` carries both `code` and `signal`, so this backend
is strictly richer than Apple's, which hardcodes `signal: 0` (`backend.rs:585`).

### 6.2 Logs

`RuntimeBackend::logs` returns `Result<Vec<u8>, RuntimeError>`
(`runtime.rs:1003-1007`) while the wire streams `LogsChunk` — streamed so that a
log larger than the message limit does not fail as a size error
(`engine.proto:470-473`). The client concatenates `data` chunks in order.

**An error chunk mid-stream discards the partial buffer and returns `Err`.**
Returning partial data beside a swallowed error is the silent-failure shape this
project forbids; the trait's signature has no way to say "here is some of it, and
also it broke."

## 7. Testing

All PLAN until run.

A fake `EngineTransport` built from `tokio` channels, following
`gascan-apple/tests/backend_fake_runner.rs`, which is the established shape for
this in the workspace.

| Area | Cases |
|---|---|
| Mapping | one per method, both directions, including the `LOCALHOST` and image round-trips |
| Refusals | one per bullet in §4.5, plus §4.4's mixed-sandbox request and §4.6's two port shapes |
| Errors | all twelve accepted codes; `injected_failure`, `unsupported_version` and an unknown code each rejected and each naming the code |
| Logs | concatenation across ≥ 2 chunks; a mid-stream error discarding the partial buffer |
| Exec | initial stdin, live stdin, resize, signal, close, exit, a terminal server error frame, and drop-cancellation |
| Validating constructors | a `Created` naming a resource outside the request's topology is refused by `CreateOutcome::new` — the boundary property of §4.2, asserted rather than assumed |
| The extracted classifier | in `gascan-core`: all three `SandboxLabel` states against all three `ResourceKind`s, pinning that a container requires `name == sandbox_id` and a volume or network does not. `gascan-apple`'s existing tests passing unchanged is the parity half (§3.1) |

**Two mutation flips must be shown, not asserted.** A test that does not fail when
the thing it tests is broken is not a test:

1. deleting the `LOCALHOST` synthesis must fail the port round-trip test
2. accepting an unknown error code must fail the rejection test

`cargo clippy --all-targets -- -D warnings` is part of the bar, not only
`cargo test`: on 2026-08-07 a new test using `expect_err` passed `cargo test` and
was rejected by clippy.

## 8. Deliberately not done

| | Why |
|---|---|
| No conformance-suite extraction | P5.3 |
| No backend selection wiring in `gascand` | not in P5.2's roadmap line; it would make an untested transport reachable at runtime |
| U5 unresolved | P5.4. `PrepareImage` sends a digest and fails when the engine lacks the content, which is the contract's stated position (`engine.proto:308-313`) |
| No Rust server, not even as a test double | the kickoff's standing prohibition; a double here would make a wrong client look correct |
| No proto change | it is published and pinned by signed tag. A defect in it is a contract change with a cost |
| No second parser of `arca-pin.json` | `scripts/sync-arca-proto.sh` owns what the pinned contract means |
| No `RemovalProof` stability cache | §4.3 — nothing requires it |
| `gascand`'s 14 `"gascan"` literals untouched | §3.3 |

## 9. The `tonic` arm

`ChannelTransport` implements `EngineTransport` over
`gascan_engine_proto::v1::sandbox_engine_client::SandboxEngineClient<T>`.
**VERIFIED 2026-08-08** against the generated output rather than assumed from the
service name — the kickoff's rule is that a proto is not reviewed by reading it:
`grep -n` on the freshest `OUT_DIR` copy of `arca.engine.v1.rs` finds
`pub mod sandbox_engine_client` at `:614` and `pub struct SandboxEngineClient<T>`
at `:628`, and finds **no** `sandbox_engine_server`, which independently confirms
§8's client-only generation.

It follows the existing UDS dial at `gascan/src/client.rs:494-499` —
`Endpoint::from_static` plus `connect_with_connector` over a
`tokio::net::UnixStream` — rather than inventing an endpoint story. `tonic` 0.12,
`prost` 0.13, `tower` 0.4 and `hyper-util` 0.1 are already workspace dependencies
(`Cargo.toml:29-33`).

It is thin by design: each unary method is one call plus
`TransportError::from(tonic::Status)`, and the two streaming methods bridge
`tonic`'s streams onto the channel pair in §2.4. **It has no live counterpart
until P5.1**, and the first genuine integration risk it carries — whether Arca's
generated server agrees with this client frame-for-frame on `Exec` — is P5.1's to
find. Naming it here means the next session does not discover it as a surprise.
