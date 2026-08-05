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
- **Arca pivots from Docker-compatible runtime to Gas Can's sandbox engine.**
  Docker compatibility is discontinued, not reduced.
- **Gas Can absorbs Arca**, not the reverse. The deciding factor is which release
  pipeline survives: Gas Can's verifies signed tags and emits
  `build-manifest.json`; Arca's builds a DMG.
- **The Vas Solutus identity does not survive.** It existed for the
  Docker-runtime positioning the pivot ends. "Arca" survives as the engine's name
  inside the tree.
- **Sandboxes need peer channels.** Agent-to-agent collaboration is a real
  requirement, each agent in its own sandbox.
- **Sidecars are not a networking feature.** A Postgres for the project under
  development runs inside the agent's own sandbox on the guest's Docker.
- **Agents get `dockerd` in-guest.** Host-side buildx integration is deleted.
- **Arca owns the wire protocol; Gas Can owns the behavioural specification.**
  The engine decides what it can be asked; the product decides what a correct
  answer is.
- **`Sources/DockerAPI/` is deleted early, not last.** `gascan-apple` is the
  migration safety net, so retaining it buys nothing.
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
- **Not green:** superproject `swift build` — 109 errors of upstream API drift in
  `ContainerBridge`. That is the next task and is larger than the merge was.
- Still untouched: P0.4 functional pass. Nothing has been booted.

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
