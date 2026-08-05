# Arca → Gas Can Monorepo Merge

Date: 2026-08-04
Status: Draft for review

Move Arca into the Gas Can repository. Gas Can absorbs Arca, not the reverse.

Related: `2026-08-04-sandbox-engine-contract.md`, `2026-08-04-arca-sandbox-backend.md`,
`arca/Documentation/SANDBOX_ENGINE_PIVOT.md`.

## 1. Why Gas Can absorbs Arca

The repository should be the product. After the pivot, users install Gas Can and
Arca is an implementation detail with exactly one consumer.

The deciding factor is which release pipeline survives. Gas Can's packaging
refuses to build from an untrusted source revision, verifies against a signed tag,
and emits `build-manifest.json` recording the source revision plus a SHA-256 for
every installed executable. Arca's builds a DMG. Gas Can's is the one worth
keeping, and it should be the outer shell.

**The supply-chain argument is the strongest one for merging at all.** Once Arca
ships inside Gas Can's `.pkg`, `build-manifest.json` must cover Arca's binaries.
Across two repositories, Gas Can attests artifacts it did not build — extending
trust to a second pipeline and hand-verifying the handoff. In one repository, Gas
Can builds the engine and the manifest is trivially honest.

Secondary: 419 commits of history versus 25, and both repositories are
single-author, so no contributor is stranded and no community is split.

## 2. Measured starting state

All figures taken 2026-08-04 from the checkouts at `~/code/arca` and
`~/code/gascan`.

### 2.1 The submodule

`arca/.gitmodules` points `containerization` at
`git@github.com:Vas-Solutus/arca-containerization.git`, a fork of
`apple/containerization`. Both `origin` and `upstream` remotes are configured in
the submodule checkout.

| Fact | Value |
|---|---|
| Superproject pins | `f48a6c7`, 2025-12-03 |
| Fork's own `origin/main` | `502b715`, 2025-12-09 — **4 commits ahead of the pin** |
| Last upstream merge | `76cd1d4`, 2025-12-01 |
| `upstream/main` | `5796abe`, 2026-07-31 |
| Upstream commits since last merge | **267** |
| Upstream releases since last merge | 0.35.0 → 0.40.2 |

### 2.2 The fork delta

Measured as `git diff --name-status $(git merge-base origin/main upstream/main) origin/main`:

**38 files changed, 12,056 insertions, 73 deletions.**

That shape is better than the raw numbers suggest:

- **22 files are additions**, almost entirely `vminitd/extensions/arca-services/**`
  — a directory upstream does not have. Added files cannot conflict.
- **16 files are modifications**, and 73 deletions across all of them means the
  changes are overwhelmingly additive rather than rewrites.

The conflict surface is those 16 files, and upstream churn on them is uneven:

| File | Upstream commits since base |
|---|---|
| `Sources/Containerization/LinuxContainer.swift` | 38 |
| `vminitd/Sources/vminitd/Server+GRPC.swift` | 17 — **deleted upstream** |
| `Sources/Containerization/ContainerManager.swift` | 13 |
| `vminitd/Sources/vmexec/vmexec.swift` | 12 |
| `vminitd/Sources/vminitd/Application.swift` | 10 |
| `vminitd/Sources/vmexec/RunCommand.swift` | 10 |
| `vminitd/Sources/vminitd/ManagedProcess.swift` | 9 — **deleted upstream** |
| `Sources/Containerization/Mount.swift` | 8 |
| `vminitd/Sources/vmexec/ExecCommand.swift` | 7 |
| `Sources/Containerization/LinuxProcessConfiguration.swift` | 6 |
| `Sources/ContainerizationOS/Mount/Mount.swift` | 5 |
| `Sources/ContainerizationOS/User.swift` | 4 |
| `vminitd/Sources/vmexec/Mount.swift` | 3 |
| `.gitignore` | 3 |
| `vminitd/Sources/vminitd/RuncProcess.swift` | 2 — **deleted upstream** |
| `Sources/ContainerizationOCI/ImageConfig.swift` | 1 |

"Deleted upstream" verified with `git cat-file -e upstream/main:<path>`.

## 3. Risks, in priority order

**R1 — Three modified files no longer exist upstream.** `Server+GRPC.swift`,
`ManagedProcess.swift`, and `RuncProcess.swift` are all under
`vminitd/Sources/vminitd/`, which upstream appears to have restructured. Git will
raise modify/delete conflicts, and there is no mechanical resolution: each requires
understanding where the functionality moved upstream and re-applying the
modification there. This is the concentrated risk of the whole update.

**R2 — `LinuxContainer.swift` absorbed 38 upstream commits** while Arca carries
local modifications to it. Highest conflict probability of any single file.

**R3 — The shipping build is missing a security fix.** The superproject pins
`f48a6c7`, but the fork's `origin/main` is 4 commits ahead and includes `502b715`,
*"fix: Skip mounting on absolute symlinks to prevent rootfs escape"*, plus three
RPC additions (`CreateVolumeOverlay`, `CreateDirectMount`, `GenerateHostsFile`).
A rootfs-escape fix exists in the fork and is not in what Arca builds.

**R4 — A 12.4 MB prebuilt binary is committed to git.**
`vminitd/extensions/arca-services/arca-services` is a stripped, statically linked
aarch64 Linux ELF, 12,976,312 bytes in the object store — committed even though
`build.sh` beside it cross-compiles the same thing from the Go source in the same
directory.

This is worse than repo bloat. That binary runs inside every sandbox guest, and
nobody can verify it corresponds to the source next to it. It is precisely what
Gas Can's `build-manifest.json` discipline exists to prevent, and merging it into
the repository whose release policy makes verifiability a selling point would
import a contradiction.

**R5 — Permanent fork maintenance.** 267 upstream commits in 8 months is the
recurring rate. Absent a plan, this tax repeats indefinitely.

## 4. The submodule decision

Three options were considered.

**A. Keep it as a submodule inside Gas Can.** Least change. Preserves the normal
`git merge upstream/main` workflow that the configured remotes already support.
Cost: a submodule inside a monorepo is a wrinkle, and that part of the tree remains
a second git repository during development.

**B. Subtree it into Gas Can.** One tree, one history. Cost: upstream merges become
`git subtree pull` rather than a plain merge. Given §2.1 shows upstream merges are
frequent and large, making them clunkier is the wrong trade.

**C. Stop forking.** Move `vminitd/extensions/arca-services/**` out of the
containerization tree entirely — it is 22 added files in a directory upstream does
not have, so nothing binds it there — then reduce or upstream the 16 modifications
until `apple/containerization` can be consumed as an ordinary SwiftPM dependency
pinned to a release tag.

**Decision: A now, C as the stated direction.**

Take A for the merge, because changing the dependency mechanism and the repository
topology simultaneously produces an unreviewable change. But C is the destination,
and §2.2 shows it is plausible: the additions are already separable, the
modifications are small and additive, and the fork's history shows upstream accepts
PRs. The 16 modified files are the work list. Each should end up upstreamed,
expressed through an extension point, or documented as a permanent local patch with
a reason.

Without C, R5 is unbounded.

## 5. Sequencing

**The submodule update happens before the merge, in `arca` as it stands.** Doing a
267-commit upstream reconciliation and a repository merge in one change is
unreviewable, and if the reconciliation goes badly it should not also have
destabilised the migration.

### Phase 0 — Submodule currency (in `arca`, before anything else)

0.1 Move the superproject pin from `f48a6c7` to the fork's `origin/main`
`502b715`, picking up the rootfs-escape fix and three RPCs (R3). Build and test.
This is worth doing today regardless of every other decision here.

0.2 Delete the committed `arca-services` binary and build it in CI from
`build.sh` (R4). Do this before the merge so the blob never enters Gas Can's
history.

0.3 Merge `upstream/main` into the fork. Expect R1 to dominate: resolve the three
modify/delete conflicts by locating where upstream moved each responsibility.
Expect R2 second.

0.4 Full functional pass on the updated fork — boot a sandbox, exercise WireGuard
peers, filesystem, process, and overlayfs services.

Phase 0 has a real chance of uncovering that some Arca modification is no longer
viable against restructured upstream code. Better to find that here than mid-merge.

### Phase 1 — Merge

1.1 `git subtree add` Arca into Gas Can, preserving history. Not a flat copy —
Gas Can's release policy verifies source revisions, so provenance has operational
value.

1.2 Relocate into the layout in §6.

1.3 Carry the submodule across, still pointing at the fork.

### Phase 2 — Build consolidation

2.1 One CI orchestrating Swift, Rust, Go, and protobuf codegen. Arca's Makefile
already drives Swift and Go, so this is consolidation rather than new capability.

2.2 Path-based triggers from the start, so a Rust-only change does not rebuild the
Swift engine.

2.3 Extend `build-manifest.json` to cover engine and guest binaries — the payoff
identified in §1.

### Phase 3 — Fork reduction

Begin option C. Move `arca-services` out of the containerization tree into
`guest/`, then work the 16-file list.

## 6. Layout

```
gascan/
  crates/            Rust: gascan, gascand, gascan-core, gascan-engine,
                     gascan-apple, gascan-proto, gascan-e2e
  engine/            Swift: was arca/Sources/
  guest/             Go: destination for arca-services in Phase 3
  containerization/  submodule, unchanged — tracks the fork, which tracks Apple
  proto/             gascan/v1 + engine/v1
  docs/  packaging/  scripts/
```

"Arca" survives as the engine's name inside the tree. It is a good name and it
marks the boundary usefully. The Vas Solutus identity does not survive; it existed
for the Docker-runtime positioning that the pivot ends.

## 7. Consequences for the other specs

Technical content is unaffected. Cross-repository mechanics change:

- **Contract §3 (ownership)** — the proto becomes a directory. Keep the conceptual
  split as a design discipline: the engine decides what it can be asked, the
  product decides what a correct answer is. It happened to be enforced by a repo
  boundary; it should not need one.
- **Contract §3.1 (anti-drift)** — Arca's CI no longer needs a Rust toolchain to
  run Gas Can's conformance suite. It is one CI and one test.
- **Contract §9 (versioning)** — largely removable. `RuntimeCapabilities.version`
  stays, because `gascan-apple` still drives an externally-versioned runtime.
- **`arca/Documentation/GASCAN_INTEGRATION.md`** — stops being a pointer. Its five
  obligations fold into the contract as explicit design constraints.
- **`arca/Documentation/SANDBOX_ENGINE_PIVOT.md`** — moves under `docs/`, escaping
  the `Documentation/.gitignore` that currently ignores `*.md` by default with a
  five-file allowlist.

## 8. Do now, independent of everything

Two items should not wait for a decision on any of the above:

1. **Phase 0.1** — the pin is behind a rootfs-escape fix that already exists.
2. **Phase 0.2** — an unverifiable 12.4 MB binary runs in every guest.

Neither depends on the merge, the pivot, or the monorepo question.

## 9. Non-goals

- Preserving Arca as an independently installable product.
- Subtreeing the containerization fork (§4 option B).
- Preserving the Vas Solutus identity.
- Merging Gas Can into Arca.
