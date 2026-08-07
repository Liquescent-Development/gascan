# Sandbox Engine Contract — Gas Can ↔ Arca

Date: 2026-08-04
Status: Draft for review
Scope: the boundary between Gas Can and Arca. Neither side's internals.

Companion documents:

- Arca side: `arca/Documentation/SANDBOX_ENGINE_PIVOT.md`
- Gas Can side: `docs/superpowers/specs/2026-08-04-arca-sandbox-backend.md`

Read this one first. It constrains both.

## 1. What changes

Arca stops being a Docker-compatible container runtime and becomes Gas Can's
sandbox engine. Gas Can stops requiring a separately-installed Apple `container`
and bundles Arca instead.

Today Gas Can's README requires the user to install Apple `container` 1.1.0 and
states that Gas Can does not bundle it. `crates/gascan-apple/src/backend.rs`
drives that runtime by shelling out to its CLI through `CommandRunner`. That
backend stays — see §8 — but it stops being the shipping default.

## 2. Why the boundary is shaped this way

Gas Can's existing design is built around provable narrowness. `CreateRequest`
is sealed: `PolicyCompiler` is its only construction path, enforced by
`compile_fail` doctests in `crates/gascan-core/src/runtime.rs`. `NetworkMode`
defaults to `Offline` (`crates/gascan-core/src/manifest.rs:128`). The workspace
sets `unsafe_code = "forbid"`. `doctor` reports one fact per capability, and
`NetworkIsolation::Proven` (`runtime.rs:30`) gates offline mode.

A Docker-compatible engine is philosophically opposed to that. The Docker API is
designed to express everything a container can do, so every endpoint it exposes
is a way to say something `PolicyCompiler` was built to make unsayable. An engine
speaking Docker on a socket is a policy bypass surface sitting beside the policy
gate.

The contract below therefore has one governing rule.

**The protocol is the policy.** Anything not expressible in the wire format
cannot be requested, and therefore never needs validating, logging, or defending.

## 3. Ownership

| Artifact | Owner | Rationale |
|---|---|---|
| The wire protocol (`.proto`) | **Arca** | Arca is the server and versions independently. A server's wire behaviour must not be defined in a repo it does not control. |
| Behavioural specification | **Gas Can** | `crates/gascan-core/src/fake_runtime.rs` (903 lines) is already an executable specification of `RuntimeBackend`. It becomes the conformance suite. |
| Bundle composition and release | **Gas Can** | Gas Can ships the `.pkg` containing both. |

This split is deliberate. Arca decides what it can be *asked*; Gas Can decides
what a correct answer *is*.

### 3.1 Anti-drift

A document describing two repositories drifts, because the other repository's
changes never touch it. The binding mechanism is therefore not this document.

**Gas Can publishes a conformance suite. Arca's CI runs it against a built
Arca.** A contract violation fails Arca's build, not a doc review. This document
explains the contract; the suite enforces it.

## 4. What the protocol must not be able to express

These are constraints on the wire format itself, not validations applied to it.
If a field does not exist, no code path needs to reject it.

| Not expressible | Instead |
|---|---|
| An arbitrary host path to mount | A project root, and nothing else. Volumes are engine-owned and referenced by name. |
| An arbitrary image reference to pull | A content digest the engine already holds. Absent it, the request fails. This removes the registry client, registry auth, and Keychain access from the engine entirely. |
| A bind address for a published port | Loopback is implied. `RuntimeCapabilities.loopback_publish` (`runtime.rs:43`) already names the only supported case. |
| A network topology, or any mutation of one | Peer channels, declared at create. See §6. |
| A change to a running sandbox's topology | No such RPC exists. See §6.2. |

## 5. What the protocol must express

Derived from `RuntimeBackend` (`crates/gascan-core/src/runtime.rs:717`) and
`RuntimeCapabilities` (`runtime.rs:37`). The engine's service is a superset of
nothing else.

Lifecycle: create, start, stop, remove, inspect, list owned resources.
Interaction: exec with TTY and signal forwarding, logs with a since-timestamp.
Declaration: capabilities, including version, bind mounts, named volumes, tty,
signals, loopback publish, resource limits, and network isolation.

~~For calibration: Gas Can's entire north-facing API is 188 lines of proto and 12
RPCs (`proto/gascan/v1/gascan.proto`). The engine protocol should land in the
same neighbourhood. If it grows past roughly double that, something Docker-shaped
has crept back in.~~

**Calibration refreshed 2026-08-07.** `proto/gascan/v1/gascan.proto` is **240 lines
with 14 RPCs** at `9a8efe3` (VERIFIED, `wc -l`), not 188 and 12 — it grew 28% while
the figure quoted here did not. The intent is unchanged, but the metric is: compare
**declaration lines**, excluding comments and blanks, because raw line count
compares commenting style rather than surface. On that metric the north-facing API
is 200 lines and the delivered engine proto is 275, with 11 RPCs against 14. See
`2026-08-07-arca-engine-proto-design.md` §11.

## 6. Peer channels

Sandboxes may need to reach each other — agent-to-agent collaboration, with each
agent in its own sandbox.

### 6.1 What does *not* need a channel

A sidecar service for the project under development — a Postgres for a webapp —
runs **inside** the agent's own sandbox, on the guest's own Docker. Same trust
domain, same lifetime, no cross-VM networking, no service discovery.

This is the case that would otherwise have justified a general container network,
and it resolves in-guest. What remains is the genuinely different case: two
separate trust domains needing a narrow, explicit, mediated path.

### 6.2 The channel model

A channel is declared at sandbox creation: sandbox A may reach sandbox B on a
named set of ports. Arca translates that into WireGuard peer configuration plus
nftables rules. It is immutable for the sandbox's lifetime.

**Granting a channel requires recreating the sandbox, and this is a security
property rather than a simplification.** No runtime grant RPC exists, so there is
no surface a running agent can reach or influence. Were one to exist, it would be
a privilege escalation path: a compromised agent A persuades the orchestrator to
open a channel to B, and now A can attack B. Removing the path is stronger than
guarding it.

This mirrors `PolicyCompiler` being the only construction path for
`CreateRequest`. It must be stated as intent, or a later change will "improve" it
by adding the missing API.

### 6.3 The revoke asymmetry

Granting escalates privilege; revoking never does. Revocation is therefore safe
to expose at runtime in a way that granting is not.

v1 ships no revoke RPC, because `gascan down <sandbox>` is a larger hammer that
certainly works and is the kill switch. The asymmetry is recorded here because
the reflex, when channel management is next touched, is to add a symmetric
add/remove pair — and the two directions are not symmetric.

### 6.4 WireGuard is the right substrate

A peer reaches only peers it holds a key and an allowed-ips entry for. East-west
policy is expressed by which peers were configured, not by firewall rules someone
remembered to write. Docker's default bridge has the opposite property: everything
on it can reach everything. Arca's existing `WireGuardNetworkBackend.swift` is
already the better shape; the work is narrowing it, not replacing it.

## 7. Threat model

**Adversary.** Code running inside a sandbox. Assume arbitrary execution — the
agent may be prompt-injected, may run hostile dependencies, may actively try to
escape.

**Assets.** The host filesystem beyond the mounted project root, host credentials,
the host's network position, and *other sandboxes*.

**Barriers, in order:**

1. **The VM boundary.** Primary and load-bearing. Everything else is depth.
2. **The protocol's inexpressibility** (§4). What cannot be requested cannot be
   misused.
3. **Egress policy**, enforced in-guest at the packet layer.
4. **Peer isolation**, deny-by-default via WireGuard peer configuration (§6.4).

Two properties distinguish this from a single-sandbox model:

**East-west is an attack path.** An agent that goes wild can attack a neighbouring
agent's sandbox. §6.2's immutability and §6.4's deny-by-default peer model are the
controls. Every channel that exists was declared by a human editing a manifest;
none can come into being at runtime.

**The sidecar case does not widen the boundary.** Because sidecars run in-guest
(§6.1), a project needing a database does not put a second VM on a shared network.
The blast radius of a compromised sidecar is the sandbox that ran it.

**Not defended against, by decision:** CPU exhaustion by a runaway sandbox. macOS
provides no `cgroup cpu.max` equivalent and no hard thread affinity on Apple
silicon, so a sandbox can degrade host responsiveness. The mitigation is QoS class
selection and admission control on concurrent sandboxes, not a guarantee.

## 8. Migration

Gas Can keeps working throughout. The safety net is `gascan-apple`, not any
retained Arca surface.

1. Arca stands up the sandbox protocol. `Sources/DockerAPI/` is deleted early
   rather than retained — it has no consumer once Gas Can is on the new path, and
   keeping two API surfaces on an engine under active refactor means every engine
   change must satisfy both, including the one whose semantics are being escaped.
2. Gas Can builds `gascan-arca` against the protocol.
3. `gascan-arca` must pass the conformance suite (§3.1) and the existing
   `gascan-e2e` coverage before it is eligible to become default.
4. The default flips. `gascan-apple` remains as a second implementation and
   reference — it is what keeps `RuntimeBackend` honest as an abstraction, and
   removing it would leave a one-implementation trait.

No step deletes a working path before its replacement carries load.

## 9. Versioning

Arca ships inside Gas Can's `.pkg` but builds from its own repository with its own
version. The bundle pins an exact Arca version, and Gas Can's
`build-manifest.json` — which already records a SHA-256 for every installed
executable — covers the bundled engine.

`RuntimeCapabilities.version` (`runtime.rs:38`) carries the engine version, and
`RuntimeError::UnsupportedVersion` (`runtime.rs`) already exists for the mismatch
case. Gas Can refuses to drive an engine whose version it does not recognise,
exactly as it does for Apple `container` today.

## 10. Measurements needed

Two numbers, neither of which should be reasoned about:

**M1 — recreate cost.** §6.2 makes a channel grant a recreate. A recreate loses
process state and everything outside the project root, and re-runs provisioning
(`crates/gascan-core/src/provision.rs`, whose `AppliedState` hashes tool state and
setup scripts). If this is seconds, the design question is academic; if
provisioning a polyglot workspace takes minutes, the ergonomics change what
`apply` should do.

**M2 — provisioning cost in isolation.** The portion of M1 attributable to
provisioning rather than VM boot, since only one of them is optimisable by Arca.

## 11. Deliberate non-goals

- Docker API compatibility in Arca, in any form.
- A general-purpose container network. Peer channels only.
- Runtime topology mutation.
- Registry access from the engine.
- Multi-consumer support for Arca. Gas Can is the only consumer; a second one is
  a reason to revisit §3, not something to design for now.
