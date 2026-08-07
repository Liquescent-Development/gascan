# Arca Engine Proto — design

Date: 2026-08-07
Status: Draft for review
Roadmap step: **P3.1**, `docs/superpowers/plans/2026-08-04-arca-integration-roadmap.md`
Resolves: **U4 — the engine protocol's actual shape**

Governing contract: `docs/superpowers/specs/2026-08-04-sandbox-engine-contract.md`.
Gas Can side: `docs/superpowers/specs/2026-08-04-arca-sandbox-backend.md`.
Arca side: `arca/Documentation/SANDBOX_ENGINE_PIVOT.md`.

The artifact this design produces is a single file **in Arca**, not in Gas Can:
`~/code/arca/proto/arca/engine/v1/engine.proto`. Arca owns the wire protocol
(contract §3); Gas Can owns the behavioural specification. This document is the
reasoning, and it lives with the reasoning.

## 1. Scope

P3.1 defines the proto and resolves U4. It does **not** implement either side, and
it does not wire codegen into either build — that is P3.2. P3's exit is
deliberately modest: *proto exists, both sides generate, nothing implements it
yet.*

Four decisions were taken with the maintainer before drafting, and each is
recorded at its point of use below:

| # | Decision |
|---|---|
| 1 | v1 expresses **today's `RuntimeBackend` surface only**. P6's peer channels and egress policy are `reserved` field blocks, not fields. |
| 2 | Errors travel as a **typed result in the response body**, uniformly. gRPC status codes are for transport faults. |
| 3 | The image is a **structured digest**, so a tag-only reference is unconstructible. |
| 4 | The file lives at Arca's **repository root** under `proto/arca/engine/v1/`, not beside the guest-facing protos in `Sources/ContainerBridge/proto/`. |

## 2. The surface is one RPC per trait method — eleven, not ten

Derived from `RuntimeBackend` (`crates/gascan-core/src/runtime.rs:990-1009`).

| RPC | Trait method | Shape |
|---|---|---|
| `Capabilities` | `capabilities` | unary |
| `Inspect` | `inspect` | unary, three-armed result |
| `Create` | `create` | unary, partial-evidence failure arm |
| `PrepareImage` | `prepare_image` | unary |
| `CreateContainer` | `create_container` | unary, shares `CreateResponse` |
| `Start` | `start` | unary |
| `Stop` | `stop` | unary |
| `Remove` | `remove` | unary |
| `Exec` | `exec` | bidirectional stream |
| `Logs` | `logs` | server stream |
| `ListResources` | `list_resources` | unary |

**Correction to an existing spec.** `2026-08-04-arca-sandbox-backend.md:31` says
"the ten `RuntimeBackend` methods". It is **eleven** — `runtime.rs:991-1008` lists
`capabilities`, `inspect`, `create`, `prepare_image`, `create_container`, `start`,
`stop`, `remove`, `exec`, `logs`, `list_resources`. The count is not cosmetic: the
missing one is `prepare_image`, which is the method that would grow a registry
client if nobody were watching it (§6 below).

Nothing is invented and nothing is merged. A method here that no `RuntimeBackend`
method needs is a method `PolicyCompiler` cannot gate, which is precisely the
shape contract §2 exists to prevent.

## 3. What the format cannot express (contract §4)

These are properties of the wire format. If a field does not exist, no code path
needs to reject it.

| §4 forbids | Mechanism |
|---|---|
| An arbitrary host path to mount | `CreateRequest.project` is `ProjectMount`, **singular**. There is no second mount to name. `PolicyCompiler` already emits exactly one (`crates/gascan-core/src/policy.rs:389-398` rejects anything but `[canonical_root → WORKSPACE_TARGET, writable]`), so the singular field is not a narrowing — it is what the product already produces. |
| An arbitrary image reference to pull | `ImageDigest { repository, sha256_hex }`. A tag is not a field, so "whatever `:latest` means today" cannot be said. |
| A bind address for a published port | `PortMapping { host_port, guest_port }`. No address field. Loopback is the contract, and `RuntimeCapabilities.loopback_publish` (`runtime.rs:43`) already names it as the only supported case. |
| A network topology, or any mutation of one | `Network` is a `oneof { offline, networked_name }` on `CreateRequest` only. There is no `AddNetwork`, `Connect`, or `UpdateAllowedIPs` — note that Arca's existing `arca.wireguard.v1` has all three, and that is the guest-facing service, not this one. |
| A change to a running sandbox's topology | No RPC accepts a network or a channel after create. Contract §6.2's immutability is enforced by the **absence of a method**, not by a check that could be relaxed. |

`ProjectMount` carries `writable` as no field at all: `policy.rs:394` rejects a
non-writable project root, so a read-only case does not exist and therefore is not
expressible. `Volume` is the same.

## 4. What the format must express (contract §5)

Lifecycle, interaction, and declaration, all present in §2's table. The one place
this design adds structure the trait does not have is `Capabilities.contract_minor`
(§7).

## 5. Errors — a typed result in the body, uniformly

**Decision 2.** Every response carries its outcome in a `oneof result`. gRPC status
codes are reserved for transport faults — an unreachable engine, a broken stream —
and carry no engine semantics.

```proto
message EngineError {
  string code = 1;
  string resource = 2;
  string message = 3;
}
```

`code` carries Gas Can's `RuntimeError::code()` values verbatim
(`runtime.rs:1055-1073`): `not_found`, `resource_conflict`, `ownership_mismatch`,
`invalid_state`, `unsupported_capability`, and the rest. That makes the mapping in
`gascan-arca` a table rather than a judgment, and it means a new engine failure
mode cannot quietly become `command_failed`.

Three consequences, in descending order of how much they forced the decision:

**`Create` must not lose partial evidence.** `CreateFailure`
(`runtime.rs:742-812`) carries `created: Vec<RuntimeResource>` *alongside* its
error, because a create that fails after making two of three volumes must report
those two or they leak — nothing afterwards knows they exist. A bare gRPC status
cannot express that. `CreateResponse` is therefore
`oneof { Created, CreateFailed }`, and `CreateFailed` carries both arms of the
information. This alone would have justified the decision; the uniformity is what
makes it a rule rather than an exception.

**`Inspect` has three arms, not two.** `inspect` returns
`Result<Option<RuntimeSandbox>, _>` (`runtime.rs:992`) — a sandbox that does not
exist is an *answer*. `InspectResponse` is
`oneof { Sandbox, Absent, EngineError }`. Collapsing `Absent` into an error would
make "it is not there" indistinguishable from "I could not tell", and those two
demand opposite behaviour from a reconciler.

**Everything else is `oneof { Ack, EngineError }`**, which is `AckResponse`.

The cost is real and worth naming: standard gRPC tooling — retry policies,
error-rate dashboards, `grpc_status` metrics — sees success on a failed create.
That is accepted because the alternative loses partial-create evidence in
`google.rpc.Status` details, which clients routinely discard, and this protocol has
exactly one class of consumer whose correctness depends on not discarding it.

## 6. `PrepareImage` does not fetch, and this design does not answer U5

`PrepareImage` materialises a rootfs for content **the engine already holds**.
Contract §4 removes the registry client, registry auth, and Keychain access from
the engine entirely, and this is the one method that would grow them back. Absent
content is a failure. It is never a fetch.

**How the engine comes to hold the content is deliberately unanswered.** That is
**U5**, owned by **P5.4**, and the roadmap records it as *"a genuine gap in the
current specs, not just an implementation detail"*
(`2026-08-04-arca-integration-roadmap.md:474`). Gas Can's offline image bundle
machinery must reconcile with it. Resolving it here by adding a source field would
be answering a spec gap by accident, in the one file whose compatibility burden
makes a wrong answer expensive.

## 7. Compatibility, carried from the first commit

The 2026-08-05 weight increase makes this a real published contract with more than
one consumer over time — Gas Can now, a Docker-compatible Arca later — so the
burden starts at the first commit rather than at the first external user.

- **Major version is the package path.** `arca.engine.v1`. A breaking change is a
  new package, never an edit.
- **`Capabilities.contract_minor`** carries additive contract revisions. A consumer
  that speaks `arca.engine.v1` and reads `contract_minor = N` knows exactly which
  additive fields it may find populated. It is distinct from
  `Capabilities.engine_version`, which is the engine's own version and is what
  contract §9 gates on (`RuntimeError::UnsupportedVersion`, `runtime.rs:1030`).
  Conflating them would tie an engine bugfix release to a contract revision.
- **Field numbers are never reused.** Removals become `reserved`. Every message
  carries a trailing `reserved`, following `proto/gascan/v1/gascan.proto`, which
  does this on nearly every message.
- **P6's blocks are reserved now**, with the owning phase named in a comment:
  `Capabilities` reserves 10–19 (10 = `egress_policy`, 11 = `peer_channels`) and
  `CreateRequest` reserves 12–19 (12 = peer channels). **Decision 1**: reserved,
  not present. A capability field nothing enforces is a surface that exists and is
  undefended, which is the precise failure mode contract §2 names.

**The mechanical breaking-change check is P3.3's, and is not faked here.** `buf` is
not installed on this machine (`which buf` → not found, VERIFIED 2026-08-07) and
Arca has no CI (P2.3 open), so a check added now would be inert. What P3.3 needs is
recorded rather than pretended: a checked-in `FileDescriptorSet` and a
`buf breaking` invocation against it, in Arca's CI.

## 8. Three shape decisions, and why

### 8.1 Ownership labels cross the wire; classification does not

`OwnerLabels { managed_by, sandbox_id }` is stored verbatim by the engine, echoed
back on every `Resource`, and **never interpreted**.

Gas Can's `ResourceOwnership::{GasCanOwned, Foreign, Mismatched}`
(`runtime.rs:429-435`) is a *judgment about labels*, and it stays in the consumer —
which is where it already lives for the other backend, in `gascan-apple`'s
`inventory()` (`crates/gascan-apple/src/backend.rs:54`). An engine that decided
"this is Gas Can's" would be deciding a policy question inside the component the
policy boundary exists to constrain.

This also settles `ListResources`: it returns **every** resource the engine holds,
labelled or not. `Resource.owner` is absent for unlabelled ones. Gas Can's drift
detection depends on seeing foreign resources — `ReconcileFinding` covers
unknown-unowned — so filtering them engine-side would break it silently.

`RemovalProof` (`runtime.rs:460-481`) has **no wire representation**. It is an
`Arc`-identity capability that is unforgeable only within one process, and
serialising it would convert a compile-time guarantee into a bearer token. `Remove`
instead carries exact `ResourceIdentity` values plus the caller's `OwnerLabels`,
and the engine refuses any resource whose stored labels differ. The proof stays a
Rust-side property of `gascan-arca`.

### 8.2 `Logs` is a server stream of chunks — not unary, and not `follow`

`RuntimeBackend::logs` returns one `Vec<u8>` (`runtime.rs:1003-1007`), so unary
would match the trait most literally. It is rejected because a workspace log larger
than gRPC's default 4 MiB message limit would fail as a *size error* rather than as
a log — a failure whose message would say nothing about logs at all, which is the
opposite of this project's instrumentation habit.

The stream therefore carries **one logical buffer, chunked**, which the client
concatenates before returning it. The trait's signature is unchanged.

**There is deliberately no `follow` field.** The trait has no follow, `LogsRequest`
in Gas Can's north-facing API has one because *that* is the user-facing surface
(`proto/gascan/v1/gascan.proto:138`), and adding one here is the first step back
toward a general container API. It is exactly the kind of drift the 400-line size
gate is a proxy for.

### 8.3 `Exec` is bidirectional, and the first frame is `ExecStart`

`exec` returns an `ExecSession` (`runtime.rs:327-337`) carrying an `ExecInput`
sender and an `ExecOutput` receiver, so the wire shape is forced:
`stream ExecClientFrame` → `stream ExecServerFrame`. This mirrors
`ClientFrame`/`ServerFrame` in `gascan.proto:217-240`, which solves the same
problem one layer up.

The first client frame must be `ExecStart`; any other first frame is a protocol
error, and exactly one `ExecStart` may appear per stream. `ExecRequest.stdin`'s
pre-supplied blob (`runtime.rs:289`) maps to a `stdin` frame followed by `close` —
the streaming form is a superset, so no second entry point is needed.

**No session token.** `gascan.proto:228` needs one because `Attach` is a *separate*
stream that must be bound to an earlier `Run`/`Shell`. Here the stream that starts
the exec is the stream that carries it, so the binding is the connection itself.

`argv` is `repeated bytes`; environment is `string`/`string`. `execve` takes bytes,
and a consumer holding a non-UTF-8 argument must be able to send it. This is the
same split Gas Can's own API already made — `repeated bytes argv` beside
`EnvironmentVariable { string name; string value; }` at `gascan.proto:122-128`.
`gascan-arca` converts from `Vec<String>` losslessly.

## 9. Type mapping

`gascan-arca` (P5.2) owns this translation; it is recorded here so P5.2 does not
re-derive it.

| Gas Can | Wire | Note |
|---|---|---|
| `RuntimeVersion` | `Version` | |
| `NetworkIsolation` | `Isolation` | Same tri-state. `PROVEN` must mean observed. |
| `RuntimeCapabilities.bind_mounts` | `Capabilities.project_mount` | Renamed: only a project root is expressible, so "bind mounts" would overstate. |
| `OwnershipMetadata` | `OwnerLabels` | Verbatim, uninterpreted. |
| `ResourceOwnership` | *(none)* | Consumer-side judgment. §8.1. |
| `ResourceIdentity`, `ResourceKind` | `ResourceIdentity`, `ResourceKind` | |
| `RuntimeResource` | `Resource` | Minus `RemovalProof`. §8.1. |
| `CreateRequest.image` (`name@sha256:…`) | `ImageDigest` | `immutable_image_reference` (`runtime.rs:626`) validates the string; the mapping splits it. A mismatch fails at the boundary rather than being coerced. |
| `RuntimeBindMount` (`Vec`, always len 1) | `ProjectMount` (singular) | A length other than 1 is a boundary failure. |
| `RuntimeVolume` | `Volume` | `writable` and per-volume `ownership` dropped: always true, always the request's. |
| `RuntimePort` | `PortMapping` | `host_address` dropped: always loopback (`policy.rs:163,451,462`). |
| `RuntimeResourceLimits` | `ResourceLimits` | Four `optional` scalars, one-to-one. |
| `RuntimeNetwork` | `Network` | `oneof`. |
| `RuntimeUser` | `User` | |
| `ContainerState` | `SandboxState` | |
| `RuntimeSandbox` | `Sandbox` | |
| `RecreateRequest` | `CreateContainerRequest` | `retained` is `repeated Resource`. |
| `CreateOutcome` / `CreateFailure` | `Created` / `CreateFailed` | §5. |
| `RemoveRequest` | `RemoveRequest` | Identities plus `OwnerLabels`. §8.1. |
| `ExecRequest`, `ExecInput`, `ExecOutput` | `ExecStart`, `ExecClientFrame`, `ExecServerFrame` | §8.3. |
| `RuntimeError` | `EngineError` | `code` is `RuntimeError::code()` verbatim. §5. |

## 10. The file

Field numbering, comments, and `reserved` blocks as they will be committed.

```proto
syntax = "proto3";

package arca.engine.v1;

// One method per method of Gas Can's RuntimeBackend trait. Eleven, and a superset
// of that trait is how Docker shapes return: a method here that no RuntimeBackend
// method needs is a method the consumer's policy compiler cannot gate.
service SandboxEngine {
  rpc Capabilities(CapabilitiesRequest) returns (CapabilitiesResponse);
  rpc Inspect(InspectRequest) returns (InspectResponse);
  rpc Create(CreateRequest) returns (CreateResponse);
  rpc PrepareImage(PrepareImageRequest) returns (PrepareImageResponse);
  rpc CreateContainer(CreateContainerRequest) returns (CreateResponse);
  rpc Start(StartRequest) returns (AckResponse);
  rpc Stop(StopRequest) returns (AckResponse);
  rpc Remove(RemoveRequest) returns (AckResponse);
  rpc Exec(stream ExecClientFrame) returns (stream ExecServerFrame);
  rpc Logs(LogsRequest) returns (stream LogsChunk);
  rpc ListResources(ListResourcesRequest) returns (ListResourcesResponse);
}
```

The full text is the file itself; it is not duplicated here, because two copies of
a contract drift and this document is not the binding artifact. The message
definitions follow the mapping in §9 and the constraints in §3, §5, §7 and §8.

**One naming choice came out of generating rather than out of designing.** The
result `oneof` is named `outcome`, not `result`. Named `result` it produced seven
`pub enum Result` types in the Rust output, shadowing `std::result::Result` at
every use site in the client that P5.2 has to write. A `oneof`'s *name* is not on
the wire — only its member fields are — so the rename is free now and a breaking
change later. VERIFIED 2026-08-07: 0 `pub enum Result`, 7 `pub enum Outcome` in
the regenerated output.

This is the §12 verification paying for itself. Reading the file would not have
found it.

## 11. Size gate — breached as written, met as intended

The roadmap sets one: *"past roughly 400 lines, something Docker-shaped has crept
back in"* (`2026-08-04-arca-integration-roadmap.md:255`), calibrated against Gas
Can's north-facing API.

**VERIFIED 2026-08-07, `wc -l` and `awk '!/^[[:space:]]*(\/\/|$)/'`:**

| | `arca/proto/arca/engine/v1/engine.proto` | `gascan/proto/gascan/v1/gascan.proto` |
|---|---|---|
| Raw lines | **483** | 240 |
| Declaration lines (no comments, no blanks) | **275** | 200 |
| Messages | 43 | 37 |
| Enums | 4 | 5 |
| RPCs | **11** | 14 |

**Stated plainly: the gate is breached on the metric as literally written, and met
on the metric it was trying to capture.** 208 of the 483 lines are comment or
blank. The calibrating file carries 40. Comparing a heavily-commented file to a
sparsely-commented one on raw line count measures commenting style, not surface —
this is the same failure mode as the exec-latency probe, and it is why both numbers
are reported rather than the flattering one.

On surface the engine proto is *smaller* than the API it was calibrated against:
11 RPCs against 14. Message count is higher (43 vs 37), and the cause is
identifiable rather than mysterious — **decision 2 costs roughly nine messages**
in result plumbing (`Ack`, `AckResponse`, `Absent`, `Created`, `CreateFailed`,
`ResourceList`, and the four response wrappers). That is the price of §5, paid
knowingly.

The contract's own calibration has also moved and should be corrected when §5 is
next touched: it says *"188 lines of proto and 12 RPCs"*
(`2026-08-04-sandbox-engine-contract.md:89`), against 240 and 14 today at `9a8efe3`.
The anchor the gate was derived from grew by 28%, which is itself the phenomenon
the gate exists to catch.

**Recommendation: keep the gate at 400 and restate it as declaration lines.** Not
raw lines, and not raised. At 275 there is real headroom, and the next person to
add a message will feel it.

## 12. Verification — run, not planned

A proto that has not been compiled is not defined. All VERIFIED 2026-08-07 against
`arca` branch `feat/engine-proto`, exit codes captured directly and not through a
pipe:

| Check | Command | Result |
|---|---|---|
| Compiles | `protoc --proto_path=proto --descriptor_set_out=… engine.proto` | **rc=0**, 6.6 KB descriptor |
| Swift server generates | `protoc … --swift_out=Visibility=Public --grpc-swift_out=Client=false,Server=true,Visibility=Public` | **rc=0**, `engine.pb.swift` 3,366 lines + `engine.grpc.swift` 487 |
| Rust client generates **and compiles** | `cargo build` on a scratch crate using `tonic_build` 0.12 / `prost_build` 0.13, matching `crates/gascan-proto/build.rs` | **rc=0**, 1,655 generated lines, `SandboxEngineClient` and `SandboxEngineServer` both present |

Plugin versions: `protoc` 35.1, `protoc-gen-swift` 1.38.1, `protoc-gen-grpc-swift`
**1.27.0** — which is exactly the version `arca/scripts/generate-grpc.sh:39`
requires to match Arca's `grpc-swift` dependency, so P3.2 inherits no version
conflict.

Generation was checked rather than assumed: the Rust output was grepped for the
client and server symbols, because a generator that silently emits an empty module
exits 0 too. `cargo build` compiling the generated module is the stronger witness
and is why the Rust arm builds rather than merely generates.

Neither generator is wired into a build. That is P3.2, and doing it here would make
P3.1 unreviewable.

## 13. Deliberately not done

- **No implementation, either side.** P3's exit says so explicitly.
- **U5 is not resolved.** §6.
- **U6 is not resolved.** Cross-sandbox channel validation belongs to P6.3; the
  peer-channel field block is `reserved`, not designed.
- **No codegen wiring.** P3.2.
- **No `buf` check.** §7 — inert without CI, and honesty about that is the point.
- **Arca's `SANDBOX_ENGINE_PIVOT.md` is not rewritten.** It predates the 2026-08-05
  reversal and still says `Sources/DockerAPI/` is deleted (`:57-66`, `:199`), which
  the reversal negates. Correcting it is real work and it is not P3.1's.
