# Arca Integration Handoff

Date: 2026-08-04
Session origin: design exploration conducted in `~/code/firecracker`, concluding in
Gas Can and Arca.

This document carries what a fresh session needs and cannot recover from the specs
alone: why alternatives were rejected, what was measured, what was checked and
found absent, and which of the previous session's conclusions were wrong.

## Product Boundary

Gas Can is a local sandbox for agentic coding on Apple-silicon Macs. This work
replaces its external Apple `container` dependency with a bundled engine derived
from Arca, and adds egress policy and peer channels — the capabilities Apple's
runtime structurally cannot provide.

This is not a VMM project. No hypervisor is being written.

## Authoritative Decisions

- **Do not port Firecracker and do not write a VMM.** Explored at length and
  rejected. Arca already reaches further than a Firecracker fork would after
  months of work.
- **Apple's `container` is insufficient for one structural reason: egress
  control.** It routes through vmnet, so the host cannot see or filter what a
  container reaches. Secondary gaps: memory freed in-guest is never returned to
  the host (documented in Apple's own technical overview), no snapshot, macOS 26
  floor, no I/O rate limiting, no published threat model.
- ~~**Arca pivots from Docker-compatible runtime to Gas Can's sandbox engine.**
  Docker compatibility is discontinued, not reduced.~~
  **SUPERSEDED 2026-08-05** — see "Decisions reversed 2026-08-05" below.
- ~~**Gas Can absorbs Arca**, not the reverse. The deciding factor is which release
  pipeline survives: Gas Can's verifies signed tags and emits
  `build-manifest.json`; Arca's builds a DMG.~~
  **SUPERSEDED 2026-08-05.** The release-pipeline reasoning still holds and is why
  Gas Can builds the engine; it does not require absorbing the source.
- ~~**The Vas Solutus identity does not survive.** It existed for the
  Docker-runtime positioning the pivot ends. "Arca" survives as the engine's name
  inside the tree.~~
  **SUPERSEDED 2026-08-05.** Arca continues as its own project and keeps its
  identity. Only Gas Can's naming is Gas Can's to decide.
- **Sandboxes need peer channels.** Agent-to-agent collaboration is a real
  requirement, each agent in its own sandbox.
- **Sidecars are not a networking feature.** A Postgres for the project under
  development runs inside the agent's own sandbox on the guest's Docker.
- **Agents get `dockerd` in-guest.** Host-side buildx integration is deleted.
- **Arca owns the wire protocol; Gas Can owns the behavioural specification.**
  The engine decides what it can be asked; the product decides what a correct
  answer is.
- ~~**`Sources/DockerAPI/` is deleted early, not last.** `gascan-apple` is the
  migration safety net, so retaining it buys nothing.~~
  **SUPERSEDED 2026-08-05.** It is not deleted at all. It is an independent
  SwiftPM target, so Gas Can's engine build simply excludes it.
- **Peer channels are declared at create and immutable for the sandbox's
  lifetime.** No runtime grant RPC exists, and its absence is a security property:
  a compromised agent must have no path to request reach toward a neighbour.
  Granting requires a recreate; revoking never escalates and is therefore safe at
  runtime, but v1 ships no revoke path because `gascan down` covers it.
- **The user surface for topology change is `apply`**, not `down` then `up`.
  Whether a change requires a recreate is the reconciler's decision.
- **Egress is log-only in v1.** Full visibility, no blocking.
- **Snapshots are out of scope.** The seam is designed; the implementation is not.
- **The containerization fork stays a submodule through the merge.** Stopping
  forking entirely is the stated direction, not an aspiration.

## Roadmap Status

P0.1 and P0.2 implemented 2026-08-04 (see "Completed 2026-08-04"); nothing beyond them.
Five documents written, all uncommitted:

| Document | Location |
|---|---|
| Roadmap | `docs/superpowers/plans/2026-08-04-arca-integration-roadmap.md` |
| Contract | `docs/superpowers/specs/2026-08-04-sandbox-engine-contract.md` |
| Gas Can side | `docs/superpowers/specs/2026-08-04-arca-sandbox-backend.md` |
| Merge | `docs/superpowers/specs/2026-08-04-arca-monorepo-merge.md` |
| Arca side | `arca/Documentation/SANDBOX_ENGINE_PIVOT.md` |
| Arca pointer | `arca/Documentation/GASCAN_INTEGRATION.md` |

The two Arca files are **invisible to git**: `arca/Documentation/.gitignore`
ignores `*.md` by default with a five-file allowlist. Decide whether to allowlist
them or move them; do not assume they are tracked.

## Verified Environmental Facts

Measured 2026-08-04 on macOS 26.5.1 build 25F80, arm64, unless noted.

### Entitlements

Established by compiling a probe, signing it three ways, and running it:

- Unsigned: `hv_vm_create` → `0xfae94007` (`HV_DENIED`).
- Ad-hoc signed with `com.apple.security.hypervisor`: `HV_SUCCESS`,
  `max_vcpu_count = 64`. `codesign -dv` reported `Signature=adhoc`,
  `TeamIdentifier=not set`.
- Ad-hoc signed with `com.apple.security.virtualization`: VZ configuration
  validation stops reporting the missing entitlement.
- Ad-hoc signed with `com.apple.vm.networking`: **SIGKILL at exec**, exit 137.
  A/B/A tested on one binary — `{hypervisor}` → 0, `{hypervisor, vm.networking}` →
  137, `{hypervisor}` → 0.

**Neither entitlement Arca needs requires an Apple grant.** The restricted one
does not degrade — it makes the binary unlaunchable, so it must never appear in an
entitlements plist without a grant.

`Arca.entitlements` declares virtualization, network client and server, and
user-selected file access. It sets `com.apple.security.app-sandbox` to `false`,
commented "Disable App Sandbox for development/testing," and ships that way.

### macOS platform

- Seatbelt supports default-deny syscall filtering:
  `(deny syscall-unix ...)` plus `(allow syscall-unix (syscall-number SYS_madvise) ...)`
  appear in profiles under `/System/Library/Sandbox/Profiles/` (511 files).
- The machinery is SPI, not API. `/usr/lib/libsandbox.1.dylib` exports
  `_sandbox_compile_string`, `_sandbox_compile_file`, `_sandbox_apply`;
  `/usr/lib/system/libsystem_sandbox.dylib` exports `_sandbox_init_with_parameters`,
  `_sandbox_apply_bytecode`. Public `sandbox.h` is marked deprecated and offers
  only named profiles.
- **Seatbelt is process-scoped, not thread-scoped** — no thread-scoped exports in
  `libsandbox.1.dylib`.
- No hard CPU affinity on Apple silicon (`THREAD_AFFINITY_POLICY` is a hint,
  returns `KERN_NOT_SUPPORTED` on ARM) and no `cgroup cpu.max` analog.
- `clonefile(2)` available since macOS 10.12; root volume is APFS; cloning a
  64 MiB tree measured `real 0.00`.

### The containerization submodule

| Fact | Value |
|---|---|
| Superproject pins | `f48a6c7`, 2025-12-03 |
| Fork `origin/main` | `502b715`, 2025-12-09 — 4 commits ahead |
| Last upstream merge | `76cd1d4`, 2025-12-01 |
| `upstream/main` | `5796abe`, 2026-07-31 |
| Upstream commits since merge | 267 |
| Upstream releases since merge | 0.35.0 → 0.40.2 |

Fork delta vs merge base: **38 files, 12,056 insertions, 73 deletions.** 22 files
are additions under `vminitd/extensions/arca-services/`, which upstream does not
have and which therefore cannot conflict. 16 are modifications.

Three modified files no longer exist upstream, verified with
`git cat-file -e upstream/main:<path>`: `vminitd/Sources/vminitd/Server+GRPC.swift`,
`ManagedProcess.swift`, `RuncProcess.swift`. All under `vminitd/Sources/vminitd/`;
upstream restructured that directory.

`Sources/Containerization/LinuxContainer.swift` absorbed 38 upstream commits while
carrying Arca modifications — highest single-file conflict probability.

`vminitd/extensions/arca-services/arca-services` is a committed stripped
statically-linked aarch64 Linux ELF, 12,976,312 bytes in the object store, beside a
`build.sh` that cross-compiles from the Go source.

**Corrected 2026-08-04.** An earlier revision of this document said `build.sh`
"cross-compiles the same output". It does not reproduce the blob. Rebuilding the
pinned source with go1.26.3 yields 13,172,898 bytes (sha256 `c7f3802b…`) against the
committed 12,976,312 bytes (sha256 `4464ff2d…` at `f48a6c7`, `cb4f3326…` at
`502b715`); `go.mod` declares `go 1.24.0`, and the committed binary carries no Go
BuildID while a fresh build does. `build.sh` produces a working equivalent, not a
reproduction, and the blob's provenance cannot be verified against the source beside
it. That is the argument for deleting it — not reproducibility.

A second fact was missed entirely: **`go.mod` was untracked.** `go.sum` was tracked;
`go.mod` was excluded by `~/.gitignore_global:235` (`*.mod`, a rule intended for
Linux kernel module artifacts, in a C/kernel block alongside `*.ko` and `*.smod`).
A clean checkout therefore could not build `arca-services` at all:

```
$ git archive origin/main vminitd/extensions/arca-services | tar -x -C "$tmp" --strip-components=3
$ cd "$tmp" && ./build.sh
go: go.mod file not found in current directory or any parent directory
```

This inverts P0.2's ordering: tracking `go.mod` must precede deleting the ELF, or the
deletion removes the only artifact that worked. Both are done — see below.

Nothing consumes the committed copy: `scripts/build-vminit.sh` rebuilds from source at
lines 62-73 and packages that fresh output at line 134
(`--add-file .../arca-services:/sbin/arca-services`), overwriting the checked-in file
on every build.

### Repositories

- Both AGPL-3.0, identical 34,523-byte LICENSE files. Bundling is clean.
- Both single-author. Arca 25 commits, Gas Can 419.
- Arca is on `main`; Gas Can on `feat/default-ssh-workstation` with unrelated
  in-flight changes under `.superpowers/sdd/`.
- The submodule has both `origin` and `upstream` remotes configured.

## Investigated and Found Absent

Recorded so nobody re-derives them.

- **Arca has no egress policy engine.** `TCPProxy.swift` and `UDPProxy.swift` are
  ingress port-mapping proxies — the doc comment reads *"Used for
  `-p 127.0.0.1:8080:80` style port mappings."* The egress-capable primitive is
  guest-side nftables (`initializeFirewall()` at
  `arca-services/cmd/arca-services/main.go:54`) plus the in-guest DNS resolver
  (`internal/dns/`, 282 lines). A policy engine on top does not exist yet.
- **No CONNECT proxy, MITM layer, or SOCKS component exists anywhere in Arca**,
  and none should be added — nftables enforcement survives an agent that ignores
  `HTTP_PROXY`.
- **`crates/gascand/src/reconcile.rs` is types-only**, 15 lines, aimed at drift
  detection. There is no reconciler. `DesiredState`/`ActualState` are real and
  drive `up_inner` in `service.rs`.
- **Firecracker has no filesystem-sharing device of any kind.** `VIRTIO_ID_FS` is
  a generated constant with no implementation. This was decisive in rejecting it.
- **Hypervisor.framework exposes no dirty-page-tracking API.**

## Decisions reversed 2026-08-05

Four of the Authoritative Decisions above were reversed, deliberately, after P0
finished. They are struck through in place. The reasoning is here so nobody
restores them from the older text.

**Arca survives as its own Docker-compatible project.** Gas Can takes what it
needs; Arca is not converted into the sandbox engine and its Docker support is
not discontinued.

The decisive argument is not cost. Under the original plan P4 **permanently
destroys** Arca's Docker compatibility — `Sources/DockerAPI/` deleted, then P4.3
de-Dockers `ContainerManager.swift`. There is no second copy. Keeping Arca whole
preserves the option to revisit Docker compatibility later, and it makes P4 stop
being a one-way door. The cost is a divergence tax: two copies of the engine
sources, fixes found in Gas Can not flowing back, and a merge to face if Arca is
ever revived. Arca should be expected to freeze rather than be maintained in
parallel.

**Gas Can consumes Arca as a pinned source dependency, not by absorbing it.**
The monorepo merge was never about source coupling — P3 always made the boundary
a protocol. It was about shipping: Gas Can ships one signed package containing an
engine it built itself, and `build-manifest.json` must attest every shipped
executable. Building Arca from a pinned commit satisfies that. Gas Can's pipeline
has to build Swift either way — P2.1 already commits to "one CI orchestrating
Swift, Rust, Go, and protobuf codegen" — so this costs nothing P2 was not already
paying, and the pin is the provenance.

This also makes an existing correction cohere. Correction 3 below settled that
Arca owns the wire protocol because "the engine is the server and versions
independently; its wire behaviour cannot be defined in a repository it does not
control." That reasoning sat awkwardly against co-locating the source. It no
longer does.

**Arca's contract to its consumers is the `.proto`, not a library.** Arca is
Swift and cannot publish a Rust crate; `gascan-arca` is Gas Can's client and
lives in Gas Can, as P5.2 already says. The versioned artifact Arca publishes is
the engine proto. Both sides already have the machinery: Arca has
`scripts/generate-grpc.sh` and protos under `Sources/ContainerBridge/proto/`, and
Gas Can already has a `gascan-proto` crate.

Rejected: exposing Arca through FFI. It would pull a daemon that needs
virtualization entitlements and manages VMs into Gas Can's address space,
inheriting its process lifecycle and crash behaviour. A socket is strictly better.

**`Sources/DockerAPI/` is excluded by the engine build, not deleted.** It is
already its own SwiftPM target, so target selection keeps it out of the shipped
binary while Arca keeps it. This is the whole of P4.1 under the new plan.

### The open question this creates

P4's exit criterion — "the engine carries no concept the protocol cannot
express" — is unachievable for Arca if Arca stays Docker-capable. It remains
achievable for what Gas Can *ships*, but only partly by target selection:
`DockerAPI` splits out cleanly; the Docker semantics inside `ContainerBridge` do
not, because the engine needs that target.

Decision taken 2026-08-05: **Arca grows a seam** (build flag or target split) so
those concepts stay out of the engine build, rather than Gas Can shipping an
engine carrying unreachable Docker code. The security property would otherwise
have to rest entirely on the protocol boundary — defensible, since code no proto
method reaches cannot be invoked, but it forces the threat model to argue from
unreachability instead of absence, which is the weaker claim.

Spot check on the size of that seam, 2026-08-05 — **not a full analysis, five
greps against `Sources/ContainerBridge/ContainerManager.swift`:** `registry` and
`pullImage` appear **zero** times (registry work lives in `ImageManager.swift`,
a separate file); `healthCheck` 12 times, with `HealthChecker.swift` already
separate; `restartPolicy` 9; `autoRemove` 1. The roadmap's "4,518 lines with
Docker concepts woven through" overstates the interleaving of the three concerns
P4.3 names. The file is genuinely 4,528 lines with 38 public entry points, so the
size is real — but the seam is about which entry points the engine build exposes,
not about untangling registry code. P4.3's estimate deserves re-deriving before
it is scheduled.

## Corrections to Earlier Conclusions

The previous session reached four conclusions it later reversed. Do not re-adopt
them.

1. **A `gascan-docker` backend that would also drive Docker Desktop and Colima.**
   Wrong: such runtimes report `NetworkIsolation::Unsupported` and are useless for
   Gas Can's default mode. Widening the install base to runtimes that cannot
   provide the guarantee undercuts the product.
2. **Retaining Arca's Docker API until the replacement carries load.** Wrong:
   `gascan-apple` is the safety net. Keeping it doubles the API surface on an
   engine under active refactor, and the retained one has the semantics being
   escaped.
3. **Gas Can owning the wire protocol.** Wrong: the engine is the server and
   versions independently; its wire behaviour cannot be defined in a repository it
   does not control. The behavioural specification stays with Gas Can.
4. **That macOS has no seccomp equivalent.** Understated. SBPL supports
   default-deny `syscall-unix` filtering; the real caveats are that it is SPI and
   process-scoped.

## Completed 2026-08-04

P0.1 and P0.2 are done. Three branches, none merged, nothing on any `main`.

| Branch | Repo | Commits |
|---|---|---|
| `fix/track-go-mod-drop-prebuilt-binary` | `arca-containerization` | `943d3b3`, `5754902` |
| `fix/submodule-currency` | `arca` | `6829cdb` |
| `fix/swift-6.3-sending-closures` | `arca` | `0910463` |

- **P0.2a** (new step, not in the original roadmap): `go.mod` tracked, and the `*.mod`
  rule removed from `~/.gitignore_global`. Clean-checkout `build.sh` now exits 0.
- **P0.2b**: ELF removed from the index and added to the existing build-artifact block
  in the fork's `.gitignore`. Verified with `git check-ignore -v` that the rule matches
  only the binary, not `cmd/arca-services/` or the parent directory; 20 files tracked.
- **P0.1**: pin `f48a6c7` → `5754902`, a clean fast-forward (0 divergent commits).
  The three new RPCs are dormant — no caller exists in `arca/Sources/`.
- The blob remains in the fork's history; it is only removed from the index. Deferred
  to P8.1, which moves `arca-services` out of the containerization tree anyway.

**The blob differs between the old pin and the new one** (`4464ff2d…` → `cb4f3326…`).
Moving the pin alone would have swapped one unverifiable binary for another, which is
why P0.2 was sequenced before P0.1.

### Arca did not build, and it was not the pin

`swift build` failed under the installed Apple Swift 6.3.3 on two strict-concurrency
errors in `Sources/ContainerBridge/{TCPProxy,UDPProxy}.swift`. Verified pre-existing:
it reproduced identically at both `f48a6c7` and `5754902`, and neither file imports
anything from the submodule. Fixed in `0910463`; `swift build` now exits 0 in both
debug and release, with no `@unchecked Sendable`, `nonisolated(unsafe)`,
`@preconcurrency`, or `Package.swift` concurrency change.

This broke the **release** path, not just development: `publish` → `notarize` →
`dist-dmg` → `release` → `all` → `codesign` → `swift build`. The last release
(v0.2.4-alpha, 2025-12-02) predates the old pin (2025-12-03), consistent with the
build having broken at a later toolchain upgrade.

Consequence for the roadmap: **P0.3's 267-commit merge would have started from a tree
that did not compile**, so there was no green baseline to reconcile against. There
still is not one for the guest side — `scripts/build-vminit.sh` has not been run.

### Known-unfixed race, found while fixing the above

`UDPProxy` resolve-or-create is not atomic. Swift actors are reentrant, so the actor
yields at `await bootstrap.bind(...)` and two datagrams from one client can each
observe no channel and each create one; the second overwrites the first's mapping and
orphans that channel. Pre-existing, unchanged by `0910463`, needs an in-flight-creation
record on the actor.

Confirmed by reduction, not inference: an actor whose method is check → `await
Task.sleep` → write-back, invoked twice concurrently for one key under
`swiftc -swift-version 6`, reports `creates: 2`. The same reentrancy is why awaiting
`bind()` under actor isolation does **not** serialize datagram handling across clients
— the actor is not held across the suspension. Both properties follow from the same
mechanism, so a future fix that serializes creation to close the race must not do so
by holding the actor across the bind.

### P0.3 — upstream merge done; superproject not yet adapted

267 commits merged (`a1085d8`), guest build fixes on top (`f02cdf9`), both on fork branch
`merge/upstream-main`. Superproject pin + packaging on `merge/upstream-containerization`
(`4e27394`). All pushed, nothing merged to any `main`.

Full reasoning — every kept/dropped fork divergence and why — is in
`docs/status/arca-upstream-merge-rationale.md`. Do not re-derive it.

- **U1 was wrong** in the original handoff and is corrected there: upstream renamed
  `Sources/vminitd/` → `Sources/VminitdCore/` rather than deleting three files. git pairs 18
  of 19 automatically; only `Server+GRPC.swift` needed a hand port.
- **U2 answered for every conflicted file.** Notably the fork's rootfs-escape fix is
  superseded by upstream's `openat2`/`RESOLVE_IN_ROOT`, which is strictly stronger.
- Toolchain now matches upstream: Swift 6.3, static SDK `6.3-RELEASE_static-linux-0.1.0`.
  The SDK target is `make linux-sdk`; upstream renamed it from `cross-prep`.
- **Green:** fork host build, guest build, `arca-vminit:latest` image.
- ~~**Not green:** superproject `swift build` — 109 errors.~~ Resolved 2026-08-05,
  see below.
- ~~Still untouched: P0.4 functional pass. Nothing has been booted.~~ Done
  2026-08-05, see below.

### P0.3 superproject adaptation and P0.4 functional pass — 2026-08-05

**P0 is complete and landed.** Both PRs merged as merge commits, deliberately not
squashed, so every SHA cited in these documents stays reachable from `main`.

| Repository | State |
|---|---|
| `Vas-Solutus/arca-containerization` | `#1` merged → `main` `9847c35`; tagged `upstream-merge-0.40.2` |
| `Vas-Solutus/arca` | `#46` merged → `main` `b20be7c`; tagged `gascan-engine-baseline`; pins `f02cdf9` |

Both tags are signed and pushed. `gascan-engine-baseline` is the commit Gas Can's
engine pin should be based on.

**Merge method matters here and is not incidental.** `Vas-Solutus/arca`'s ruleset
`10300321` originally set `allowed_merge_methods: ["squash"]`. Squashing would
have collapsed `b8903f7` (compile adaptation only) and `9c2db5a` (a behaviour
change to mount plumbing) into one commit and minted a new SHA, invalidating every
anchor in these documents and destroying the separation that keeps the merge
reviewable. The ruleset was changed to `["merge","squash"]` on 2026-08-05;
`deletion`, `non_fast_forward`, `required_signatures` and all review parameters
were left untouched. **Keep merge commits allowed** — Gas Can pins Arca by commit,
so a policy that rewrites SHAs on every merge works against what the repository is
now for.

The review requirement (`required_approving_review_count: 1`,
`require_last_push_approval: true`) is unsatisfiable for a solo maintainer, so
`#46` needed `--admin`. Expect that on every future merge until there is a second
reviewer or the count is set to 0.

**API drift adapted** (`b8903f7`). After `swift package clean`, 107 error lines
across 11 unique sites in six categories, plus a seventh that only surfaced in
`DockerAPI` once `ContainerBridge` compiled — the handoff's "109 is a floor"
expectation held. `swift build` exit 0 at `-c debug` from clean,
`--build-tests`, and `-c release`; 0 errors each.

Two adaptations changed behaviour rather than only satisfying the compiler, both
forced by the new types and both recorded in the commit:

- `buildLinuxCapabilities` is now throwing. `CapabilityName(rawValue:)` rejects
  names Linux does not define, so an unrecognised `--cap-add` fails container
  creation instead of reaching the OCI spec verbatim.
- A nil `ContainerStatistics` cpu/memory/pids category errors rather than
  reporting a fabricated zero. Docker's `/stats` payload has no representation
  for "unknown" in those objects.

Upstream's new `defaultOCICapabilities` turned out to be exactly the 14 Docker
defaults Arca already used, so the stricter default was not weakened.

**P0.4 passed** on everything except the k3d case. A container boots — the first
time anything has run on the post-merge tree. `waitForServicesReady` gates start
on vsock 51819, which opens only after the wireguard, filesystem and process
services initialise, so every successful `docker run` is also evidence the guest
services came up. Verified: networking (address, default route, DNS, outbound
HTTP), PTY (`/dev/pts/0`), `exec`, `logs`, volumes, file binds including `:ro`,
capabilities, and `stats` including the streaming path.

**k3d is out of scope, not deferred.** It is a Docker-compatibility concern, and
under the 2026-08-05 reversals Docker compatibility lives in Arca rather than
Gas Can. The rationale doc's open question — whether fork commit `502b715`'s
motivating symptom returns now that upstream mounts at the resolved path instead
of skipping — is Arca's question to answer if it revives Docker compatibility. It
is not a Gas Can gate.

**One merge regression found and fixed** (`9c2db5a`). `docker run -v vol:/data`
failed with `errno 2`. Upstream's `LinuxContainer.start()` now ends with
`cleanAndSortMounts`, sorting by destination path depth; at the merge base the
same line preserved caller order verbatim (`git grep cleanAndSortMounts 27947cd
-- Sources` finds nothing). Arca's bind read from the share's own in-container
destination, so a container path shallower than the share sorted ahead of it.
Depth was the discriminator: `/data` and `/opt/data` failed, `/a/b/c/d`
succeeded, share at depth 3. Fixed by sourcing binds from `/run/virtiofs/<tag>`,
which `create()` mounts before any container mount is applied and which
upstream's own virtiofs-to-bind transform already uses.

**Two pre-existing defects found, deliberately not fixed.** Both are Arca-internal
Docker semantics the merge never touched, and both are now tracked as Arca's own
backlog: `Vas-Solutus/arca#47` and `#48`.

- `generateContainerName()` draws from 6 adjectives × 6 nouns with no uniqueness
  check and no retry, and nothing acts on `HostConfig.autoRemove`, so
  `docker run --rm` leaves the container behind. Names accumulate against a
  36-name pool until `docker run` fails with `Conflict. The container name '<x>'
  is already in use` — 2 of 6 identical runs failed with 17 distinct names
  present. This briefly read as a mount bug; it is not.
- `docker start` on a stopped container on a bridge network fails with
  `already connected to network bridge`, then upstream's `invalidState` on
  retry. `WireGuardNetworkBackend.cleanupStoppedContainer` is a deliberate no-op,
  so the attachment is meant to survive a stop and the start path's re-attach
  validation rejects it. The same sequence succeeds on `--network host`, which
  localises it to the bridge backend.

### There is no CI

`gh run list -R Vas-Solutus/arca` returns `[]` — zero workflow runs ever, and no
`.github/` directory in the tree or its history. The 20+ signed releases were published
locally via `make publish`, which chains `check-publish-env` → `notarize` → signed git
tag → `gh` release upload. So P0.2's "build it in CI from `build.sh`" has no CI to run
in; standing one up belongs with P2, not P0.

## Current Unfinished Work

- All six documents are drafts. None reviewed, none committed.
- **U5 and U6 in the roadmap are genuine spec gaps**, not implementation details.
  U5: how image digests reach an engine forbidden from touching a registry, and how
  that reconciles with the existing offline image bundle machinery. U6: validating
  a peer-channel target across sandboxes when `CreateRequest` is sealed and built
  from a single manifest. Both need design folded back into the contract.
- ~~Roadmap steps P0.1 and P0.2 are do-now and depend on no decision here.~~ Done
  2026-08-04; see "Completed 2026-08-04" above.

## Starting the next phase — 2026-08-05

P0 is closed. The next work is **P1.1** (pin Arca in Gas Can) and **P1.2** (build
the engine targets only), ~~with P4.3's target split entangled — see the roadmap's
"Sequencing: P1.2 partially depends on P4.3" before planning around the phase map.~~

**Corrected 2026-08-05.** The entanglement is with **P5.1, not P4.3**. Design and
evidence in `docs/superpowers/specs/2026-08-05-arca-engine-pin-design.md`;
the roadmap section is struck through in place. In short, VERIFIED by
`swift package describe --type json` in `~/code/arca` (exit 0): `DockerAPI` is its
own target but the only shippable executable reaches it through
`Arca → ArcaDaemon → DockerAPI`; Arca publishes **no library products**; and
`Sources/ArcaDaemon/` is entirely the Docker HTTP server, so **no engine
executable exists to build**. Moving P4.3 earlier would not unblock P1.2.

Two further facts found while resolving it, both recorded because they cost real
time to establish:

- **The pinned commit is not maintainer-signed.** `git verify-commit b20be7c`
  exits 1 (`Can't check signature: No public key`, RSA `B5690EEEBB952194` —
  GitHub's web-flow key), and `%G?` reports `E` for `b20be7c` against `G` for
  `9c2db5a` and below. `b20be7c` is GitHub's merge commit. The maintainer-signed
  anchor is the annotated tag `gascan-engine-baseline`, so provenance must run
  through the tag and assert `refs/tags/<tag>^{}` equals the pinned revision —
  the idiom `packaging/macos/release-common.sh:17-22` already uses.
- **`packaging/macos/package.sh:64-69` signs without `--entitlements`**, while
  Arca's `Makefile:62` signs with them and `Arca.entitlements` declares
  `com.apple.security.virtualization`. Nothing to fix in P1 — no engine binary
  ships — but an engine signed the Gas Can way could not create a VM, so **P7.3
  must not discover this late**.

Three conventions in these documents are load-bearing. They are why this phase
moved quickly, and they decay silently if dropped:

- **Every claim is marked VERIFIED or PLAN**, and a PLAN is never promoted without
  running something. The rationale doc states this; it applies to this document
  too.
- **Past-tense claims carry their anchor inline** — command, SHA, file:line, exit
  code — or they come out. Rules can ship bare; events cannot.
- **Corrections are recorded, not quietly edited.** Superseded conclusions stay
  struck through in place with a pointer, because the next reader has no way to
  tell which parts were verified unless it is written down.

Two calibrations from the 2026-08-05 session:

- **Use a fresh main session, not a subagent, for the target split.** It is a
  whole-file boundary judgment needing one coherent view of how `ContainerBridge`
  is used. The earlier calibration — subagents earn their keep on bounded,
  read-only sweeps and not on adaptation work — held again: a code-reviewer agent
  dispatched against the API-drift diff went idle without reporting, and its
  claims had to be verified directly anyway.
- **Capture exit codes directly, never through a pipe.** `cmd | tail` returns
  `tail`'s status. Redirect to a file and read `$?`. Three "exit code 0" reports in
  the prior session were false for this reason.

## Starting the next phase — P1.4, written 2026-08-05

**P1.1 and P1.2 are complete and on PR #44** (`Liquescent-Development/gascan`),
branch `arca-integration`. ~~Not merged. The engine-pin CI gate is **red**, for a
reason outside Gas Can's code.~~ **Superseded 2026-08-05: the gate is green and
PR #44 is merged** — see "P1.4 complete" and "PR #44 merged" below.

| Item | State |
|---|---|
| Pin, trust anchor, build script, contract test | landed, all 14 release contract tests exit 0 |
| `build-manifest.json` | `schema: 2`, carries the `engine` pin object |
| `.github/workflows/engine-pin.yml` | runs; **fails** in dependency resolution |
| Arca | `main` `b20be7c`, tag `gascan-engine-baseline`, unchanged by this work |

### What P1.2 could not deliver, and why it is not P4.3's fault

VERIFIED by `swift package describe --type json` in `~/code/arca`, exit 0. Arca's
only shippable executable reaches `DockerAPI` transitively —
`Arca → ArcaDaemon → DockerAPI` — Arca publishes **no library products**, and
`Sources/ArcaDaemon/` is entirely the Docker HTTP server. **No engine executable
exists to build.** The blocking dependency is **P5.1, not P4.3**; moving P4.3
earlier would not unblock it. P1.2 therefore landed partial by necessity: the
pipeline builds the pinned source and the manifest attests the pin, while the
binary half is booked against P5.1 and P4.3. Full design and evidence in
`docs/superpowers/specs/2026-08-05-arca-engine-pin-design.md`.

### ~~The next task is P1.4~~ — DONE 2026-08-05, see "P1.4 complete" below

Read the roadmap's "P1.4 — the pin is not cold-buildable" section first; it
carries the measurements. In short: replace `swift-ip` with an internal
IPv4/CIDR type in Arca, because Arca's `Package.resolved` pins two commits that
no longer exist upstream. **Re-pinning to `swift-ip` 0.3.10 would work and was
deliberately rejected** — it re-enters the same lottery and does nothing about
the decay.

**On preserving the vanished objects — do not spend time on this.** They were
mirrored to `~/code/vendor-mirrors/{swift-grammar-186ad640,swift-hash-c8396969}.git`
on 2026-08-05, both commits verified present with `git cat-file -t`. That is
enough, and no further durability work is warranted.

~~Until they exist off-machine, `gascan-engine-baseline` is one disk failure from
being permanently unbuildable.~~ **Overstated, corrected same day.** Three reasons
it does not matter: P1.4 removes `swift-grammar` and `swift-hash` from Arca's
graph entirely, so the new tag is cold-buildable and the baseline is superseded
rather than preserved; a mirror is inert anyway, because `Package.resolved`
records the upstream URL and SwiftPM will not consult another without an explicit
`swift package config set-mirror` or a hand edit; and no release has ever shipped
against this pin, so there is no artifact to reproduce.

The only case that would revive the concern is someone needing to build Arca at
`b20be7c` on a cold machine before P1.4 lands — a second developer, or a CI job
stood up in the meantime.

The replacement itself starts with a design decision, so **brainstorm before
implementing**: whether the type lives inside `ContainerBridge` or as its own
small target, and what its API is. The surface to satisfy is exactly
`IP.V4(String)`, `IP.V4(value: UInt32)`, `.value`, `IP.Block<IP.V4>(String)` and
`.base`, used in `Sources/ContainerBridge/StateStore.swift` and
`WireGuardNetworkBackend.swift`.

Sequence after that: implement with tests → build and functional pass → PR to
Arca `main` (**merge commit, never squash**) → new signed tag → bump
`engine/arca-pin.json` in Gas Can → watch PR #44's gate go green.

### Two open items from P1, neither blocking

- `--prune --prune-tags` in `scripts/build-arca-engine.sh` ships untested. The
  contract test builds a fresh cache per case, so tag-revocation-against-a-warm-cache
  needs a two-run case that does not exist.
- Exit **75**, used by the new cache lock, is not in the documented taxonomy
  (64 malformed pin, 65 provenance failure, 69 missing tool). The lock is
  `mkdir`-based with an EXIT trap; every path except SIGKILL releases it, and a
  SIGKILL strands it until someone runs `rmdir`. `flock` was rejected because it
  is Homebrew-only on macOS, which would mean either a CI dependency or fallback
  logic.

### Calibrations earned this session

- **The CI gate paid for itself on its first run.** It caught a supply-chain
  defect that every local build hides behind a warm cache. Do not let it stay red.
- **A final review caught a Critical the plan itself created.**
  `git checkout --detach <rev>` on a cache already at that revision is a no-op,
  so the script verified a tag and then compiled a worktree it never asserted
  matched. Reproduced, fixed, and then found *still* partly open — top-level
  `git clean` skips gitlink directories, so a file planted inside a submodule
  survived. Closed with `submodule foreach --recursive git clean -qffdx`.
  **Verify the reset, not just the signature.**
- **Subagents reported reliably here, against the previous session's
  experience** — but two returned their reports as plain text that never
  arrived, and both had actually finished. If one goes idle without reporting,
  ask it to resend before re-dispatching or assuming failure.
- **Reviewers earned their keep by running mutations, not by reading.** The
  useful findings came from neutering a check and confirming the suite went red.
  A reviewer that only reads agrees with the code.

## P1.4 complete — 2026-08-05

**The engine-pin gate is green.** VERIFIED: run
`https://github.com/Liquescent-Development/gascan/actions/runs/31055299650`,
`status=completed`, `conclusion=success`, `headSha=f562e6e`, on a hosted
`macos-26` runner. `gh pr checks 44` reports Passed: 1, Failed: 0. The **four**
preceding runs were all `failure` — `31038778615` (`12b4a91`), `31039127696`
(`afb04f2`), `31042100578` (`8be4ec6`), `31042404662` (`58ae69f`) — so this is
the gate's first green and it closes P1.4. **P2 is unblocked.**

~~The two prior runs (`58ae69f`, `8be4ec6`) were both `failure`.~~ **Corrected
2026-08-05**: that was written from `gh run list -L 3`, which showed only the two
most recent. The gate was red for four runs.

Two later runs also passed, confirming the pin holds across subsequent commits:
`31055959462` (`efde07d`) `success`.

`swift-ip` is gone from Arca, replaced by an internal `ArcaIP` target. Design and
plan: `docs/superpowers/specs/2026-08-05-arca-internal-ip-type-design.md` and
`docs/superpowers/plans/2026-08-05-arca-internal-ip-type.md`.

| Item | Value |
|---|---|
| **Pinned revision** | `d66c320c09e1dfc4f37aafa1fb27e36aa5cabe5d` (merge commit, two parents: `b20be7c` + `9bb1a7e`) |
| New pin tag | `gascan-engine-ip-internal` — signed, `verify-tag` exit 0 |
| Arca PRs | `#49` (the replacement) and `#51` (comment-only follow-up), both merged with `--merge`, not squashed |
| Arca `main` **now** | `7da8f77` — **ahead of the pin**, see below |
| Gas Can pin bump | `f562e6e` on `arca-integration` |
| Pins dropped | 6 of 38 — `swift-ip`, `swift-bson`, `swift-json`, `swift-grammar`, `swift-hash`, `swift-unixtime` |

**Arca `main` is deliberately ahead of the pinned revision, and that is not
drift.** `#51` was comment-only, so the pin was not moved and the tag was not
re-cut: re-pinning would have invalidated the cold-build and functional evidence
gathered against `d66c320` in exchange for nothing executable. VERIFIED after
`#51` merged: `git rev-parse refs/tags/gascan-engine-ip-internal^{}` still
returns `d66c320c…`, and `verify-tag` against `engine/allowed-signers` still
exits 0. **The pin resolves through the tag, never through `main`** — which is
exactly why `scripts/build-arca-engine.sh` asserts `refs/tags/<tag>^{}` equals
the pinned revision rather than trusting a branch.

### Evidence, with anchors

- **Equivalence to `swift-ip` 0.3.3 is differentially VERIFIED, not asserted** —
  across 18,580,063 vectors with 0 mismatches, which is a large sample and not an
  exhaustive proof. A harness compiled the real library (revision `ba4efb6`) as
  one module and `ArcaIP` as another and ran identical vectors through both:
  `checks=18580063 mismatches=0`, exit 0. The four sampled domains, so a future
  reader can see where the coverage is thin: 5,000,000 random `UInt32` values out
  of 2³² for formatting and reparse; all 33 prefix lengths × 20,000 random bases
  for `base`/`bits`/`range`; 8 boundary-weighted probes per block for `contains`;
  and roughly **60 hand-written malformed strings** for parser rejection. That
  last domain is the thinnest, and it is also the one the leniency decision was
  made to protect, since it governs how existing SQLite rows re-parse. The harness was itself validated by two
  independent negative controls — breaking `IP.V4.description` produced
  10,418,664 mismatches, and breaking `IP.Block.mask` produced 2,921,191
  including `contains`-specific ones. Without the second control, the Block
  comparisons could have been vacuous and nobody would have known.
- **Cold build VERIFIED**: `COLD_BUILD_RC=0` from a fresh clone at `9bb1a7e` with
  `HOME`, `--cache-path` and `--scratch-path` all redirected to a temp dir.
  Isolation confirmed by inspection: the isolated cache held 423 MB of freshly
  cloned dependency repositories, none from `tayloraswift`, and the isolated
  `HOME` was empty because cache and scratch captured everything.
- **Gas Can's own gate script VERIFIED cold**: `GATE_RC=0`, `ContainerBridge`
  release build complete in 173.18s, zero `tayloraswift` mentions.
- **Release contract suite**: 14/14 exit 0.
- **Functional pass** against a daemon proven by `nm`/`strings` to carry 108
  `ArcaIP` symbols and zero swift-ip module symbols: gateway `172.31.0.1` on
  `172.31.0.0/16`; two auto-allocated addresses distinct and in-subnet;
  `--ip 172.31.9.9` accepted; `--ip 10.9.9.9` rejected with rc=125,
  `IP 10.9.9.9 not in subnet 172.31.0.0/16` — the `IP.Block.contains` path.
- **Test baseline established, not assumed**: 125 distinct failing tests on both
  the before and after trees, differing by 10 in each direction. All ten
  after-only failures were individually traced to daemon-socket or `docker` CLI
  causes; none is an assertion about an IP, subnet, gateway, CIDR or allocation.
  `NetworkIPAMTests` behaves identically on both sides — same 14 failures, same
  causes.

### Corrections to this document's own earlier claims

- ~~The surface to replace is exactly `IP.V4(String)`, `IP.V4(value:)`, `.value`,
  `IP.Block<IP.V4>(String)` and `.base`.~~ **Understated.** Characterization
  found ten shapes, not five: those plus `Block.contains(_:)`, `Block.range`
  with both `.lowerBound` and `.upperBound`, `String(describing:)`, and
  `Equatable`. `String(describing:)` was the dangerous omission — its output is
  persisted to SQLite and returned on the wire, so a divergence there would have
  corrupted stored state rather than failing loudly.
- ~~The review requirement is unsatisfiable for a solo maintainer, so expect
  `--admin` on every future merge.~~ **No longer true.** The user relaxed ruleset
  `10300321` on 2026-08-05: `require_last_push_approval` is now `false`, as are
  `require_code_owner_review` and `required_review_thread_resolution`. PR #49
  merged with a plain `--merge`. `allowed_merge_methods` remains `["merge"]` and
  `required_signatures` remains in force, so squash is still impossible.

### Known defect, deliberately preserved — worse than first recorded

`IP.Block.range.upperBound` is the **broadcast address**, not broadcast−1, and
Arca's allocator treats that bound as inclusive. **The behaviour was left exactly
as it was**, because changing it would have made the differential equivalence test
impossible to write. Filed as `Vas-Solutus/arca#50`.

**Read the live path, not the dead one.** VERIFIED by
`grep -rn "allocateIP" Sources/ Tests/` in `~/code/arca`, which returns only the
definition at `WireGuardNetworkBackend.swift:1008` and a prose mention at `:223`:
`allocateIP` **has no callers**. `#49`'s corrected comment landed at `:1034-1037`,
inside that dead function. The live allocation path is
`WireGuardNetworkBackend.swift:382-408` (`attachContainer`) →
`rangeEnd = Int64(block.range.upperBound.value)` →
`StateStore.allocateAndReserveIP` (`StateStore.swift:964-1051`).

~~and it carries no marker.~~ **Closed by `#51` (merged, commit `74127f2`).** The
live path and `allocateAndReserveIP`'s `rangeEnd` parameter doc now both carry the
defect note, and both point explicitly away from `allocateIP`. Comment-only;
`swift build --configuration release --target ContainerBridge` exits 0. The pin was
deliberately not moved for it. Whoever fixes `#50` should fix `attachContainer` /
`allocateAndReserveIP` — or delete `allocateIP`, which appears to be dead.

~~Reachability: ~65,533 containers on a `/16`, immediate on a `/30`.~~
**Understated — corrected 2026-08-05 by the final review.** Both figures are
accurate for the wasted-address symptom, but the same expression is
**daemon-fatal** on `/31` and `/32`. `docker network create --subnet` takes the
string verbatim with no prefix-length validation
(`WireGuardNetworkBackend.swift:178-179`). On `--subnet 10.0.0.0/31`:
`rangeStart` becomes `10.0.0.2` and `rangeEnd` becomes `10.0.0.1`, so the first
attach silently allocates an address **outside the network**, and the second
reaches `StateStore.swift:1012`, `for ip in rangeStart...rangeEnd` — a closed
range with `lowerBound > upperBound`. Swift traps and **the daemon process dies**,
taking management of every running container with it. `/32` behaves the same way.
A related trap sits at `WireGuardNetworkBackend.swift:1082` on the live
network-create path: `--subnet 255.255.255.255/32` overflows `UInt32`.

So the exposure is not a wasted address at extreme scale; it is an
API-triggerable daemon crash on a two-character input, reachable on the second
container. Entirely pre-existing — `swift-ip`'s `onesMasked` produced the identical
bound and every arithmetic line involved is untouched by this change — but the
severity belongs in the record.

### What is still unverified

- **`StateStore`'s SQLite round-trip through a daemon restart.** The functional
  pass never restarted the daemon, so the `IP.V4(String)` → `Int64` → re-read
  path was not exercised end to end. The harness proves the arithmetic, not the
  persistence path.
- **Octet-boundary and broadcast-adjacent allocation at runtime.** Allocated
  addresses were `.2` and `.3`. The harness swept all 33 prefix lengths with
  boundary probes, so the arithmetic at those edges is proven; the daemon's
  allocator was simply never driven there.
- **Daemon-dependent test suites** cannot run in the agent sandbox at all, so
  they are unverified rather than proven equivalent.

### Calibration earned

- **A negative control is not optional for a differential test.** The first one
  perturbed `IP.V4.description`, which cannot affect `contains` — it returns a
  `Bool` computed independently on each side. A second control against the Block
  mask was needed before "0 mismatches" meant anything about half the surface.
- **`git rm --cached` beats deleting** when files are tracked but gitignored.
  `.gitignore:1` had ignored `.superpowers/` all along; 43 files were tracked
  from earlier merged PRs, and four earlier plans carried hand-written "do not
  stage these" clauses to work around it. Untracked in `26c7995`; every file
  stays on disk.
- **The plan itself contained an error the implementer caught.** Task 7 specified
  `cargo test --test '*'` for the release contract suite. Wrong — the suite is 14
  shell scripts at `tests/release/*-contract.sh`, and no cargo target runs them.
  It was found because the dispatch said to verify rather than assume.

### PR #44 merged — 2026-08-05

VERIFIED: PR #44 `MERGED` at `2026-08-05T23:51:51Z` as merge commit
`52a6fa0d35c1e950cc86738c1ffd5a203590940a`, carrying P1.1, P1.2, P1.4 and all
the documentation. `git rev-list --parents -n 1 52a6fa0` shows two parents
(`f6356f9` + `abcc1fa`), so it is a genuine merge commit.

**Merged with `--merge`, deliberately.** Gas Can has no branch protection ruleset
and permits squash and rebase (`allow_squash_merge: true`,
`allow_rebase_merge: true`), so the method was a choice, not a constraint. These
documents cite Gas Can commit SHAs — `f562e6e` for the pin bump, `cdd85b5` for
the design, and others — and a squash would have minted new SHAs and invalidated
every one. VERIFIED after merging with `git merge-base --is-ancestor`: `f562e6e`,
`cdd85b5`, `efde07d`, `4ef1f16`, `954f505` and `abcc1fa` are all still reachable
from `main`. **Keep merging Gas Can this way for as long as these documents cite
its SHAs** — the same reasoning that already applies to Arca.

The engine-pin gate was green on four consecutive runs before the merge:
`31055299650` (`f562e6e`, the pin bump), `31055959462` (`efde07d`),
`31056660871` (`4ef1f16`), `31057240620` (`abcc1fa`).

`delete_branch_on_merge` is `false`, so `arca-integration` still exists. New work
should branch from `main`, not from it.

## Fresh-Session Restart Procedure

From the Gas Can repository root:

```sh
git status --short
git branch --show-current
git -C ~/code/arca status --short
git -C ~/code/arca/containerization log -1 --format='%h %ci %s'
git -C ~/code/arca ls-tree HEAD containerization
```

Then read, in order:

1. this handoff;
2. `docs/superpowers/plans/2026-08-04-arca-integration-roadmap.md`;
3. `docs/superpowers/specs/2026-08-04-sandbox-engine-contract.md`;
4. `docs/superpowers/specs/2026-08-04-arca-monorepo-merge.md`;
5. `docs/superpowers/specs/2026-08-04-arca-sandbox-backend.md`;
6. `~/code/arca/Documentation/SANDBOX_ENGINE_PIVOT.md`.

To reproduce the submodule measurements:

```sh
cd ~/code/arca/containerization
git fetch upstream && git fetch origin
BASE=$(git merge-base origin/main upstream/main)
git rev-list --count 76cd1d4..upstream/main          # upstream commits since merge
git diff --stat $BASE origin/main                    # fork delta
git diff --name-status $BASE origin/main | awk '$1=="M"{print $2}'
```

## Deferred Work

- **Snapshots.** Out of scope by decision; the state-save seam is designed so the
  implementation is not a retrofit.
- **virtio-fs live mounts.** The threat model prefers copy-on-write; Apple's
  containerization supports virtio-fs if this reverses.
- **macOS 15 support.** Both products require macOS 26. This was the only
  remaining argument for a lower-level VMM and it was judged too thin to act on.
- **App Sandbox for the engine daemon** — roadmap U9.
- **The shelved Firecracker design.** Lives at
  `~/code/firecracker/docs/superpowers/specs/2026-08-01-macos-microvm-backend-design.md`,
  uncommitted, in a repository unrelated to this work. Its §3 platform facts are
  reproduced above; the rest is superseded. Do not act on it.

## P2.1 in flight — 2026-08-05/06

**Path chosen: P2.1, not P5.1.** P5.1 turned out not to be unblocked in the way the
prior handoff implied — the roadmap has `P5 — Depends on: P3` (`roadmap:345`) and
P5.1 is "implement the engine **service**", which needs the proto that P3.1 has yet
to design (U4). P2.1 had no design gap in front of it.

Design: `docs/superpowers/specs/2026-08-05-gascan-ci-consolidation-design.md`.
Plan: `docs/superpowers/plans/2026-08-05-gascan-ci-consolidation.md`.
**Read §11 of the spec first** — it holds what the first real runs found, including a
correction to the spec's own baseline.

### State of the three open PRs

| PR | Branch | Base | Contents |
|---|---|---|---|
| #46 | `ci/p2-1-consolidated-pipeline` | `main` | spec + plan, docs only |
| #47 | `fix/pty-completion-line-assertion` | `main` | the born-red PTY test fix, `ba667f4` |
| #48 | `ci/p2-1-pipeline` | **#47** | the pipeline, 8 commits |

**#48 is stacked on #47 deliberately.** Without #47's fix, `cargo test --workspace`
fails on a test that has never passed, so the `rust` job could not go green. GitHub
retargets #48 to `main` once #47 merges. **Nothing is merged** — the Claude Code
permission classifier refuses `gh pr merge`, and routing around it with
`gh api --method PUT .../merge` is the same irreversible action and was not done.
Merge each with `--merge`; never squash.

### Why CI paid for itself twice on its first two runs

Both are pre-existing defects that no human or agent had seen, because nothing ever
ran this code.

1. **A test red since birth.** `fake_backend.rs:589` searched the raw PTY transcript
   for `"✓ Sandbox is running"` while `presentation.rs:636-642` emits the marker as
   `"\u{1b}[32m✓\u{1b}[0m"`, so an SGR reset sits between glyph and text. `20de03d`
   added the colored marker at 13:27:34 on 2026-07-22; `6d01465` added the assertion
   at 13:31:53, **four minutes later**. `git merge-base --is-ancestor 20de03d 6d01465`
   exits 0. Two weeks red, invisible.
2. **The tree did not compile on its own pinned toolchain.**
   `cargo build --workspace` under `RUSTUP_TOOLCHAIN=1.85.0` exits 101 with
   `error[E0658]` — a let-chain at `crates/gascan/src/daemon.rs:1182`, unstable
   before Rust 1.88, while `rust-toolchain.toml` pins 1.85.0 and `Cargo.toml:8`
   declares `rust-version = "1.85"`. Then clippy failed at **eleven**
   `format_collect` sites. Both fixed on #48.

### The trap that hid both, and will hide the next one

**`RUSTUP_TOOLCHAIN=1.95.0` is exported in the development environment, and it
overrides `rust-toolchain.toml`.** `rustup toolchain list` reports `1.95.0 (active)`
inside the repository. Every local Rust measurement is therefore about 1.95.0 while
CI uses 1.85.0 — which is how a tree that cannot compile on its pinned toolchain
went unnoticed.

**Prefix Rust commands with `RUSTUP_TOOLCHAIN=1.85.0` until that export is removed**,
or the numbers do not describe CI. This is the Rust equivalent of the warm-SwiftPM-cache
trap, and it cost the same kind of time.

### Open decision for the maintainer: MSRV

The let-chain was rewritten as a nested `if` to honour the declared 1.85 floor,
because that was the smaller, reversible choice. **Bumping `rust-toolchain.toml` and
`Cargo.toml`'s `rust-version` to ≥1.88 is an equally valid answer and is a policy
call.** If the pin moves, `9bee529` can be reverted.

### BLOCKING: do not turn the ruleset on yet

`cargo test --workspace` is flaky — 3 red / 1 green locally, **a different test each
time**, all in the daemon-readiness family. Root-caused by measurement, not inference:
cargo runs **6–8 test binaries concurrently** (sampled with `ps`), each independently
defaulting to `--test-threads` = `num_cpus` = 10, so ~60–80 concurrent test threads on
10 cores, against hard 5-second wall-clock deadlines
(`FIXTURE_DAEMON_DIAGNOSTIC_DEADLINE`, `client.rs:20`). Isolated, the same binary is
green 5/5. Pre-existing: red on a base branch that changes zero Rust.

Two details that matter for whoever fixes it. The fixture "daemon" is a `#!/bin/sh`
script (`daemon.rs:3390`), not the 41 MB binary — so Gatekeeper/`syspolicyd` does not
explain it, and 5s is ~500× what the script needs. And the wait loop never checks
whether the child is alive, so it cannot tell "slow" from "dead" and reports an empty
`stderr` either way. Close that diagnostic gap before choosing a fix.

**Reducing parallelism or adding retries would hide this, not fix it.** Task 8 of the
plan (the ruleset) is explicitly gated on it.

### Settled by measurement

- **Hosted `macos-26` has no Apple container runtime.** The probe failed with
  `container: command not found`, exit **127**, on `ProductVersion: 26.5.2`. The heavy
  Apple e2e tier cannot run on hosted runners — independent of the candidate-image
  problem. Promotes a PLAN claim; D4 stands on evidence.
- **`gate` reddens correctly.** Run `31074653442`: `changes=success`,
  `engine=success`, `rust=failure`, `contracts=failure`, **`gate=failure`**. The
  aggregation propagates failure, so the required check would have blocked. The green
  and `skipped` directions still need confirming.
- **`changes` and `engine` work on real runners**, so the shell classifier is sound and
  the folded-in engine-pin build survived the move into `ci.yml`.
- **U3 answered: path filters are nice, not mandatory.** 1:00.07 for 902 tests and
  1:57.82 to compile every test binary, both warm, against 7m21s–8m38s for the engine
  build. They earn their keep on the `engine` job alone.
- **`actions/checkout` defaults to `fetch-depth: 1`**, and
  `release-script-contract.sh` resolves `HEAD~1`. Four contracts failed on it before
  `fetch-depth: 0` was added to that job.

### A pre-existing stash, untouched

`git stash list` shows `stash@{0}: f6356f9 release: prepare Gas Can 0.1.20 (#43)`. It
predates this session and was not created or dropped here. Someone should establish
what it holds before it is lost.

### Calibration earned

- **A subagent challenged a claim in its own instructions and was right.** The commit
  message it was handed said "the repository has no CI"; it checked, found two
  workflows, and reworded to the accurate, anchored version. `workspace-bundles.yml:96`
  does run `cargo test`, but against the produced gascamp bundle's vendored tree, not
  this workspace. Instructions to subagents are not exempt from the anchor convention.
- **Each clippy fix uncovered the next one.** Clippy stops at the first failing crate,
  so "one lint site" became eleven across five crates, one crate at a time. Expect
  excavation, not a single fix, the first time a linter runs over a codebase.
- **`printf '--- ...'` aborts under `set -eu`** — `printf` parses `---` as an option
  and exits 2. It was in the plan's contract-runner text and only fired on the failure
  path, so the diagnostic branch had never worked. Use `printf --`.

### Where PR #48 actually stands after two CI runs — start here

`ci / gate` is **red**, and the remaining failures are understood, not mysterious.
Run 2 (`06d4c67`): `changes=success`, `engine=success`, `rust=failure`,
`contracts=failure`, `runtime-probe=failure` (expected, §11.5), `gate=failure`.

**What the two fixes achieved.** Clippy now passes on the pinned toolchain — the
`rust` job's failure moved from lint to tests. `release-script-contract.sh` now
passes, and `publish-contract.sh` moved rc=128 → rc=1, so `fetch-depth: 0` was the
right diagnosis for the `HEAD~1` family.

**Task A — 14 tests need a binary path CI does not provide.**
`cargo test --workspace` fails at `-p gascan-e2e --test apple_apply`:
62 passed / **14 failed** / 8 ignored, every failure
`Error: "workspace-built gascan binary is unavailable"` from
`crates/gascan-e2e/tests/apple_common/mod.rs:503-506`. Those lines read
`CARGO_BIN_EXE_gascan-e2e-cli` and `CARGO_BIN_EXE_gascan-e2e-daemon` with
`std::env::var_os` — i.e. at **runtime**. Cargo documents `CARGO_BIN_EXE_<name>` as
a **compile-time** variable for `env!()`, set while building an integration test.
It is present in the local environment and absent on the runner, which is why these
14 pass here and fail there. They carry no `#[ignore]`, so they are not part of the
22-test heavy set — they were miscategorised as hermetic, including by this
document's §3.3 baseline.

*Do not guess the fix.* Establish first whether cargo sets these at runtime at all,
or whether local runs inherit them from something ambient. `env!` at compile time is
the documented mechanism and is probably the correct change, but that is a change to
a pre-existing test helper and deserves its own diagnosis.

**Task B — three release contracts still fail on a hosted runner.**
`distributable-package rc=65`, `publish rc=1`, `signal rc=1`. Causes not yet
established; the `HEAD~1` family is fixed, so these are different. They pass
locally (15/15, `status=0`), so each is a local-versus-runner divergence like Task A.

**Task C — the flaky suite**, §11.7 of the spec. Still gates the ruleset.

Sequence: A and B make `ci / gate` green; C makes it trustworthy; only then the
ruleset (plan Task 8).

**A caution earned twice tonight.** Every remaining failure is a case of "green
locally, red on the runner", and both already-diagnosed instances had the same
shape: the local environment silently supplies something CI does not
(`RUSTUP_TOOLCHAIN=1.95.0` overriding the pin; `CARGO_BIN_EXE_*` present in the
shell). Suspect ambient environment first, and measure with the runner's assumptions
rather than this machine's.

### Amended after the toolchain bump — 2026-08-06

Read this before acting on the three tasks above; one of them is gone and the
toolchain situation is resolved.

**The pin now tracks the compiler in use: 1.95.0** (`a335aad`), with
`Cargo.toml`'s `rust-version` following. The maintainer chose this after the
scaffolding history was traced: `d6978fa` (2026-07-13) set both files in one commit
and neither was revisited, and 1.85.0 was there only because it is the floor for
`edition = "2024"`. Pinning `rust-toolchain.toml` **to** an MSRV floor is what
guaranteed the local-versus-CI split, since `RUSTUP_TOOLCHAIN` overrides the file.
No crate sets `publish` and Gas Can ships as a signed `.pkg`, so the MSRV promise
protected no consumer.

Consequences, all VERIFIED:

- **The let-chain rewrite was reverted** (`ceddd86`). `daemon.rs:1182` reads
  naturally again; let-chains are stable from 1.88.
- **The `hex::lower` refactor was kept** (`b2003df`). 1.95's clippy does not require
  it, but eleven copies collapsed into one is worth having on its own merits.
- **33 clippy findings were cleared to reach a green lint gate** — 11
  `format_collect`, 26 `collapsible_if`, 1 `manual_is_multiple_of`, across 13 files,
  with **zero `#[allow(...)]` added** (`2d4a15e`, `1f31836`, `8347b48`).
  `cargo clippy --workspace --all-targets -- -D warnings` exits **0** with
  `RUSTUP_TOOLCHAIN` unset, confirmed not cache-clean by touching every `.rs` and
  re-running.

**Two things from that sweep worth carrying forward.**
`cargo clippy --fix` would not have been safe here: clippy's own suggestion at
`service.rs:2694` emitted invalid Rust (a stray brace mid-condition), and three
sites needed `||` parentheses it omitted — one of them `ssh/manager.rs:647`, an SSH
host-key check where a wrong collapse would have weakened a security guard. Read
every suggestion before applying it.

And the warm-cache trap appeared a **third** time, in a new tool. An earlier
`cargo clippy` reported exit 0 in 13.4 seconds by reusing cached lint results;
touching `gascan-core` forced a real re-lint and 22 findings appeared that had been
there all along. The pattern across this project is now: warm SwiftPM cache (P1.4),
`RUSTUP_TOOLCHAIN` overriding the pin, and cached clippy results. **Before trusting
any green from a tool, ask what it cached.**

### CI run 3 — `31113833927`, and a correction

`changes=success`, `engine=success`, `runtime-probe=failure` (expected, §11.5),
`rust=failure`, `contracts=failure`, `gate=failure`. CI reports
`rustc 1.95.0` and **clippy passed** — the first time the Rust workspace has cleared
lints in CI.

> ~~**Task A — 14 tests need a binary path CI does not provide.** …
> `CARGO_BIN_EXE_gascan-e2e-cli` … read at **runtime** … present in the local
> environment and absent on the runner.~~
> **WRONG, and dissolved.** Those 14 failures do not appear in run 3 at all — zero
> occurrences of `workspace-built gascan binary is unavailable`. They were an
> artifact of the earlier run's build ordering, where clippy failed first, not an
> env-var lifetime problem. The evidence that should have prompted a re-examination
> was already in hand: a subagent could not reproduce them locally, and that was
> read as *consistent with* the theory rather than as a reason to doubt it.
> **A theory that explains a non-reproduction too comfortably deserves suspicion.**

**What is actually left, in order:**

1. **Task B — three release contracts fail only on the runner.**
   `distributable-package rc=65`, `publish rc=1`, `signal rc=1`. Unchanged across all
   three runs and **undiagnosed**. They pass 15/15 locally. Given that two of the
   three local-versus-runner divergences this session were ambient environment,
   suspect that first: compare what the contract scripts read from the environment
   against what a fresh runner provides. Their logs are reachable per-job with
   `gh api /repos/Liquescent-Development/gascan/actions/jobs/<id>/logs` even while a
   run is in progress.
2. **Task C — the flaky suite**, §11.7 of the spec. `rust` is now down to this alone:
   run 3 failed only `logs_since_and_follow_emit_new_byte_exact_records_then_cancel`,
   which passes locally. Root cause is recorded; the fix is not chosen.
3. **Then the ruleset** — plan Task 8. Not before B and C.

### PR state at handover

| PR | State | Notes |
|---|---|---|
| #46 | **MERGED** `29318c3` | spec + plan |
| #47 | **MERGED** `d5cb601` | the born-red PTY test fix |
| #48 | **OPEN**, base `main`, 12 commits | the pipeline; red on Tasks B and C only |
| #49 | **OPEN** | this handoff |

`ci / gate` has never been green, so **do not add the ruleset yet** — never require a
check that has not passed.

## Session of 2026-08-06 (later) — Task B closed, `ci / gate` green once, Task C partly

> ~~`ci / gate` has never been green, so **do not add the ruleset yet**.~~
> **Superseded.** `ci / gate` reported **success** on run `31121170624`, head `64ee3ee`:
> `changes`, `rust`, `contracts` and `engine` all success. That is the first green gate,
> so the "never require a check that has never passed" bar is now met. The ruleset is
> still **off**, for the different reason recorded under Task C below.

### Task B — closed, VERIFIED on a hosted runner

Two distinct root causes, both the predicted ambient-environment shape.

1. **`signal` rc=1 — the contract required Gas Can to be installed on the host.**
   `signal-contract.sh` overrode only `GASCAN_RELEASE_GASCAN`, while
   `release-smoke.sh:48-50` resolves three binaries and lines 51-54 exit **69** if any is
   not executable. This Mac has all three under `/usr/local/bin` (4.9M / 8.2M / 28.4M,
   mode 755), so the two unset ones silently resolved to a real install. Reproduced
   exactly before changing anything: with `GASCAN_RELEASE_GASCAND=/nonexistent/gascand`,
   `release-smoke.sh` exits **69** with `installed gascand is unavailable` — the runner's
   output verbatim. Fixed by pinning all three seams the script already publishes and
   already allowlists (`release-smoke.sh:10-12`); `/usr/bin/true` is faithful because
   `gascan_release_test_signal` runs at line 404, after the traps at 402-403 and before
   `gascan_release_preflight_daemon` at 405, so none of the three is ever executed
   (`5564566`).

2. **`distributable-package` rc=65 and `publish` rc=1 — one cause.** The allowlist
   hardcoded 23 entries: twelve real paths plus eleven AppleDouble `._` records. Those
   records belong to the **build host**. VERIFIED locally: a file created under `TMPDIR`
   carries `com.apple.provenance` and `pkgbuild` emits the paired form; the xattr
   **cannot be stripped** — `xattr -c` and `xattr -rc` both exit 0 and it survives — so
   normalising at build time is not available. The runner's payload listed 12 entries
   with no records. The gate now holds the twelve canonical paths, derives the pairing
   from them with `sed` so the two cannot drift, and accepts either representation but
   only in full: a partial set or an unpaired record is still a rejection (`64ee3ee`).
   `verify-package.sh:29-33` carried the same host-dependence latently and would have
   rejected any package built off a developer Mac; it now accepts either and requires the
   whole payload to agree on one.

VERIFIED on the runner: run `31121170624`, job `92688788633` —
`ci-run-release-contracts: 15 contract(s), status=0`.

Also fixed while in the file: `signal-contract.sh`'s two cleanup assertions **could not
fail**. A `!`-prefixed command is exempt from errexit, so `! compgen -G ...` returned 1
on leftovers and the script carried on to print PASS. Demonstrated with a planted
leftover: the bare form exits 0, the replacement exits 1.

### Task C — the diagnostic gap is closed; the suite is not fixed

**Closed the gap first, as instructed.** `TokioDaemonSpawner::spawn` ended with
`let _child = command.spawn()?` and `DaemonStartupMonitor` carried only a file and an
owner token, so liveness was unobservable *by construction* — the gap was missing
plumbing, not a bug in the loops. The monitor now optionally retains the child and
exposes `exited()`; both wait loops check for death before the clock and report the exit
status, each re-reading its diagnostic first because these fixtures write and then exit
(`50029ae`). Mutation-verified: without the child retained the new test fails at 5.01s,
with it 0.16s.

`FIXTURE_DAEMON_DIAGNOSTIC_DEADLINE` became `FIXTURE_DAEMON_HANG_CEILING` at 60s
(`e8519ea`), and the same treatment reached the rest of the family (`bc89c56`).

**The family is three kinds of site, not one**, which is why "fix the flaky suite" kept
sprawling:

| Kind | Sites | Treatment |
|---|---|---|
| Incidental setup/teardown waits — the clock is not the property | 2 in `crates/gascan`; `autostart.rs` 167, 286, 327, 536 | liveness + hang-only ceiling |
| Relational bounds — the *relation* is the property | `ssh_config.rs` readiness policy (3 uses) | name once, scale together |
| **Absolute latency assertions — the elapsed time IS the property** | `autostart.rs:767` only | **unsolved** |

The third kind is the open design question: `assert!(started.elapsed() < 2s)` cannot be
relaxed without deleting the test, and a required check cannot host an absolute-latency
assertion on a runner whose load is not ours. It needs an answer and does not have one.

### The measurement lesson, which cost most of the session

**Do not trust `ps -o pcpu` as a current CPU reading.** It is a decaying, lifetime-
weighted average. It was read as live usage and produced a confident, wrong claim that
two desktop applications were consuming ~190% CPU; an instantaneous `top -l 2` showed
nothing of the sort.

What *is* VERIFIED is that this machine's exec latency for a freshly written
`#!/bin/sh` script moved across the session: **0.155s**, then **2.304s**, then
**0.187s** — and earlier in the day, ~**30s** with a warm second exec of the same file
at 0.015s. `explicit_state_path_bypasses_default_migration` failed **0/3** in isolation
during the slow window and passed **5/5** once it cleared, and it failed identically at
`64ee3ee`, before any of this branch's Rust changes, so it is the host and not a
regression.

**Consequence: the flake rates measured locally this session — 2/6, 4/6, 7/8 — are not
comparable to each other** and no claim is made from them. They were taken on an
instrument that was drifting. CI is the arbiter for whether the fixes moved the rate.

### Two things still open, honestly

- **The publish-window race is UNCONFIRMED.** Reading the code, `gascand` creates its
  instance record inert at mode `0200`, writes it, and publishes by `fchmod`-ing to
  `0600` (`gascand/src/socket.rs:246-305`), so the mode change *is* the commit. A reader
  during that window sees `0200`, and `validate_file_stat` demands `0600`, and the
  readiness loop (`daemon.rs:1244-1270`) treats that `Unsafe` as **terminal** although it
  already retries two other transient `Unsafe` cases. That is a coherent story for the
  one observed `state Unsafe` failure — but it did **not** reproduce in 6 full-workspace
  runs, so it is a hypothesis. Per this project's own hard-won lesson, it was not fixed
  on that evidence. `validate_file_stat` now names which of its four conditions fired and
  carries the observed mode, links and uid, so the next occurrence answers the question.
- **The `ssh-keygen` rejection**, seen once. `SshError::KeygenRejected` carried nothing
  and the exit status lived only in a `#[cfg(test)]` `eprintln!` that cannot reach a
  `gascand` spawned as a real binary by an e2e test. It now carries a `KeygenOutcome`
  distinguishing an exit code, death by signal, and no status at all (`83ee5bf`).

**Three diagnostics this session existed but could not reach the path that failed** — the
wait loop's empty stderr, one message covering four file faults, and this. Each cost an
investigation the message should have ended.

### `ci.yml` is not on `main` yet, and that reorders the plan

**VERIFIED.** `git ls-tree --name-only origin/main .github/workflows/` lists only
`engine-pin.yml` and `workspace-bundles.yml`. `ci.yml` exists **only** on
`ci/p2-1-pipeline`. Two consequences the plan did not account for:

- **Task 6 Step 7 cannot run before #48 merges.** It asks for a docs-only PR to prove
  `ci / gate` goes green with `rust` and `engine` **skipped** — the case that proves the
  whole topology. PR #50 is exactly that PR, but it branches from `main`, so it carries
  no `ci` workflow and reports no checks at all. That is correct behaviour, not a
  failure. The step has to follow the merge, not precede it.
- **Merging #48 deletes `engine-pin.yml` from `main`**, because the pipeline branch folds
  that build in as the `engine` job. That is the design (spec §5.1) and the job has been
  green on real runners, but it is worth knowing the standalone workflow disappears at
  the same moment.

### GitHub Actions stopped triggering

Runs for `bc89c56`, `83ee5bf` and `8ded364` were **never created** — `gh pr view 48` shows the head
moving and `statusCheckRollup` empty. Actions is enabled (`allowed_actions: all`) and the
`ci` workflow is `active`; the repo is public and org-owned, so this is not minutes
exhaustion. Earlier runs in the same window were queued and then cancelled without
runners ever being assigned (`31124221354`, `31124719097`). Treat as GitHub-side and
retry; it is not a configuration fault in the tree.

> **Confirmed, not just suspected** — see "The Actions outage, named" in the next
> session's section below. This diagnosis was right; it now has an anchor.

### PR state at handover

| PR | State | Notes |
|---|---|---|
| #46 | **MERGED** `29318c3` | spec + plan |
| #47 | **MERGED** `d5cb601` | born-red PTY test fix |
| #48 | **OPEN**, head `8ded364` | the pipeline, plus Tasks B and C; `origin/main` merged in |
| #49 | **MERGED** `e6ef8c0` | the previous handoff |
| #50 | **OPEN** | this record: roadmap + handoff, docs only. Doubles as Task 6 Step 7 **once `ci.yml` is on `main`** |

**The ruleset is still off.** The gate has gone green once, so requiring it is now
permitted — but the suite still fails intermittently, the third category of timing site
has no answer, and the last three commits have no CI result at all. Decide it on CI's
observed rate, not on this machine's.

### Order of operations for whoever picks this up

1. Confirm `ci / gate` on `8ded364` once Actions is creating runs again. Anchor the claim
   to that run ID; every push re-triggers CI, so a gate claim without a run ID is worth
   nothing.
2. Merge #48 with `--merge`. That puts `ci.yml` on `main` and removes `engine-pin.yml`.
3. **Then** Task 6 Step 7 becomes possible: PR #50 should go green with `rust` and
   `engine` reporting `skipped`. Until step 2, #50 legitimately has no checks.
4. Merge #50.
5. Only then the ruleset (plan Task 8), and only if CI's own flake rate justifies it.
   Require `ci / gate`; set `allowed_merge_methods` to `["merge"]`; then confirm it
   actually blocks by checking `mergeStateStatus` is `BLOCKED` on a red PR. A ruleset
   that does not block is not enforcement.

### Closing thoughts, 2026-08-06 (later)

**The pipeline kept paying for itself.** Task B was two contracts that had never been
run anywhere but a developer Mac, and both encoded that Mac's peculiarities as the
definition of correctness: one required Gas Can to be *installed*, the other required
the build host to attach `com.apple.provenance`. Neither is a subtle bug. Both were
invisible because nothing had ever executed them elsewhere. That is now three
categories of defect the gate has surfaced before it was ever required.

**Diagnostics are the deliverable more often than fixes are.** Three separate times this
session a diagnostic existed but could not reach the path that failed: a wait loop
reporting an empty stderr whether the child was slow or dead; one message covering four
distinct file faults; an `ssh-keygen` exit status locked behind `#[cfg(test)]` in a
binary that end-to-end tests spawn for real. Each cost an investigation that the message
itself should have ended in one line. When something is hard to diagnose, the first move
is usually to make it say more, not to guess better.

**My worst habit this session was trusting an instrument I had not checked.** I measured
flake rates across three arms and reported them as if comparable, while the machine
underneath moved by more than an order of magnitude — freshly-written-script exec
latency went 0.155s, 2.304s, 0.187s, and ~30s earlier in the day. Then I explained the
resulting numbers with a confident claim about two applications' CPU use that came from
misreading `ps -o pcpu`, which is a lifetime-weighted average rather than a current
sample; `top -l 2` showed neither process among the top eight. The lesson is the same one
this project already learned three times with warm caches, wearing new clothes: **before
trusting a number, ask what the tool was actually measuring.** Two of my conclusions had
to be withdrawn for exactly this reason, and both withdrawals are recorded above rather
than edited away.

**What I deliberately did not do.** The publish-window race is a coherent, code-anchored
story for a failure seen once, and it did not reproduce in six attempts, so it stays a
hypothesis and the code stays unchanged. This project has been bitten before by a theory
that explained a non-reproduction too comfortably. The sharpened `validate_file_stat`
message means the next occurrence will simply say which condition fired, which is worth
more than a fix aimed at a guess.

**The one genuinely unsolved thing** is the third category of timing site:
`autostart.rs:767`, where `assert!(started.elapsed() < 2s)` *is* the property under test.
Condition-based waiting cannot help, because there is no condition to wait for — the
claim is about elapsed time itself. A required check on a shared runner cannot host that
assertion honestly, and neither relaxing it nor deleting it is obviously right. It wants
a design decision, not a patch.

## Session of 2026-08-06 (evening) — blocked by a GitHub incident, not by the tree

### The Actions outage, named

**VERIFIED.** `githubstatus.com/api/v2/summary.json`, fetched `2026-08-06T21:36Z`:
GitHub Actions is in `major_outage`. Incident "Incident with Actions", started
**`2026-08-06T15:22:49Z`**, status `Investigating`, impact critical. Its own wording:
*"Webhook triggers remain throttled, preventing many push and pull request events from
triggering workflow runs"*, with **~15% of webhooks processing** and runners *"being
assigned invalid jobs"*.

Corroborated inside this repository:

- `gh api repos/:owner/:repo/commits/8ded364/check-runs` → `total_count: 0`. The head of
  #48 has **no checks at all**, so step 1 is not merely unconfirmed, it is unanswerable.
- `gh api repos/:owner/:repo/actions/runs/31124719097` → `run_attempt: 2`,
  `created_at 17:59:14Z`, `run_started_at 19:42:46Z`, `updated_at 20:42:53Z`. It was
  picked up **1h43m** after creation and then burned exactly **60 minutes** to its
  timeout with every job `cancelled`. That is the incident's "invalid jobs", not a fault
  in `ci.yml`.
- `gh api "repos/:owner/:repo/actions/runs?per_page=5"` at `21:36Z`: the newest run in
  the repository is still `31124719097` from `17:59:14Z`. **No run of any workflow has
  been created here in 3h37m.**

The preceding section's call — GitHub-side, not a configuration fault in the tree — was
correct, and now carries an anchor instead of an inference.

**Trigger attempt.** #48 was closed and reopened at `21:37:57Z`. `ci.yml` declares a bare
`on: pull_request`, so the default activity types apply and `reopened` is a valid
trigger; a push would have worked too but would have moved the head and cost the
`8ded364` anchor, which step 1 exists to establish. Head verified unchanged at
`8ded364866f3b275fa7964219ed7e316c109a556` afterwards. Whether the webhook survives the
throttle is the open question.

**Nothing was merged.** The order of operations is load-bearing precisely because step 1
gates it, and step 1 has no evidence. Merging #48 on the strength of run `31121170624`
would be anchoring a green to a *different tree*: `64ee3ee` predates `3b04633`,
`e8519ea`, `bc89c56`, `83ee5bf` and the `origin/main` merge, i.e. all of Task C's changes
and the ssh-keygen diagnostic.

### `autostart.rs:767` is misclassified, and that changes the answer

Line numbers below are `git show ci/p2-1-pipeline:crates/gascan-e2e/tests/autostart.rs`.
The taxonomy filed `accepted_socket_without_http2_cannot_block_initial_probe` (`762-803`)
under *"absolute latency assertions — the elapsed time IS the property"*. Reading the
whole test, that is not what the 2s is:

- `775` — the holder thread accepts, unlinks the socket, then sleeps **3s** before
  dropping the stream.
- `785`, `798` — the deadline and the assertion are both **2s**.

The 3s and the 2s are **coupled**. If the initial probe blocked on an accepted-but-mute
socket it could not finish before the peer let go at 3s, so `< 2s` is a discriminator
against the hold, not a performance budget. Had 2s been an absolute latency requirement,
the holder's 3s would be arbitrary — it is not. **This is a category-2 relational bound
with the relation left unnamed**, the same shape as the `ssh_config.rs` readiness policy
that was already given the "name once, scale together" treatment.

That reclassification is not the whole answer, because of a second defect that scaling
alone does not fix:

- `783-786` — `started` is taken **before** `spawn()`, so the measured interval
  **includes process-spawn latency**. That is exactly the quantity this machine was
  measured swinging `0.155s → 2.304s → 0.187s` within one session. Spawn alone can
  exhaust the entire 2s budget while the probe behaves perfectly. **The clock starts on
  the wrong event.**

Three options, ranked, none of them applied — this is the design decision the previous
session flagged and it is still the maintainer's:

1. **Start the clock at the accept, inside the holder thread, and name the relation.**
   The peer releases at `t_accept + HOLD`; assert the CLI finished before some fraction
   of `HOLD` measured from `t_accept`. Spawn latency is excluded *by construction* rather
   than by tolerance, and the two constants stop being able to drift apart. Residual
   risk: whatever the CLI does *after* abandoning the probe (the socket has been
   unlinked, so plausibly an autostart) is still inside the measured window.
2. **Make the probe's outcome observable and drop the wall clock entirely.** If the CLI
   emitted a structured event on abandoning the probe, the test could assert *ordering* —
   abandoned before the peer released — with no duration in it at all. This is the
   project's own "make it say more rather than guess better" principle applied to a
   timing test, and it is the only option that a shared runner can host honestly.
3. **Scale both constants together under one load multiplier.** Cheapest, preserves the
   discriminator, but keeps spawn latency inside the measurement and so only widens the
   window in which this machine can still lose.

**PLAN, not VERIFIED**: options 1-3 are reasoning from the source, and nothing was run.
The line citations and the two constants are facts of the file.

### `autostart.rs` — the vacuous pass is a soundness bug with a mechanical fix

`daemon_attest_rejects_a_symlink_without_sending_protocol_bytes` (`806-862`) fails
**open**. Both of its reader-thread timeouts yield an empty buffer — `826` returns
`Ok(Vec::new())` when nothing ever connects within 1s, and `841` breaks with `read = 0`
when nothing is sent within 1s — and the assertion at `857-860` is that the buffer **is
empty**. A machine slow enough that `daemon-attest` has not connected inside 1s makes the
test pass **without ever observing the behaviour under test**.

Unlike `762-803`, this needs no design decision, because a real condition is available.
`852` calls `.output()`, which **waits for the CLI to exit**. Once it has exited it can
no longer send bytes, so "no connection ever arrived" becomes *conclusive* rather than
ambiguous. Waiting on process exit instead of a 1s wall clock separates the two cases the
current code conflates:

| Observation | Today | Should be |
|---|---|---|
| CLI exited, never connected | passes (empty) | **passes** — refusing to connect is the strongest possible correct behaviour |
| CLI exited, connected, sent nothing | passes (empty) | passes |
| CLI exited, connected, sent bytes | fails | fails |
| CLI still running, deadline expired | **passes (vacuous)** | **cannot occur** — the wait is on exit |

Not fixed here, and deliberately so: the fix belongs in a test file that #48 already
touches, and committing to `ci/p2-1-pipeline` right now would move the head off
`8ded364` and destroy the anchor step 1 is waiting on. It is the first thing to do after
#48 merges.

### Task 8's plan is written against a repository state that no longer holds

Two corrections, both found while preparing Task 8 read-only during the outage. Neither
was applied — the ruleset is still step 5 and steps 1-4 are not done.

**1. `rulesets` is not `[]`.** Task 8 Step 1 expects it to be, and Step 2 `POST`s a *new*
ruleset. **VERIFIED** `gh api repos/:owner/:repo/rulesets/20492137`: a ruleset named
**`main protection`** (id `20492137`) has been `active` since `2026-08-05T21:47:45-07:00`,
carrying `deletion`, `non_fast_forward`, `required_signatures`, and a `pull_request` rule
with `required_approving_review_count: 0` and
`allowed_merge_methods: ["merge", "squash", "rebase"]`. It has **no**
`required_status_checks`, so "the ruleset is still off" was right about the *gate* while
protection itself was already on.

`POST`ing a second ruleset over the same `~DEFAULT_BRANCH` would leave merge-method
policy stated in two places and make the effective behaviour depend on how GitHub
combines overlapping rulesets. **`PATCH` the existing one instead** — one ruleset, one
place, no union semantics to reason about. `PATCH` replaces the `rules` array wholesale,
so the payload must restate the three rules being kept.

**2. The required context is `gate`, not `ci / gate`.** Task 8 Step 2 specifies
`context=ci / gate`. **VERIFIED** `gh api repos/:owner/:repo/commits/64ee3ee/check-runs`:
the check runs are named `gate`, `contracts`, `engine`, `rust`, `runtime-probe`,
`changes` — **bare job names** — all from app `github-actions`, `id 15368`. `ci / gate`
is the UI's *display* form, `workflow / job`. Requiring the literal string `ci / gate`
would require a check that never reports, and GitHub would hold it pending forever. That
is the same failure this project already documented for workflow-level `paths:` filters,
arriving by a different door. Pin `integration_id: 15368` so the requirement cannot be
satisfied by a same-named check from another app.

The corrected payload is prepared but **not applied**.

**3. A consequence worth weighing before step 5.** The existing ruleset has
`bypass_actors: []` and reports `current_user_can_bypass: "never"`. Adding
`required_status_checks` to it therefore creates enforcement with **no override for
anyone, including the maintainer**. Given that the suite is still intermittently red and
`autostart.rs`'s category-3 site has no answer, a flaky `gate` would not merely be noisy
— it would be unbypassable. Either accept that, or add a deliberate `bypass_actors`
entry as part of the same change rather than discovering the need during an incident.
That is a maintainer's call and is recorded here rather than made.

## Session of 2026-08-06 (night) — the sequence completed; `ci / gate` is required

All five steps of the previous handoff's order of operations are done. Every claim below
carries its run ID, SHA or API response.

### What landed

| Step | Result | Anchor |
|---|---|---|
| 1. `gate` green on `8ded364` | **VERIFIED** | run `31129682364`, `gate` job `92717021965` = `success` |
| 2. Merge #48 `--merge` | **VERIFIED** | `c87787cca1b0d9f617e9e58ec74a989b7336a029`, parents `e6ef8c0` + `8ded364` |
| 3. Task 6 Step 7 (skipped topology) | **VERIFIED** | run `31131481000`: `gate` `success`, `rust` **skipped**, `engine` **skipped** |
| 4. Merge #50 | **VERIFIED** | `4852b04786404cb2e6d2fd0e5ee4a22398e7325a`, two parents |
| 5. Task 8 ruleset + enforcement | **VERIFIED** | run `31134223492`, `gate` job `92729989072` = `failure`, `mergeStateStatus: BLOCKED` |

`ci.yml` is on `main` and `engine-pin.yml` is gone (`git cat-file -e origin/main:.github/workflows/engine-pin.yml` fails). Both directions of the gate are now proven on real runners: green with every job running (`31129682364`), green with `rust` and `engine` `skipped` (`31131481000`), and red propagating (`31134223492`).

### The ruleset, as actually applied

Ruleset `20492137` (`main protection`) was **`PUT`**, not `POST`ed — no second ruleset
was created. **The endpoint is `PUT`; `PATCH` returns 404.** That cost one round trip.

Final state, from the API's own response: `required_status_checks` =
`[{"context": "gate", "integration_id": 15368}]`, `strict_required_status_checks_policy:
false`; `pull_request.allowed_merge_methods` = `["merge"]`; `deletion`,
`non_fast_forward` and `required_signatures` preserved; `enforcement: active`.

**`bypass_actors`** = `[{"actor_type": "OrganizationAdmin", "bypass_mode": "always"}]`,
added *after* the blocking proof, deliberately. Had it gone on first, a red PR might have
reported mergeable **to me**, and "bypass works" would have been indistinguishable from
"enforcement is broken". Applying it bare, proving `BLOCKED`, then granting bypass keeps
the two claims independent. GitHub normalises `actor_id` to `null` for this actor type.

> **Correction, recorded in place.** I predicted #51's `mergeStateStatus` would change
> once the bypass landed. **It did not** — it stayed `BLOCKED` while
> `current_user_can_bypass` went `"never"` → `"always"`. `mergeStateStatus` reports the
> *branch policy* and is viewer-independent; it does not account for who may override.
> **Practical consequence: `gh pr view` and the web UI will keep saying `BLOCKED` on a
> flaky PR even though an org admin can merge it.** `BLOCKED` is not evidence the bypass
> is missing — `current_user_can_bypass` is the signal that answers that question.

The override was **not** tested by merging #51, which deliberately broke `cargo fmt`;
proving an API-stated fact by putting broken formatting on `main` is not worth it. #51 is
`CLOSED` and its branch deleted.

### The flake is real on CI, and it is not event-dependent

**VERIFIED, and this is the strongest evidence the project has.** `8ded364` and
`c87787c` have the **same tree hash**, `2c7de3044efbb8c05c9864feb1d0ad7b1031f01d`
(`git rev-parse <sha>^{tree}`). The PR run checked out `refs/pull/48/merge`, whose tree
equals `8ded364`'s because `e6ef8c0` is a direct parent. Same content, no caching (D8),
same pinned toolchain, both `macos-26`, 18 minutes apart:

| Run | Commit | Event | `rust` |
|---|---|---|---|
| `31129682364` | `8ded364` | pull_request | success |
| `31130737502` | `c87787c` | push | **failure** |
| `31131820848` | `4852b04` | push | success |

**1 failure in 3.** No rate is claimed from n=3 — what is established is that it is
**non-zero on CI**, on identical input, which no local measurement could have shown.

**A push/pull_request divergence was hypothesised and ruled out.** `ci.yml` contains
exactly five `if:` conditions (lines 38, 64, 79, 97, 113). Only two behaviours depend on
the event: `runtime-probe` is PR-only (`ci.yml:97` — which fully explains its `skipped`
on push versus `failure` on pull_request), and `ci-detect-changes.sh` forces all three
areas true on non-PR events because a push has no reliable base. Neither touches the
`rust` job's body, which contains **zero** conditionals. The same code ran the same way
and diverged.

**Maintainer's decision:** accept the flakiness for now, re-run flaky jobs, and keep
watching for the cause. The `OrganizationAdmin` bypass exists so a flake — or another
Actions outage in which no run can even be created — cannot wedge the repository.

### The failure named a new site, and it said nothing

`autostart.rs` passed **16/16** in the failing run, including
`accepted_socket_without_http2_cannot_block_initial_probe` and
`explicit_state_path_bypasses_default_migration`. Task C's fixes are holding. The failure
was `doctor_human_output_names_each_check` at `doctor.rs:763:5` — and it printed an
**empty message**, because the assertion's message was the child's `stderr` while
human-mode `doctor` writes its report to **stdout**.

Four of the six sites in that file already reported status, stdout and stderr; two had
drifted onto the bare-`stderr` form, and one of those is the site that failed. All six
are now consolidated onto one `describe_output` helper (`947a058`), which also names the
exit status — `ExitStatus`'s `Debug` prints a plain exit code 1 as
`unix_wait_status(256)`.

**Mutation-verified**, not merely compiled: with the assertion forced false, the message
is now `exit code 0, stdout=Gascan is ready\n  Host  2/2 checks passed…, stderr=`. The
identical failure previously printed nothing. `cargo fmt --all --check` rc=0,
`cargo clippy --workspace --all-targets -- -D warnings` rc=0,
`cargo test -p gascan-e2e --test doctor` **11 passed** (full module path, so the
silent-zero-tests trap does not apply).

That is the **fourth** time this session's family of work has hit "a diagnostic existed
but could not reach the path that failed". It remains the highest-yield class of fix in
this project.

### Still open, unchanged

- **`autostart.rs:767`** — reclassified above as a relational bound whose clock starts
  before `spawn()`. Three options are written up; **the design decision is still open**.
- **The publish-window race** — still a hypothesis, still not fixed, `validate_file_stat`
  still sharpened for the next occurrence.
- **The `ssh-keygen` rejection** — `KeygenOutcome` in place, awaiting a recurrence.
- **`autostart.rs:802`'s vacuous pass** — analysed in the previous section; the fix is
  mechanical (wait on process exit, not a 1s wall clock) and was **not** applied.

### The `ssh-keygen` rejection is narrowed to one invocation — 2026-08-06 (night)

Last session's `KeygenOutcome` instrumentation paid off twice tonight, and the answer is
**not** where the previous note assumed. **VERIFIED** by running the real binary:

| Invocation (`env -i /usr/bin/ssh-keygen …`) | Exit |
|---|---|
| `-y -f /dev/null` → `Load key "/dev/null": invalid format` | **255** |
| `-y -f /dev/fd/9` (descriptor absent) → `Bad file descriptor` | **255** |
| `-y -f <valid key>` (control) | 0 |
| `-q -t ed25519 -N "" -C gascan-managed -f <fresh path>` (control) | 0 |
| same, target already exists → `Overwrite (y/n)?` then EOF | 1 |
| same, parent directory missing → `Saving key … failed` | 1 |

**255 is an argument/usage rejection, not a filesystem error** — the generate path's
failures exit **1**. So `KeygenRejected(Code(255))` (`identity.rs:424`) can only be the
**public-key derivation** at `identity.rs:275-293`, `ssh-keygen -y -f /dev/fd/<N>`, which
reads the private key through a descriptor duplicated by
`rustix::io::fcntl_dupfd_cloexec(private_file, 3)` and mapped in with `fd_mappings`. The
generate call at `identity.rs:164` is excluded by its own exit codes.

Two candidate causes remain, and both fit a failure that only appears under load:

1. **The descriptor never reached the child** — `Bad file descriptor`. The dup targets
   the lowest free fd **≥ 3**, and fd numbers are process-global, so under a
   multithreaded tokio runtime with concurrent spawns fd 3 is contended.
2. **The child read the wrong bytes** — `invalid format`.

**They are distinguished by a single line of stderr, which the error throws away.**
`run_configured_ssh_keygen` keeps only a `Sha256` of stderr behind `#[cfg(test)]`
(`identity.rs:407-414`), which cannot reach a `gascand` spawned as a real binary — the
same gap `KeygenOutcome` was created to close, one level further in. **Next step: carry a
bounded, redacted stderr prefix (or at minimum discriminate those two known messages) in
`KeygenOutcome`.** That single line decides between the two hypotheses; guessing between
them without it would repeat the mistake this project has already paid for twice.

**Reproduced locally**, so this one is not CI-only:
`cargo test --workspace` on `main` + both open PRs gave **1078 passed, 1 failed, 22
ignored** across 47 binaries, the failure being
`ssh_image_apply_preserves_fingerprints_while_accepting_new_inspected_automatic_port`
with `SshConfigUnsafe(KeygenRejected(Code(255)))` — the same error CI hit in run
`31136420663`.

### The required check was turned on, and then turned back off — the decision that closed the session

**The gate became required and then stopped being required, deliberately, within about
two hours.** Both halves are recorded because the reversal is the useful part.

`ci / gate` was made required on ruleset `20492137`, and enforcement was proven (run
`31134223492`, `gate` job `92729989072` = `failure`, `mergeStateStatus: BLOCKED`). The
suite could not carry it. Measured that night — **7 `rust` executions, 2 green, 5 red,
across 4 distinct tests in 3 crates**, each re-run surfacing a different one:

| Run | `rust` | Failing test |
|---|---|---|
| `31129682364` | green | — |
| `31130737502` | red | `doctor_human_output_names_each_check` (gascan-e2e) |
| `31131820848` | green | — |
| `31134866220` att. 1 | red | `presentation::tests::interactive_progress_replaces_message_and_finishes_with_checkmark` |
| `31134866220` att. 2 | red | the same test again |
| `31136420663` att. 1 | red | `complete_unknown_policy_matching_observed_ssh_up_establishes_policy` (gascand) |
| `31136420663` att. 2 | red | `provision_and_health_kill_point_phase_matrix_has_exact_recovery_status` (`reconcile.rs:965`) |

**Maintainer's decision, and it governs the next session:** *"Can we just skip CI and
test locally until this thing is built? We are letting a perfect CI system get in the way
of actually working on this thing."* And: *"For now if it passes the suite on this
machine, it's a pass, we can make CI stable once we have completed everything else and we
know it works with Arca as a backend."*

Ruleset `20492137` was re-`PUT` without `required_status_checks`. It now carries
`deletion`, `non_fast_forward`, `required_signatures` and `pull_request` with
`allowed_merge_methods: ["merge"]`, plus an `OrganizationAdmin` bypass. Both open PRs
went `BLOCKED` → `UNSTABLE` and merged. **CI still runs and still reports; it does not
gate.** Do not re-require it without being asked.

> **Note on the endpoint**, since it cost a round trip: repository rulesets are updated
> with **`PUT`**, not `PATCH`. `PATCH` on `/repos/{owner}/{repo}/rulesets/{id}` returns
> **404**. `PUT` replaces the `rules` array wholesale, so every rule being kept must be
> restated in the payload.

### What landed on `main`

| PR | Merge commit | Contents |
|---|---|---|
| #48 | `c87787cca1b0d9f617e9e58ec74a989b7336a029` | the consolidated pipeline; `engine-pin.yml` folded in as the `engine` job |
| #50 | `4852b04786404cb2e6d2fd0e5ee4a22398e7325a` | P2.1 record, U3 resolved |
| #53 | `ee3be3b490433eeeca65c226f8dad28424d868e2` | `presentation.rs` condition-based waiting |
| #52 | `9623f4be53e773c93bdedf2a5ceca76941017f81` | `doctor.rs` diagnostics, Task 8 corrections, this record |

All four are true merge commits with two parents each. `main` is `9623f4b`.

### The local suite, which is now the bar

**VERIFIED.** `cargo test --workspace` on `main` + both fixes:
**1078 passed, 1 failed, 22 ignored, 47 binaries.** The single failure is the
`ssh-keygen` issue narrowed in the section above — `ssh-keygen -y -f /dev/fd/<N>` at
`identity.rs:275-293`, exit 255, two candidate causes separated by one line of stderr the
error currently discards.

**That is the first thing to fix next session**, and it is product work rather than CI
work: it is the only locally reproducible failure, and it sits in the managed-SSH path
that P3 and P5 both depend on.

### Closing thoughts, 2026-08-06 (night)

**The pipeline kept paying, and then started charging.** It surfaced two release
contracts that encoded one developer Mac as the definition of correctness, and it
produced the `ssh-keygen` reproduction that is now nearly solved. Then it was made
blocking over a suite with a measured red rate, and the rest of the session went to
servicing the gate rather than the integration. Both facts are true at once. The error
was not building the pipeline; it was making it load-bearing before the suite could hold
the load, and then continuing to fix individual tests after the third distinct failure
should have said the cause was shared.

**Six times tonight a diagnostic could not reach the path that failed** — the doctor
assertion printing an empty stderr, the bare `assert!` in `presentation.rs`, and the
`ssh-keygen` stderr reduced to a `Sha256` behind `#[cfg(test)]`, among others. Two were
fixed and mutation-verified. The `KeygenOutcome` added *last* session is what made
tonight's narrowing possible at all: it turned "ssh-keygen failed, unknown why" into
"exit 255", and exit 255 turned out to exclude an entire call site. **Instrumentation
compounds; guesses do not.**

**What I got wrong, recorded rather than smoothed over.** I asserted a 16.7 ms margin
between a 12 Hz redraw and a 100 ms sleep as the cause of the `presentation.rs` flake;
setting the sleep to **0 ms** and watching it pass 10/10 disproved it outright. I
predicted #51's `mergeStateStatus` would change once the bypass landed; it stayed
`BLOCKED`, because that field reports branch policy and is viewer-independent. And I told
the maintainer "five distinct tests" when it was five failing *executions* of **four**
distinct tests. The first two are corrected in place above; the third is corrected here.

## Session of 2026-08-07 — the `ssh-keygen` rejection said what it was, and the obvious fix lost

`ci / gate` was left non-required throughout, per the governing decision. No CI work was
done. PR #54 was already merged at session start (`70bd9ba`, the merge commit of
`docs/close-2026-08-06`); `main` began and ends this session as a descendant of it.

### The instrumentation paid off on the first reproduction

`KeygenOutcome` alone could not separate the two candidate causes, because both exit
**255**. `SshError::KeygenRejected` now carries a `KeygenRejection`: the outcome, a
**bounded redacted `KeygenMessage`**, and a `MappedDescriptor` witness.

Redaction replaces the exact pathname strings the invocation was given, rather than
guessing at what a path looks like — `ssh-keygen` only ever echoes back its own file
argument, so that removes the sole caller-specific content and leaves its fixed message
table intact. **Mutation-verified**: with a deliberately unreadable key at mode 0644 the
message came back as
`@@@… WARNING: UNPROTECTED PRIVATE KEY FILE! @@@… Permissions 0644 for '<path>' are too…`
— redaction, whitespace collapsing and the 200-character bound all visible in one line.

**VERIFIED, and it settles the question the last session left open.** Reproduced by
looping five gascand test binaries (`apply_setup ssh_config ssh_identity lifecycle
reconcile`) while a full workspace run loaded the machine — **run 5 of 8**:

```
Unpublished(KeygenRejected(KeygenRejection {
  outcome: Code(255),
  message: KeygenMessage("/dev/fd/7: Bad file descriptor") }))
```
in `ssh_config::rejects_symlink_hard_link_fifo_and_unsafe_generated_targets`.

**Cause (a) is confirmed and cause (b) is excluded.** The descriptor never reached the
child. The child did not read wrong bytes.

### The second witness says the parent was fine

`MappedDescriptor` re-resolves the parent's own `/dev/fd/<N>` immediately after `spawn()`
returns — while the mapping still owns the descriptor, and the last instant at which the
parent's view can still explain the child's. Every captured occurrence reads
**`parent descriptor intact`**.

> **Correction, recorded in place.** The witness first compared `st_dev` *and* `st_ino`
> and therefore reported **`Replaced`** on every single rejection. That was my bug, not
> evidence. **Measured**: `stat` of a file directly and through its own `/dev/fd/<N>`
> entry agree on `st_ino` and **disagree on `st_dev`** — Darwin's `fdesc` filesystem
> reports the real inode but substitutes its own device. The witness now compares the
> inode only. Any earlier `Replaced` reading is void.

So: the parent held the private key at that number when the child was forked, and the
child still could not open it. **The loss is in the fork/exec path, not a parent-side
descriptor stomp.**

### The obvious fix was tried, measured, and reverted

`command_fds` maps `parent_fd == child_fd` by merely clearing `FD_CLOEXEC`, which leaves
the child depending on the parent's allocation surviving `exec`. Giving the child a fixed
number instead (3) makes `command_fds` take its `dup2` branch, which *installs* the
descriptor and fails loudly if it cannot. That reasoning is sound and the result is
**wrong**.

| Arm | Scheme | Amplifier failures under load |
|---|---|---|
| A | child descriptor pinned to 3 | **6 / 28** |
| B | child descriptor = parent's number (shipped) | **0 / 28** |

Same machine, same background load (`top -l 2`: 44.1% user for A, 44.5% for B), same
amplifier parameters, built and run back to back. Arm A also failed **twice in a single
`cargo test --workspace --no-fail-fast` run**, including inside the amplifier at round 3
of 4. **The change was reverted.** The comment at `identity.rs`
`derive_public_key_with_spawn_hook` records the measurement so the next person does not
re-derive the same appealing wrong answer.

**This is the third time this project has paid for a plausible mechanism.** The
difference is that this one cost one A/B instead of a session, because the amplifier
existed to measure it.

### What made the measurement possible

`crates/gascand/tests/ssh_identity_concurrency.rs` — concurrent identity derivation on a
multi-threaded runtime, with descriptor churn and unrelated `fork`/`exec` traffic from
other threads, driving both `ensure_host_identity` and the
`open_revalidated_identity` route that runs its spawn on a scoped thread inside a freshly
built runtime. Tunable by `GASCAN_IDENTITY_STRESS_ROUNDS` / `_WIDTH`.

It does **not** reproduce the defect on an idle machine (0 failures in ~15 000 spawns);
it reproduces under load. A failure of this test is the known defect, not a new flake,
and its header says so.

### Where the defect stands — NOT FIXED

**The mechanism is still unknown, and the rejection is still possible.** What is now
established, each with its anchor above:

- It is `Bad file descriptor`, not bad key bytes. **VERIFIED.**
- The parent's descriptor was intact at fork. **VERIFIED.**
- Pinning the child's descriptor number is worse, not better. **VERIFIED, 6/28 vs 0/28.**
- `std` wires the child's stdio **before** running `pre_exec` closures, so the mapping is
  the last thing to happen before `exec` and nothing in `std` can undo it. **VERIFIED**
  by a standalone probe: a `pre_exec` writing to descriptor 1 landed inside the captured
  pipe, ahead of the child's own output.

The open question is therefore narrow: **what closes, or fails to deliver, a descriptor
that is open and non-`FD_CLOEXEC` in the child at the moment `execve` is called?** The
next step is a minimal standalone reproduction — the production spawn shape with
`/bin/sh -c 'ls -1 /dev/fd'` as the child — which arm A's ~20% failure rate makes cheap
to obtain, and which would answer it directly rather than by argument.

### Suite state

**VERIFIED.** `cargo test --workspace --no-fail-fast`, twice, after the revert:
**1376 passed, 1 failed, 22 ignored** both times, with **zero** `Keygen` occurrences.
`ssh_image_apply_preserves_fingerprints_while_accepting_new_inspected_automatic_port` —
last session's single reproducible failure — **passes**.

`--no-fail-fast` matters and is new: without it `cargo test --workspace` stops at the
first failing binary, so an early flake hides every later binary, including the gascand
ones this session needed. The first hunt lost a whole run to exactly that.

The one remaining failure is a **different** and previously unrecorded flake, in
`gascan-e2e/tests/fake_backend.rs`, and it was a different test each time:
`real_pty_large_output_waits_for_capacity_without_exiting` (`:1087`),
`interactive_streamed_operation_failure_clears_spinner_before_error` (`:625`),
`follow_logs_emit_exactly_one_terminal_for_shutdown_or_backend_error` (`:1475`). All
three are bare `assert!`s that print nothing — the same "a diagnostic existed but could
not reach the path that failed" family, now the fifth occurrence. Note that the two
`real_pty_*` "signal test mutex poisoned" errors seen alongside the first are
**consequences** of it poisoning a shared mutex, not three independent failures.
Per the governing decision this belongs to the flaky-suite family and was left alone.

### Closing the session — the next one starts on the roadmap

Everything above is defect and instrumentation work. It is finished and parked. **The
next session's subject is `docs/superpowers/plans/2026-08-04-arca-integration-roadmap.md`,
starting at P3.1.**

**Why P3.1 is the next thing.** P3 is the fan-out point: P4 and P5 both depend on it, and
its exit is deliberately modest — *"proto exists, both sides generate, nothing implements
it yet."* P1 is `partial by necessity` and stays that way; its binary half is booked
against P5.1 and P4.3 and must not be "finished" opportunistically. So nothing else is
unblocked, and P3.1 carries **U4** with it.

**What P3.1 needs before code.** The proto is derived from `RuntimeBackend`, constrained
by contract §4 (what must be *inexpressible*) and §5 (what must be *expressible*), and —
per the 2026-08-05 weight increase — it is a **published contract with more than one
consumer over time**, so its compatibility burden is real from the first commit. It lives
in Arca. Note that `arca` has been untouched for four sessions and is still `main
7da8f77`, clean; the pin resolves via tag `gascan-engine-ip-internal` to commit
`d66c320c` (the annotated tag *object* is `dfdf8b9` — different thing).

**Open decisions are collected in the register below rather than scattered through this
document.** None of them block P3.1.

### Decision register — what is waiting on the maintainer

> **SUPERSEDED.** D1, D2, D3 and D6 were all decided or answered after this table was
> written. It is kept because the maintainer decided from *this* list; the current state
> is the register at the end of this document.

| # | Decision | Blocks | State |
|---|---|---|---|
| D1 | `autostart.rs:767` — which of three options | nothing; test currently passes | three options written up above, none applied |
| D2 | How much further to chase the `ssh-keygen` descriptor defect | P3/P5 only if it worsens | mechanism unknown; cause class VERIFIED |
| D3 | `gascan-e2e/tests/fake_backend.rs` flake | local-suite-green bar | newly recorded this session, uninstrumented |
| D4 | Delete `runtime-probe` from `ci.yml` | nothing; cosmetic but persistent | spec §7.2 says the job is temporary; §11.5 recorded its VERIFIED answer |
| D5 | `stash@{0}` `f6356f9` — keep or drop | nothing | **ANSWERED below**; maintainer's call whether to drop |
| D6 | The flaky suite as one shared cause | the local-suite bar, eventually | **largely answered**: Spotlight indexing `target/` was the dominant cause; excluding `~/code` took a workspace run from 37 failures to 1 |
| D7 | How the health check should treat modes `0200` (inert / tombstoned) | nothing yet; it is a real flake | mechanism now VERIFIED, remedy is a design choice |

**D5 is answered.** `git stash show --name-only stash@{0}` lists **exactly one file**:
`.superpowers/sdd/progress.md`, and `git check-ignore -v` confirms `.gitignore:1:.superpowers/`
matches it. The stash therefore holds **no tracked content at all** — it is an append to the
gitignored SDD progress log from the 0.1.20 release era, recording task-completion notes for
signed-release-distribution, actionable-errors and release-driver. By this project's own
convention (`docs/superpowers/` is tracked; `.superpowers/` is disposable scaffolding) there is
nothing in it to lose. It was left in place rather than dropped, because dropping it is not mine
to do.

**U4, U5 and U6 are not in this register.** They are design work inside the roadmap —
U4 belongs to P3.1 and is next session's actual subject; U5 belongs to P5.4 and U6 to
P6.3. They are not maintainer decisions to be made in advance of that work.

**Explicitly still true and unchanged:** `ci / gate` is **not** a required check and
should not be made one; ruleset `20492137` carries `deletion`, `non_fast_forward`,
`required_signatures` and `pull_request` with `allowed_merge_methods: ["merge"]`, plus an
`OrganizationAdmin` bypass. A green `cargo test --workspace` on this machine is the bar.
The `autostart.rs` symlink test (`daemon_attest_rejects_a_symlink_…`) still fails **open**;
its fix is mechanical, needs no decision, and is a good warm-up task for whoever wants one.

## Session of 2026-08-07 (continued) — the latency probe was measuring the wrong thing

### D3 resolved: `fake_backend.rs` failures now say what the child printed

**Maintainer's decision: instrument it.** Done, and the scope was larger than the three
observed failures. A parser that strips string literals and looks for a top-level comma
found **50** `assert!` blocks in `crates/gascan-e2e/tests/fake_backend.rs` asserting
`.success()` with **no message at all** — including all three that failed today
(`:625`, `:1087`, `:1475`). An earlier count of 14 was wrong because it skipped any block
containing a string literal, which excluded two of the three.

`describe_output`/`describe_status` moved from `tests/doctor.rs` into
`crates/gascan-e2e/src/lib.rs`, joined by `succeeded(Output) -> Output` and
`status_succeeded(&ExitStatus)`, both `#[track_caller]`. Every test binary in this crate
defines its own `Environment`, so a shared helper had to live in the lib rather than be
copied a third time. `succeeded` returns the output so a bare
`assert!(x.status.success())` becomes `succeeded(x)` without the caller naming a
temporary; where the value is used afterwards it rebinds (`let x = succeeded(x);`).

**Mutation-verified.** Pointing the `up` in
`real_pty_large_output_waits_for_capacity_without_exiting` at a nonexistent root now
prints
`exit code 64, stdout=, stderr=Error: cannot use `/nonexistent-root-for-mutation` as a
project root: No such file or directory (os error 2)`
at `fake_backend.rs:1064` — the call site, because of `#[track_caller]`. The same failure
previously printed `assertion failed: env.invoke(...)?.status.success()` and nothing else.
`cargo test -p gascan-e2e --test fake_backend --test doctor` passed 3/3 after the rewrite;
`cargo fmt` and `clippy -D warnings` clean.

### The exec-latency probe is invalid, and every comparison made with it was blind

> **Correction to a load-bearing convention.** The trap reads: *"Before comparing any two
> test runs, measure it: write a fresh script, `time` it."* **A shell script cannot see
> this phenomenon.** Measured within one minute, on this machine:
>
> | What was executed | Time |
> |---|---|
> | `#!/bin/sh` script — what the trap prescribes | **0.005 s** |
> | Rust test binary already executed once | **14.2 s** |
> | The same bytes copied to a brand-new path | **32.8 s** |
> | Another new path | **23.5 s** |
>
> Three orders of magnitude apart, at the same instant. Every "both arms saw the same
> machine state" check made with the script probe was measuring something immune to the
> effect it was written to detect.

**The named cause is macOS's own security and indexing daemons over freshly built
binaries.** `top -l 2 -o cpu` during the slow window: `syspolicyd` **42.5%**, then
`spotlightknowledged` **193.7%**, `corespotlightd` 27.1%, `XprotectService` 16.5% — with
the machine reporting 65% *idle* CPU and a load average of 8.10. Gatekeeper evaluates
unsigned, newly written Mach-O files, and Spotlight indexes `target/`. As the indexers
settled, cold exec fell from 32.8 s to **0.753 s** while the script stayed at 0.005 s.

**This is very likely the long-unexplained instability**, including the `~30 s` figure
recorded earlier. It also explains a run in this session that was abandoned after 10
minutes: **10 failures, every one in `autostart.rs`** — the daemon-spawn tests, whose
budgets are seconds — followed by a hang in `attach_bridge`. That run was environmental.
The same tree had already passed `1376 passed, 1 failed` **twice**, and no `gascand`,
`arca` or stray `cargo` process was alive at the time (checked by `ps`, not assumed).

**The correct probe** is to `cp` a built test binary to a new path and `time` it — a new
path matters, because an already-evaluated binary is cheaper than a fresh one. Two lines,
and it is the only form that sees the effect.

**Not applied, because it changes the machine rather than the repository:** excluding
`target/` from Spotlight indexing is the obvious mitigation and would plausibly stabilise
the whole suite. That is the maintainer's call, not mine — it is a system setting.
**This is now the leading candidate for D6**, and it is a much cheaper hypothesis than
the `--test-threads` oversubscription one, which remains unmeasured.

### Spotlight was the cause, and the suite came back — 2026-08-07

**The maintainer excluded `~/code` from Spotlight indexing** (107 repos, 40 build trees;
`~/code/gascan/target` alone is 18 GB across 174,739 files). Verified in effect:
`mdfind -onlyin /Users/kiener/code -name Cargo.toml` returns **nothing**.

**VERIFIED, comparing whole runs with the corrected probe:**

| | cold exec after a full workspace run | failures |
|---|---|---|
| Before the exclusion | **7.818 s** | **37** |
| After the exclusion | **0.188 s** | **1** |

**Two confounds, stated rather than buried.** (1) The maintainer had concurrent `cargo`
builds for other projects running during the earlier measurements, so machine state was
never as controlled as "same load" claimed — this is a third candidate for the
long-unexplained latency swings, alongside Spotlight and Gatekeeper, and it means the
`6/28 vs 0/28` descriptor A/B, though large and taken back-to-back with matching CPU
readings, is not airtight. (2) Adding a path to the privacy list makes macOS **delete**
the existing index for it, and that teardown is itself expensive: the `probe_before` in
the bracketed run read **40.389 s** because it was taken during exactly that, which is
why the before/after pair inside one run is not the comparison that matters.

**The Gatekeeper half is untouched by the exclusion.** `syspolicyd` was at 42.5% in the
first slow window and 33.7% after the suite went green. It is not currently breaking
anything, but a Spotlight exclusion does not address it and it should not be assumed gone.

**D3 and D1 are verified against a full workspace run**: 1376 passed, 1 failed, 22
ignored — identical to the two runs taken before either change.

### The publish-window race is no longer a hypothesis

The single remaining failure was `doctor_uses_the_callers_workspace_after_the_daemon_launch_directory_is_deleted`
(`gascan-e2e/tests/doctor.rs:782`):

```
started daemon did not become healthy and current (state Unsafe):
protected runtime file is unsafe: mode is not 0600
  (mode 0200, links 1, uid 501, expected uid 501)
```

`validate_file_stat` (`crates/gascan/src/daemon.rs:3057`) named which of its four
conditions fired and carried the observed value, which is the whole reason this is
legible. Its own doc comment at `daemon.rs:3050-3056` had already written down what the
value means: **"mode 0200 is the daemon's own not-yet-published record (`gascand` creates
it inert and publishes by chmod-ing to 0600)"**. `INSTANCE_TOMBSTONE_MODE` is the same
`0o200` (`daemon.rs:18`, applied at `:1373`).

So the reader observed the record **inside the publish window** and reported a legitimate
transient daemon state as tampering. That is the mechanism the previous session
hypothesised, now with an instance captured and the condition named.

**Not fixed, and deliberately so** — the previous instruction was not to fix it on a
single unreproduced sighting, and the remedy is a design question rather than a
mechanical one: the health-check path must distinguish "inert, not yet published" and
"tombstoned" from "unsafe", and decide whether that means retry, treat-as-absent, or
wait. **This is a new entry for the decision register.** The instrumentation that made it
legible was added two sessions ago and cost nothing to carry until it fired —
instrumentation compounds.

### The confirmation run, which removes the confound — 2026-08-07

The measurement above was taken while Spotlight was still tearing down its index and
while the maintainer had concurrent `cargo` builds running, so it could not separate the
exclusion from those. A second run on merged `main` (`780efbd`), with neither in flight,
is clean:

| | cold exec before run → after run | result |
|---|---|---|
| Before the exclusion | 0.235 s → **7.818 s** | 37 failed |
| After the exclusion | 0.247 s → **0.184 s** | **0 failed** |

**`cargo test --workspace --no-fail-fast` rc=0: 1377 passed, 0 failed, 22 ignored.** Zero
`Keygen` occurrences. This is the first fully green workspace run in this family of
sessions, and by the governing decision — *"if it passes the suite on this machine, it's
a pass"* — **the bar is met on `main`**.

The within-run comparison is what matters: exec latency stayed **flat** across an entire
workspace run rather than degrading 33×. D6's conclusion no longer rests on a single
confounded observation.

**Still true:** `syspolicyd` is untouched by a Spotlight exclusion and sat at 33.7% after
the suite went green. If timing flakes return *without* `corespotlightd` being hot,
Gatekeeper is the place to look. And the publish-window race (**D7**) did not fire in this
run — it is intermittent, not fixed.

## Session close, 2026-08-07 — the bar is met and the next subject is P3.1

### Final state

**VERIFIED.** `main` is `6f88e79`, clean, **zero open PRs**.
`cargo test --workspace --no-fail-fast` **rc=0: 1377 passed, 0 failed, 22 ignored**, with
cold-binary exec flat across the run (0.247 s → 0.184 s). By the governing decision this
**is** a pass, and it is the first unqualified one in this family of sessions.

`arca` is `7da8f77`, clean, and has now been **untouched for five sessions**. The pin still
resolves through tag `gascan-engine-ip-internal` to commit
`d66c320c09e1dfc4f37aafa1fb27e36aa5cabe5d`; the annotated tag *object* is `dfdf8b9` and is
a different thing. Arca's `main` is deliberately ahead of the pin. **P3 is where that
changes** — the proto lives in Arca, so the next session is the first to write there.

Three PRs landed today, each a true merge commit: **#55** (the `ssh-keygen` rejection names
its own cause; the appealing fix disproved), **#56** (D3, D1, the probe correction),
**#57** (the Spotlight confirmation).

### Decision register — current

| # | Decision | State |
|---|---|---|
| D1 | `autostart.rs:767` | **RESOLVED** — option 2, via the peer rather than a new CLI event (#56) |
| D2 | Chasing the `ssh-keygen` descriptor defect | **DECIDED: defer.** Amplifier is in the suite and will announce a recurrence |
| D3 | `fake_backend.rs` silent failures | **RESOLVED** — 50 sites instrumented (#56) |
| D4 | Delete `runtime-probe` from `ci.yml` | **OPEN.** Real, cosmetic, and CI work — so deferred by the governing decision |
| D5 | `stash@{0}` `f6356f9` | **ANSWERED**: one gitignored file, no tracked content. Dropping it is the maintainer's call |
| D6 | The flaky suite as one shared cause | **ANSWERED**: Spotlight indexing `target/`. Excluding `~/code` took a run from 37 failures to 0 |
| D7 | How the health check should treat mode `0200` | **OPEN, new.** Mechanism VERIFIED; the remedy is a design choice |

### Closing thoughts, 2026-08-07

**Two measuring instruments were confidently wrong, and neither was caught by reasoning.**
The project's exec-latency probe timed a shell script — 0.005 s while a freshly built
binary took 32.8 s at the same instant — so every "both arms saw the same machine state"
check made with it was blind. My own descriptor witness compared `st_dev` through
`/dev/fd`, which Darwin synthesises, so it reported `Replaced` on **every** rejection and
would have sent the next session hunting a descriptor stomp that never happened. Both were
found by testing the instrument itself against a known answer. **Verify the instrument,
not only the result** is the lesson this session paid for, and it generalises past this
repository.

**The environment was a larger cause than the code.** A Spotlight setting took a workspace
run from 37 failures to 0. Three sessions of "flaky suite" work were, in substantial part,
chasing a macOS indexer. The caution is specific: **before attributing an intermittent
failure to the product, check what the machine was doing.** That is cheap and it was
skipped for a long time.

**Which puts one earlier conclusion back in play.** The `ssh-keygen` descriptor defect was
characterised as *load-dependent* — it never reproduced idle and reproduced readily under
load. That load was, at least partly, Spotlight. The `6/28 vs 0/28` A/B still stands, since
both arms ran back to back under the same conditions, but **the defect's reproduction rate
on a healthy machine is now unknown**. Re-measure it before assuming it behaves as
recorded. It may be far rarer, and it may not reproduce at all.

**Instrumentation kept compounding, across sessions.** `KeygenOutcome` (added two sessions
ago) narrowed the failure to one call site; the redacted message added this session
answered which of two causes it was on the *first* reproduction; and `validate_file_stat`'s
four-way fault naming — added for a race that had been seen exactly once and would not
reproduce — fired today and turned that hypothesis into a mechanism. None of it was
expensive, and none of it required knowing in advance which one would pay.

**The fix that lost is worth remembering as a shape.** Mapping the child to a fixed
descriptor number was well reasoned: it replaces a passive assumption with an active
`dup2` that fails loudly. It was also 6 failures in 28 against 0 in 28. The reasoning was
not sloppy; it was simply not evidence. The A/B cost one iteration and is now recorded at
the call site so the next person does not re-derive the same appealing wrong answer.
