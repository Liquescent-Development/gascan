# Arca Sandbox Backend

Date: 2026-08-04
Status: Draft for review

The Gas Can side of the Arca pivot: a second real `RuntimeBackend`, the network
model that Arca's capabilities unlock, and the conformance suite that keeps both
honest.

Governing contract: `docs/superpowers/specs/2026-08-04-sandbox-engine-contract.md`.
Arca side: `arca/Documentation/SANDBOX_ENGINE_PIVOT.md`.

## 1. `gascan-arca`

A new crate implementing `RuntimeBackend` (`crates/gascan-core/src/runtime.rs:717`)
against Arca's gRPC service.

`gascan-apple` is the shape to follow but not to copy. It is 2,218 lines across
eight modules, and its structure reflects driving a CLI: `AppleCommandBuilder`
constructs argv, `CommandRunner` (`crates/gascan-apple/src/command.rs:41`) abstracts
process execution for testability, `translate.rs` (448 lines) maps between Gas Can
types and CLI flags, and `inspect.rs` parses JSON output back.

Talking to a typed gRPC service removes most of that. There is no argv to build,
no output to parse, and no version-specific flag translation. What must be
preserved is the *seam*: `gascan-apple` is testable without Apple `container`
installed because `CommandRunner` is a trait. `gascan-arca` needs the equivalent —
a transport trait — so its tests do not require a running Arca.

Expected shape: a transport trait, a thin type mapping, and the ~~ten~~ **eleven**
`RuntimeBackend` methods. Materially smaller than `gascan-apple`.

**Corrected 2026-08-07.** `runtime.rs:991-1008` lists eleven: `capabilities`,
`inspect`, `create`, `prepare_image`, `create_container`, `start`, `stop`,
`remove`, `exec`, `logs`, `list_resources`. The one the count dropped is
`prepare_image` — the method that would grow a registry client if nobody were
watching it (contract §4). The type mapping P5.2 needs is written out in
`2026-08-07-arca-engine-proto-design.md` §9 so it is not re-derived.

## 2. Network model

Today `NetworkMode` (`crates/gascan-core/src/manifest.rs:128`) is binary —
`Networked | Offline`, defaulting to `Offline`. Arca's egress policy engine and
peer channels are two different capabilities, and they are orthogonal: a sandbox
can be egress-policied *and* have peer channels. They must not be one enum.

**Isolation axis** — `NetworkMode` gains a middle variant:

- `Offline` (default, unchanged) — no egress. Fail-closed.
- `EgressPolicy` — egress permitted only to declared destinations.
- `Networked` (unchanged) — unrestricted egress.

**Membership axis** — a separate manifest field declaring peer channels: this
sandbox may reach that sandbox on these ports. Empty by default.

Both changes are additive. Existing manifests deserialize unchanged: a new enum
variant nobody names is inert, and a channel list defaults to empty. That is a
convenient accident rather than a compatibility goal.

### 2.1 Capability gating

The existing pattern is the model. `PolicyCompiler::compile` refuses `Offline`
unless the backend reports `NetworkIsolation::Proven` (`crates/gascan-core/src/policy.rs:169`),
and refuses published ports on an offline sandbox (`policy.rs:178`), with
`PolicyError::OfflineUnavailable` and `OfflinePortsForbidden` (`policy.rs:255`).

The new modes gate identically. `RuntimeCapabilities` (`runtime.rs:37`) gains:

- `egress_policy: NetworkIsolation` — reusing the existing `Proven | Unsupported |
  Unverified` triple (`runtime.rs:30`) rather than inventing a parallel type.
- `peer_channels: bool`.

`gascan-apple` reports `Unsupported` and `false` for both. That is not a
deficiency to paper over — it is the honest answer, and it is precisely the gap
that justifies bundling Arca. A manifest requesting egress policy on the Apple
backend must fail at compile time with a clear reason, never degrade to
`Networked`.

New `PolicyError` variants: egress policy unavailable, peer channels unavailable,
channel target not resolvable, and channel declared alongside `Offline` (which is
contradictory — an offline sandbox reaching a peer is not offline).

### 2.2 Channels are declared, never mutated

Contract §6.2 states the property: no runtime grant path exists, so no running
agent can reach or influence one. Gas Can enforces this by having no RPC for it —
`proto/gascan/v1/gascan.proto` gains no channel-management method.

The user surface is `apply`. The user edits the manifest and applies; whether a
topology change requires a recreate is the reconciler's decision, not something
the user must know.

Being accurate about what exists: `crates/gascand/src/reconcile.rs` is 15 lines of
types only — `ReconcileFinding` covering unknown-owned, unknown-unowned,
missing-owned, and ownership-mismatch, plus `ReconcileReport`. It is drift
detection, not a reconciler. Topology reconciliation is new work. But
`DesiredState`/`ActualState` are real and drive `up_inner` in
`crates/gascand/src/service.rs`, and provisioning is content-hashed through
`AppliedState`'s tool hash and setup SHA-256 (`crates/gascan-core/src/provision.rs`),
so the declarative shape is already there.

Keeping the surface declarative leaves room to optimise: if a safe in-place peer
addition is ever found, `apply` gets faster and no interface changes.

**Revoke asymmetry** (contract §6.3): granting escalates privilege, revoking never
does. v1 ships no revoke path because `gascan down <sandbox>` is a larger hammer
that certainly works. Recorded so that the next person to touch channel management
does not add a symmetric add/remove pair by reflex.

## 3. Doctor

`DoctorFacts` (`crates/gascan-core/src/doctor.rs`) gains one fact per new
capability — egress policy and peer channels — following the existing one-fact-per-
capability discipline alongside `offline`, `loopback_publish`, and
`resource_limits`.

With Arca these can be proven more strongly than with Apple `container`. Apple's
backend infers isolation from CLI flags; Arca's guest exposes `DumpNftables`, so
the fact can be established by dumping the live ruleset and asserting on it rather
than by trusting a flag was honoured. `NetworkIsolation::Proven` should mean
proven.

## 4. Conformance suite

`crates/gascan-core/src/fake_runtime.rs` is 903 lines and already functions as an
executable specification of `RuntimeBackend`. It becomes the binding contract
between the repositories.

**Work:** extract the behavioural assertions from the fake into a suite that runs
against *any* `RuntimeBackend` implementation, then run it against all three —
`fake_runtime`, `gascan-apple`, and `gascan-arca`. Capability-gated cases skip
where a backend honestly reports the capability absent, which means the suite also
tests that capability reporting is truthful.

**Cross-repo execution:** Arca's CI runs this suite against a built Arca. That
requires a Rust toolchain in Arca's CI, which is a smaller cost than the
alternative of maintaining language-neutral protocol test vectors. The point is
that a contract violation fails Arca's build rather than a document review — the
document drifts, the suite cannot.

## 5. Migration

`gascan-apple` carries load throughout. It is the safety net, which is why no
Arca-side Docker surface needs retaining (contract §8).

Flip criteria — all must hold before `gascan-arca` becomes default:

1. Conformance suite passes against a built Arca.
2. Existing `gascan-e2e` coverage passes on the Arca backend.
3. `doctor` reports every capability as proven, not unverified.
4. M1 and M2 (§6) are measured, not estimated.

`gascan-apple` stays after the flip. Two implementations are what keep
`RuntimeBackend` honest as an abstraction; removing it leaves a one-implementation
trait, which is a shape that rots.

## 6. Measurements

**M1 — recreate cost.** Channel grants require a recreate. A recreate loses
process state and everything outside the project root, and re-runs provisioning. If
this is seconds, §2.2's ergonomics are a non-issue; if it is minutes, `apply`
deserves an in-place path and the design conversation reopens.

**M2 — provisioning cost, isolated from VM boot.** Only one of the two is
optimisable by Arca, so the split matters for knowing where to spend.

Both are numbers to obtain, not to reason about. Neither blocks starting §1.

## 7. Non-goals

- A Docker-API backend. A generic Docker runtime would report
  `NetworkIsolation::Unsupported` and be useless for Gas Can's default mode;
  widening the install base to runtimes that cannot provide the guarantee
  undercuts the product.
- Removing `gascan-apple`.
- Runtime channel mutation.
- Sidecar orchestration. Project services run inside the sandbox on the guest's
  own Docker (contract §6.1); they are not a Gas Can networking feature.
