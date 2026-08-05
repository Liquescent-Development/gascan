# Arca Integration Roadmap

Date: 2026-08-04
Revised: 2026-08-05 — P0 complete; P1 and P4 restructured
Status: Draft for review

**Revision note (2026-08-05).** Arca is no longer absorbed into Gas Can. It
survives as its own Docker-compatible project, and Gas Can consumes it as a
pinned source dependency across a protocol boundary. The reasoning is in
`docs/status/arca-integration-handoff.md` under "Decisions reversed 2026-08-05";
do not restore the older shape from the source specs, which predate it. P1 and P4
change substantially, P3 gains weight, and P2, P5, P6, P7 are unaffected in
substance.

Linearizes four specs into one dependency-ordered path. The specs say *what*; this
says *in what order*, *what runs in parallel*, and *what is not yet known*.

Source specs:

- `docs/superpowers/specs/2026-08-04-sandbox-engine-contract.md` — the boundary
- `docs/superpowers/specs/2026-08-04-arca-sandbox-backend.md` — Gas Can side
- `docs/superpowers/specs/2026-08-04-arca-monorepo-merge.md` — the merge
- `arca/Documentation/SANDBOX_ENGINE_PIVOT.md` — Arca side

Each spec numbers its own phases. Those numbers are superseded here.

## Destination

Gas Can ships one signed, notarized package containing its own binaries and a
bundled sandbox engine that Gas Can built itself. The engine speaks a narrow
purpose-built protocol, not Docker. Agents run in VM-isolated sandboxes with
policy-controlled egress and explicit peer channels. `build-manifest.json` attests
every shipped executable.

## Phase map

```
P0 ── P1 ── P2 ── P3 ─┬─ P4 ──┐
                      └─ P5 ──┴─ P6 ── P7
      P8 ────────────────────────────────►  (continuous from P1)
```

---

## P0 — Submodule currency

**Where:** `arca`, before the merge. **Blocks:** everything.

A 267-commit upstream reconciliation and a repository merge in one change is
unreviewable. If the reconciliation goes badly it must not also have destabilised
the migration.

| Step | Work | Status |
|---|---|---|
| P0.1 | Move the superproject pin off `f48a6c7`. | ✅ Superseded by P0.3, which pins `f02cdf9`. |
| P0.2 | Delete the committed `arca-services` ELF; build it in CI from `build.sh`. | ⚠️ **Partial.** Blob out of the index and `go.mod` tracked. "Build it in CI" has no CI to run in — `gh run list` returns empty and there is no `.github/` in the tree or its history. Deferred to P2.1. |
| P0.3 | Merge `upstream/main` (267 commits); resolve **U1** and **U2**; adapt the superproject to the resulting API drift. | ✅ `Vas-Solutus/arca-containerization#1`, `Vas-Solutus/arca#46`. Both open, both fast-forward. |
| P0.4 | Functional pass. | ✅ except k3d, which is now out of scope — see below. |

**Exit:** fork builds against current upstream, sandbox boots, all guest services
respond, no binary in git. **Met**, with P0.2's CI half carried to P2.1.

The blob's removal no longer needs to precede P1: under the 2026-08-05 revision
Arca's history is never imported into Gas Can's, so nothing of Arca's can enter
it. The reason to have done it stands on its own — the binary's provenance could
not be verified against the source beside it.

k3d was dropped rather than deferred. It is a Docker-compatibility concern, and
Docker compatibility now lives in Arca. The open question about fork commit
`502b715`'s motivating symptom belongs to Arca, not to a Gas Can gate.

---

## P1 — Arca as a pinned dependency

**Depends on:** P0. **Restructured 2026-08-05** — was "Monorepo merge".

The merge was never about source coupling; P3 always made the boundary a
protocol. It was about shipping: Gas Can ships one signed package containing an
engine it built itself, and `build-manifest.json` must attest every shipped
executable. A pinned source dependency satisfies that without importing anything.

| Step | Work |
|---|---|
| P1.1 | Add Arca to Gas Can as a source dependency pinned by commit. The pin is the provenance; record it in `build-manifest.json`. |
| P1.2 | Teach Gas Can's build to build the engine targets only — `Sources/DockerAPI` excluded by target selection. **Amended 2026-08-05**, see below. |
| P1.3 | Nothing to carry. Arca keeps its own containerization submodule; Gas Can has no relationship with the fork. |
| P1.4 | **Added 2026-08-05. DONE 2026-08-05.** Make the pinned Arca cold-buildable. Replace `swift-ip` with an internal IPv4/CIDR type in Arca, re-tag, bump the pin. ~~**Blocks P2.**~~ **P2 unblocked** — the gate is green. See below. |

~~**Exit:** Gas Can's pipeline produces an engine binary from a pinned Arca commit,
and the manifest attests it.~~

**Exit, amended 2026-08-05** — design in
`docs/superpowers/specs/2026-08-05-arca-engine-pin-design.md` §3. The original
exit conflated two claims that cannot land together yet:

| Claim | P1 | Deferred to |
|---|---|---|
| Pipeline builds pinned Arca source | ✅ | |
| Pin recorded as provenance | ✅ | |
| Produces an engine **binary** | | **P5.1** — none exists |
| That binary is Docker-free | | **P4.3** |

The old shape — `git subtree add` preserving history, then relocation — was
justified by Arca ceasing to exist, which made Gas Can the only place provenance
could live. Arca now survives and stays reachable, so history lookups go there.
A full-repo subtree would also have imported `Sources/DockerAPI/` into Gas Can's
permanent history purely so P4.1 could delete it — the same mistake P0.2 avoided
with the `arca-services` blob.

Costs nothing extra in build capability: P2.1 already commits Gas Can's CI to
orchestrating Swift.

### P1.4 — the pin is not cold-buildable (discovered 2026-08-05, **RESOLVED 2026-08-05**)

**RESOLVED.** VERIFIED: engine-pin run
`https://github.com/Liquescent-Development/gascan/actions/runs/31055299650`,
`conclusion=success`, `headSha=f562e6e`, on a hosted `macos-26` runner — the
gate's first green, after **four** consecutive failures (`31038778615` `12b4a91`,
`31039127696` `afb04f2`, `31042100578` `8be4ec6`, `31042404662` `58ae69f`).
Arca `main` is now
`d66c320c09e1dfc4f37aafa1fb27e36aa5cabe5d`, tagged `gascan-engine-ip-internal`,
and Gas Can's pin points at it. Six pins dropped from Arca's 38. Full record in
`docs/status/arca-integration-handoff.md` under "P1.4 complete — 2026-08-05";
design in `docs/superpowers/specs/2026-08-05-arca-internal-ip-type-design.md`.

The replacement is differentially VERIFIED against `swift-ip` 0.3.3 across
18,580,063 vectors with 0 mismatches — a large sample, not an exhaustive proof —
with the harness itself validated by two independent negative controls. Sampled
domains and the thin spot are named in the handoff.

**The measurement table below understated the surface.** It listed five call
shapes; characterization found ten, the omissions being `Block.contains(_:)`,
`Block.range`'s two bounds, `String(describing:)` and `Equatable`.
`String(describing:)` mattered most — its output is persisted to SQLite and
returned on the wire, so a divergence would have corrupted stored state silently
rather than failing loudly. The "~150 lines" estimate held; the churn estimate
was never tested, because the re-pin path was not taken.

The rest of this section is preserved as written, for the reasoning that led to
the decision.

Found by P1.2's own CI gate, on its first run. This is the argument for the gate,
paid back immediately.

**VERIFIED.** PR #44, run `31038778615`, conclusion `failure`. Arca's
`Package.resolved` at `gascan-engine-baseline` pins `swift-grammar` at
`0dac977b…` and `swift-hash` at `ea0b9fc3…`. In a **fresh clone** of either
upstream, `git cat-file -t` returns `fatal: could not get object info` — the
commits do not exist. A cold re-resolve of `swift-ip` at `exact: "0.3.3"` fails
differently and explains why: `swift-json` 1.2.0 now resolves to `efeea1d4…`
against SwiftPM's fingerprint record of `a3493697…`. The `tayloraswift` family
rewrote history and migrated org mid-`0.3.x` — `tayloraswift/*` → `rarestype/*`,
with `swift-grammar` → `gram` and `swift-hash` → `h`.

**Where the objects survive.** VERIFIED 2026-08-05: the vanished commits existed
only in `~/Library/Caches/org.swift.swiftpm/repositories/swift-grammar-186ad640`
and were copied to `~/code/vendor-mirrors/` the same day, both verified present
with `git cat-file -t`.

~~This is decaying, not merely broken: a cache clear would make
`gascan-engine-baseline` permanently unbuildable by anyone, and off-machine
mirrors are needed.~~ **Overstated, corrected same day.** P1.4 removes these
packages from Arca's graph, so the new tag is cold-buildable and the baseline is
superseded rather than preserved; a mirror is inert without an explicit
`swift package config set-mirror`, since `Package.resolved` records the upstream
URL; and no release has shipped against this pin. The local copies are sufficient
and no further durability work is warranted.

**Decision: replace, do not re-pin.** `swift-ip` 0.3.10 does resolve cold
(VERIFIED, exit 0 with isolated SwiftPM state), so a version bump would turn the
gate green. It was rejected because it re-enters the same lottery and does
nothing about the decay.

The measurements that decided it:

| | Re-pin to 0.3.10 | Replace |
|---|---|---|
| Arca's usage surface | — | 2 files; `IP.V4(String)`, `IP.V4(value:)`, `.value`, `IP.Block<IP.V4>(String)`, `.base` |
| Churn to absorb | 46 insertions / 88 deletions in the two files Arca uses, across 7 pre-1.0 releases | ~150 lines of new pure-function code |
| Pins removed from Arca's 38 | 0 | **6** — `swift-ip`, `swift-bson`, `swift-json`, `swift-grammar`, `swift-hash`, `swift-unixtime` |
| Recurrence risk | unchanged | eliminated |

`IP.V4`'s stored property is now `storage: UInt32` in 0.3.10 while Arca reads
`.value` in 8 places, so even the cheap path is not free.

Nothing else in Arca reaches those 6 packages — its direct dependencies are
containerization, swift-nio, swift-log, swift-argument-parser, grpc-swift,
SQLite.swift, SWCompression and swift-ip.

**Blocks P2**, whose deliverable is a working pipeline: P2.1 cannot stand up CI
around an engine build that fails on every runner, and P2.2 cannot attest a pin
nobody can rebuild. Does **not** block P3's protocol work, nor Arca-side work
like P4.3 and P5.1, which still build locally off the warm cache.

---

## P2 — Build consolidation

~~**Depends on:** P1, **including P1.4** — see above.~~
**Dependency satisfied 2026-08-05.** P1.4 is done and the engine-pin gate is
green, so P2 is unblocked. P1 remains partial by necessity — no engine *binary*
is produced, because none exists to build; that half is booked against P5.1 and
P4.3, per the amended exit table above.

| Step | Work |
|---|---|
| P2.1 | One CI orchestrating Swift, Rust, Go, protobuf codegen. Path-based triggers from the start — resolve **U3**. |
| P2.2 | Extend `build-manifest.json` to cover engine and guest binaries. |

**Exit:** one pipeline; the manifest attests every shipped executable. This is the
payoff that justified merging.

---

## P3 — Protocol

**Depends on:** P1. **Fan-out point.**

| Step | Work |
|---|---|
| P3.1 | Define the engine proto. Derived from `RuntimeBackend`; constrained by contract §4 (what must be inexpressible) and §5 (what must be expressible). Resolve **U4**. |
| P3.2 | Codegen wired both sides — Swift server, Rust client. |
| P3.3 | **Added 2026-08-05.** Publish and version the proto as Arca's contract to its consumers. It lives in Arca, per "Arca owns the wire protocol". |

**Exit:** proto exists, both sides generate, nothing implements it yet.

**Weight increased 2026-08-05.** With Arca surviving as an independent project,
the proto is a real published contract with more than one consumer over time —
Gas Can now, a Docker-compatible Arca later — rather than an internal detail of a
merged tree. It is the only thing holding the two together, so it carries the
compatibility burden alone.

Machinery already exists on both sides: Arca has `scripts/generate-grpc.sh` and
protos under `Sources/ContainerBridge/proto/`; Gas Can has a `gascan-proto` crate.

Arca cannot publish a Rust crate — it is Swift. `gascan-arca` (P5.2) is Gas Can's
client and lives in Gas Can. FFI was rejected: it would pull a daemon needing
virtualization entitlements and managing VMs into Gas Can's address space.

Size gate: past roughly 400 lines, something Docker-shaped has crept back in.

---

## P4 — Docker removal

**Depends on:** P3. **Parallel with P5.**

**Restructured 2026-08-05.** Nothing is deleted from Arca. Docker support stays
there; the engine build excludes it.

| Step | Work |
|---|---|
| P4.1 | **Exclude**, do not delete, `Sources/DockerAPI/`. It is already its own SwiftPM target, so this is target selection in P1.2. |
| P4.2 | Exclude host-side buildx integration. Agents get `dockerd` in-guest. |
| P4.3 | Add a seam in Arca — **target split**, decided 2026-08-05 — keeping restart policies, health checks and registry access out of the engine build. Requires a change **in Arca**, since these live inside `ContainerBridge`, the target the engine needs. |

**Exit:** the shipped engine carries no concept the protocol cannot express.
Note the narrowing: this is now a property of Gas Can's build output, not of
Arca. Arca deliberately retains all of it.

Without P4.3's seam, the security property would rest entirely on the protocol
boundary — defensible, since code no proto method reaches cannot be invoked, but
it forces the threat model to argue from unreachability rather than absence. The
seam was chosen over accepting that.

### Seam shape: target split, not a build flag (decided 2026-08-05)

Move Docker semantics into a separate SwiftPM target depending on the engine
target — `ContainerBridgeDocker → ContainerBridge`, arrow pointing at the engine.
Gas Can depends only on `ContainerBridge`. Do it incrementally: create the target,
move the clearly-Docker pieces first (restart policy, health-check wiring,
`autoRemove`), and let the engine target stay temporarily fat.

**A build flag was rejected primarily because Arca has no CI.** `gh run list`
returns empty and there is no `.github/` in the tree or its history, so an
engine-only `#if` configuration would be built for the first time by Gas Can's
pipeline — a different repository, at pin-bump time, where the breakage presents
as Gas Can's build failing. A target split has one configuration; both targets
compile on every plain `swift build`.

Secondary reasons:

- The seam exists to support a **security** claim — the engine cannot be asked to
  reach a registry. A compiler-enforced dependency edge supports that in a threat
  model; a remembered `#if` does not, and nothing prevents future Docker code
  being added outside a guard.
- Swift `#if` around a declaration forces the same guard onto every caller, which
  spreads through a file with 38 public entry points and heavy internal coupling.
- The dependency direction encodes the right model: Docker becomes one front-end
  *consuming* the engine, with Gas Can's proto as another. That extends the
  existing grain — `DockerAPI` is already its own target, `ImageManager` and
  `HealthChecker` already separate files.

The hard part is unchanged either way: deciding the engine/Docker boundary inside
`ContainerManager.swift`. A flag only defers that decision while accumulating
conditionals.

### Sequencing: P1.2 partially depends on P4.3

~~P1.2 ("build the engine targets only") works **today** in partial form, because
`Sources/DockerAPI` is already an independent target. A genuinely engine-only
build additionally needs P4.3's split, which the phase map currently places after
P3. So either P1.2 lands partial and tightens when P4.3 arrives, or P4.3 moves
earlier. Do not plan around the current ordering without resolving this.~~

**SUPERSEDED 2026-08-05** — resolved in
`docs/superpowers/specs/2026-08-05-arca-engine-pin-design.md` §2.2, §2.3 and §7.
The dependency is on **P5.1, not P4.3**, and moving P4.3 earlier would not
unblock P1.2.

VERIFIED by `swift package describe --type json` in `~/code/arca` (exit 0),
anchored to the pin: `git rev-parse b20be7c^{tree} 9c2db5a^{tree}` both report
`3139b8398f203c40d2fbe309ba7fb15d4c7094b0`.

- `DockerAPI` is genuinely its own target, but the only shippable executable
  reaches it transitively: `Arca → ArcaDaemon → DockerAPI`. Target selection buys
  nothing at the executable level. The claim held only for a *library* build.
- Arca exposes **no library products** — the only two products are the `Arca` and
  `ArcaTestHelper` executables. So Gas Can also cannot consume `ContainerBridge`
  as a SwiftPM dependency.
- **There is no engine executable at all.** `Sources/ArcaDaemon/` is entirely the
  Docker HTTP server, and the only protos under `Sources/ContainerBridge/proto/`
  are `tapforwarder.proto` and `wireguard.proto`, both guest-facing. The engine
  service is P5.1.

P1.2 therefore lands **partial by necessity**: the pipeline builds the pinned
source and the manifest attests the pin, while the binary half is booked against
P5.1 (it must exist) and P4.3 (it must be Docker-free).

**P4.3's cost estimate needs re-deriving.** The original "4,518 lines with Docker
concepts woven through" overstates the interleaving of the three concerns it
names. Spot check 2026-08-05 against `Sources/ContainerBridge/ContainerManager.swift`
— five greps, not a full analysis: `registry` and `pullImage` appear zero times
(registry work lives in `ImageManager.swift`, already separate); `healthCheck` 12
times with `HealthChecker.swift` already separate; `restartPolicy` 9;
`autoRemove` 1. The file is genuinely 4,528 lines with 38 public entry points, so
its size is real — but the seam is about which entry points the engine build
exposes, not about untangling registry code.

---

## P5 — Engine service and backend

**Depends on:** P3. **Parallel with P4.**

| Step | Work |
|---|---|
| P5.1 | Implement the engine service in Swift against existing ContainerBridge machinery. Do not wait on P4. |
| P5.2 | `gascan-arca` crate implementing `RuntimeBackend` over gRPC. Preserve a transport-trait seam so tests need no live engine. |
| P5.3 | Extract the conformance suite from `fake_runtime.rs`; run against fake, apple, and arca backends. |
| P5.4 | Resolve **U5** — how image digests reach the engine without registry access. |

**Exit:** `gascan-arca` passes conformance and existing `gascan-e2e`.

---

## P6 — Network model

**Depends on:** P5.

| Step | Work |
|---|---|
| P6.1 | Narrow the network stack: remove multi-network topology, connect/disconnect, service-discovery DNS. Keep WireGuard, repurpose the resolver as a policy point. |
| P6.2 | Egress policy engine: allowlist API, resolver pins names into nftables sets, connection events streamed to host. |
| P6.3 | Peer channels: declaration, translation to WireGuard peer config plus nftables. Resolve **U6**. |
| P6.4 | Gas Can side: `NetworkMode` third variant, channel manifest field, `PolicyCompiler` gating, new `RuntimeCapabilities` fields, `doctor` facts. |

**Exit:** a sandbox can be offline, egress-policied, or open; channels work; `doctor`
proves each by dumping live nftables rather than trusting a flag.

P6.2 is the differentiator — the thing Apple's `container` structurally cannot do.

---

## P7 — Cutover

**Depends on:** P6, P2.

| Step | Work |
|---|---|
| P7.1 | Resolve **U7** (M1/M2 measurements). |
| P7.2 | Flip the default backend to `gascan-arca`. |
| P7.3 | Bundle the engine in the `.pkg`. |

**Exit gates**, all required: conformance passes; `gascan-e2e` passes on Arca;
`doctor` reports every capability proven rather than unverified; M1 and M2 measured.

`gascan-apple` stays after cutover. Two implementations keep `RuntimeBackend` honest.

---

## P8 — Fork reduction

**Continuous from P1.** Not on the critical path; unbounded cost if never started.

**Revised 2026-08-05.** This is now Arca's work, not Gas Can's — Arca owns the
containerization submodule and Gas Can has no relationship with the fork. Gas Can
benefits indirectly, through the Arca commit it pins.

One durability point survives the reversal and arguably matters more because of
it. The fork's history — the 267-commit merge, `a1085d8`, `f02cdf9`, and every
resolution the rationale doc justifies — exists in exactly one repository,
`Vas-Solutus/arca-containerization`. Neither Arca nor Gas Can can reconstruct the
guest without it. P8.3 ends that exposure by consuming `apple/containerization`
as an ordinary dependency; until then, that repository is load-bearing for both
projects. Tag it.

| Step | Work |
|---|---|
| P8.1 | Move `arca-services` out of the containerization tree into `guest/`. Nothing binds it there but history. |
| P8.2 | Triage the 16 modified files: upstream it, express it through an extension point, or document it as a permanent patch with a reason. Resolve **U8**. |
| P8.3 | Consume `apple/containerization` as an ordinary SwiftPM dependency at a release tag. |

**Exit:** no fork. 267 commits per 8 months stops being a recurring tax.

---

## Known unknowns

Each blocks a specific step. None should be guessed at.

**U1 — Where upstream moved the deleted files' responsibilities.**
`Server+GRPC.swift`, `ManagedProcess.swift`, and `RuncProcess.swift` carry Arca
modifications and no longer exist upstream; all three are under
`vminitd/Sources/vminitd/`, so upstream restructured vminitd. Modify/delete
conflicts have no mechanical resolution.
*Resolve by:* reading upstream's vminitd restructuring commits before starting the
merge. *Blocks:* P0.3.

**U2 — Whether any Arca modification is no longer viable.**
Restructured upstream code may have removed the seam a modification depended on.
*Resolve by:* falls out of P0.3. *Blocks:* P0 exit.

**U3 — Consolidated CI wall time.**
Determines whether path filters are mandatory or merely nice.
*Resolve by:* measuring after P1.1. *Blocks:* P2.1 design.

**U4 — The engine protocol's actual shape.**
P3 is design work, not transcription. `RuntimeBackend` gives the method list; the
message types, streaming shape for exec and logs, and capability encoding are open.
*Resolve by:* P3.1 design. *Blocks:* P4, P5.

**U5 — How image digests reach the engine.**
Contract §4 forbids registry access from the engine and requires a digest "the
engine already holds." It does not say how it comes to hold one. Gas Can has
offline image bundle machinery (`docs/evidence/offline-image-build.md`,
`docs/evidence/connected-workspace-image.md`) that must reconcile with this.
**This is a genuine gap in the current specs, not just an implementation detail.**
*Resolve by:* P5.4 design, and fold the answer back into the contract. *Blocks:*
P5 exit.

**U6 — Cross-sandbox channel validation.**
A channel names another sandbox. `CreateRequest` is sealed and built by
`PolicyCompiler` from one manifest; validating that a channel target exists, is
owned by the same operator, and consents is a cross-sandbox concern the sealed
single-manifest model has no place for today.
**Also a genuine spec gap.**
*Resolve by:* P6.3 design. *Blocks:* P6.3.

**U7 — Recreate cost (M1) and provisioning cost (M2).**
Channel grants require a recreate. If that is seconds, the ergonomics are
academic; if minutes, `apply` deserves an in-place path and P6.3's design reopens.
M2 isolates the provisioning share, since only that part is optimisable by the
engine.
*Resolve by:* measurement, obtainable any time after P5. *Blocks:* P7.1.

**U8 — Fork-reduction feasibility.**
How many of the 16 modified files are upstreamable, how many can become extension
points, how many are permanent.
*Resolve by:* P8.2 triage. *Blocks:* P8.3.

**U9 — Whether the engine daemon can run under App Sandbox.**
`Arca.entitlements` sets `com.apple.security.app-sandbox` to `false`, commented as
a development decision, and it ships that way. A narrower engine needs permission
to do less, so the pivot is when this becomes tractable — but whether App Sandbox
composes with `com.apple.security.virtualization` for a long-lived daemon is
untested.
*Resolve by:* a spike, any time after P4. *Blocks:* nothing; improves the second
barrier in the threat model.

## Sequencing notes

- ~~**P0.1 and P0.2 are do-now.**~~ Done 2026-08-05, except P0.2's CI half, which
  moved to P2.1 because no CI exists to run it in.
- **P1 no longer gates on repository surgery.** It is a pin plus a build change,
  so P3 can start as soon as P1.1 lands rather than waiting on a relocation.
- **P3 is the fan-out.** Before it, work is serial. After it, Swift removal (P4)
  and the backend (P5) proceed independently.
- **P5.1 must not wait on P4.** Implementing the engine service against existing
  machinery unblocks the Rust side; de-Dockering can follow.
- **P8 starts early and finishes late.** Deferring it entirely is the one choice
  here with unbounded cost.
- **U5 and U6 are spec gaps**, discovered while sequencing. Both need design work
  folded back into the contract, not just implementation.
