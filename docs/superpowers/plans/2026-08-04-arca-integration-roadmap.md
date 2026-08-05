# Arca Integration Roadmap

Date: 2026-08-04
Status: Draft for review

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

| Step | Work |
|---|---|
| P0.1 | Move the superproject pin from `f48a6c7` to the fork's `origin/main` `502b715`. Picks up a rootfs-escape fix plus `CreateVolumeOverlay`, `CreateDirectMount`, `GenerateHostsFile`. |
| P0.2 | Delete the committed `arca-services` ELF; build it in CI from `build.sh`. Must precede P1 so the blob never enters Gas Can's history. |
| P0.3 | Merge `upstream/main` (267 commits, 6 minor releases). Resolve **U1** and **U2**. |
| P0.4 | Functional pass: boot a sandbox, exercise WireGuard peers, filesystem, process, overlayfs. |

**Exit:** fork builds against current upstream, sandbox boots, all guest services
respond, no binary in git.

P0.1 and P0.2 are independent of every decision in these specs and should not wait
for roadmap approval.

---

## P1 — Monorepo merge

**Depends on:** P0.

| Step | Work |
|---|---|
| P1.1 | `git subtree add` Arca into Gas Can, preserving history. |
| P1.2 | Relocate to the layout in the merge spec §6. |
| P1.3 | Carry the submodule across, still pointing at the fork. |

**Exit:** one repository, everything builds, history preserved.

---

## P2 — Build consolidation

**Depends on:** P1.

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

**Exit:** proto exists, both sides generate, nothing implements it yet.

Size gate: past roughly 400 lines, something Docker-shaped has crept back in.

---

## P4 — Docker removal

**Depends on:** P3. **Parallel with P5.**

| Step | Work |
|---|---|
| P4.1 | Delete `Sources/DockerAPI/` and its target. |
| P4.2 | Delete host-side buildx integration. Agents get `dockerd` in-guest. |
| P4.3 | De-Docker `ContainerManager.swift`: restart policies, health checks, registry pull/push/auth. |

**Exit:** the engine carries no concept the protocol cannot express.

P4.3 is the largest single work item in the roadmap — 4,518 lines with Docker
concepts woven through.

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

- **P0.1 and P0.2 are do-now.** They fix a missing security fix and an
  unverifiable binary. Neither depends on any decision in these specs.
- **P3 is the fan-out.** Before it, work is serial. After it, Swift removal (P4)
  and the backend (P5) proceed independently.
- **P5.1 must not wait on P4.** Implementing the engine service against existing
  machinery unblocks the Rust side; de-Dockering can follow.
- **P8 starts early and finishes late.** Deferring it entirely is the one choice
  here with unbounded cost.
- **U5 and U6 are spec gaps**, discovered while sequencing. Both need design work
  folded back into the contract, not just implementation.
