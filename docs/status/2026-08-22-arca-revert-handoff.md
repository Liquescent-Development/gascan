# Handoff: the arca revert is half landed, and PR 2 resumes at Task 7

Date: 2026-08-22
Branch: `docs/arca-revert-handoff`
Written mid-execution. PR 1 is merged; PR 2 has one of eight tasks done.

---

## Read this first

The engine defect that blocks P5's second exit clause is being fixed in **a different
repository**. Nothing in `gascan` has changed except documentation.

The defect: the arca engine attached one block device per OCI layer against a 26-letter
alphabet, so Gas Can's 35-layer workspace image failed at `create` with `no free indices
are available for allocation`. Reproduced 2026-08-21 on host `newcombe` with the engine
built from arca `c545612`; 1-layer alpine gave `2 passed (1 suite, 5.03s)` and the
workspace layout gave the allocator error, with the engine logging
`layer_devices_start=/dev/vdc layers=36 total_mounts=38 writable_device=/dev/vdb` and
`duration_seconds=14.83 layers=36` — every layer unpacked, so attachment was the failure.

The fix reverts arca's fork-local overlay-per-layer handling to upstream Containerization's
single composed rootfs, which makes the device count constant regardless of layer count.

**The authoritative documents live in the arca repo, on PR #60:**

- `Documentation/DESIGN-revert-to-upstream-rootfs.md` — the spec, and the binding authority
- `Documentation/PLAN-revert-to-upstream-rootfs.md` — 14 tasks across three repositories

**The recovery map is the SDD ledger:**
`/Users/kiener/code/arca/.superpowers/sdd/PLAN-revert-to-upstream-rootfs/progress.md`

It is git-ignored, ~40KB, and holds every ruling made on the maintainer's behalf with its
cost-if-wrong. It also holds the task briefs, implementer reports and reviews. **Read it
before resuming.** After a context loss, trust it and `git log` over recollection.

---

## Where things stand

| | |
|---|---|
| **arca-containerization PR #2** | **MERGED** as `a5803b6` on `merge/upstream-main` |
| **arca PR #60** (design + plan) | open, 9 commits, deliberately held until PR 2 completes |
| **arca `revert/upstream-rootfs`** | PR 2's branch, 4 commits, **not yet pushed, no PR** |
| **gascan PR #92** | open, unchanged by this work |

PR 1 merged as a merge commit, not a squash: `parents=6304122 10f408c`, and all four cited
SHAs (`90c8b0d`, `ecdcdd6`, `dee6375`, `10f408c`) verified reachable from
`origin/merge/upstream-main`. Every SHA citation in the design and plan still resolves.

Tasks 1–4 (PR 1, submodule) and Task 5 (PR 2, parent) are complete and reviewed.
**Task 6 was ruled already satisfied** — its mutation matrix exists four times over in the
Task 5 reports and reviews, at twelve mutations and per-assertion granularity against the
three the task specifies. Task 11 consumes those matrices directly.

**Resume at Task 7.** Tasks 7–14 remain.

---

## Two constraints that will break things if forgotten

**1. arca's submodule pointer may only be advanced by PR 2.**

PR 1's `10f408c` deleted `LinuxContainer.swift`'s empty-destination mount filter. That
filter was *not* inert: its producer is `OverlayFSMounter.buildMounts` in the parent's
`ContainerBridge`, at `:109` (the writable device) and `:135` (each layer device), reached
from `ContainerBridge/ContainerManager.swift:1288`. PR 2 removes the producer (Task 9) and
advances the pointer (Task 12) in the same merge, so no state exists where the parent has
the producer and lacks the filter. A pointer bump without Task 9 sends empty-destination
block mounts into the OCI spec for every container.

**2. The submodule is deliberately held at `6304122` for Tasks 5–11.**

The parent's gitlink says `6304122` and the working tree must match it, so PR 2 builds
against the *un-reverted* submodule while Tasks 7–11 delete the parent's callers. If the
submodule checkout drifts to PR 1's head, every parent build fails on missing overlay types
and reads as a code defect. Task 12 bumps it to **`a5803b6`** — the merge commit, not
`10f408c`.

---

## What Task 7 must carry

Task 7 rewrites `ContainerManager`'s create path onto upstream's
`create(_:image:rootfs:writableLayer:networking:configuration:)`. Five items ride with it,
none of which are in its brief:

1. **`reapOrphanedStagingFiles()` has no caller.** Task 5 built and tested it; it is inert
   until cache-root initialisation invokes it once. Until then orphans accumulate at one
   full-size rootfs per crash.
2. **The shared cache slot is attached WRITABLE.** It leaves `ImageRootfsUnpacker` as
   `.block(…, options: [])`, and `Mount` derives read-only from options
   (`Mount.swift:441-443`), so the attachment is `readOnly: false`
   (`Mount.swift:371-375`). It is not written today only because Task 7 passes a writable
   layer and `LinuxContainer` mounts the rootfs as the overlay *lower* layer
   (`:42-44`, `:589-609`). `writableLayer == nil` is a configuration upstream explicitly
   supports (`:618-621`), and any caller taking it has every container writing into the
   shared slot. Verify a writable layer is always passed, and evaluate adding `"ro"` —
   `LinuxContainer.swift:434` suggests it is harmless when a writable layer is present, but
   that has not been verified end to end.
3. **Concurrency contract.** N concurrent misses on a cold image do N full unpacks and N×
   peak disk. Safe — `rename(2)` is atomic, both artefacts are verified, and identical
   content — but a real cliff on fan-out. Accept explicitly or serialise per digest.
4. **Scope PR 2 from the plan, never from `task-3-report.md`'s inventory**, which
   under-counts the parent breakage roughly threefold. Seven production sites, owned by
   Tasks 7, 8, 9 and 11; the mapping is in the ledger.
5. **The `layerCachePath` → `imageRootfsCachePath` rename** moved into Task 7 at pre-flight,
   including `EnginePaths.swift:49,90`, `EngineManagers.swift:11,66` and
   `ArcaDaemon.swift:199`. Task 10 keeps only the `layer_cache` table drop and the reclaim.

---

## Traps measured in this session, each of which cost something

- **`swift test --disable-swift-testing` runs ZERO tests in the submodule.** Measured: exit
  0, `Executed 0 tests, with 0 failures`. The suite there is entirely swift-testing. This
  plan originally specified that flag at **13 sites**; every "tests pass" it produced would
  have been green for the wrong reason. Plain `swift test` runs 600+ tests. In the *parent*
  the flag does run XCTest, but plain is a superset there too.
- **`swift package describe --type json` is not a manifest validator.** It accepted a
  manifest declaring a non-existent product with exit 0 and listed that product in its
  output. Only `swift build` caught it.
- **SwiftPM will link a stale object after a public signature change**, producing
  `Undefined symbols … volumeLabel: Swift.String?` naming a parameter no source passes. It
  can hide a real breakage as easily as invent a fake one. `rm -rf .build` before any
  build-based conclusion.
- **On macOS `swift build` compiles none of the guest** — it is all inside `#if os(Linux)`.
  Only `make vminitd` (aarch64-swift-linux-musl) exercises it.
- **`gh` had no default repo in the submodule checkout** and, with both `origin` (the fork)
  and `upstream` (apple) configured, resolved bare commands to **apple/containerization**.
  The first `gh pr create` was aimed there. Fixed with `gh repo set-default`.
- **The shell hook rewrites commands to `rtk <cmd>`**, silently dropping flags and
  truncating output. An empty grep through it is not evidence of absence. It produced two
  wrong conclusions in this session before being caught. Use `rtk proxy <cmd>`, redirect to
  a file, and read the file for anything whose count or emptiness you will cite.
- **The design's §4.1 overlay/total table under-counts.** Those are keyword matches over
  changed lines, so a doc block whose *subject* is overlay but whose text spells none of the
  identifiers scores zero. It was wrong about `Kernel+Commandline.swift` (claimed 6 of 43;
  actually 41+/3− and all overlay), `ContainerManager.swift` (claimed 14 of 53; actually 59
  insertions, both hunks overlay) and `LinuxContainer.swift` (claimed 8 of 84 at two sites;
  missed a 44-line block). It locates work; it does not bound it.

---

## Commit signing will block you

`git commit` fails intermittently with `Couldn't sign message (signer): communication with
agent failed?` and `fatal: failed to write commit object`. `commit.gpgsign=true`,
`gpg.format=ssh`, `SSH_AUTH_SOCK` is 1Password's agent. `ssh-add -l` succeeds and holds the
key; the *sign* operation is what fails, and it reproduces outside git with
`ssh-keygen -Y sign`. It reads as 1Password locked or an unanswered approval prompt, and it
cleared twice on retry after the maintainer unlocked.

**Do not work around it with `-c commit.gpgsign=false`.** Every commit on these branches is
signed; an unsigned one among them is a policy change, not a fix. Stage the work, write the
message to a file, and ask.

---

## Still true, and not closed by this work

- **U5 remains open.** The harness provisions the image itself with `skopeo` and
  `arca-engine image load`; a shipped `.pkg` cannot. A green arca suite is evidence about
  the product on arca and is not evidence about U5.
- **`PrepareImage` is not fixed.** The revert makes upstream's per-image unpack reachable,
  which is what it would need. Task 11 only stops its comment asserting a reason that has
  become false.
- **Named volumes are not rewired.** `CreateVolumeOverlay` depends on a writable `/mnt` and
  on `/dev/vdb` in `/proc/mounts`, both of which PR 1 removes. It has no caller, and it
  already failed against `/mnt/vdb/upper` — a path the fork never created — before this
  work. Design §4.1 and §7 carry the citations.
- **A correctly sized, truncated-but-still-parseable rootfs is detected by nothing** in
  `ImageRootfsUnpacker`. Measured: `EXT4.EXT4Reader` accepts the promoted artefact cut to
  1.56% of its length and accepts a megabyte of zeros over its metadata region, because the
  formatter pads to a whole block group and the tail is padding the tree walk never visits.
  Accepted because the slot is only ever written by a promotion, so reaching it needs
  corruption *after* promotion. If any future caller adds a second writer to that path, that
  reasoning collapses.
