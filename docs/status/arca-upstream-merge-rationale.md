# Arca ⇄ upstream containerization merge — rationale record

Date: 2026-08-04
Scope: P0.3. Merging `apple/containerization` `upstream/main` into the Arca fork.

Records **why** each fork divergence was kept or dropped. The conflict text itself is
reproducible at any time with `git merge`; this reasoning is not. Roadmap P8.2 needs
this record regardless of how the merge ends.

**Status marking is load-bearing.** Entries marked VERIFIED were established by a command
whose output was read. Entries marked PLAN are intended resolutions that nothing has yet
compiled or executed. Do not promote a PLAN to VERIFIED without running something.

## Resuming

### Branch topology — two branches subsume the other four

Do not try to land the superseded ones; they point at pre-merge state.

```
fork (Vas-Solutus/arca-containerization)
  merge/upstream-main  f02cdf9   <- CONTAINS fix/track-go-mod-drop-prebuilt-binary (943d3b3, 5754902)
                                    plus a1085d8 (merge) and f02cdf9 (guest fixes)

superproject (Vas-Solutus/arca)
  merge/upstream-containerization  4591a21  <- CONTAINS fix/swift-6.3-sending-closures (0910463)
                                               plus 4e27394 (pin + packaging) and 4591a21
  fix/submodule-currency  6829cdb  <- SUPERSEDED. Pins f48a6c7 -> 5754902, i.e. pre-merge.
                                      merge/upstream-containerization pins f02cdf9 instead.
```

VERIFIED with `git merge-base --is-ancestor` in both repositories.

### First commands

```sh
cd ~/code/arca
git checkout merge/upstream-containerization
git submodule update --init --recursive     # fork should land on f02cdf9
swift package clean                          # MANDATORY — see the trap below
swift build -c debug 2>&1 | grep -E "error:" # expect ~109
```

`swift package clean` is not optional. Without it a stale build plan referencing the
now-removed `ContainerizationOS/Signals.swift` collapses all 109 errors into a single
`missing inputs` failure, which looks like a broken checkout rather than API drift.

### Machine state that is not in any repository

- The Swift 6.3 static SDK is installed on this machine
  (`swift-6.3-RELEASE_static-linux-0.1.0`). On a different machine, run
  `make linux-sdk` from `containerization/vminitd`. Upstream renamed that target from
  `cross-prep`; the old name no longer exists.
- `~/.gitignore_global` had its `*.mod` line removed so Go module files are not ignored
  globally. Backup at `~/.gitignore_global.bak-20260804`. On a different machine this must
  be redone, or `go.mod` will appear ignored again in every Go project.

## Coordinates

| | |
|---|---|
| Merge base | `27947cda9cf452ea0900f0bb11e3576207986380`, 2025-12-01 |
| Fork side | `merge/upstream-main`, branched from `5754902` (carries the P0.2 work) |
| Upstream side | `upstream/main` = `5796abe`, 2026-07-31 |
| Conflicts | 11 files (10 content, 1 modify/delete) |

## U1 — RESOLVED (VERIFIED)

**The handoff's conclusion was wrong and should not be re-adopted.** It recorded that
`Server+GRPC.swift`, `ManagedProcess.swift` and `RuncProcess.swift` "no longer exist
upstream" and that the resulting "modify/delete conflicts have no mechanical resolution."

Upstream did not delete them. It renamed the directory, extracting a library target:

```
vminitd/Sources/vminitd/  ->  vminitd/Sources/VminitdCore/
```

`Application.swift` stays behind in the executable target; 19 files moved. New upstream
files in that target: `AgentCommand.swift`, `InitCommand.swift`, `PauseCommand.swift`,
`Logging.swift`. New sibling targets `Cgroup` and `vmexec`.

VERIFIED by `git diff --find-renames --diff-filter=R --name-status $BASE upstream/main`:
git pairs 18 of the 19 automatically, including two of the three feared files —
`ManagedProcess.swift` at R092 and `RuncProcess.swift` at R099.

Only `Server+GRPC.swift` fails to pair at the default threshold; it pairs at R045 with
`--find-renames=25%`. Upstream grew it 1178 → 1862 lines. That is the one file needing a
hand port, not three.

## U2 — findings so far

Whether each Arca modification is still viable. One entry per divergence examined.

### `Sources/ContainerizationOS/Mount/Mount.swift` — RESOLVED (VERIFIED analysis, PLAN correctness)

Six fork commits touched this file. They do not share a fate:

| Fork change | Disposition | Reason |
|---|---|---|
| Absolute-symlink skip (`502b715`) | **DROPPED** | Superseded. Upstream added `secureResolveInRoot` using `openat2` + `RESOLVE_IN_ROOT`, which resolves the target inside the rootfs at kernel level. Strictly stronger: it prevents the escape *and still performs the mount*, where the fork's heuristic prevented the escape by skipping the mount. |
| `arca-file-bind` option marker | **DROPPED** | Superseded. Upstream auto-detects file binds by `stat`ing the source (`leafIsFile`, `sourceIsNonDir`); no caller-supplied marker needed. |
| `target` vs `self.target` (`8f25db3`) | **DROPPED** | Superseded; upstream's `mountToTarget` now uses the `target` parameter. |
| Debug `fputs` to stderr | **DROPPED** | Unconditional per-mount stderr noise. |
| Bind-mount remount rewrite | **KEPT** | **Not superseded.** VERIFIED that upstream still carries the merge-base logic — remount only when `MS_BIND｜MS_RDONLY` are both set. The fork remounts every bind mount in two steps, which also covers a writable bind over a read-only source filesystem. Marked in-file as an ARCA PATCH for P8.2 triage. |

Two consequences that outlive this file:

- **Cross-repo follow-up.** `arca-file-bind` is still emitted by the superproject at
  `Sources/ContainerBridge/ContainerManager.swift:3907` (`["bind", "arca-file-bind"]`).
  Upstream's `parseMountOptions` appends unrecognised options to the mount *data* string.
  `MS_BIND` makes the kernel ignore data, so this is believed harmless, but the marker is
  now dead and should be removed from the superproject. Not done. PLAN.
- **Open question for P0.4.** Fork commit `502b715` says the motivating symptom was k3d's
  serverlb failing `ENOENT` because "the second tmpfs mount shadowed the first". Upstream
  mounts at the resolved path rather than skipping, so the *escape* is definitely fixed but
  the *original symptom* may return. Cannot be settled by reading. Needs the k3d functional
  test. PLAN.
- The fork's second remount deliberately ignored its errno (`// Don't fail here`). Preserved
  verbatim so this merge changes no behaviour, with the smell documented in-file. Swallowing
  the errno is wrong and should be narrowed or logged in a **separate** change, not smuggled
  into a merge resolution.

### `.gitignore` — RESOLVED (VERIFIED)

Pure union. Upstream added `kernel/vmlinuz-x86_64`; the fork added the Go build-artifact
block from P0.2. Both kept.

### `vminitd/Sources/VminitdCore/ManagedProcess.swift` — ANALYSED, NOT RESOLVED

Fork commit `f48a6c7` ("Reorder network setup to run before container.start()") split a
single `state.withLock { … }` into two phases: acquire the PID under the lock, then perform
the pid-acknowledgement and PTY-FD exchange **outside** it, on the reasoning that the network
namespace already exists because `AddNetwork` now runs before `container.start()`.

Upstream modified the same region **in place**, still inside the lock.

PLAN: take upstream's updated inner logic and re-apply the fork's two-phase restructure
around it. This is not a pick-a-side resolution — choosing upstream wholesale silently drops
the ordering fix; choosing the fork wholesale silently drops a year of upstream changes to
that block.

### Remaining conflicts — ALL RESOLVED

Merge committed as `a1085d8` on `merge/upstream-main`.

| File | Disposition |
|---|---|
| `Containerization/ContainerManager.swift` | Both sides kept — fork's OverlayFS `create`/`unpackWithOverlay`, upstream's `releaseNetwork`/`createEmptyFilesystem`. Pure additions on both sides. |
| `Containerization/LinuxContainer.swift` | Both sides kept in hunks 1–2. Hunk 3: upstream's rewrite taken (holding tags, virtiofs transform, `useInit`, socket staging), with the fork's `/dev/vd*` filter folded into its filter chain. |
| `Containerization/LinuxProcessConfiguration.swift` | **Upstream.** Fork's delta was an optional `capabilities` defaulting to nil; upstream's is non-optional with the restricted OCI baseline, plus `noNewPrivileges`. Strict superset, more secure default. |
| `Containerization/Mount.swift` | **Upstream.** Fork's entire delta was `debugLog` stderr instrumentation. Upstream also moved VirtioFS to a central `VZMultipleDirectoryShare`. |
| `vmexec/RunCommand.swift` | Upstream's `FileDescriptor` types and buffered ack read; fork's deferred namespace joining preserved. `setupNamespaces` now returns `(flags, toJoin)` instead of upstream's bare `Int32`. Highest-risk resolution in the merge — see below. |
| `vmexec/vmexec.swift` | `logToConsole` kept (10 call sites); `standardErrorLock`/`standardError` dropped with upstream. |
| `vminitd/Application.swift` | **Upstream** (118-line subcommand shell). Fork's ~200 lines relocated to new `VminitdCore/ArcaBoot.swift`, called from `AgentCommand.bootstrap()` at two points. |
| `vminitd/Server+GRPC.swift` | Hand-ported to `VminitdCore/Server+GRPC.swift`: OverlayFS-at-rootfs mount, block-device skip, DNS comment, `ARCA_GROUP_ADD`. Dropped the DNS "is writable" probe, which wrote a test file on every call. Old path `git rm`'d. |
| `.gitignore` | Union. |

`LinuxContainer.swift` was flagged in the handoff as the highest single-file conflict risk
(38 upstream commits over fork modifications). That was **overestimated** — three hunks, two
of them pure additions.

The genuinely risky resolution is `RunCommand.swift`'s namespace ordering, because both
options compile and the difference only appears at runtime on the networking path.

### Fallout beyond the conflicts

`EXT4.Formatter.unpack` became `async` upstream. `OverlayFSUnpacker.swift` — an Arca-only
file, so never conflicted — called it synchronously and had to be updated. Worth expecting
more of this class: fork-only files that consume upstream APIs are invisible to conflict
detection and only surface at build time.

## Toolchain

VERIFIED. The fork lags upstream by a Swift major-minor:

| | fork | upstream |
|---|---|---|
| `.swift-version` | `6.2` | `6.3.0` |
| `vminitd/Package.swift` tools version | `6.2` | `6.3` |
| Static Linux SDK | `swift-6.2-RELEASE_static-linux-0.0.1` | `swift-6.3-RELEASE_static-linux-0.1.0` |

Upstream SDK checksum `d2078b69bdeb5c31202c10e9d8a11d6f66f82938b51a4b75f032ccb35c4c286c`.

This is why `scripts/build-vminit.sh` fails today (VERIFIED, host toolchain is 6.3.3):

```
error: module compiled with Swift 6.2 cannot be imported by the Swift 6.3.3 compiler:
  .../swift-6.2-RELEASE_static-linux-0.0.1.artifactbundle/.../Foundation.swiftmodule
make: *** [vminitd] Error 2
```

The merge brings upstream's 6.3 SDK pin, so `make cross-prep` must be re-run after it lands
to install the 6.3 artifact bundle. Until then the guest cannot build and P0.4 is blocked.

## Exit criteria for this merge

| # | Criterion | Status |
|---|---|---|
| 1 | No conflict markers; merge commits | ✅ `a1085d8` |
| 2 | 6.3 static SDK installed | ✅ via `make linux-sdk` (upstream renamed it from `cross-prep`) |
| 3 | Fork `swift build` green | ✅ exit 0, zero errors |
| 4 | `build-vminit.sh release` green | ✅ exit 0; `arca-services` verified present in the rootfs tar |
| 5 | Superproject `swift build` green | ❌ **109 errors** — see below |
| 6 | P0.4 functional pass incl. the k3d case | ❌ not started; guest has never been booted |

## The remaining work: superproject API drift

Branches: fork `merge/upstream-main` (`f02cdf9`), superproject
`merge/upstream-containerization` (`4e27394`). Both pushed. The superproject branch is
deliberately committed in a non-building state so the merge work is not lost.

The fork is green on both host and guest. What does not compile is **this repository's
consumption of it** — eight months of upstream API drift, concentrated in `ContainerBridge`:

| Symptom | Files |
|---|---|
| `ContainerManager.VmnetNetwork` no longer a nested type | `ContainerManager.swift`, `SharedVmnetNetwork.swift` |
| `capabilities` now non-optional `LinuxCapabilities` | `ContainerManager.swift:1485` |
| signal APIs take `Signal`, not `Int32` | `ContainerManager.swift:2774` |
| `ContainerStatistics` memory/cpu members optional | `ContainerManager.swift:2882-2883` |
| `hashMountSource` out of scope | `ContainerManager.swift:3773,3878` |
| `Interface.address` removed | `VmnetNetworkBackend.swift:149` |

**Trap worth knowing:** a stale `.build` plan referencing the now-removed
`ContainerizationOS/Signals.swift` collapses all 109 errors into a single
`missing inputs` failure. Run `swift package clean` first, or the real list stays hidden.

## Post-merge fixes applied (`f02cdf9`)

Four upstream API changes that conflict detection could not surface, because the affected
code was fork-only or hand-ported. Expect this class to keep appearing:

- `GRPCStatus` → `RPCError` (upstream migrated grpc-swift v1 → v2 / GRPCCore).
- `ISO8601DateFormatter` unavailable — upstream narrowed `vmexec` from `Foundation` to
  `FoundationEssentials`. Reimplemented via `strftime` rather than widening the import back.
  `String(cString:)` is also a build error here; this target treats deprecations as errors.
- `build-initfs.sh` gained `--add-file SRC:DEST`. `cctl rootfs create` no longer assembles a
  rootfs; it consumes a prebuilt tar. Staging in that script rather than post-processing the
  tar is what also places the file in the **ext4 initfs** — both derive from one staging dir.
- `EXT4.Formatter.unpack` became `async` (`OverlayFSUnpacker.swift`).
