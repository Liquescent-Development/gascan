# Handoff: the arca layer ceiling, and what the next session does

Date: 2026-08-21
Branch: `docs/product-e2e-on-arca-design`
Written after a brainstorming session that produced a design, then found the design's
central assumption to be false. Nothing is implemented. No code outside `docs/` changed.

---

## Read this first

You are picking up **P5's second exit clause** — the product-level `gascan-e2e` suite
running on arca. A design for it exists and is committed:
`docs/superpowers/specs/2026-08-21-product-e2e-on-arca-design.md`.

**That design is sound except for §4, which is blocked by an engine defect found while
validating it.** §4.1 of the same document records the defect, the controlled
experiment that found it, and the measured scope of the fix. Read the design before
this file's task list; this file assumes it.

**The task in front of you is arca-side, in a different repository, and it is a
prerequisite.** The gascan-side e2e work cannot start until it lands.

---

## What was decided, and by whom

The maintainer decided on 2026-08-21, in session:

1. **Apple's `container` runtime will not be supported once arca works.** Firecracker
   on Linux is a possibility, not a plan, and nothing is designed for it. This is why
   the design quarantines Apple-specific code for deletion rather than building a
   permanently pluggable abstraction (design §3).
2. **Revert arca to upstream Containerization's single-composed-rootfs approach**
   rather than raising the device ceiling or shrinking the workspace image. The
   maintainer's reasoning, in their words: *"We're not really trying to be docker here
   and even if we were I don't think fighting the upstream with a different
   implementation is a great strategy long term... we get that work for free as they
   improve it and we merge upstream into our fork over time."*
3. **Stop before implementation** and hand off. That is why this file exists.

Design decisions already settled and not to be relitigated: the real workspace image
(not the stub fixture); the 8 tests needing no predecessor image; one shared test body
with per-backend fixtures; Apple's tier diagnostic-only and never a merge gate.

---

## The defect, in one paragraph

The arca engine attaches **one block device per OCI layer**. Device tags come from a
26-letter alphabet; `vda` is initfs and `vdb` the writable overlay, leaving 24 for
layers. Gas Can's approved workspace image has 35, so `gascan up` on arca fails at
`create` with `no free indices are available for allocation`. This is a **product
defect, not a test-fixture problem** — any image over ~24 layers fails for any user.
Full evidence, mechanism, file:line citations and the measured scope of the revert are
in design §4.1. Do not re-derive them from scratch; do re-verify any you rely on.

---

## Task list for the next session

**Do the arca work first. It is a different repository: `/Users/kiener/code/arca`,
with `containerization/` as a submodule of `Vas-Solutus/arca-containerization`.**

1. **Confirm the ceiling still reproduces** before changing anything. The command is in
   design §4.1's table — point `GASCAN_ARCA_BASE_OCI_LAYOUT` at a workspace layout and
   run the arca engine tier. A layout already exists at
   `.artifacts/e2e-image-probe/workspace-oci` (2.9 GB, real disk); if it is gone,
   recreate it with the `skopeo copy` in design §4. **Re-derive, do not trust.**

2. **Establish what `OverlayFSClient` is for.** Measured 2026-08-21: it has **no
   external references** in `Sources/`, and with its generated protobuf that is ~722
   lines. If it is genuinely dead, the revert is smaller than the headline number and
   deleting it is uncontroversial. If something reaches it dynamically, the estimate
   changes. **This is the first thing to settle** because it moves the scope.

3. **Design the revert** before writing it. It spans two repositories and both sides of
   the VM boundary (`ArcaBoot.swift` is guest-side, 312 lines, absent upstream). The
   measured surface is in design §4.1. It deserves its own design document in the arca
   repository, not a direct edit.

4. **Verify the ceiling is gone** by the same experiment that found it — the workspace
   image creating and running, not a unit test asserting a device count.

5. **Then, and only then, the gascan-side e2e work** in the committed design.

---

## Traps, each of which has already cost someone time

- **The e2e harness silently tests Apple.** `backend_selection` returns `Apple` from
  its `(false, false)` arm (`crates/gascan-core/src/backend.rs:168`), so a dropped
  variable produces a green run that never touched arca. Design §5 specifies the guard
  — read the daemon's own recorded `backend` field — **and requires mutation-testing
  it**, because an untested guard and no guard are worth the same.

- **A green suite is not evidence that U5 is closed.** The harness loads the image
  itself via `skopeo` and `arca-engine image load`; a shipped `.pkg` cannot. Design §4
  states this and it must not be quietly dropped.

- **Do not fix a failing test by loosening its assertion.** Design §7 sorts every
  failure into three categories and only one is a test edit. This defect is itself an
  instance: shrinking the workspace image would have turned the suite green while
  leaving a live product defect behind it.

- **`grep -c '#\[ignore'` overcounts.** It matches doc comments. `arca_startup.rs:10`
  is prose, so arca carries 2 attributes and not 3. `START-HERE.md` still records 3 —
  **unfixed, and worth fixing.** Anchor the pattern: `grep -cE '^[[:space:]]*#\[ignore'`.

---

## State at handoff

- Branch `docs/product-e2e-on-arca-design`, off `main` at `899a29f`. Two commits, both
  documentation. No code changed anywhere, in either repository.
- The arca repo working tree is untouched by this session; every arca command run was
  read-only (`git log`, `git grep`, `git show`, `wc`).
- `.artifacts/e2e-image-probe/` holds the probe layout (2.9 GB real) and three engine
  state roots seeded from it. The state roots are APFS clones and consume **0 bytes**
  (measured: two additional `image load` runs moved `df` by 0 MiB against an apparent
  5.9 GiB). Delete the whole directory freely; step 1 above recreates what it needs.
- CI remains deferred by maintainer instruction and is not a criterion for anything here.

## Measurements taken this session, for reuse

- `skopeo copy` of the approved workspace image: **38 s**, 2.9 GB, 35 layers,
  publicly pullable with no auth, single-platform `linux/arm64`.
- `arca-engine image load` into a fresh state root: **~1.2 s and 0 bytes** of real disk
  (APFS clone-on-write). Per-test state roots are therefore fine; the design does not
  need a shared engine store.
- Arca engine tier against 1-layer Alpine: **2 passed, 6.44 s**, including a full
  `up`/`exec`/`logs`/restart/`down` against a real VM. This is the baseline to compare
  against once the revert lands.
- Cold unpack of 36 layers: **18.74 s**. Unpacking was never the failure.
