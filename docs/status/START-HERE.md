# START HERE

This file is the session entry point. It is written to be read cold, and it is
addressed to you, the agent. Follow it as instructions — there is nothing to paste.

Rewritten 2026-08-18 after **MILESTONE 4 MERGED.** Both pull requests are merged as true merge
commits. Everything above the `Where the work is` heading is current; everything below it is
history.

---

## MILESTONE 4 IS MERGED. THE OFFLINE PROOF REFUTED, AND THAT IS THE RESULT THAT SHIPPED.

**Read these four, in this order.**

| | |
|---|---|
| Design | `docs/superpowers/specs/2026-08-16-p5-1-milestone-4-product-wiring-design.md` |
| Plan | `docs/superpowers/plans/2026-08-16-p5-1-milestone-4-product-wiring.md` |
| **The offline evidence** | `docs/evidence/2026-08-18-arca-engine-offline.md` — **read this before touching anything about offline** |
| Ledger | `.superpowers/sdd/2026-08-16-p5-1-milestone-4-product-wiring/progress.md` — untracked and git-ignored **by the maintainer's decision, confirmed 2026-08-18. Do not commit it.** It holds ~170KB of rulings and deferred-finding dispositions and exists only on this machine; that is intended. |

**RE-VERIFY EVERY SHA BELOW WITH `git log -1` AND `git ls-remote`.** This file has gone stale
on its own SHAs repeatedly, including inside a single edit three lines below its own warning
about it. That is why no head SHA is written here: every edit to this file moves it.

| | merged into `main` as | merge commit | parents |
|---|---|---|---|
| Gas Can | #77 | `d65801d` | `7e8bb5c` + `6600201` — three SHAs, verified with `git rev-list --parents -n1` |
| Arca | #59 | `6460a210` | `5e11704` + `ae92360` — three SHAs, likewise |
| `containerization` submodule | — | `6304122`, unchanged | `git ls-remote git@github.com:Vas-Solutus/arca-containerization.git refs/heads/merge/upstream-main` returns it |

**Merged `main` was verified green after the fact, not before**: `cargo fmt --all --check` 0,
`cargo clippy --workspace --all-targets -- -D warnings` 0, `cargo test --workspace` **1496
passed, 0 failed, 49 ignored**, run at `d65801d`.

**The three milestone-4 branches were deleted after merging** — Gas Can
`feat/milestone-4-product-wiring` and `docs/milestone-4-merged`, Arca `feat/milestone-4-engine`
— each confirmed an ancestor of its `main` with `git merge-base --is-ancestor` first. Both
repositories are on `main` with clean worktrees. **Start from `main`; there is no branch to
check out.** Other stale branches predate this milestone and were deliberately left alone.

**The tag `gascan-engine-m4` and its release are published and verified. They were NOT re-cut
and must not be.** The signed tag object is `d143a66`, pointing at commit `c545612`; the
release carries `vmlinux-arm64.gz` (9,092,349 bytes) and `vminit-oci-arm64.tar.gz`
(73,739,738 bytes), and `engine/arca-pin.json` names that revision and both assets by
transport digest and content digest.

### THE HEADLINE, UNCHANGED: THE OFFLINE PROOF REFUTED. `CERTIFIED_ENGINE_REVISION` STAYS `None`.

**MEASURED against the pinned engine `c545612b`: an `offline` sandbox has full internet
egress.** Thirteen violations, against a positive control in which every probe succeeded on a
networked sandbox, and with `Sandbox::boot` asserting the compiled `CreateRequest` carried
`RuntimeNetwork::Offline` before anything was observed.

Do not set the constant. Do not change `capabilities.offline` from `.unverified`. The plan's
Task 15 acceptance pair is **UNREACHABLE** and its instructions must not be followed as
written. `crates/gascan-arca/tests/live/network.rs`'s
`an_offline_sandbox_has_no_egress_at_either_privilege_level` **FAILS BY DESIGN** — do not
weaken it to make the tier green. Reaching `Proven` is Arca work, not a re-tag of this tree.
**The fail-closed default is what the evidence vindicates.**

### WHERE THINGS STAND: BOTH MERGED

- **Gas Can #77** — "the Arca engine wired into the product, and the offline proof that
  refuted". Merged `d65801d`.
- **Arca #59** — "landing 1: the engine's half, sealed by gascan-engine-m4". Merged `6460a210`.

Both descriptions were rewritten to match their contents before either left draft. Arca's had
been wrong about three things: tasks 5-7 ARE done, the submodule DID move to `6304122`, and the
tag is published.

  **A fifth review was run against `4134b54..e14be74`**, because the landing review had covered
  only `5e11704..4134b54` and three commits landed after it — including the ~4,600-line kernel
  recipe. It is committed at `docs/status/review-arca-tail.md`. No Critical, and **nothing
  required re-cutting the tag**: every published digest verifies. Three findings were fixed in
  Arca `ae92360` — a stale `vmlinux` that defeated the post-build guard, a release document
  naming the wrong commit for the tag, and a `make build-assets` recipe that did not build what
  the doc said it built. **Two are NOT fixed and are real**: the kernel toolchain is unpinned,
  and the required-config assertion checks a text file rather than the built artefact.

**Arca merged first, then Gas Can**, both as true merge commits. The ruleset that enforces it
is `main protection` on each repository — `allowed_merge_methods: ["merge"]` plus
`required_signatures`, and **no required status checks**, which is why the by-design red
`engine` job did not block. There is no classic branch protection on either `main`; querying
that endpoint returns 404 and tells you nothing.

**The next milestone starts from `main` in both repositories.** The open items below are what
it inherits.

### WHAT LANDED THIS SESSION

| | What | Commit |
|---|---|---|
| Item 1 | the backend-scoped controller store | `ae75595` |
| Item 2a | every Arca startup failure reaches the user by name | `f081e61` |
| Item 2b | `gascan doctor` answers without a daemon | `fb7d4b0` |
| Hardening | the doctor status crossed the wire through two hand-written tables | `0bf6d75` |
| Review fixes | nine defects four reviewers found | `de14a94` |
| The reviews | committed verbatim | `436c5b4` |

**Each item was proven by mutation**, and each commit message records the mutations and their
results. `de14a94` also **corrects four claims in `ae75595`'s own message** that the review
showed do not reproduce — read that block before trusting any number in `ae75595`.

**The review found a Critical.** `gascan doctor` replaced a live daemon's answer about its own
engine: `runtime.cli` is measured from `GASCAN_ENGINE_BIN`, which is per-process, and the host
facts were applied unconditionally. A shell without the variable turned a healthy check into a
failure while the user's sandboxes ran on that engine; a shell with a newer path masked the
daemon's honest report of a deleted one. **No test could have caught it** — the e2e harness
sets the variable on the CLI and the spawner forwards it, so the two always agree there.

**It also found that `uninstall.sh --remove-data` destroyed one backend's sandboxes and then
deleted every backend's store** — this milestone's own harm, reintroduced at uninstall time and
made deterministic rather than accidental. It refuses now, and reads which backend was
enumerated from the daemon's instance record rather than owning a second copy of the rule.

**All five review files are committed at `docs/status/review-*.md`**, unedited, including the
findings that were deliberately NOT fixed and why. The fifth, `review-arca-tail.md`, exists
because the Arca landing review had covered `5e11704..4134b54` and three commits landed after
it; it found three things worth fixing and two — an unpinned kernel toolchain, and a
required-config assertion that checks a text file rather than the built artefact — that are
real, unfixed, and narrow the reproducibility claim until they are done.

### THE ONE THING TO DO NEXT

**DONE, on a branch, not yet merged — `fix/daemon-instance-publish-race`.** Verify the head with
`git log -1` and `git ls-remote`; do not trust a SHA written here. Open item 1 below now records
what is left rather than what to do, and **what is left is real** — see its residual section.

**The next assignment is the maintainer's to choose, and this file does not choose it.** The
list below is carried state. If you want a recommendation: item 1's residual (`retire_held_record`
still walks the destination through 0200-with-content, the last in-tree producer of the state
this milestone removed from `gascand`) is the natural continuation and is contained; item 2c is
the one that needs a decision rather than an implementation, and **must not be decided alone**.

### WHAT IS OPEN

1. **THE DAEMON INSTANCE RECORD'S PUBLISH RACE IS FIXED, ON A BRANCH. WHAT REMAINS IS THE
   READER'S HALF, AND IT IS NOT FIXED.**

   **What it was.** `write_instance_record` (`crates/gascand/src/socket.rs`) created the record
   at its final path inert — 0200, empty — wrote the content, `sync_all`-ed, then chmod-ed to
   0600. Across that fsync the path was mode 0200 **with content**, which
   `crates/gascan/src/daemon.rs`'s `validate_file_stat` calls "written but never published" and
   `inspect_with` turns into `DaemonState::Unsafe`. Retirement had the mirror-image bug: `Drop`
   chmod-ed to 0200 and then truncated, walking out through the same state.

   **MEASURED**, one polling observer against 2000 publish-and-retire cycles, in a temporary
   probe not retained in the tree: the original showed 0200-with-content **12,131,645 of
   24,438,995 samples — about half of every sample taken**; with publication renamed, **6,812 of
   31,664,387**, all of it retirement; with both renamed, **0 of 47,124,057**. Both are now
   staged under a private name and renamed into place, which is what `SocketPaths::bind` in that
   same file already did for the socket. The bounded 64-cycle form of that probe is committed as
   `no_reader_ever_sees_an_illegal_state_across_start_and_stop`.

   **THE HEADLINE CORRECTION, AND DO NOT LOSE IT: THE PATH IS NOT DOWN TO THREE FACES.** A first
   draft of the `validate_file_stat` comment claimed it was, and two independent reviewers caught
   it. `gascan`'s own reclaim, `retire_held_record` (`crates/gascan/src/daemon.rs:1453-1455`),
   still does `fchmod(0200)` then `ftruncate(0)` on a **published** record that
   `validate_held_published_record` has just proven is still linked at the destination. So the
   destination still goes 0600-with-content → **0200-with-content** → 0200-empty on the
   `recover_stale_published_record` path — the ordinary "previous daemon was SIGKILLed, next
   `gascan start` cleans up" path. And `inspect` (`daemon.rs:1966`) takes **no** lifecycle lock
   while `start_with` (`:1171`) does, so a concurrent `gascan status` can still sample it.

   It is two syscalls wide rather than an `fsync`, and the record there has been proven dead
   twice over, so the verdict it produces is unflattering rather than false. **That is why it was
   not folded into the same change**: `validate_retired_tombstone` (`:1548`) requires the held
   descriptor's inode to still be *at the name* and requires `st_nlink == 1`, and a rename
   unlinks it — so staging-and-renaming there means rewriting that validation against the new
   tombstone rather than the old descriptor. That is a design change to the reclaim protocol, not
   a mechanical one.

   **Also true, and also not fixed: the reader has no retryable verdict.** Every disagreement
   between two of its observations is terminal. `validate_instance_tombstone`
   (`daemon.rs:2842-2880`) re-opens the tombstone by name and returns a terminal
   `PermissionDenied` if a successor published in between; `open_published_record` (`:2611`)
   reports a legitimate concurrent *stop* as `Unsafe`. Both are pre-existing and both are now
   narrower — a rename rather than an fsync — but the shape is unchanged: **every narrow window
   is a terminal verdict waiting for a loaded machine.**

   **One window was introduced by the fix and is benign today:** `clear_inert_destination` unlinks
   the tombstone, so `daemon.rs:2852` can now return `ENOENT` where it could not before.
   `read_instance_record_for_inspection` maps `NotFound` to `Ok(None)`, so `inspect_with` is
   unaffected; only `read_attested_instance` (`:937`) propagates it, and that has no non-test
   callers yet. **It will matter when Task 6 wires it.**

2. **(2c) A PRODUCTION STDERR DESTINATION FOR THE DAEMON IS STILL DEFERRED**, as its own
   decision with a privacy dimension — a daemon log holds sandbox names, project paths and
   guest output. (2a) and (2b) have landed, which was the precondition. Raise it with the
   maintainer with a concrete proposal for where the file lives, its mode, how it is bounded
   and what is redacted. **Do not decide it alone.**
3. **~60 deferred Minor findings** with rulings in the ledger, plus the minors in this
   session's four review files and the previous whole-landing review. Task 6 M3 and Task 7 O1
   are still open.
4. **The process-level legacy-migration coverage was dropped and nothing replaced it.**
   `ae75595` narrowed it and its admission was a category slip, corrected in `de14a94`. A
   regression in the Apple path's `main.rs` wiring would be caught by no test in the suite.
5. **A guest that refuses at boot is loud in the guest and silent to the host** — `Start` never
   returns and the only diagnostic is in `bootlog.log`.
6. **The guest-side ordering instrument** at
   `.superpowers/sdd/.../carry-layer_report-live-test.rs` is **git-ignored**. Land it in the
   live tier before it is lost.
7. **One untested ordering, labelled as such in the code**: in `crates/gascand/src/engine.rs`,
   swapping the exit-status and timeout checks leaves the supervisor suite green.
8. **A pre-existing stash is on the stack and is not ours.** Leave it.
9. **SIX FILE-PROTOCOL VALUES ARE DECLARED INDEPENDENTLY IN BOTH CRATES, AND ONE OF THEM DOES
   NOT EVEN SHARE A NAME.** Found by review on 2026-08-18. `DIRECTORY_MODE` 0o700
   (`gascand/src/socket.rs:14`, `gascan/src/daemon.rs:16`); 0o600, which is `SOCKET_MODE` in one
   (`socket.rs:15`) and `FILE_MODE` in the other (`daemon.rs:17`) — **which is how a duplicate
   survives review: nothing greps it up**; `INSTANCE_TOMBSTONE_MODE` 0o200 (`:16`, `:18`);
   `SOCKET_NAME` (`:17`, `:19`); `INSTANCE_NAME` (`:18`, `:20`); `LIFECYCLE_LOCK_NAME` (`:19`,
   `:21`).

   The publish-race fix **makes this mildly worse**, and that is worth stating plainly: it adds a
   stronger unwritten rule — the instance path shows exactly three faces — asserted by a test in
   `gascand` and consumed by a classifier in `gascan`, with nothing mechanically connecting them.
   Change `gascan`'s `INSTANCE_TOMBSTONE_MODE` and `gascand`'s tests still pass while the
   classification silently breaks, which is the exact failure that cost five workspace runs.

   `crates/gascan-core` is already a path dependency of **both** (`gascan/Cargo.toml:15`,
   `gascand/Cargo.toml:10`), so the shared home exists and costs no new crate. **Its own commit**
   — folding it into a fix buries the fix under a cross-crate move.

### WHAT WAS RUN, AND WHAT CI DOES WITH IT

Every CI step, run locally and alone at `436c5b4`:

| Step | Result |
|---|---|
| `cargo fmt --all --check` | exit 0 |
| `cargo clippy --workspace --all-targets -- -D warnings` | exit 0 |
| `cargo test --workspace` | **1496 passed, 0 failed, 49 ignored** |
| `scripts/ci-check-ignored-tests.sh` | 49, matching the baseline |
| `scripts/ci-run-release-contracts.sh` | **15 contracts, status 0** |

The `gascan-arca` live tier was last run at `3882a52` — **24 passed, 1 failed in 568s**, the
failure being the offline test that fails by design, with the engine's `virtualization`
entitlement verified as `1` before the run. **It was not re-run after `3882a52`**, because
nothing since touches an engine code path and re-running it needs a rebuilt, re-signed engine.

**CI'S `engine` JOB IS RED, AND IT WAS RED BEFORE THIS MILESTONE.** Its live-tier step sets
only `GASCAN_ARCA_ENGINE_BIN`; the tier needs four variables and absence is a `panic!`, never a
skip. `cargo test -p gascan-arca --test live -- --list --ignored` reports **25 tests**.

**CORRECTION, MEASURED ON CI AT `436c5b4`: the long-standing claim that "only the 5 in
`connect.rs` and `read_rpcs.rs` need nothing but the binary" IS FALSE, and the remedy it
implies does not work.** That run produced `0 passed; 25 failed` with only
`GASCAN_ARCA_ENGINE_BIN` set, and the five in question panicked on
`GASCAN_ARCA_KERNEL_PATH` like the rest. The reason is `EngineInputs::from_environment`
(`crates/gascan-arca/tests/live/common/mod.rs:86-105`), which every `LiveEngine` goes through
and which reads **three** variables unconditionally; `base_oci_layout()` reads the fourth.

**An `#[ignore]` reason is not a requirements list**, and those five said only
`GASCAN_ARCA_ENGINE_BIN` while needing three. They now name all three. I had "verified" the old
claim by reading the ignore reasons rather than the harness, which is how a false statement
survives a check — the check has to touch the thing that decides, and here CI did.

**Do not make that job green by deleting tests, and do not try to select a subset — there is no
subset that runs on the binary alone.** It needs a runner with the artifacts.

### EIGHT THINGS THAT WILL COST A SUCCESSOR REAL TIME

1. **`ps -A` CANNOT ENUMERATE THE PROCESS TABLE ON THIS HOST.** Measured: 31 entries against
   `launchctl list`'s 544, and it omits even the calling shell. **No `ps | grep` absence check
   is evidence of anything here.** To find an engine, read `<socket>.lock`, which holds its pid
   (`Sources/ArcaEngine/EngineServer.swift:551`) — **never `pkill -f`**, which destroyed a
   session's shells once. If an engine dies with `vmnet_return_t(rawValue: 1001)`, force-quit
   `InternetSharing`.
2. **A UNIX SOCKET PATH IS 104 BYTES AND THE ENGINE REFUSES A LONGER ONE** rather than
   truncating. MEASURED: an engine given a socket under a deep scratch path initialised every
   manager and then died on `unixDomainSocketPathTooLong`, having bound nothing. Both e2e tiers
   use `/private/tmp` and one-letter names for that reason —
   `crates/gascan-e2e/tests/arca_startup.rs` included, where the daemon refuses a longer one
   with `path must be shorter than SUN_LEN` before it spawns anything.
3. **`--disable-swift-testing` IS NOT PORTABLE BETWEEN THE REPOSITORIES.** On the submodule it
   runs **0 tests and exits 0** — a false green. The submodule's suite is plain `swift test`.
4. **THE ENGINE LOSES ITS ENTITLEMENT TO EVERY `swift test`.** Re-sign after the last one and
   verify `codesign -d --entitlements - <bin> 2>&1 | grep -c virtualization` prints `1`. This
   is why no Swift suite was re-run this session and why no fresh Arca test count is quoted:
   the counts in the tests carry their own anchors instead.
5. **NEVER GIVE SUBAGENTS NAMES WHERE ONE IS A PREFIX OF ANOTHER**, and **REQUIRE EVERY REVIEW
   TO BE WRITTEN TO A FILE BEFORE THE REPLY.** MEASURED a third time on 2026-08-18: all four
   reviewers went idle without ever delivering a reply, and **all four files survived**. The
   file is the deliverable; the reply is not.
6. **NEVER RUN THE WORKSPACE SUITE BESIDE ANOTHER CARGO OR CONTRACT JOB**, and note that a
   solo run is internally parallel too — see open item 1, which is why. Exonerate by diff PLUS
   isolation, never by probability. **Corrected 2026-08-18: this said "open item 2", which is the
   deferred stderr destination. The race is open item 1. Trap 8 below carried the same wrong
   pointer, and the heading above said SEVEN over a list of eight.**
7. **A REVIEW'S SCOPE IS NOT THE PR'S SCOPE UNLESS SOMEONE CHECKED.** Arca's landing review
   covered `5e11704..4134b54`; three commits landed after it, one of them ~4,600 lines of
   kernel recipe, and no review had covered them until one was run before merging. Check what a
   review's own header says it read before treating a PR as reviewed.
8. **A TEST FIXTURE THAT WRITES THEN CHMODS RACES ITS OWN READER.** `rust` went red on CI at
   `436c5b4` while green locally: `DelayedPublicationSpawner` did `fs::write` then
   `set_permissions(0o600)` from a spawned thread, and `fs::write` creates at the umask, so the
   readiness poller could see the record at **0644** and report `Readiness { state: Unsafe }`,
   which is terminal. REPRODUCED deterministically by widening the window with a 20ms sleep.
   Fixed by staging and renaming. ~~**The production publisher has the same shape** — see open
   item 2~~ — **it did, and it was fixed the same way on 2026-08-18; see open item 1. The
   pointer said item 2, which is a different item.** The two other spawner fixtures do not have
   the shape, because they run synchronously inside `spawn()` before any poller starts.

   **The fixture's own comment claimed its rename was "what the production publisher achieves by
   creating the file inert and chmod-ing it last." It was not.** Chmod-ing last still showed
   content at the published path for the length of an `fsync`; the comment credited production
   with a safety it did not have. Corrected in place. **A fixture comment that describes
   production is a claim about code it cannot see, and this one was wrong for eleven days.**

### THE LIVE TIER'S ENVIRONMENT, AND THE DAEMON'S IS THREE

The `gascan-arca` live tier needs four, all undefaulted — absent means `panic!`:

```
GASCAN_ARCA_ENGINE_BIN      second line of scripts/build-arca-engine.sh
GASCAN_ARCA_KERNEL_PATH     ~/Library/Application Support/dev.gascan/engine/vmlinux
GASCAN_ARCA_VMINIT_LAYOUT   ~/Library/Application Support/dev.gascan/engine/vminit
GASCAN_ARCA_BASE_OCI_LAYOUT an OCI layout with one small linux/arm64 image carrying sh and nc
```

The `gascan-e2e` arca tier needs only the **first and last**: the daemon resolves the kernel
and vminit itself, from what `gascan engine fetch` installed, which is the point.

**The daemon's own environment is THREE:** `GASCAN_ENGINE_BIN`, `GASCAN_ENGINE_SOCKET` and
`GASCAN_ENGINE_STATE_ROOT`, all undefaulted and all required when `GASCAN_ARCA_BACKEND` is set.
Since this session, **each one's absence reaches the user by name** rather than as a readiness
timeout, and `gascan doctor` answers with real host facts even when the daemon cannot start.
`crates/gascan-e2e/tests/arca_startup.rs` is the instrument, and it needs no engine at all.

### WHERE THE CONTROLLER STORE LIVES NOW

**Scoped by backend, except Apple.** `~/Library/Application Support/dev.gascan/controller/`
holds `state.sqlite3` for Apple and `<backend>/state.sqlite3` for every other backend, named
with `BackendSelection::as_str()` so the directory and the daemon instance record cannot drift.
Apple stays unscoped because moving it would orphan every existing install's records while
their containers kept running.

The legacy runtime database is Apple's alone, and the reason is a date: `9c6933e` landed the
durable store on 2026-08-04, `7f9e8e6` landed the first non-Apple backend on 2026-08-17, and
`git merge-base --is-ancestor 9c6933e 7f9e8e6` confirms the order. A scoped store does not read
that location at all — not to migrate it, and not to refuse on its leftover sidecars.

## Where the work is

**EVERYTHING FROM HERE DOWN PREDATES 2026-08-17 AND DESCRIBES MILESTONE 3 AND EARLIER.** It is kept
for its reasoning, which is still good, and for the traps, which still bite. **It is not current
state** — the section above is. In particular, anything below saying milestone 4 is undesigned, or
that nothing is in flight, is stale.

**P5.1 MILESTONE 3 IS MERGED — 2026-08-16. SIX OF SIX TASKS DONE, BOTH PULL REQUESTS LANDED, NOTHING
OPEN, NOTHING IN FLIGHT.**

| | merged as | |
|---|---|---|
| Arca | **`5e11704`** | PR #58, from `feat/engine-rpc-surface` |
| Gas Can | **`da211d3`** | PR #75, from `docs/p5-1-milestone-3-design` |
| Submodule | **`3f68806`** | `containerization` on `merge/upstream-main`; it did NOT move this milestone, and both PRs left the pointer untouched |

**Both are true merge commits — `git rev-list --parents -n1` returns three SHAs for each, so nothing
was squashed** and the per-task history this file cites is intact. **Start the next piece of work on
a fresh branch off `main`.** Verify every SHA above with `git log -1` rather than trusting one
written here; this file has gone stale on its own SHAs six times.

~~**WHAT TO DO NEXT: MILESTONE 4, and it needs a design before it needs code.**~~ **DONE — see the
milestone-4 section at the top of this file.** It was designed, planned, and its first landing is
half built. Its design caught two false premises in the *parent* design before anything was built,
and its plan then carried three wrong instructions of its own, each caught by an implementer
re-deriving from source. The scope paragraph below is still the right description of what milestone
4 covers; it is only the "start by designing it" instruction that is spent.

### THE PRE-MERGE REVIEW ROUND, AND THE THREE DEFECTS IT COST — 2026-08-16 (late)

**Both PRs were reviewed before merge and both were BLOCKED by it. Three engine defects and one
false test came out, and none of them was visible to a passing suite** — Arca's was 221/0 green with
all three present. Arca `06a5162` and Gas Can `1a16158` are what closed them.

1. **The ten-second teardown bound was inert.** `completes()` raced `Task.value` against a sleep;
   `Task.value` is not cancellation-aware and `withTaskGroup` drains every child, so the race
   returned exactly when the work it bounded returned. The `Exec` handler could still hang forever.
2. **`forceKill` discarded the SIGKILL in the `startExec` window.** `execNotStarted` can only mean
   "not started yet" — `ExecManager` assigns `process` once (`ExecManager.swift:316`) and never
   clears it — so the log line "it may already have exited" said the one thing it cannot mean.
3. **A real client reset ran no teardown at all**, and only a VM could show it. A reset does not
   arrive as a stream failure: gascan's relay breaks on its own cancellation and drops the sender,
   which reaches the engine as an ordinary end of input, so `clientReset` read **false** and the
   session went into `await execution.value` — the one wait cancellation cannot interrupt. MEASURED:
   an exec of `sh -c "sleep 3600"` whose client dropped the session left `sleep 3600` in the guest's
   process table 30s later; 58 execs started and 57 deleted across that test region.

**THE FOURTH FINDING IS THE ONE TO READ TWICE, BECAUSE IT IS ABOUT A TEST AND NOT A DEFECT.** The
resize test shipped **the control instead of the subject**. `f59bbe2`'s own message records that the
variant *with* a readiness handshake **passed against the broken engine** — and that is the variant
that was committed, under the name
`a_resize_sent_before_the_process_starts_still_reaches_the_guests_terminal`. Reverting the engine fix
would have left both repositories green. **A test named for a window that closes the window before
testing it is worse than no test**, because the name is what a successor trusts. The handshake is
gone; the test now sends the resize with nothing read, and it passes against the fixed engine.

**Two process lessons, both paid for:**

- **A subagent dispatched in the background can go idle having delivered nothing.** Of four review
  and fix agents dispatched this way, one answered a retrieval request with a full review, one
  answered nothing across three probes, and two went silent after their work was already on disk.
  **Implementation output survives a lost report; a review does not.** Dispatch reviewers
  synchronously; a fixer may go in the background because the code is durable.
- **Verify a fix's test by mutation yourself rather than trusting the report.** Both engine
  mutations were re-run by the controller and fail **disjoint** sets of tests, which is what proves
  neither test rides on the other's fix. That check is what the resize test failed.

**ALL ELEVEN CONTRACT METHODS NOW ANSWER FOR REAL.** `unsupported_capability` appears nowhere in
this engine's answers. `tty` and `signals` are `true`, each earned by a live test that was SEEN TO
FAIL against a one-line mutation, so **`offline` is the only capability flag still false** and it
belongs to milestone 4. Anything below this line saying three RPCs refuse, or that `tty` and
`signals` are false, is history — it is left in place with its reasoning and marked.

| | |
|---|---|
| Design | `docs/superpowers/specs/2026-08-15-p5-1-milestone-3-rpc-surface-design.md` — **three of its premises were false; see below** |
| Plan | `docs/superpowers/plans/2026-08-15-p5-1-milestone-3-rpc-surface.md` — all six tasks, and one ruling in it was reversed (`:757`) |
| Ledger | `.superpowers/sdd/2026-08-15-p5-1-milestone-3-rpc-surface/` — disposable scaffolding, untracked, 21MB |

**THE LEDGER WAS NOT DELETED, AND THE INSTRUCTION TO DELETE IT HAS NEVER ONCE BEEN FOLLOWED.** This
file has said "delete it when the branch merges" since milestone 1. All four ledgers are still on
disk — `2026-08-05-arca-engine-pin` (300K), `2026-08-10-…-milestone-1` (1.0M),
`2026-08-12-…-milestone-2` (2.0M), `2026-08-15-…-milestone-3` (21M). **Either delete them as a set or
stop writing the instruction**; a rule that four successors have declined to follow is telling you
something. Nothing in them is load-bearing: everything that must outlive a milestone is in this file.

**Milestone 3 is "finish the RPC surface", and that scope was a ruling, not the original plan.**
`CreateContainer` had no milestone; it was the third RPC answering `unsupported_capability`, and
**P5's exit criterion cannot be met while it refuses.** Ruled 2026-08-15 into milestone 3 as its
first task.

| Task | What | State |
|---|---|---|
| 1 | `CreateContainer` | **done** — 3 review rounds, live tier 15/15 |
| 2 | `runUntilQuiesced` | **done** — 2 review rounds, live `shutdown::` 3/3 |
| 3 | the `unpackLayerToCache` call test | **done** — 3 review rounds |
| 4 | `ExecManager.signalExec` | **done** — 1 review round |
| 5 | `Logs` | **done** — 2 fix rounds, re-review clean, live test **run and passing**; 2 Minors deferred (below) |
| 6 | `Exec`, then the `tty` and `signals` flips | **done** — 4 VM-free mutations run, 2 live mutations run, **1 review round: 3 behavioural defects found and fixed**, live tier 20/20 |
| 7 | the pre-merge review of both PRs, and its **3 engine defects + 1 false test** | **done** — Arca `06a5162`, Gas Can `1a16158`; 2 controller mutations run, failing disjoint sets; live tier 21/21 |

**Task 7 was not planned.** It is the pre-merge review round, added because the PRs were reviewed
before merging rather than after. **It found more real defects than any single task in this
milestone**, in code that had already passed six task-level reviews and a 20/20 live tier.

**SUPERSEDED THE SAME DAY — read the next paragraph before believing this one.** It said: both
branches are pushed, Arca `feat/engine-rpc-surface` on `git@github.com:Vas-Solutus/arca.git` and Gas
Can `docs/p5-1-milestone-3-design` on `https://github.com/Liquescent-Development/gascan.git`,
verified with `git ls-remote --heads` against the local HEADs and both trees clean. That was true
when written and task 6 has moved both since.

**RESOLVED, AND BOTH BRANCHES ARE PUSHED AGAIN — 2026-08-16 (late).** The maintainer unlocked
1Password, the same probe then exited 0, and Arca's task-6 commit went in as `af22685` with `%G?` =
`G`.

**BOTH BRANCHES HAVE MOVED SINCE, and the SHAs in the sentence above are the task-6 commits, not the
branch tips.** The review round added two commits to each. **As of the last push:
Arca `feat/engine-rpc-surface` at `8679113`, Gas Can `docs/p5-1-milestone-3-design` at `281c6bd`**,
both verified with `git ls-remote --heads` against their local HEADs, both trees clean.
**Read HEAD with `git log -1` rather than trusting any SHA here** — this is the fifth time a SHA in
this file has gone stale under a following commit, and the fifth time is not the last.

**The block below is kept because the failure is worth recognising on sight, not because it is
current.** It cost nothing this time only because the probe is one command.

**Arca's task-6 work was written, verified and staged — and would not commit. 1Password refused to
sign.** The trap this file already records, behaving exactly as recorded: the probe

```bash
cd ~/code/arca && echo test | ssh-keygen -Y sign -n git -f <(git config --get user.signingkey)
```

fails with `Couldn't sign message (signer): communication with agent failed` while `ssh-add -l`
lists the key happily, because listing needs no authorisation and using one does. `git commit` was
then attempted once and failed the same way — **`fatal: failed to write commit object`, no commit
object created, `git log -1` still `2248035`**, with all four files staged in the index. Nothing is
lost and nothing is half-applied.

**What a successor must do: unlock 1Password at the keyboard, then**

```bash
cd ~/code/arca && git commit -F \
  ~/code/gascan/.superpowers/sdd/2026-08-15-p5-1-milestone-3-rpc-surface/arca-task6-commit-message.txt
```

The message is saved there verbatim, with every figure it cites, because a staged index is not a
durable place to keep prose. **Never `--no-gpg-sign`.** Verify `%G?` is `G` afterwards.

Gas Can's half was committed and signed throughout — `faf35ed`, `test(arca): drive Exec live, and
retire the unimplemented-method list`, `%G?` = `G` — because **signing is inverted between the two
repositories** and only Arca's key needs the agent. ~~Neither branch has been pushed since task
6.~~ **Both are pushed; see the resolution above.** That struck-through sentence was true for the
twenty minutes between writing it and the maintainer unlocking 1Password, which is the second time
this section has falsified itself about pushing in two days.

**Neither is a PR yet, and the `containerization` submodule has NOT moved this milestone** — so
milestone 2's hard-won rule that the submodule must be pushed and reachable *before* Arca's merge
does not bite here. **Re-check it before opening Arca's PR anyway**, because a submodule pointer that
moves late is exactly how a fresh clone breaks at `git submodule update --init --recursive`.

**Verify every SHA with `git log -1` rather than trusting one written here.** They go stale on every
pass over this file, and this milestone has already proved that four times — including once in this
very section, which said "neither branch is pushed" for the twenty minutes between writing it and
pushing them.

**SUPERSEDED TWICE IN ONE DAY, AND BOTH SUPERSESSIONS ARE INSTRUCTIVE.** This paragraph first said
opening the two pull requests was the only open item; the pre-merge review then found three engine
defects and one false test, so it was not. It then said to merge them; **they are merged** — Arca
`5e11704` (#58) and Gas Can `da211d3` (#75), 2026-08-16 (late), both true merge commits.

**The merge rules, kept because they apply again next milestone:** merge commits only,
`allowed_merge_methods` is `["merge"]`, never squash. `ci / gate` is not a required check and does
not block. The `engine` job is red by design against the unbumped pin — **do not bump the pin to
make it green**; that is milestone 4's and it needs a signed tag. Re-check the `containerization`
submodule before any Arca merge: this time it was identical on `origin/main` and the branch tip, so
the PR moved no pointer, but a pointer that moves late is how a fresh clone breaks at
`git submodule update --init --recursive`.

### TASK 6 WAS REVIEWED AND THE REVIEW FOUND THREE REAL DEFECTS — 2026-08-16 (late)

**All three were in code that had already passed a 19/19 live tier**, which is the point worth
carrying: the tier proved `Exec` works, not that it behaves well when a client does something
unusual.

1. **A frame the engine would not act on ended the exec and SIGKILLed the guest.** One unmapped
   signal number destroyed a healthy process. Now a refusal is reported and the session continues;
   only a protocol violation or a client reset ends it.
2. **A signal arriving before `startExec` recorded the process was fatal** — Ctrl-C in the first
   tens of milliseconds refused the exec before the shell ran. Now held for the process, per task
   4's ruling that a signal must never vanish silently.
3. **An unbounded wait plus a best-effort kill could hold the RPC handler forever.** Now bounded
   once the session has decided to end.

**THE FIX FOR (2) WAS WRONG IN A WAY ONLY A LIVE TEST COULD SEE.** It waited only when the call
*threw* `execNotStarted` — which `signalExec` does and **`resizeExec` does not**, returning silently
in the identical situation (`ExecManager.swift:325-328`). So resize was still dropped while the
wrapper looked like it covered both. The new
`exec::a_resize_sent_before_the_process_starts_still_reaches_the_guests_terminal` caught it, and the
instrument was checked before the subject: the same test with a readiness handshake in front of the
resize passed, so the trap was sound and the window was real.

**Two things from that round that will save a successor real time:**

- **`ReportFindings` does not exist in a subagent's tool set.** The first reviewer was told to report
  through it, could not, and went idle twice having produced nothing. The controller has that tool;
  a subagent does not. **Do not require a subagent to report through a tool without checking it has
  one** — ask for the fields in its reply instead.
- **The entitlement trap bit the controller within the hour of citing it to a subagent.** A single
  `swift test` run *after* re-signing failed all five of the next tier's tests with `engine exited
  with exit status: 1 before accepting a connection`. MEASURED: `codesign -d --entitlements -`
  reported **0** matches for `com.apple.security.virtualization` before re-signing and **1** after,
  and the same tests passed with nothing else changed. **Re-sign after the last `swift test`, not
  before it.**

### WHAT TASK 6 FOUND, AND IT IS THE ENTRY TO READ FIRST

**`Exec` deadlocked every consumer that had to write before the guest would speak, and neither
repository's unit tests could see it.** grpc-swift accepts an RPC implicitly when the first response
message is sent, so an engine that says nothing sends no response *headers* — and **tonic's
bidirectional call does not return a stream to its caller until those headers arrive**
(`gascan-arca/src/channel.rs:177-182`). A consumer with a shell, a REPL, or anything waiting on
stdin was stuck inside `backend.exec()`, unable to send the input that would produce the output that
would release it. Fixed with one line, `await context.acceptRPC(headers: [:])`.

**Three things about how it was found are worth more than the fix:**

1. **The first live exec passed straight through it.** `sh -c 'echo out; echo err 1>&2; exit 3'`
   writes before it is asked for anything, so it flushed the headers itself and returned a correct
   exit status of 3. The hang was the *second* exec in the same test, `cat`. **The RPC that works is
   the one that happens to speak first.**
2. **The first ten-minute hang produced no diagnostic at all**, because the test was blocked in an
   await with no bound — `backend.exec()` itself. `drain`'s 60-second bound was never reached
   because the test never got that far. **A bound on the wrong await tells you nothing.** Every await
   **that touches a live session** is bounded now — `Sandbox::exec`, `drain`, `send`, `refusal` and
   `read_until`. **`Sandbox::boot`'s are deliberately NOT**, so a boot failure reads the same here as
   in `lifecycle.rs` (`exec.rs:186-198`). This sentence read "every await in `exec.rs` is bounded now"
   until 2026-08-16, which `f8e3f79` had already retracted **in the file itself** — the retraction did
   not propagate here, and two later edits to this section left it standing.
3. **Arca's suite is at 221 passing with and without the fix.** Only the live tier can see it.

**A PLAN PREMISE WAS FALSE AND IT CHANGED WHAT THE LIVE TEST ASSERTS.** The plan said `signals`
would be earned by "reading the number back in `Exit.signal`". **Nothing carries a signal number
across the guest boundary:** vminitd reaps with `wait4` and `Command.toExitStatus` collapses the
status to `128 + N` (`ContainerizationOS/Command.swift:306-315`), and **`ExitStatus` carries no signal
number** — its two fields are `exitCode` (`ExitStatus.swift:23`) and `exitedAt` (`:25`). This file said
"one field" until 2026-08-16; Arca `008dfe5` had already corrected that overstatement on the engine
side and the correction did not propagate here. gascan's Apple backend reports `signal: 0` for the same reason
(`gascan-apple/src/backend.rs:604`), so reporting anything else would also make the two backends
distinguishable by their framing. **`Exit.signal` is 0 and delivery is observed in `code`** — 143
for SIGTERM and 137 for SIGKILL, asserted with two numbers so an engine that hardcoded one fails.
That is the fifth stale or false premise this milestone has caught in its own documents.

**Three things carried, none of them blocking task 6:**

1. **Task 5's two deferred Minors, with the fix already worked out.**
   `LogReader.swift:164` (`unreadableTimestamp`) and `LogWriter.swift:221` (`unknownEncoding`) each
   put an **unbounded `String`** from a decoded entry onto the wire — the same defect the round fixed
   for a third arm. Measured at 300,028 and 300,043 characters against 580 for the fixed path. One
   line each through the existing helper; `ContainerLogCodec.quoted` is `private` and needs a
   `String` overload or `internal` visibility, while `quotedLineLimit` is already public and tested.
   Rated Minor because reaching them needs a crafted `combined.log` in the engine's state root —
   **but `LogReader` exists to handle the foreign-file case, so "needs a foreign file" is this
   component's threat model rather than an exemption.**
2. **The open attribution question on the drain grace** — see the falsified limit recorded in the
   shutdown section below. First observation of the 10s grace firing against a real client;
   **attribution is open and must be measured, not assumed.**
3. **The SDD ledger** at `.superpowers/sdd/2026-08-15-p5-1-milestone-3-rpc-surface/progress.md` holds
   the per-round detail and every report. **It is disposable scaffolding** — everything that must
   outlive the milestone is already in this file. Delete it when the branch merges.

### WHAT MILESTONE 3 HAS COST AND BOUGHT SO FAR — read this before writing a spec

**Three premises in the milestone-3 DESIGN DOCUMENT were false, and all three were caught by
implementers before they were built on.** They are listed here rather than quietly fixed because the
design is committed and a reader will meet it:

1. **§2.2 as written verified the wrong list.** It said confirm each *retained* resource; the
   container mounts `create.volumes`, a separate field, so `retained: []` bypassed the guard
   entirely — **the exact silent failure §2.2 exists to prevent**. Amended in Gas Can `474d195`.
2. **§2.5's "nothing has ever parsed those lines back" is FALSE.** Three call sites parse container
   log entries — two in `DockerAPI`, one in `ArcaDaemon` — all with a default-options
   `ISO8601DateFormatter`, which **cannot parse a fractional-seconds stamp**, and all three `continue`
   past what they cannot parse. **The change the design ruled would have made Arca's Docker surface
   report every container as having said nothing.**
3. **`combined.log` was never written by anything.** Seven references, all derivations or
   registrations, no writer. **A `Logs` reading it as designed would have returned an empty log for
   every container**, and a test using a real writer would have agreed.

**The pattern: every one was found by an implementer re-deriving from the source instead of
transcribing the brief.** Which is also why the plan's later tasks were expanded to *requirements*
rather than step-level code — see the note on task 4 in the plan, which records that the one
step-level brief contained three wrong details and the requirement-level ones did not.

 Both rulings closed, both review rounds done,
nothing open. Gas Can `main` at merge commit `e968ae1` (PR #71), Arca `main` at `b3ffdf5` (PR #57),
submodule `containerization` at `3f68806` on `merge/upstream-main`, reachable from its own remote so a
fresh clone resolves. **Both are true merge commits — two parents each, nothing squashed** — so the
per-task history this file cites by SHA is intact. All five landings, every task reviewed.
Twelve were planned; **nine** were added on maintainer rulings after a review, a spike or a
measurement found something real — 3b, 3c, 6b, 13a, 13b, the two follow-ons that closed the rulings
(**16** named volumes, **17** the shutdown crash), and the two review rounds over tasks 13-17 (**18**
and **19**), which between them found a Critical, seven Importants and seventeen Minors in work that
had shipped unreviewed. The table below is authoritative for which is which. **Everything is merged
and pushed. Start the next piece of work on a fresh branch off `main`.**

**What that means and does not mean.** The engine now creates, starts, inspects, stops and removes a
real sandbox in a real VM, and a published port is reachable from a test process. **Both maintainer
rulings are closed on measurements, and both branches are merged.** What is left is milestone 3.
**Its scoping question is settled and it is designed** — see "what comes after the merge" and
`docs/superpowers/specs/2026-08-15-p5-1-milestone-3-rpc-surface-design.md`.

### The three things a new session most needs to know

1. **`Create`, `Start`, `Stop`, `Remove` and a published port ALL WORK END TO END, measured.** The
   sentence this file led with for two days — "nothing has ever been executed end to end" — is retired.
2. **`named_volumes` IS NOW TRUE and the defect behind it is FIXED** (2026-08-14 evening, Arca
   `1d453cf` / submodule `ca47c87`, Gas Can `41ac39a`). All four capability flags this milestone
   planned are claimed.
3. **THE GRACEFUL-SHUTDOWN CRASH IS FIXED, and the thing this file said about it for two days was
   wrong.** It did NOT need a container: an engine that created none still crashed 1 time in 96. The
   engine waited on its LISTENING socket closing instead of on its ACCEPTED connections draining, so
   it shut its event-loop group down under live channels. **12/32 → 0/32** on the container case;
   **6/192 → 0/192** interleaved. Every live test now asserts a clean exit status.
4. **If the engine dies with `vmnet_return_t(rawValue: 1001)`, force-quit `InternetSharing`.** It cost
   an hour before anyone tried it.
5. **The `containerization` submodule moved to `ca47c87`.** Any worktree build, and anything that
   rebuilds the guest, must use that revision — `f02cdf9` predates the volume fix and a guest built
   from it measures the old behaviour.

| | |
|---|---|
| Arca | `feat/engine-state-ownership`, HEAD `1d453cf`, based on `cc316b65` — **read it with `git log -1`; the SHA here has gone stale on every pass over this file** |
| Arca submodule | `containerization` on branch `merge/upstream-main`, HEAD **`ca47c87`** — carries the guest (`vminitd`) and the EXT4 label code. **Not `f02cdf9`; that predates the volume fix.** |
| Gas Can | `docs/p5-1-milestone-2-design`, based on `6847d1e` — **HEAD is whatever commit last touched this file**, so read it with `git log -1`, do not trust a SHA written here — 28+ commits |
| Design | `docs/superpowers/specs/2026-08-12-p5-1-milestone-2-engine-lifecycle-design.md` |
| Plan | `docs/superpowers/plans/2026-08-12-p5-1-milestone-2-engine-lifecycle.md` |
| Parent design | `docs/superpowers/specs/2026-08-10-p5-1-engine-service-and-wiring-design.md` |
| Governing roadmap | `docs/superpowers/plans/2026-08-04-arca-integration-roadmap.md` — P0-P8; P0-P4 done, P5 current |

**THIS IS THE CURRENT TABLE — re-run and verified 2026-08-16 (late), after the PRE-MERGE REVIEW
ROUND and its three engine fixes (Arca `06a5162`). Both trees clean. It replaces the table taken
after task 6, whose figures are given in the right-hand column so the deltas are visible:**

| | | after task 6 |
|---|---|---|
| `swift test --disable-swift-testing --filter ArcaEngineTests` | `Executed 228 tests, with 0 failures` | 221 — +4 `ExecTeardownTests`, +3 cancellation tests |
| `swift test --disable-swift-testing --filter ArcaTests.NetworkPruneGateTests` | `Executed 3 tests, with 0 failures` | 3 |
| the live tier, `-- --ignored --test-threads=1` | **21 passed / 0 failed**, 289.04s, 3 non-ignored filtered out | 20 / 234.34s — plus the reset test |
| `env -u RUSTUP_TOOLCHAIN cargo test --workspace --no-fail-fast` | **74 targets / 1435 passed / 1 failed / 43 ignored**, counting only the 77 `test result:` lines whose filtered-out count is 0, as the overcounting trap requires | 74 / 1435 / 1 / 42 |
| `scripts/ci-check-ignored-tests.sh` | `43 ignored test(s), matching the baseline` | 42 |
| `cargo fmt --all --check` | exit 0 | — |
| `cargo clippy --workspace --all-targets` | no issues found | — |

**The live tier went RED before it went green, and that is the entry worth keeping.** The first run
after the fixes was **19 passed / 2 failed**: the new reset test, which had found defect 3 above, and
`shutdown::the_engine_exits_cleanly_with_a_client_channel_still_open`. The second run, after defect 3
was closed, is the 21/21 in the table.

**A SECOND SHUTDOWN DEFECT IS NOW CHARACTERISED, AND IT IS NOT THE KNOWN ONE.**
`shutdown::the_engine_exits_cleanly_with_a_client_channel_still_open` fails about **1 shutdown in
288** with `exit status: 1` — the engine's own deliberate error exit, not the kernel's 143. **It is a
different test and a different exit code from the exit-143 startup race** this file records
elsewhere; do not fold the two together. **Attributed by measurement rather than by argument**, since
the engine changed and the empty-diff exoneration was therefore unavailable: the identical signature
(`95 x exit status: 0, 1 x exit status: 1`) reproduced on `8679113` with **none** of the fixes
applied, 1 of 288 shutdowns, and did not appear with them, 0 of 288. **So it is pre-existing.** One
event cannot distinguish "unchanged" from "improved" and no such claim is made. **Milestone 4's**,
with the other shutdown race.

**The deltas, accounted for rather than accepted.** Against the branch as it stood after task 5:
**ignored +4** — `exec::` gains **four** and `read_rpcs::` swaps a retired name for a new one, so +5
added and −1 removed; **passed +0**, because all four new live tests are `#[ignore]`d;
**targets +0**, because `exec.rs` is a module of the existing `live` target. Passed plus failed is
**1436**, which is the plan's baseline exactly.

**MEASURED 2026-08-16 from the baseline file at each commit**, because this paragraph said "+3" and
"all three" for a day after the table above it was updated to 42:
`git show <sha>:tests/ci/expected-ignored-tests.txt | grep -c .` gives **38** at `b2b7a0e` (task 5),
**41** at `faf35ed` (task 6), **42** at `f59bbe2` (the review round). 38 → 42 is +4. The stale figure
was written at `faf35ed`, when +3 was correct, and left standing when `f59bbe2` added the resize test
and updated the table four lines above it. **That is the sixth self-falsifying claim this milestone,
and the second in this very section.**

**The fix round of 2026-08-16 (late) adds one more**, `exec::a_reset_before_the_process_starts_still_
kills_the_guest`, taking the baseline to **43** — `scripts/ci-check-ignored-tests.sh` reports
`43 ignored test(s), matching the baseline`. **The live tier has NOT been re-run since**, so the
20/20 in the table above predates both that test and the engine fixes it exists to catch. Do not
quote it as covering them.

**FOUR WORKSPACE RUNS, FOUR DIFFERENT SINGLE FAILURES, ALL IN `crates/gascan-e2e`, ALL EXONERATED
THE WAY THIS FILE REQUIRES — BY DIFF AND ISOLATION, NOT BY PROBABILITY.** Read them as a set, which
is the same reading that identified D7: different tests, one crate, one load condition.

| run | test | how it failed |
|---|---|---|
| 1 | `daemon_stderr_sink_survives_the_launching_cli` | D7 — `mode is 0200 … written but never published (mode 0200, size 375 …)`, **the same test and the same size as the first occurrence this file ever recorded**, 2026-08-12 |
| 2 | `daemon_kill_and_restart_preserve_runtime_truth` | `state Unsafe: interrupted daemon instance descriptor changed while opening it` |
| 3 | `no_sandbox_status_error_is_actionable_and_keeps_usage_exit` | `left: Some(70), right: Some(64)` — a daemon that failed to start, so the CLI returned the wrong exit code |
| 4 | `accepted_socket_without_http2_cannot_block_initial_probe` | `panicked at crates/gascan-e2e/tests/autostart.rs:809:5: exit code 70` — 2026-08-16 (late), the fourth distinct test and the second to surface a bare exit 70 |

**UPDATED 2026-08-18: runs 1 and 2 are attributed and their cause is fixed; runs 3 and 4 are
NOT, and this fix must not absorb credit it has not earned.** Run 1 is the publish race
directly. Run 2 is the same defect seen through a different door: "interrupted daemon instance
descriptor changed while opening it" is `crates/gascan/src/daemon.rs:2732`, inside
`open_interrupted_tombstone`, which is reachable **only** when the path is 0200-with-content —
so making that state unreachable from `gascand` closes it. Runs 3 and 4 remain unattributed;
both are bare exit 70 from a daemon that failed to start, and nothing measured this session ties
them to this defect. **If a workspace run goes red on either of those two tests again, it is not
this defect and the trap note below still applies to it.**

`git diff e9468d8..HEAD -- crates/gascan-e2e/ crates/gascan/ crates/gascand/` is **empty** and
nothing is uncommitted in those crates, so the branch cannot have caused any of them. Each target
passes alone: `autostart` **16/16** with zero occurrences of `mode is 0200`, `fake_backend`
**28/28**, twice. Load averages were 3.3-4.9 throughout, which is the condition this file records
these scaling with.

**Do not read this as four green runs.** It is one accounted-for failure per run, exonerated
individually. **A clean local `cargo test --workspace` has never been achieved on this branch**, and
the standing rule that a green local workspace is the bar is therefore met only by isolation, which
is weaker. The root causes this file already names are the place to start when someone is asked to
fix them.

**Run 4's exoneration, 2026-08-16 (late), and it is the same shape as the other three.** The diff
above is still empty and `git status --porcelain` over those three crates is still empty, so the
branch cannot have caused it; `cargo test -p gascan-e2e --test autostart` alone is
**`16 passed; 0 failed`**. Four runs, four distinct tests, one crate. **That the failing test is
different every time is the finding** — a branch-caused failure does not wander.

---

**The 2026-08-14 table, kept as history. Re-run and verified then, after the review of tasks 13-17
and every fix BOTH ROUNDS produced — Arca `c68bd0a`..HEAD with submodule `30b9c8f`..HEAD, Gas Can
`53925e5`..HEAD. Both trees clean:**

| | |
|---|---|
| `swift test --filter ArcaEngineTests` | `Executed 160 tests, with 0 failures` — 151, plus 6 `LayerCacheRoleTests` and 3 `ShutdownObserverTests` |
| `swift test --filter ArcaTests.NetworkPruneGateTests` | `Executed 3 tests, with 0 failures` |
| `env -u RUSTUP_TOOLCHAIN cargo test --workspace --no-fail-fast` | exit 0 — **1436 passed / 0 failed / 36 ignored across exactly 74 targets** reporting `0 filtered out` |
| the live tier, `-- --ignored --test-threads=1` | **14 passed / 0 failed**, 216s, 3 non-ignored filtered out |
| `cargo fmt --all --check` | exit 0 |
| `cargo clippy --workspace --all-targets` | no errors |
| `scripts/ci-check-ignored-tests.sh` | `36 ignored test(s), matching the baseline` |
| `make vminit-rebuild` | 42s — the guest carries the new `ArcaBoot`, so the tier's 14/14 measures it |

**The earlier figures this table carried — 151 tests and an 84s tier — were measured against Arca
`9fac267`, before the review round.** They were left standing when the review section was added 560
lines below with different numbers, so the page briefly carried two "current" tables that disagreed.
That is the exact failure this milestone keeps writing traps about, in the file that holds the traps.

**The workspace deltas are accounted for rather than accepted**, which is the standing rule.
Against the 1436 / 33 / 74 baseline of the same morning: **ignored +3**, exactly
`shutdown::the_engine_exits_cleanly_{after_a_container_has_been_created,with_a_client_channel_still_open,with_nothing_holding_a_connection}`,
all three added to `tests/ci/expected-ignored-tests.txt`; **passed +0** because all three are
`#[ignore]`d; **targets +0** because `shutdown.rs` is a module of the existing `live` target.
Against the older 1435 / 26 baseline the earlier reconciliation still stands, below.

**The first of those two workspace runs was RED and it was not this branch.** It failed
`daemon_start_identity_is_stable_across_caller_locale_and_timezone` with **`mode is 0200 ... written
but never published`** — D7, the third recorded occurrence, and the section below is about it.
Settled the way this file requires rather than by probability: `git diff 6847d1e..HEAD --
crates/gascan-e2e/ crates/gascan/ crates/gascand/` is **empty**, so the branch cannot have caused it,
and `cargo test -p gascan-e2e --test autostart` alone is **16 passed, 0 failed**. The re-run above is
exit 0 with **zero** occurrences of `mode is 0200`. `pgrep -fl "cargo test"` was empty and recorded
before each run; `test result:` lines with a non-zero filtered-out count were excluded as the
overcounting trap requires.

**The live tier's four environment variables** — none defaulted, absent is a `panic!` with a directive:

```bash
export GASCAN_ARCA_ENGINE_BIN=~/code/arca/.build/arm64-apple-macosx/debug/arca-engine
export GASCAN_ARCA_KERNEL_PATH=$HOME/.arca/vmlinux
export GASCAN_ARCA_VMINIT_LAYOUT=$HOME/.arca/vminit
export GASCAN_ARCA_BASE_OCI_LAYOUT=/tmp/alpine-oci   # skopeo copy --override-os linux \
    # --override-arch arm64 docker://docker.io/library/alpine:3.20 oci:/tmp/alpine-oci:alpine:3.20
```

**The engine must be ad-hoc signed or it never creates a socket:**

```bash
cd ~/code/arca && swift build --product arca-engine && codesign --force --sign - \
  --options runtime --timestamp --entitlements Arca.entitlements \
  .build/arm64-apple-macosx/debug/arca-engine
```

### FIRST THING TO DO

**UPDATED 2026-08-15. Nothing is mid-flight. Milestone 2 is merged and milestone 3 is designed.**

**Read `docs/superpowers/specs/2026-08-15-p5-1-milestone-3-rpc-surface-design.md`, then write its
implementation plan.** That design is approved; do not re-brainstorm it. It carries six pieces —
`CreateContainer`, `Exec`, `Logs`, `ExecManager.signalExec`, and carried follow-ups (a) and (b) —
in a stated order, with §5 fixing what proves each one.

**Two things in it will save a session each.** It is **Arca-side Swift plus live tests**: Gas Can's
half is already implemented and tested, so no Gas Can PR is on the critical path (§2.1). And the
live-tier fixtures are one call each to affordances milestone 2 already built —
`layout_running` (`crates/gascan-arca/tests/live/common/mod.rs:737`) writes a one-image OCI layout
running any command, which is what both `Exec` and `Logs` need (§5.2).

**The everything-below-here for milestone 2 is history now.** The paragraphs on Landing 5, Task 13's
first hour and the two maintainer rulings are kept for their reasoning, not as current state.

The design records why the engine owns a private state root and why that made `initialize()` safe when
milestone 1 had rejected it. The plan carries the landings; 3, 4 and 5 were all expanded *after* the
task that preceded them ran, so they reflect what the machine actually does rather than what the code
appeared to say.

### THE ONE THING THAT MATTERS MOST ABOUT THE CURRENT STATE

**UPDATED 2026-08-13 (late). `Create` NOW WORKS END TO END, AND IT DID NOT BEFORE.** The sentence this
section led with for two days — "nothing has ever been executed end to end" — is retired for `Create`
and still true for everything after it. Measured by the controller from Gas Can's live tier against a
branch-built, ad-hoc-signed engine: a real engine on a real socket, its store seeded with `arca-engine
image load`, then `PrepareImage` → `Ok` and `Create` → `Ok` with three volumes, a network and **a
container**. That was the first container this engine has ever created, and closing it took two
unplanned Arca fixes (Tasks 13a and 13b below).

**SUPERSEDED — `Start`, `Stop` and `Remove` ALL RUN NOW**, driven end to end by the live tier
(`lifecycle::create_start_inspect_stop_and_remove_drive_a_real_container`, among others). The bullets
below are kept because their *reasoning* about what a VM-free test can and cannot reach is still
correct and still governs where a new test belongs. **Read them as history, not as current state.**

- **`Start` is entirely unproven.** `startContainer` is unreachable without a VM — it throws
  `notInitialized` first. The one test that drives it says so in its own name
  (`testStartOfAnOwnedSandboxReachesStartContainerAndStopsOnlyForWantOfAVM`).
- **Port publishing has three silent gates** and is provable only from the live tier.
- **`Remove` of a RUNNING container is untested** — that state is unreachable VM-free, because
  `loadPersistedState()` recovers every persisted `running` row as exited/137 first.
- ~~**The live tier cannot spawn the branch engine at all**~~ — **FIXED and proven**, Gas Can `776a71c`.
  It passes all four options now, with `--kernel-path` and `--vminit-layout` arriving as
  `GASCAN_ARCA_KERNEL_PATH` and `GASCAN_ARCA_VMINIT_LAYOUT`.
- ~~**Every capability flag is still `false`.**~~ — **SUPERSEDED TWICE.** `project_mount`,
  `loopback_publish`, `resource_limits`, `named_volumes` (2026-08-14) and — since 2026-08-16 —
  `tty` and `signals` are all `true`, each earned by a live test that fails without it. **`offline`
  is the only one left, and it is milestone 4's.**

**Landing 5 exists precisely to close this gap, and Task 13 is a step change in kind, not just the next
item.** It is the first task needing a real engine process, a real kernel and a real VM.

~~The three RPCs still answering `unsupported_capability` are **`CreateContainer`, `Exec` and
`Logs`**~~ — **ALL THREE ARE IMPLEMENTED as of 2026-08-16, and no method answers
`unsupported_capability` any more.** `CreateContainer` was milestone 3's task 1, `Logs` its task 5
and `Exec` its task 6. The line anchors this paragraph used to carry are gone with the refusals;
re-derive anything you need with `grep -n`, as this file's own trap requires.

### What milestone 2 has landed

| Task | Arca | Gas Can |
|---|---|---|
| 1 `ContainerManager` takes its storage roots | `8fd2757`..`1ff4304` | — |
| 2 `listContainers` gains `includeInternal` | `bd80701`..`4b34bfc` | — |
| 3 `listNetworks` throws | `8b3e16f`..`1201f4a` | — |
| 3b the prune-gate swallows | `493e5ce`..`1c6a851` | — |
| 3c the gate runs DockerAPI-side tests | `fede19c` | `142d199`..`cd00388` |
| 4 the three path options | `b93ef76`..`029c01d` | — |
| 5 vminit into the engine's own store | `e1b5d9a`..`595a450` | — |
| 6 `initialize()` before serving | `a0796c4`..`85b5023` | — |
| 6b sign the engine | `014c84b`..`db6bedc` | `a45edd4`..`c8e2c5b` |
| 7 `Inspect` | `db6bedc`..`40078e7` | — |
| 8 `ListResources` | `40078e7`..`40a1d55` | — |
| 9 `image load` subcommand | `40a1d55`..`65650b2` | `0fa74fe` |
| 10 `PrepareImage` | `65650b2`..`05b909a` | — |
| 11 `Create` — closed after 4 fix rounds | `05b909a`..`7511957` | — |
| 12 `Start`, `Stop`, `Remove` — passed review, no fix round | `7511957`..`9db2f7d` | `1726c77` |
| 13 the live tier's spawn, proven | — | `776a71c` |
| 13a the create path's container directory | `9db2f7d`..`1a78ef3` | — |
| 13b the container log directory | `1a78ef3`..`5e52aae` | — |
| 13 the lifecycle and the published port | — | `1020002`, `288b75c` |
| 14 the capability flips — **three of four** | `5e52aae`..`6c77bb8` | `782de04`, `1ce26d6` |
| 15 the workspace suite, run alone | — | — |
| 16 named volumes mount, and the fourth flag — **follow-on, after the ruling** | `1d453cf`, submodule `ca47c87` | `41ac39a` |
| 17 the shutdown crash, and the rate instrument — **follow-on, after the ruling** | `9fac267` | `3290af6` |
| 18 the review of 13-17, and its 1 Critical / 6 Important / 7 Minor | `c68bd0a`, submodule `30b9c8f` | `53925e5` |
| 19 the re-review of 18's fixes, and its 1 Important / 10 Minor | `8a26e15`, submodule `3f68806` | `455f328` |
| — the merge | `b3ffdf5` (PR #57) | `e968ae1` (PR #71) |

**Tasks 3b, 3c, 6b, 13a and 13b were not in the approved plan.** Each was added on a maintainer ruling
after a review — or, for 13a, a controller spike — found something real: a `try?` that let
`docker network prune` delete an in-use network, a gate that could not see the test guarding it, an
engine that could not start a container because nothing signed it, a hardcoded container directory that
broke `Create` and wrote into Apple's shared store, and a log directory that did the same to
ArcaDaemon's. **The last two are both Task 1 misses of the same shape** — a path that should have been
rooted when `ContainerManager` gained its roots and was not.

**Tasks 16 and 17 were not in the plan either**, and neither came from a review: each closed one of the
two maintainer rulings of 2026-08-14, and each was found by driving the real thing rather than by
reading. Both sections below record them.

**A CLAIM THIS FILE BRIEFLY CARRIED AND WHICH IS FALSE: "`ContainerBridge` now derives no state root it
was not given."** Task 13b reported it, this file repeated it, and 13b's reviewer disproved it within the
hour. **Do not restore it.** The true, narrower form:

> `ContainerLogManager` was the last `ContainerBridge` path a caller could not root **at all** — it took
> no parameter, so the derivation was unreachable from outside. **Three defaulted path parameters
> remain**, each falling back to an unrooted location that gets written to:
> `VolumeManager.volumesBasePath` (`VolumeManager.swift:51`, defaulting to `~/.arca/volumes`, which
> `initialize()` then creates), `ImageManager.imageStorePath` (`ImageManager.swift:16`, defaulting to
> Apple's shared store), and `ArcaConfig`'s `~/.arca` defaults (`Config.swift:29-30`, `:42`).

**No engine caller relies on any of them** — `EngineManagers.swift:71-74` and `:111` pass explicitly —
so this is pre-existing and not an engine defect. But `ArcaDaemon.swift:271` passes `nil` with the
comment `// Use default ~/.arca/volumes`, and **`ArcaTestHelper/main.swift:43` calls
`try ImageManager(logger: logger)` omitting the argument entirely**, so a live in-package caller already
takes Apple's shared store by default. Design §3's "none takes a default" is not yet true of
`ContainerBridge` as a whole; it is true of everything this milestone touched.

**The grep that finds these and that a `urls(for:in:)`-shaped search misses**, because
`VolumeManager`'s fallback is spelt `NSString(...).expandingTildeInPath` with no `FileManager` call and
no `Application Support` in it:

```bash
grep -rn --include='*.swift' -E '(Path|path|Root|root|Dir|dir|URL|url)[A-Za-z]*:\s*(String|URL)\?\s*=\s*nil|(Path|path|Root|root|Dir|dir):\s*(String|URL)\s*=\s*"' Sources/ContainerBridge/
```

**The lesson is worth more than the finding: an exhaustiveness claim is only as good as the search that
backs it, and a search built from the shape of the bug you just fixed will miss the ones spelt
differently.** Task 13b's greps looked for `FileManager.default.urls`, `expandingTildeInPath` and
`Application Support` — the vocabulary of the defect it had in hand — and found nine hits it could
account for. The grep above looks for the *shape of the seam* instead, and finds three more.

**Task 11 cost four fix rounds and every one was the same defect** — a claim that outran the code.
Seven instances across commit messages, source comments and reports; **every one caught by a reviewer
running a mutation, none by reading.** Task 12 shipped none, which is the first task this milestone
that did not. The rule that changed it is in the traps section below.

### The milestone's thesis, and that it now holds

Milestone 1 rejected calling `initialize()` because `ContainerManager`'s restore loop **writes** —
a persisted `running` container is marked exited/137 and written back (`ContainerManager.swift:317`,
write at `:333`). Against a state root shared with a live `ArcaDaemon` that declares the daemon's
containers dead.

**That hazard belongs to sharing a root, not to writing.** The engine now owns
`~/Library/Application Support/dev.gascan/engine/`, and against its own root the same write is
correct. VERIFIED by running it: the isolation probes —
`/usr/bin/find ~/.arca -newermt '-5 minutes'` and the same over
`~/Library/Application Support/com.apple.containerization` — came back **empty**, three separate
times, cross-checked with `-newer <marker>`. The engine built its own 512MB `initfs.ext4` inside
its own state root and never touched Apple's.

**THAT MEASUREMENT IS NARROWER THAN IT READS, AND CREATING A CONTAINER BROKE IT — 2026-08-13.** Those
probes ran when **no container had ever been created**, so they measured startup and nothing else. The
moment a container was created, two paths wrote outside the state root, and both are now fixed:

- **The container directory.** `ContainerManager.swift:1144` hardcoded
  `~/Library/Application Support/com.apple.containerization` while `getRootfsPath` derived the same
  directory from `manager.imageStore.path`. Task 1 moved one and not the other. **This also broke
  `Create` outright**, with `NSPOSIXErrorDomain Code=2` — Apple's manager, rooted at the private store,
  found no directory. Fixed in Arca `9b29399`, adjudicated in `1a78ef3`.
- **The container log directory.** `ContainerLogManager` derived
  `~/Library/Application Support/com.apple.arca/logs` for itself and took no root at all, so every
  engine wrote container stdout/stderr into ArcaDaemon's one shared directory **and deleted out of it on
  remove**. Fixed in Arca `5e52aae` (Task 13b).

**The re-measurement, and it is the one to quote:** with the fix, creating a container leaves Apple's
shared `containers/` directory count **unchanged (253 → 253)** and puts the directory under the
engine's own `<state-root>/images/containers/`. With `manager.imageStore.path` swapped for
`ImageStore.default.path`, the same run drives the count **253 → 254**, leaves the engine's own
directory empty, and fails `Create` — while **`swift test --filter ArcaEngineTests` stays at 149
passing**. **That pair is the whole argument for the live tier: Arca's suite cannot see this, and
nothing in that repository can.**

**The vmnet `host` network does not collide.** Two concurrent engines on different state roots
took `192.168.93.0/24` and `192.168.95.0/24`, both listening, allocation released on exit. The
`host` name is a row in each engine's own `state.db`, and `VmnetNetworkBackend`'s `isDefault`
guard reads a per-instance dictionary — not a host-wide namespace. **Limit:** no live `ArcaDaemon`
was run alongside; that case is inference from the identical code path, not observation.

### What shipped

**Arca** (`~/code/arca`, now on `main`) — seven task commits `bc03394..e74aff0`, a
dependency fix `8fc1ca5`, a comment fix `f5fde96`, and two answering the adversarial
review: `16abeec` (Inspect and ListResources) and `b3390b8` (the socket path and the
shutdown path). **30 tests pass** (`swift test --filter ArcaEngineTests`, exit 0); it was
27 before the review added three to `EngineServerTests`.

**Gas Can** (`~/code/gascan`, now on `main`) — design and plan (`33d37f9`, `4981b39`,
`77ff591`, `b36d18f`), Tasks 8-11, then the review wave `140b274..351a646`.

| Task | Commit | What |
|---|---|---|
| 8 | `f75d069`, `ddb4f6a` | `scripts/build-arca-engine.sh` builds the engine product, runs its tests in the verified clean checkout, and prints the binary path as a second stdout line |
| 9 | `cb81024` | the live harness — a real engine on a real socket, and the `connect` error paths |
| 10 | `2fe3711` | live coverage of the read RPCs — since replaced, see below |
| 11 | `c0e0cc8`, `aebf558`, `fb50d4c` | `tests/release/engine-targets-check.sh` — neither `arca-engine` nor `ArcaEngine` reaches `DockerAPI` or `ArcaDaemon` |

**1435 tests pass, 0 fail, 26 ignored** across 74 targets reporting `0 filtered out`
(`cargo test --workspace --no-fail-fast`, exit 0). It was 28 ignored: the live tier's
`Inspect` and `ListResources` tests folded into one that covers all ten unimplemented
methods, so the tier went from 8 tests / 6 ignored to 6 / 4, and nothing else moved.

### What the engine actually does

**SUPERSEDED 2026-08-16: ALL ELEVEN ARE IMPLEMENTED.** The paragraph below was true at `9db2f7d`
and is kept for the caution that follows it, which has not expired.

**As of `9db2f7d` EIGHT of the eleven are implemented: `Capabilities`, `Inspect`, `ListResources`,
`PrepareImage`, `Create`, `Start`, `Stop` and `Remove`.** There is also an
`arca-engine image load --state-root <R> --oci-layout <L>` subcommand. **Only `CreateContainer`, `Exec`
and `Logs` answer `unsupported_capability`**; **all three are milestone 3's**, and all three landed.

**Read the "NOTHING HAS EVER BEEN EXECUTED END TO END" section at the top before you believe any of
that means the engine works.** Eight implemented means eight that return the right answers to VM-free
unit tests.

**Three things Landings 3-4 established that change what you should believe:**

1. **`Inspect` reports what the STORE holds, deliberately** — including port bindings that were
   never published, because that is what drift detection compares against. **So `Inspect` can never
   be evidence that anything was actually done.** An engine can report a successful `Create`, a
   successful `Start`, and an `Inspect` naming a port, while publishing nothing, with every check
   green.
2. **`ListResources` reports unlabelled and internal resources with `owner` unset**, while `Inspect`
   *refuses* an unlabelled container as `foreign_resource_refused`. That looks inconsistent and is
   not: one reports what is held, the other answers about a specific claimed sandbox. **Do not
   "fix" the difference.**
3. **`ImageManager.resolveImage` was widened** to accept `repository@sha256:<hex>`, because
   `createContainer` uses ONE string both to resolve and to *record*, `startContainer` re-resolves
   that recorded string after a restart, and `Inspect` must parse it as a digest reference. One
   field, three constraints, and the third forces the form the first two rejected. **This changed
   Arca's Docker surface**: `docker run|rmi|inspect repo@sha256:...` now works where it threw.
   Deliberate, accounted for in `ba1900f`'s and `de8c880`'s messages.

The older text below described the milestone-1 state and is kept for its reasoning about *why*
`Inspect` and `ListResources` were once refused.

**`Capabilities` WAS the ONE implemented method. The other ten answered
`unsupported_capability`.** The engine runs — VERIFIED by running it: `arca-engine
--socket-path … --state-root …` logs `engine listening` and creates the socket
`srw-------`, and Gas Can's live tier drives all eleven over a real socket.

`Inspect` and `ListResources` were counted as real until 2026-08-12 and are not. The
process calls `initialize()` on no manager, so each could return exactly one answer under
every input — `absent`, and an empty list. Answering `absent` without having looked is
what makes a reconciler create a duplicate of a running sandbox; an empty `ResourceList`
is a confident report of a clean host, which is the report that hides a leak. Both now
refuse instead. The reasoning is on each method in `SandboxEngineService.swift` and in
`ArcaEngineCommand.run()`.

## What is left, now that the implementation is done

**RULED 2026-08-14: BOTH OF THESE WERE TO BE FIXED. BOTH NOW ARE.** The maintainer closed both as open
questions the same day — "seems like we need to fix both of the things you left alone".

**Both sections below are kept as records and are marked CLOSED; do not work either again.** The
named-volume defect was fixed on 2026-08-14 evening, the graceful-shutdown crash late the same day.
**Nothing is open. The remaining work is the merge and the pin, and the pin belongs to milestone 4.**

**The method is what found both, and it is worth repeating.** Both were
discovered by driving the real thing and measuring the result, not by reading. Every claim that
outran the code was caught by a mutation. Keep dispatching a fresh Opus implementer per task and an
Opus reviewer after each — **and read "when a subagent goes quiet" in the traps section before you
dispatch anything**, because that cost a duplicate agent on 2026-08-14.

### CLOSED 2026-08-14 — named volumes are mounted, and `named_volumes` is true

**FIXED, verified and committed.** Arca `1d453cf` (outer) and `ca47c87` (the `containerization`
submodule, on branch `merge/upstream-main`); Gas Can `41ac39a`. **Nothing here is open work.** The
section is kept because the way it was diagnosed is worth more than the fix, and because two confident
hypotheses were wrong in instructive ways.

**What it actually was, and it is NOT what this file predicted for two days.**
`ArcaBoot.prepareOverlayFS` decided what a virtio-blk device was by *counting*: it mounted `/dev/vdb`
as the writable layer, then walked `/dev/vdc` upward until a device was missing and called everything
it found a read-only OverlayFS layer. Named volumes are attached *after* the image layers, so it
swallowed them — mounting all three **read-only as overlay lowerdirs of the container's own rootfs**.
Measured from the guest console with a two-layer image and 256 MiB / 512 MiB / 1 GiB volumes:

```
vminitd: detected 5 OverlayFS layer block devices
EXT4-fs (vde): mounted filesystem ... ro without journal.   <- the 256 MiB volume
EXT4-fs (vdf): mounted filesystem ... ro without journal.   <- the 512 MiB volume
EXT4-fs (vdg): mounted filesystem ... ro without journal.   <- the 1 GiB volume
```

Separately, `LinuxContainer.swift:797` (host code, in the engine) dropped **every** mount whose source
began `/dev/vd` from the container's OCI spec, so `vmexec` never saw the volumes either — it reported
`mountToRootfs: processing 8 mounts` and none of them were volumes.

**So the earlier measurement was true and its interpretation was wrong.** "The reviewer checked all 18
mount lines and found them mounted nowhere" was correct — `/mnt/layer{N}` lives in the VM's *root* mount
namespace, outside the container's rootfs, so from inside the container they are invisible. They were
mounted the whole time, just not as themselves and not anywhere reachable. **A negative result inside
one namespace says nothing about another.**

**Two hypotheses this file carried were both wrong**, and neither could have been settled by reading:

1. **"A real destination that nothing in the boot sequence honours."** Wrong — nothing ever got as far
   as a destination.
2. **"The `/dev/vd` skip at `Server+GRPC.swift:659` is the mechanism."** Also wrong, and it was the
   controller's own first conclusion from the source. That branch logged **zero** times: `vmexec`
   mounts OCI spec entries directly and never goes through that RPC. **The guest console log
   (`<state-root>/images/containers/<id>/bootlog.log`, which captures hvc0) is what settled it**, and it
   is the instrument to reach for first on anything guest-side.

**The fix: identity travels with the artifact.** `EXT4.Formatter` writes an `ArcaBlockDeviceRole` into
the ext4 superblock's `s_volume_name` (`arca.writable`, `arca.layer`); the guest reads it back and
mounts only what carries a role it recognises. Count and order stop mattering. Volumes are formatted
with no label (`VolumeManager.swift:192`), so the guest leaves them alone and `vmexec` mounts them from
the OCI spec at the destination the host chose. The host filter now drops only mounts with an **empty
destination**, which is the host's own statement of intent rather than a guess about the source.

**THE COST ESTIMATE IN THIS FILE WAS WRONG BY TWO ORDERS OF MAGNITUDE, AND IT DELAYED THIS FOR TWO
SESSIONS.** It priced a guest-side fix at "a different kind of day — Go 1.24+, the Swift Static Linux
SDK, ~10 GB of disk and 20-25 minutes per build". Measured on this machine:

| | this file said | measured 2026-08-14 |
|---|---|---|
| Swift Static Linux SDK | needs installing | **already installed** — `swift sdk list` shows 6.2 and 6.3 |
| Go | 1.24+ needed | 1.26.3 present |
| `make vminit-rebuild` | 20-25 min | **41 seconds**, and it writes straight to `~/.arca/vminit` |
| Reproducibility | unknown | an unmodified rebuild reproduces the deployed guest **exactly** |

The 20-25 minutes is `make build-assets`, which also builds the kernel. **Check `swift sdk list` before
believing any cost estimate in this file.**

**THE TWO HALVES CANNOT SHIP SEPARATELY, and this was measured.** With the host half applied and the
guest reverted to the pre-fix vminit, the host leaves each volume in the OCI spec while the old guest
has already claimed the same device read-only, so `vmexec` fails the second mount with **errno 16
(EBUSY)** and `Start` fails outright: `8 passed; 6 failed`, including `ports::`, `limits::` and
`mounts::the_project_root_...`, which have nothing to do with volumes. **Host-only is worse than
neither** — it converts a silent misidentification into a failure to start. A bisect landing between
them meets a broken engine.

**The instrument now is the positive test.**
`mounts::the_managed_volumes_are_mounted_at_their_declared_targets_and_writable` asserts the mount, the
block device's size behind it, and a write and readback, with `/home/workspace` as a control that must
NOT be a mount point. The capacities stay unequal (256 MiB / 512 MiB / 1 GiB) so a volume mounted at
another's target is caught by size. The old negative test is deleted.

**ITEM 1 BELOW WAS RIGHT, AND A REVIEWER TURNED IT INTO A CRITICAL. IT IS NOW FIXED** — see the review
section at the end of this file. The stale-layer-cache hazard was real, the documented mitigation named
**the wrong directory**, and the live tier **structurally could not see it**. The list is kept as
written because predicting it and then not chasing it is the lesson.

**FOUR THINGS WERE NOT VERIFIED and are the first place to look if this misbehaves:**

1. **A stale layer cache.** Every run used a fresh temp layout, so freshly-labelled layers. Layer images
   under `~/.arca/layers/{digest}/layer.ext4` written *before* this change carry no label and the new
   guest ignores them. Both commit messages say to clear that directory; **nobody has exercised what
   happens if you do not.** This is the most likely thing to bite in real use.
   — **CLOSED. The cache is now validated on every hit rather than trusted, so an unlabelled entry
   costs one unpack instead of a silently wrong rootfs.**
2. **The two new `exit(1)` paths in `ArcaBoot`** — an unreadable superblock, and "more than one
   `arca.writable` device" — are on the container boot path and were never driven. **A third has been
   added since, and is equally undriven: a writable device present with no layers.**
3. **Two of the three mutations the implementer claimed** (widening the OCI filter back; swapping
   capacities at the `createContainer` handoff) were not independently reproduced. The load-bearing one
   was.
4. **Label collision and prefix behaviour** were reviewed by reading only.

### CLOSED 2026-08-14 — the engine exits cleanly, and it was never about containers

**FIXED, verified and committed.** Arca `Sources/arca-engine/ArcaEngineCommand.swift` and
`Sources/ArcaEngine/EngineServer.swift`; Gas Can `crates/gascan-arca/tests/live/shutdown.rs` and
`live/common/mod.rs`. **Nothing here is open work.** The section is kept because the diagnosis
overturned two things this file asserted, and because the instrument it produced is now permanent.

**What it was.** `serve()` awaited `engine.onClose` — which is the **LISTENING** channel's
`closeFuture`. `ServerQuiescingHelper` closes that listener *synchronously* when a shutdown begins,
before the connections it has just asked to quiesce have gone. So `run()` shut the event-loop group
down under still-registered channels; each channel's `closeFuture` callback then tried to schedule
`ChannelCollector.channelRemoved` on a loop that no longer existed (`Cannot schedule tasks on an
EventLoop that has already shut down`), the collector therefore never reached `shutdownCompleted()`,
and it deallocated still holding the promise it mints at `QuiescingHelper.swift:141` — `Fatal error:
leaking promise`, `Trace/BPT trap: 5`, exit 133.

**The fix is one line of behaviour: wait for the ACCEPTED connections, not for the listening socket.**
`initiateGracefulShutdown` is handed a promise instead of `nil`, and `serve()` awaits that.

**THE CLAIM THIS FILE CARRIED FOR TWO DAYS — "it only happens once containers have been created" — IS
FALSE, and every hypothesis built on it was wrong.** An engine that never created a container still
crashed **1 time in 96**. The container was a correlate: it widens the window, it does not change the
bug. In particular the item this file called **"the single most promising lead"** —
`TCPProxyHandler`'s `ClientBootstrap(group: context.eventLoop)` at `TCPProxy.swift:156` — is **not on
the path at all**: `TCPProxy` runs on its own group, and the crash reproduces with no container, no
proxy and no VM. The "seven event-loop groups" map was a search of the wrong space.

**The measurements, all from `shutdown.rs`, 32 engines per figure**, two binaries built from the same
file and run **interleaved** rather than A-then-B:

| workload | before | after |
|---|---|---|
| nothing holding a connection | 1 / 96 | **0 / 96** |
| a client channel still open | 5 / 96 | **0 / 96** |
| a container created and removed | 12 / 32 (**38%**) | **0 / 32** |

**The mutation that says the fix is the ordering and not the promise.** A third binary that passes the
promise in and *still* awaits `onClose` — promise made, never waited on — runs at **22/32 (69%)**,
worse than the original, and the leaked promise simply changes address: it stops being the
collector's at `QuiescingHelper.swift:141` and becomes `ArcaEngineCommand.swift`'s own. The `Cannot
schedule tasks` line is unchanged throughout. **Passing a promise moves the bookkeeping; awaiting it
moves the bug.**

**THE FASTEST REPRODUCTION NEEDS NO VM, NO CONTAINER AND NO CARGO** — connect a raw socket to the
engine, send nothing, and `SIGTERM` once. Against the pre-fix binary the process ended on that first
signal **5 times out of 5**, crashing in 4 of them. Roughly seven seconds per sample.

**A SECOND DEFECT THE FIX EXPOSED, AND IT IS THE INTERESTING ONE.** The handler's doc comment said the
escalation existed because "a graceful shutdown waits for in-flight RPCs". **It did not.** The wait was
on the listening socket, so one signal always ended the process whatever a client was doing, and the
second signal had nothing to force — a comment describing a behaviour the code had never had. Making
the wait real made that comment true and made two more things necessary:

- **The drain is bounded to 10s**, because quiescing cannot close everything it asks to close.
  grpc-swift turns `ChannelShouldQuiesceEvent` into a GOAWAY only for a connection whose protocol it
  has finished negotiating; one that has been **accepted and has sent nothing** is in no protocol, so
  nothing closes it and the drain waits on it forever. That is not theoretical — with the drain
  unbounded, `shutdown::the_engine_exits_cleanly_with_a_client_channel_still_open` **hung past its 30s
  bound** once in roughly 200 engines, a tonic channel whose HTTP/2 preface had not been exchanged
  when the signal landed. MEASURED with the bound: exits at **10.0-10.1s**, three for three.
- **The escalation now forces the exit** — it releases the socket path and ends the process, because
  returning would either wait on the very thing being escalated past or shut the group down under live
  channels. MEASURED: second signal exits 0 and the socket is gone, five for five.

**10 SECONDS IS A POLICY AND NOTHING MEASURES IT.** It is chosen against the two clocks that already
bound the process from outside — the live tier's 30s and launchd's 20s `ExitTimeOut` — so that the
engine is what decides. An ordinary drain completes in milliseconds.

**The instrument is permanent, and the refusal is inverted.** `LiveEngine::stop()` returns the exit
status; **`LiveEngine::kill()` now asserts it**, so a regression fails whichever of the tier's tests
meets it first rather than waiting for the one module built to look for it. `shutdown.rs` says how
*often*, `kill()` says *whether*. Its three workloads vary one thing at a time on purpose: a rate that
differs between them is what says which variable is load-bearing, and it is what disproved the
container.

**NOTHING IN ARCA CAN TEST ANY OF THIS, and a test was attempted rather than assumed away.** The claim
is about two futures of a running server with an accepted connection outliving its listener, and no
unit test can hold one there: a connection grpc-swift has finished configuring is closed by the same
GOAWAY quiescing sends, and a raw socket it has not finished configuring cannot be **observed** to
have been accepted — so either fixture decides the assertion by a race. `serve()` is private and needs
a real process besides. `ArcaEngineTests` is 151 passing before and after, which is exactly the point.

**WHAT WAS NOT VERIFIED, and is the first place to look if this misbehaves:**

1. ~~**The 10s bound has never been hit by anything but the raw-socket fixture.** No real client has
   been observed to need it.~~ **FALSIFIED 2026-08-16, and this is the first observation of it.**
   A live `shutdown::the_engine_exits_cleanly_with_a_client_channel_still_open` run against Task 5's
   `2248035` came back **2 of 96 not clean: 94 × exit 0, 1 × exit 1, 1 × exit 143.** Exactly one
   engine logged `connections did not drain within the grace period; closing anyway`, `grace=10 s`.
   **That is the grace path firing against a real tonic client, which milestone 2 recorded as never
   having happened** — and it exits 1 precisely because milestone 2's re-review made it do so, to
   distinguish a timed-out drain from a completed one. The instrument worked.
   **Attribution is OPEN and must not be assumed.** The workload creates no container, so Task 5's
   log-writer changes have no obvious path into it; the machine had been under sustained load for
   hours; and 1 in 96 is one sample. **Do not conclude it is environmental without measuring** — the
   controlled shape is two binaries interleaved, per this file's standing rule.
   The `1 × exit 143` in the same run is the **known** pre-existing startup race recorded above, not
   a second new thing.
2. **The escalation path's `releaseSocketPath()` error branch** — a socket that cannot be unlinked —
   was never driven.
3. **`SWIFTNIO_STRICT=1` was not re-run.** The earlier `serve()` split was verified under it; this
   change was not.
4. **Nothing measured a client with an actually in-flight RPC** across a shutdown. Every held
   connection here was idle.

### THE REVIEW OF TASKS 13-17, AND WHAT IT CHANGED — 2026-08-14 (late)

**Tasks 13 through 17 shipped with no independent review** — the SDD ledger holds review artefacts
through task 12 and stops, 13a/13b were controller adjudications, and 17 had none at all. Two Opus
reviewers were dispatched, partitioned by repository so they could not collide on each other's build
locks, with the live tier reserved for the controller because it needs both repos at once. **Neither
came back clean.** Arca: 1 Critical, 2 Important, 1 Minor. Gas Can: 4 Important, 6 Minor.

**Every finding below is fixed.** Reports are in the session scratchpad; the durable content is here.

**THE CRITICAL WAS THE ONE THIS FILE PREDICTED AND DID NOT CHASE.** The list above says a stale layer
cache "is the most likely thing to bite in real use", and it was:

- `OverlayFSUnpacker` writes the role label only where it FORMATS a layer, and it formats only on a
  cache MISS. So the cache-HIT branch returned pre-label images unexamined, the guest's classifier
  dropped them with `is not an Arca role, leaving it alone`, and the rootfs was built from a subset of
  its image — or from none of it, with `Start` still succeeding. **A stale image is a perfectly valid
  ext4 filesystem, which is why nothing refused it: the only thing wrong with it is an absence.**
- **The documented mitigation named the wrong directory.** "Clear `~/.arca/layers`" is ArcaDaemon's
  cache; the engine's is `<state-root>/layers` (`EnginePaths.layerCache`). An operator following it
  would have cleared a directory the engine never reads.
- **The live tier structurally could not catch it.** Every live engine gets a fresh temp state root, so
  every layer is always a cache miss. `1d453cf`'s green 14/14 was fully consistent with the defect.

Fixed by validating the label on every cache hit and reformatting what fails — a 16-byte superblock
read, not a scan. `ArcaBlockDeviceRole.role(ofImageAt:)` is the predicate, and
`LayerCacheRoleTests` (4 tests) drives it against real formatted images; **mutating it to trust the
cache fails three of the four; the fourth is the control, which passes by construction and must** -- the summary line's "4 failures" counts assertions, not tests. The guest also stops degrading silently: a writable device with no layers is now
`exit(1)` rather than a container booted on the wrong rootfs. **Nothing drives that path** and it says
so in place. **Checked on this machine: no layer cache exists at all, so nothing was mis-mounting here.**

**BOTH REVIEWERS INDEPENDENTLY FOUND THE SAME DEFECT IN TASK 17, FROM OPPOSITE SIDES.** The engine
exited `EXIT_SUCCESS` both when a drain completed and when it gave up at the ten-second grace, so
`shutdown.rs` — which counts `!status.success()` — could not tell 96 completed drains from 96 that
timed out, **in the instrument that produced the fix's own numbers.** The grace path now exits **1**
and the operator's escalation still exits **0**; MEASURED, three for three and two for two. That is one
byte and one guard, deliberately not a second timing assertion beside it: a gate two places enforce is a
gate no test measures.

**THE HANG THIS FILE RECORDED AS AN ACCEPTED LIMIT WAS RATED IMPORTANT, AND THE REVIEWER WAS RIGHT.**
Nothing but the first signal completed `quiesced`, so a listening socket closing for any other reason
left the process waiting forever **while holding the flock** — which is exactly what makes
`EngineServer.start` refuse the path to a successor. An engine that can neither serve nor be replaced is
worse than the exit it replaced. It now logs and exits non-zero. Reachability is still unmeasured.

**A CLAIM OF MINE WAS FALSIFIED BY A REVIEWER WRITING THE TEST I SAID COULD NOT BE WRITTEN.** `serve()`
carried, in bold, "NOTHING IN THIS REPOSITORY CAN PROVE ANY OF IT". The reviewer wrote a probe against
`EngineServer.start` and `SandboxEngineService.forTesting()` and ran it **20 times, 20 passes**. The
accept race I cited is real but it is *setup, not assertion*, and it fails safe. The comment now states
the narrower truth: **the premise is provable here and is not yet pinned; the call site is not, and
privacy was never what stopped it** — `EngineProcess.swift` already spawns this binary — **vmnet is.**

**CARRIED, NOT DONE:** adopting that probe permanently, and moving the shutdown wait out of the
executable into `ArcaEngine` (`runUntilQuiesced`) so task 17 gets the fails-before/passes-after test it
still lacks. Both were scoped out deliberately. **Reverting `9fac267` still leaves `swift test` at 155
passing**, and Gas Can's live tier remains the only thing that catches it.

**THE SAMPLE SIZE WAS JUSTIFIED WITH A RATE TWO OF THE THREE WORKLOADS DO NOT HAVE.** `ITERATIONS = 32`
was argued from the original mixed 19%; the per-workload pre-fix rates are 1/96, 5/96 and 12/32, and at
1/96 a sweep of 32 comes up clean **71% of the time against a broken engine** — worse than the sweep of
5 the same comment rejected. Each workload now gets the count that puts a false green under 1% against
its own rate: **440 / 96 / 32**. Re-measured after the fix: **0/440, 0/96, 0/32**, 568 engines, and
because the grace path now exits non-zero those zeros mean the drains *completed*.

**Two smaller corrections worth keeping.** `shutdown.rs` claimed its three workloads were a controlled
comparison rather than A-then-B; they are three tests in one binary that run strictly in sequence, so
they are the regression guard and the interleaved comparison lives in the commit messages. And
`41ac39a`'s message says the tier is "12 carrying `#[ignore]` and 2 running in ordinary CI" — it is
**11 and 3** (`connect.rs` has two non-ignored tests, plus `supervision`). Commit messages are
immutable; the correction lives here.

**Verified after every fix, both trees clean:**

| | |
|---|---|
| `swift test --filter ArcaEngineTests` | `Executed 160 tests, with 0 failures` — 151, plus 6 `LayerCacheRoleTests` and 3 `ShutdownObserverTests` |
| `swift test --filter ArcaTests.NetworkPruneGateTests` | `Executed 3 tests, with 0 failures` |
| the live tier, `-- --ignored --test-threads=1` | **14 passed / 0 failed**, 214s — **579 engines stopped**, 568 of them in `shutdown.rs` and one in each of the other 11 |
| `cargo test --workspace --no-fail-fast` | exit 0 — **1436 / 0 / 36 across 74 targets** |
| `cargo fmt --all --check`, `clippy --workspace --all-targets`, the ignored gate | clean |

**The guest was rebuilt** (`make vminit-rebuild`, **42 seconds**, confirming the 41s on record and not
the "20-25 minutes" this file once predicted), so the live tier's 14/14 measures the new `ArcaBoot`.

### THE RE-REVIEW OF THE FIX ROUND — 2026-08-14 (late)

**Both fix rounds were re-reviewed before merging, because this project's own record says a fix round
is where the next defect lives**: Task 11 ran four rounds and rounds 2 and 3 each found NEW defects in
the previous round's fixes. That held again. **Every round-1 finding is closed**, two of them pinned by
the compiler rather than by prose, and the re-review produced **1 Important and 10 Minor** of its own,
all now fixed.

**THE IMPORTANT IS THE ONE THIS FILE KEEPS WRITING TRAPS ABOUT, AND I SHIPPED IT ANYWAY.** The
stale-cache fix was tested by `LayerCacheRoleTests`, which pinned the **predicate** and not the
**decision**. A reviewer bypassed the check at its call site — `if true || cachedRole == .overlayLayer`,
the pre-fix behaviour exactly — and `swift test` stayed at **155 passing**. The live tier could not see
it either, and for the same structural reason the original defect could not be seen: every live engine
gets a fresh temp state root, so the cache is always empty and the branch is never entered. **So every
mutation of the call site left everything green, and the commit disclosed the equivalent gap for task
17 twice in bold while saying nothing about this one. The asymmetry was the finding.**
`OverlayFSUnpacker.cachedLayerIsReusable` and `discardCachedLayer` now carry the decision, two tests
drive them over a real `{cache}/{digest}/layer.ext4` layout, and the same bypass mutation now fails.
**What is still unmeasured — that `unpackLayerToCache` calls them — is written where it lives.**

**A GUARD I ADDED COULD HAVE REFUSED A LEGITIMATE BOOT, and my justification for it was false.** The
guest's new `exit(1)` was defended as "a writable overlay with no layers is not a shape the host ever
builds deliberately". Nothing refuses a zero-layer manifest, so a `FROM scratch` image or an OCI
artifact produces exactly that shape on purpose. Such a container now dies where before it booted on
the bare initfs — **both are wrong**, since its rootfs should be empty and vminitd's is not, and the
refusal is the better of the two because it is loud. The real fix is for the host to tell the guest how
many layers it attached, so the two cases can be distinguished at all. **Carried, not done.**

**The shutdown observer is now measured.** `ShutdownObserverTests` drives the graceful path with a peer
holding the drain open (the guard must NOT fire), a control with nothing recorded (it must), and an
inverted-order case asserting that recording after the close loses the race — so the ordering the guard
rests on is load-bearing rather than incidental. `ShutdownRequests` moved from `private` in the
executable into `ArcaEngine` for it: the reviewer's probe had to re-declare the type, and **a test that
drives a re-declaration proves the re-declaration**, which is a shape this project has shipped before.

**Four smaller ones, each the same class.** "Mutating the predicate fails all four" was three of four —
the fourth is the control and passes by construction, and the summary line counts assertions rather
than tests. A comment still said the mutation leaves `swift test` "at 151 passing" in the commit that
took it to 155. Two more comments still named `~/.arca/layers` as the layer cache after the commit
message said they had all been corrected. And the widened mount filter now logs what it drops, because
"no runtime could honour it anyway" and "it vanishes without a word" are not the same outcome.

**A latent race the fix introduced, fixed with it:** two concurrent creates over one stale layer digest
could both reach the discard, and the loser's `removeItem` would throw `ENOENT` out of the RPC. Already
gone is now treated as the outcome it wanted; **everything else is rethrown**, because an entry that
survives is handed to the guest unlabelled.

**D7 FIRED TWICE MORE, AND SO DID THE KEYGEN FAULT — neither is this branch.** Two consecutive workspace
runs went red on two DIFFERENT documented flakes: `mode is 0200 ... written but never published` in
`gascan-e2e` (fourth recorded occurrence, a new test —
`environment_teardown_terminates_its_exact_live_daemon`), then
`KeygenMessage("/dev/fd/22: Bad file descriptor")` in `gascand`. Both crates are untouched by this
branch — `git diff 6847d1e..HEAD -- crates/gascan-e2e/ crates/gascan/ crates/gascand/` is **empty** —
and both targets pass alone (28/28 and 25/25). The third run is exit 0 with zero occurrences of either
signature. Load averages were 4.4-5.9 throughout, which is the condition this file records these
failures scaling with. **Three of the three known root causes have now been seen on this machine.**

### THE MERGE IS DONE — 2026-08-15, and the order it needed is worth keeping

| | branch | merged as |
|---|---|---|
| submodule `containerization` | `merge/upstream-main` @ `3f68806` | pushed, not a PR — it is a fork branch |
| Arca | `feat/engine-state-ownership`, 46 commits | `b3ffdf5`, PR #57 |
| Gas Can | `docs/p5-1-milestone-2-design`, 48 commits | `e968ae1`, PR #71 |

**THE SUBMODULE HAD TO GO FIRST, and it will again.** Arca's tree records a `containerization`
pointer; merged before that commit is reachable on
`git@github.com:Vas-Solutus/arca-containerization.git`, Arca's `main` names a submodule revision
nobody can fetch and every clone breaks at `git submodule update --init --recursive`. Verified after
the fact: `git ls-tree origin/main containerization` is `3f68806`, and
`git branch -r --contains 3f68806` in the submodule lists `origin/merge/upstream-main`.

**Merge commits, never squash.** `allowed_merge_methods` is `["merge"]`, and both landed with two
parents. A squash would have destroyed the per-task history dozens of sections here cite by SHA.

**Gas Can's PR read `mergeable=MERGEABLE, mergeStateStatus=UNSTABLE` and merged anyway**, exactly as
PR #69 did: `ci / gate` is not a required check (ruleset `20492137` carries zero
`required_status_checks`), and **CI's `engine` job is red by design** —
`./scripts/build-arca-engine.sh` exits 70 because the gate now requires
`ArcaTests.NetworkPruneGateTests`, which the pinned `gascan-engine-m1.1` / `b3390b8` does not carry.
**Do not bump the pin to make it green.** The bump belongs to milestone 4 and needs a signed tag
carrying Arca `fede19c`; a pin moved to an untagged or mid-branch revision buys a green check by
giving up the trust model `engine/allowed-signers` exists to enforce. Arca still has no CI at all.

## The superseded plan for Landing 5, kept for its reasoning

**Landing 5 is already expanded. Do not re-expand it.** Gas Can `4e40438`, amended by `f8c5ca1` (the
image-load seam) and `1726c77` (Task 12's routed findings). Read that section of the plan and follow it.

Remaining: **Task 13** the live tier, **Task 14** the capability flips, **Task 15** the workspace suite
run alone.

### WHEN THE ENGINE DIES WITH vmnet 1001, FORCE-QUIT `InternetSharing`. RESOLVED 2026-08-14.

**`Error: failed to create vmnet network with status vmnet_return_t(rawValue: 1001)`**, engine exits
before binding a socket, every live test blocked. It cost about an hour on 2026-08-13/14 and it will
recur, because the tier starts an engine per test.

**THE FIX: force-quit the `InternetSharing` process** (Activity Monitor, or by PID — **never
`pkill -f`**). Immediately afterwards `container-network-vmnet` processes appear and the engine starts
in 2s. **The maintainer suggested this twice before it was tried; both times the agent's evidence said
there was nothing to restart, and both times the evidence was wrong.**

**`sudo launchctl kickstart -k system/com.apple.NetworkSharing` DOES NOT WORK** — `150: Operation not
permitted while System Integrity Protection is engaged`. The launchd service is not the lever; the
`InternetSharing` process is.

**Confirmed from both directions by an interleaved comparison**, the same four-run matrix before and
after, alternating the pre-13a binary `9db2f7d` with the current one:

| | before | after |
|---|---|---|
| pre-13a, run 1 | `vmnet 1001` | **LISTENING**, `192.168.69.0/24` |
| current, run 1 | `vmnet 1001` | **LISTENING**, `192.168.69.0/24` |
| pre-13a, run 2 | `vmnet 1001` | **LISTENING**, `192.168.69.0/24` |
| current, run 2 | `vmnet 1001` | **LISTENING**, `192.168.69.0/24` |

`Create` then succeeded again, with the container directory under the engine's own store and Apple's
shared store unchanged across the run.

**FOUR HYPOTHESES THAT WERE WRONG, recorded because each cost time and each looked reasonable:**

1. **"Subnet exhaustion across the session."** Wrong. All four post-fix runs took **the same**
   `192.168.69.0/24`, so allocations are released on exit. The pool was never the problem.
2. **"Memory pressure."** Wrong, and it is the more embarrassing one — see the instrument note below.
3. **"Nothing is running, so there is nothing to restart."** Wrong, because the instrument was broken.
4. **"Tasks 13a/13b broke it."** Correctly ruled out, and this is the one piece of reasoning that
   held: the interleaved comparison exonerated them before any time was spent bisecting.

**A leaked engine was found on the way and is a real bug, though NOT the cause.** `arca-engine` PID
10260, started **Mon Aug 10 22:21:27**, orphaned to PID 1 — a live-tier engine that outlived the run
that spawned it by four days. Killing it changed nothing. **`kill_on_drop(true)` does not save you when
the parent is killed rather than dropped, and nothing in the tier reaps a survivor.** Worth fixing, and
worth checking for before blaming the host.

### TWO INSTRUMENTS IN THIS FILE WERE WRONG AND BOTH PRODUCED CONFIDENT FALSE CONCLUSIONS

**`ps aux` IS FILTERED HERE TOO — REDIRECT TO A FILE.** This file used to say `ps -A` is filtered but
`top` and `ps aux` are not. **Measured: `ps aux | wc -l` returned `31` on a machine with ~830
processes**, and the same pipeline later returned `832`, so it is intermittent rather than absent. That
truncation is what hid the four-day-old leaked engine and produced a confident "nothing is running to
restart". **`ps aux > file` then read the file was reliable every time.** Check the instrument: compare
the line count against `top -l 1 | grep '^Processes'` before believing a negative result.

**`vm.swapusage` AND `top`'s "unused" DO NOT MEASURE MEMORY PRESSURE. USE `memory_pressure`.** This file
told you to check `sysctl vm.swapusage` and `top` before believing a crash. Following it produced
"320 MB unused, 5.5 GB swap, 6.3 GB compressor — the machine is starved", which was **false**:
`memory_pressure` reported **`System-wide memory free percentage: 56%`** at that exact moment. macOS
keeps "unused" near zero by design, and swap and the compressor are a **high-water mark** that is never
proactively reclaimed — they describe last night, not now. The largest process on the box was 625 MB.
**Swap tells you what happened; `memory_pressure` tells you what is happening.**

**IT IS 1001, NOT 1002 — DO NOT DIAGNOSE IT AS THE SIGNING TRAP.** This file records 1002 (the SDK's
`VMNET_MEM_FAILURE`) as the *unsigned binary* failure. This is a different value, and the binary is
signed: `codesign -d --entitlements -` reports `com.apple.security.virtualization = true`. It is also
not the test harness — it reproduces running the engine straight from a shell with the same four
options, dying at `Initializing Containerization.ContainerManager`, where `initialize()` constructs
`VmnetNetwork()`.

**SETTLED BY A CONTROLLED COMPARISON, NOT BY ARGUMENT.** A-then-B on a drifting machine proves nothing,
so the pre-13a engine was built at `9db2f7d` in a worktree and both binaries were run **interleaved in
one window**:

```
round1 pre-13a (9db2f7d): VMNET FAIL -> vmnet_return_t(rawValue: 1001)
round1 current (07f62b9): VMNET FAIL -> vmnet_return_t(rawValue: 1001)
round2 pre-13a (9db2f7d): VMNET FAIL -> vmnet_return_t(rawValue: 1001)
round2 current (07f62b9): VMNET FAIL -> vmnet_return_t(rawValue: 1001)
```

`9db2f7d` started engines successfully all evening, including the run that created the first container.
**So the host is the cause and Tasks 13a and 13b are exonerated. Do not bisect this further.**

Ruled out, each measured: no `arca-engine` processes (`ps aux`); no leaked interfaces (`ifconfig -l`
shows only `bridge0`); Apple's container daemon not running (`container system status` →
`apiserver is not running and not registered with launchd`); not transient (four consecutive attempts,
each failing in ~0.15s); no `/var/db/vmnet*` or `dhcpd_leases`; 203 GB free.

**Working hypothesis: host vmnet subnet exhaustion across the session.** Every engine start creates a
`host` network with an **auto-allocated** subnet — `192.168.93.0/24`, `.95` and `.119` were observed —
and roughly fifteen engines were started. The pool is per-boot.

The worktree used for that comparison has been removed; `git worktree list` in `~/code/arca` shows only
the main tree. **If you ever need to build Arca at another revision in a worktree, it needs
`git submodule update --init --recursive` first** — `containerization` is a submodule and the build
fails with `containerization/Package.swift doesn't exist` without it. **Its HEAD is now `ca47c87`, not
the `f02cdf9` this file recorded until 2026-08-14**: `f02cdf9` predates the named-volume fix, so a
guest built from it silently measures the old behaviour. Re-derive the pointer with
`git -C ~/code/arca submodule status` rather than trusting any SHA written here.

**What is NOT blocked:** anything Arca-side and VM-free, and any Gas Can work that does not spawn a
serving engine — **including the OCI-layout writer the published-port test needs**, which is pure file
construction and can be exercised against `arca-engine image load`, a subcommand that binds no socket,
starts no VM and needs no vmnet.

### Task 13's first hour is DONE. Both steps came back, and step 2 cost two Arca fixes.

**Do not redo this.** Steps 1 and 2 below are complete and recorded; step 3 is what remains.

1. **DONE — the spawn works**, Gas Can `776a71c`. `connect::a_real_engine_accepts_the_placeholder_authority`
   → `1 passed` against a branch-built engine. Seen to fail: dropping the two new arguments gives
   `Missing expected argument '--kernel-path'`, `engine exited with exit status: 64`, reported in 0.06s
   by `await_socket`'s `try_wait()`.
   **The module prefix in the filter is load-bearing** — `-- --ignored <bare name> --exact` reports
   `running 0 tests` and exits 0. Use `connect::<name>`.
2. **DONE — `image load` works, and `Create` now works, but only after Tasks 13a and 13b.**
   `arca-engine image load --state-root <R> --oci-layout <L>` → exit 0, no kernel, no socket. The layout
   was built with the installed `skopeo`:
   `skopeo copy --override-os linux --override-arch arm64 docker://docker.io/library/alpine:3.20
   oci:/tmp/alpine-oci:alpine:3.20`. Then `PrepareImage` → `Ok`, `Create` → `Ok`.
   **THE DIGEST A REQUEST MUST NAME IS THE STORE'S, NOT THE LAYOUT'S.** The store re-wraps what it
   loads: the layout's `index.json` carries manifest `sha256:45e09956…` and
   `<state-root>/images/state.json` records `alpine:3.20` → `sha256:a019d0ba…` as an image **index**. A
   test deriving the digest from the layout it loaded names content the store does not hold. Read
   `state.json`. `policy_request_for_image` (`common/mod.rs`) is the seam, over
   `PolicyCompiler::compile_for_image`.
3. **What remains**: the lifecycle tests, the partial-failure case, and the published-port test.

**Two things step 2 turned up that change Task 13's remaining shape:**

- **`read_rpcs::every_unimplemented_method_answers_unsupported_capability_not_a_transport_fault`
  ALREADY FAILS** against the branch engine — `Inspect: None` at `read_rpcs.rs:80`, because it calls
  `expect_err` and the branch engine now implements `Inspect` and answers `absent`. The test asserts its
  own count is 10 precisely so this would fire, and says so in its own doc comment. **The count is now
  3**: `CreateContainer`, `Exec`, `Logs`. Rewriting it is Task 13's.
- **`CreateRequest` CARRIES NO ARGV, so the published-port test cannot supply its own responder.**
  `engine.proto:254-271` has no command or entrypoint field, and
  `SandboxEngineService.swift:356-357` passes `entrypoint: nil, command: nil` **deliberately** — the
  image's own config decides what runs. The environment is no way in either:
  `policy.rs:246` sets it from `guest_environment()`, a fixed map, so a manifest cannot inject a port.
  **The plan's Landing 5 expansion assumed `gascan-apple`'s `guest_argv` technique transfers with only
  the port changed. It does not.** Maintainer's ruling 2026-08-13: **the tier builds its own OCI layout**
  — reserve a port by binding `127.0.0.1:0`, then write a layout whose config carries
  `Cmd = sh -c '…nc -l -p <reserved>'`. That keeps the ephemeral-port reservation and needs no network
  at test time.

**The two artifacts the spawn needs are host state that neither repo produces:**

| Option | Path on this machine | What |
|---|---|---|
| `--kernel-path` | `~/.arca/vmlinux` | symlink into an installed `Arca.app`, 28,248,576 bytes |
| `--vminit-layout` | `~/.arca/vminit` | an OCI layout, 178 MB |

Pass them the way `GASCAN_ARCA_ENGINE_BIN` is passed — environment variable, absent means `panic!` with
a directive message. **No hardcoded `~/.arca`, no guessing fallback.**

### Three things that will decide Landing 5, established by measurement

1. **~~THE LIVE TIER CANNOT SPAWN THE *BRANCH* ENGINE~~ — FIXED 2026-08-13, Gas Can `776a71c`.** The
   diagnosis below is kept because its *consequence* still stands: `build-arca-engine.sh` builds the
   pin, the pin bump belongs to milestone 4, and **CI still cannot run Task 13's tests this milestone.**
   `#[ignore]` remains the correct state, and "run the tier at least once" still means a local run
   against a branch build, recorded with its command and output.
   **THE LIVE TIER CANNOT SPAWN THE *BRANCH* ENGINE. IT SPAWNS THE PINNED ONE FINE.**
   **CORRECTED 2026-08-13** — earlier text here said "cannot spawn an engine, and has not since Task
   4", unqualified, and that is too strong in a way that changes Task 13's shape.
   `crates/gascan-arca/tests/live/common/mod.rs:79-86` passes only `--socket-path` and
   `--state-root`. At the **pinned** revision `b3390b8` the engine declares exactly those two plus
   `--log-level` — three `@Option`s, the last with a default
   (`git show b3390b8:Sources/arca-engine/ArcaEngineCommand.swift`). **So the tier spawns the pinned
   engine correctly, and the recorded 4/4 live pass measured a working tier.** On the branch,
   `ArcaEngineCommand.swift:68-84` makes `--kernel-path` and `--vminit-layout` required; measured
   against both the pre- and post-Task-9 branch binaries: `Missing expected argument
   '--kernel-path'`, exit 64.
   **Nobody noticed because every live test is `#[ignore]`d, so nothing runs them** — a tier that
   cannot start its subject and a tier nobody runs look identical from outside.
   **The consequence for Task 13:** `build-arca-engine.sh` builds the pin, the pin bump belongs to
   milestone 4, and the pinned engine has no lifecycle RPCs — so **CI cannot run Task 13's tests this
   milestone at all**, and `#[ignore]` is the correct state rather than a quarantine. "Run the tier at
   least once" means a **local run against a branch build, recorded with its command and output**.
   The spawn also needs two artifacts nothing in the repo produces: `~/.arca/vmlinux` (a symlink to an
   installed `Arca.app`, 28,248,576 bytes) and `~/.arca/vminit` (a 178 MB OCI layout). Landing 5's
   expansion carries the rest.
2. **PORT PUBLISHING HAS THREE SILENT GATES, and Task 11 closes only the first.**
   (a) `portMapManager == nil` — now wired; (b) `getWireGuardClient` returns nil and the `if let`
   around the publish has **no `else`** — it returns nil when the container is on no WireGuard
   network; (c) the `catch` swallows by design ("Don't fail container start on port mapping errors")
   and the container is still marked running. Gate 2 is passable: `createDefaultNetworks()` makes a
   WireGuard-backed `bridge` (`isDefault`) and a vmnet `host`, and auto-attach fires for
   `networkMode` empty/`default`/`bridge`, skipping `none`/`host`. **So an offline sandbox with ports
   publishes nothing** — Task 11 refuses that combination rather than accepting it.
   **Publication is provable ONLY from the live tier**: `publishPorts` takes a non-optional
   `WireGuardClient` built against a booted VM, and the one VM-free path that reaches the gate is a
   no-op that would pass with the setter unwired. Task 11 deliberately did not write that test.
   **Task 13's shape, already worked out:** create a sandbox with a `PortMapping`, `Start`, then
   connect to `127.0.0.1:<host_port>` from the test process. Nothing weaker distinguishes a
   published port from a stored binding.
3. **TASK 14 MAY NOT FLIP `loopback_publish` UNTIL THAT TEST EXISTS AND PASSES.** A flag whose
   machinery is unproved is a claim with no instrument, and here the machinery has three ways to
   silently do nothing.

### Two contract defects for milestone 4's design pass

**1. The contract permits a combination no engine can honour.** `engine.proto`'s `Network` is a `oneof`
of `offline`/`networked_name`, and `ports` is a separate `repeated` field on `CreateRequest`, so
offline-plus-ports is expressible and nothing says which wins. Task 11 refuses it with
`unsupported_capability`. **The proto and the design should say what happens rather than leaving each
engine to decide** — this is feedback for milestone 4, and it dies in a Swift comment otherwise.

Sharpened 2026-08-13: **three components already agree** — the proto permits it, Gas Can's
`compile_ports` (`crates/gascan-core/src/policy.rs:436-438`) refuses it as `OfflinePortsForbidden`
before a request is built, and Arca's engine refuses it. Only the proto is silent. That is an argument
for writing the rule down, not for leaving it.

**2. `Remove` cannot report a partial deletion, because `AckResponse` has nowhere to put one.** Found by
Task 12's review. `CreateFailed` carries `repeated Resource created = 1` precisely so a partial create
does not leak with nothing knowing to look for it. **`AckResponse` is a bare `oneof { Ack ok; EngineError
error }`** — so a `Remove` that deletes the container and then fails on the volume is indistinguishable
on the wire from one that did nothing, and the consumer's recorded state diverges from reality with no
signal. `RuntimeBackend::remove` returns `Result<(), RuntimeError>`, so Gas Can could not carry it
either.

**Severity is bounded, which is why it is not a blocker:** Task 12's `Remove` validates *every* resource
before deleting *any*, so authorisation failures delete nothing; only a mid-deletion manager failure
produces a partial; nothing retries `Remove` (all six `gascand` call sites are single-shot); and
`ListResources` plus reconcile can rediscover the truth. **Do not fix it in the engine — it is a contract
change.**

**Four things Task 6 found that change the remaining work.** They are in the plan; they are
repeated here because missing one is expensive:

1. **Landing 3 seeds through `loadPersistedState()`, never a stub.** It is `package func`,
   VM-free, needs no entitlement, and is the only writer of `ContainerManager.containers`
   reachable without a kernel. `Tests/ArcaEngineTests/CrashRecoveryTests.swift` is the example.
2. **Task 11 must cross-wire the managers before `Create` is written.** The engine calls
   `setVolumeManager` and `setNetworkManager` as of Task 6, but **`setPortMapManager` is still
   unwired**. `ContainerManager.swift:2482` guards `publishPorts` behind it with no `else`, so an
   unwired engine **starts a container with published ports, publishes nothing, and reports
   success**. `:2730`/`:3058` guard teardown the same way.
3. **Task 14's `named_volumes` and `loopback_publish` cannot be flipped** until that wiring
   exists. A flag whose machinery is unwired is a claim with no instrument. **SATISFIED — both are
   now `true`**, `loopback_publish` at Task 14 and `named_volumes` on 2026-08-14, each with a live
   test that fails without it. The rule it states still binds for any future flag.
4. **Signing precedes the live tier.** Task 6b landed it; do not reorder Task 13 ahead of it.
   Unsigned, `initialize()` dies at `VmnetNetwork()` and the engine never creates a socket.

**Two things the engine must keep NOT doing.** "Mirror `ArcaDaemon`" is the obvious way to close
a wiring gap and it would import both:

- **`applyRestartPolicies()` calls `startContainer` and boots VMs.** In an engine that resurrects
  sandboxes the consumer believes stopped, *before the socket binds* — the consumer never sees the
  transition and reconcile meets containers it did not start.
- **The daemon's deletion of Apple's `initfs.ext4`** is the shared-store behaviour the private
  root exists to avoid.

Deliberately correct as-is: `setHealthChecker` and `setEventEmitter` are silent when unset, and the
proto has no health or events surface. Wiring an `EventManager` would build toward one
`tests/release/engine-targets-check.sh` requires the engine **not** to have.

### WHAT COMES AFTER THE MERGE, in the order the roadmap puts it

**Read `docs/superpowers/plans/2026-08-04-arca-integration-roadmap.md` for the phase map. P0-P4 are
done and P5 is current.** P5 has four steps: **P5.1** the engine service (in progress, four
milestones), **P5.2** the `gascan-arca` crate (done, merged `bd412b4`), **P5.3** the conformance suite
extracted from `fake_runtime.rs` and run against fake/apple/arca, and **P5.4** resolving U5 — how image
digests reach the engine without registry access. P6 is the network model, P7 the cutover, P8 fork
reduction.

**P5.1's own milestones: 1 skeleton (merged), 2 lifecycle (this one), 3, 4.**

- **Milestone 3 — `CreateContainer`, `Exec`, `Logs`, and `ExecManager.signalExec`.** DESIGNED
  2026-08-15: `docs/superpowers/specs/2026-08-15-p5-1-milestone-3-rpc-surface-design.md`. `tty` and
  `signals` are the two capability flags still `false`, correctly, and this is what earns them.
  It also takes carried follow-ups (a) and (b) below. **It is Arca-side Swift plus live tests** —
  Gas Can's half is already built and tested, verified 2026-08-15 (design §2.1).
- **Milestone 4 also owns three defects milestone 3 found and correctly did not fix.**
  **(a) `ContainerManager.parseSignal` (`ContainerManager.swift:2882-2911`) is wrong in both halves.**
  Its name branch maps 13 signals and **silently defaults anything unrecognised to SIGKILL** with only
  a `logger.warning`; its numeric branch (`:2889-2891`) has **no range check**, so
  `docker kill --signal 999` forwards 999 unvalidated. **Reachable today from Arca's Docker surface.**
  **(b) `EXT4.Formatter.unpack` accepts a blob that is not the archive its media type declares and
  produces an empty filesystem rather than refusing** — in production that turns a mis-typed or
  corrupt layer into a valid, correctly labelled, **empty** `layer.ext4`, which is this project's own
  defect signature reached through the miss path. Upstream in the frozen submodule.
  **(c) Single-layer blindness in the layer-cache tests** — they use a one-layer fixture, so a
  multi-layer defect is invisible. Needs a multi-layer fixture.
- **Milestone 4 — and it now owns a MEASURED ENGINE DEFECT, ruled here 2026-08-15.**
  **`arca-engine` dies with exit 143 if SIGTERM lands during startup.** **CORRECTED within the hour
  by Task 2's implementer, and the first version of this entry understated it in exactly the way the
  citation trap below describes.** The window is **not** bind-to-`SIG_IGN`: SIGTERM's disposition is
  default from `exec`, so **the whole of startup is inside it** — argument validation, the vminit
  load, and all three `initialize()` calls including the one that constructs a real `VmnetNetwork`.
  Bind-to-`SIG_IGN` is merely the part a *client* can observe, because no socket exists before bind;
  the forced spike's "immediately after spawn" arm was hitting the large part, not the sliver. The
  window closes at `signal(number, SIG_IGN)` — **re-derive that line rather than trusting a number
  here; it has already moved once, from `:381` to `:434`, under a comment-only commit.** In the
  window the signal takes its default disposition and kills the process; the engine's only deliberate
  exit is `Foundation.exit(status)` with 0 or 1, so **143 = 128+15 is always the kernel and never the
  engine** — and it is not the pre-fix 133. **MEASURED, forced rather than waited for:** spawning
  the engine and signalling immediately gives **12/12** exit-143; signalling after the socket appears
  plus 300ms gives **0/12**, interleaved in one process against one binary. Naturally it fires about
  **2 in 440** and is load-dependent, which is why the live tier's `shutdown::…with_nothing_holding_
  a_connection` arm goes red intermittently — that workload has zero slack between the connect
  succeeding and the signal, while the other two build a transport or boot a VM first.
  **It belongs to milestone 4 because the launchd plist is what makes it production-reachable**, and
  **the fix is a design change, not a one-liner**: the handler closure captures the engine, and a
  bare `SIG_IGN` before a resumed dispatch source would make a startup SIGTERM a silent no-op, which
  is worse than dying. **A SECOND WINDOW IS REASONED AND NOT MEASURED** — between `signal(…,
  SIG_IGN)` (`:381`) and `source.resume()` (`:447`) libdispatch has not yet registered the kevent, so
  a signal there is lost outright and the tier would report `"the engine ignored SIGTERM"`, which
  would read as a shutdown defect rather than a startup one.
- **Milestone 4 — everything that makes it a product.** Daemon wiring and `BackendSelection::Arca`,
  the launchd plist, installer changes, `gascan doctor` surfacing engine facts, the offline proof that
  moves `offline` off `ISOLATION_UNVERIFIED`, the **pin bump** with its signed tag, and the decision on
  how the 27 MB kernel and 163 MB vminit ship (constrained by design §2.6 to the `--kernel-path` /
  `--vminit-layout` seam). **Its design pass also owes the two contract defects** recorded below: the
  proto permitting offline-plus-ports with no stated winner, and `AckResponse` being unable to express
  a partial `Remove`.
- **Milestone 4 — A SECOND SHUTDOWN DEFECT, MEASURED 2026-08-16 (late) and distinct from the exit-143
  one above.** `shutdown::the_engine_exits_cleanly_with_a_client_channel_still_open` fails about **1
  shutdown in 288** with **`exit status: 1`** — the engine's own deliberate error exit, not the
  kernel's 143, and a **different test** from the startup race. **Do not fold the two together.**
  **Attributed by measurement rather than by argument**, because the engine changed in the same round
  and the empty-diff exoneration was therefore unavailable: the identical signature
  (`95 x exit status: 0, 1 x exit status: 1`) reproduced on `8679113` with **none** of milestone 3's
  final fixes applied — 1 of 288 — and did not appear with them — 0 of 288. **So it is pre-existing.**
  One event cannot distinguish "unchanged" from "improved" and no such claim is made. The two frozen,
  separately-signed engine binaries used for that comparison were built by stashing the working tree
  and restoring it, with every changed file verified byte-identical afterwards; **that is the method
  to reuse** when a change to the thing under test rules out the usual diff-based exoneration.

**`CreateContainer` IS MILESTONE 3'S FIRST TASK — RULED 2026-08-15. This is closed; do not
re-litigate it.** It is "recreate the container of an existing sandbox, reusing what is retained"
(`engine.proto:296-302`), and **P5's exit criterion is "`gascan-arca` passes conformance and existing
`gascan-e2e`", which cannot happen while it refuses.** It goes first because it reuses machinery
`Create` already has and flips no capability flag.

**THE CALL-SITE COUNT THIS FILE CARRIED WAS WRONG, AND IT IS THE KIND OF ERROR THAT SIZES A TASK
WRONG.** It said "`gascand` calls it in three places — `service.rs:1699`, `:1778`, `:4314`".
**There are two production call sites**, both on the image-replace path: `:1699` (`rollback_image`)
and `:1778` (`replace_image`). **`:4314` is not a call path** — it is inside
`#[cfg(test)] mod storage_tests`, which opens at `:4252`, in a `MutableCapabilitiesRuntime` test
double that delegates straight to `FakeRuntime`. Verified 2026-08-15 with
`awk 'NR<=4315 && /^(mod|#\[cfg\(test\)\])/'` over `crates/gascand/src/service.rs`.

### Still open, not started

- **Three follow-ups this milestone's reviews named and deliberately did not take. ASSIGNED
  2026-08-15.** Each is small, each closes a stated gap, and each is written up where it lives:
  **(a)** move the shutdown wait out of the executable into `ArcaEngine` (`runUntilQuiesced`) so
  task 17 gets a fails-before/passes-after test — reverting the fix still leaves `swift test` green;
  **(b)** a test that `unpackLayerToCache` actually calls `cachedLayerIsReusable`, which needs an
  `Image` fixture; **(c)** the host telling the guest how many layers it attached, so "no layers" and
  "layers I could not identify" stop being the same observation.
  **(a) and (b) are milestone 3's** — both host-side Swift, both touching code that milestone does
  not, so neither collides. **(c) is milestone 4's**, because it needs a `containerization` submodule
  change, a `make vminit-rebuild` and a guest-side measurement, and milestone 4's pin bump already
  forces a submodule decision.
- **The Minors** — 6 in Gas Can, 8 in Arca, from the milestone-1 adversarial reviews. Two Gas Can
  Minors were taken along the way. Each carries its own reproduction in the review reports.
- **D7's narrowed retry.** Unblocked by evidence; maintainer's ruling 2026-08-12 was a separate PR,
  not folded into unrelated work. See its section below.
- **Milestone 2's own deferred minors** live in the plan and in
  `.superpowers/sdd/2026-08-12-p5-1-milestone-2-engine-lifecycle/progress.md`. That ledger is
  disposable scaffolding — anything in it that must outlive the milestone belongs here or in the
  handoff.

### The adversarial reviews

**Every Critical and Important from both is fixed.** The Minors are not, and are item 2
above.

| | |
|---|---|
| Gas Can PR | https://github.com/Liquescent-Development/gascan/pull/69 (merged) |
| Arca PR | https://github.com/Vas-Solutus/arca/pull/56 (merged) |
| Findings, Gas Can | `docs/status/adversarial-review-gascan-pr69.md` — Critical 1, Important 5, Minor 6 |
| Findings, Arca | `docs/status/adversarial-review-arca-pr56.md` — Critical 1, Important 6, Minor 8 |

Both report files are left exactly as written, recording what was observed at `39be145`
and `f5fde96`; each carries a status header saying what has since been fixed. They hold
file:line, a reproduced failure scenario, and a fix for each finding, plus a section on
what was attacked and *held* — which is as load-bearing as the findings, because it says
what not to re-litigate. **Read the "attacked and could not break" sections too.**

**What the two Criticals turned out to be, because they change what you believe:**

1. **Gas Can — the signed-pin gate could verify a different object than the one it
   compiled.** `verify-tag "$tag"` unqualified beside `refs/tags/${tag}` qualified: git
   tries `refs/<name>` before `refs/tags/<name>`, so the signature gate and the identity
   gate could land on different objects. Fixed by qualifying every tag name in both
   `build-arca-engine.sh` and `sync-arca-proto.sh`, and by constraining `.tag` in the pin
   schema to `^[A-Za-z0-9._-]+$`. **The old pin was never exploited** — `gascan-engine-m1`
   has no slash and was verified independently.
   `tests/release/engine-pin-contract.sh` now carries both halves of the attack as
   negative cases, and **both were confirmed to catch it**: against the unfixed script
   `slash-tag` exits 0 (the attacker's commit is compiled) and so does `shadowed-ref`.
2. **Arca — `Inspect` and `ListResources` could never report anything.** Resolved by
   option (b): both now answer `unsupported_capability`. Calling `initialize()` was
   rejected for this milestone on evidence the review had not reached — its restore loop
   *writes*, marking every persisted `running` container exited with code 137
   (`ContainerManager.swift:316-338`), so an engine pointed at a live `ArcaDaemon`'s state
   root would declare that daemon's containers dead; and `NetworkManager.initialize()`
   ends in `createDefaultNetworks()`, which creates a vmnet network. **This dissolved
   Arca's I3, I4 and I5** — all three are properties of answers that no longer exist.

   **SUPERSEDED 2026-08-13.** That paragraph ended "milestone 2 gives ContainerBridge a
   read-only load path that neither starts a VM nor writes". **That framing is retired and
   the reasoning behind it was incomplete.** The hazard belongs to *sharing a state root*,
   not to writing: given a private root the same crash-recovery write is correct, because a
   container the engine's own StateStore records as `running` at startup died with the
   previous engine process. And the read-only path could not have survived the milestone
   regardless — `createContainer` guards on `nativeManager` (`:1584`) and `startContainer`
   does too (`:2005`), so landing `Create` lands a VM-starting writer. Milestone 2 gives the
   engine its own state root and runs `initialize()` in full. **I3 and I4 are fixed on their
   merits** (Tasks 2 and 3); **I5's ordering fix returns with `Inspect`** in Task 7 — they
   were dissolved, not solved, and the answers they were properties of are coming back.

**Which Minors are left.** Two Gas Can Minors were taken along the way because they were
load-bearing for an Important — M1 (the `runtime-probe` comment orphaned onto `gate`) and
M2 (the EXIT trap that collapsed every documented exit code to 1, which the new
pin-contract cases assert exactly). M3 (the `/tmp` socket-root leak), M4 (the product
check being narrower than its comment — the comment now says so, rather than the check
being widened), M5 and M6 remain, as do all eight Arca Minors.

## The pin is real, and now on Arca's main

**`engine/arca-pin.json` names the signed annotated tag `gascan-engine-m1.1` at
`b3390b80528f425be0109298d6a95dd863747c5d` on `https://github.com/Vas-Solutus/arca.git`.**
This resolves the blocker earlier versions of this file recorded, which said the pin named
`gascan-engine-proto-v1` at `77b293e` — a revision with no engine in it, against which
`swift build --product arca-engine` exits 1, so CI's `engine` job *failed* rather than
building something old. It does not fail any more. Do not reintroduce the old wording.

**VERIFIED end to end against this pin**, not merely resolved: `./scripts/build-arca-engine.sh`
exits 0 in 6m00s from a cold clone — signature verified against `engine/allowed-signers`,
tag target matched, clean checkout, `Executed 30 tests, with 0 failures` — and prints the
checkout and binary paths. The live tier then passes 4/4 against that binary. CI's
`engine` job did the same twice on a hosted runner in run `31621889316` (14m42s the
second time), which is the only automated verification Arca has at all — see the CI
section below.

**That measurement stands as the record it is, and the gate is red right now anyway.**
Milestone 2 Task 3c widened the gate's test filter to cover `ArcaTests.NetworkPruneGateTests`
— the suite that proves `docker network prune` declines to delete an in-use network, which
`ArcaEngineTests` structurally cannot reach — and widened the listing guard beside it, so
the gate fails rather than silently running less than it names. `gascan-engine-m1.1` /
`b3390b8` carries no such suite: `git grep -l NetworkPruneGateTests b3390b8 -- Tests` exits
1 in `~/code/arca`. **So `./scripts/build-arca-engine.sh` now exits 70 against the current
pin** — `the test gate matched no tests: … declares no ArcaTests.NetworkPruneGateTests` —
and CI's `engine` job is red with it. That is the guard working, not a regression in it.
It clears when the pin moves to a signed tag carrying Arca `fede19c` (the XCTest
conversion of that suite). **Do not bump the pin ad hoc to make CI green.** The bump
belongs with the milestone's one signed tag, once the Arca branch merges; a pin moved to
an untagged or mid-branch revision buys a green check by giving up the trust model
`engine/allowed-signers` exists to enforce.

Since PR #56 merged, `b3390b8` is an ancestor of Arca's `main`, so the pinned revision no
longer depends on a tag alone to stay reachable. The older `gascan-engine-m1` at
`f5fde96` is still pushed and still valid; it was left where it was rather than moved,
because moving a pushed signed tag rewrites what an already-verified pin resolved to.

The engine build step is `.github/workflows/ci.yml:108` — re-derive that anchor with
`grep -n 'Build the pinned Arca engine' .github/workflows/ci.yml` rather than trusting it.
It has drifted on every single pass over this file so far; assume it has drifted again.
The `printf` anchor inside `ci.yml`'s own comments had drifted from `:179` to `:208` and
was corrected the same way — `grep -n "printf '%s" scripts/build-arca-engine.sh`.

`.artifacts/arca-dev-pin.json` still exists as a *development* pin naming a
`file:///Users/kiener/code/arca` URL and a local `gascan-engine-dev` tag. `.artifacts/` is
gitignored and worthless on any other machine — it is a convenience, no longer a
substitute. It now trails Arca HEAD by three commits, so **any rebuild from it must first
move the local tag** (`git tag -f -s`) and update its revision, or
`build-arca-engine.sh`'s tag-target assertion rejects it — correctly.

## What milestone 1 answered

These were unverified before this branch. Each is now an observation, and the anchors are
in `docs/status/arca-integration-handoff.md`.

- **A real engine accepts the client's placeholder authority `http://[::]:50051`.**
- **A missing socket and a non-socket render differently, and both name the path.**
  Missing: `No such file or directory (os error 2)`. A regular file: `Socket operation on
  non-socket (os error 38)`. Both carry the io cause past tonic's opaque `transport error`,
  so `source_chain` (`crates/gascan-arca/src/channel.rs:62-78`) does what it claims.
- **The engine claimed nothing it had not earned** — every capability flag came back
  `false` and `offline` came back `Unverified`.
- **The measured socket path is 41 bytes of `sun_path`'s 103.** Headroom, but the harness
  asserts the length before binding rather than meeting the cap as a mystery bind failure.
- **The engine's first-ever execution costs ~997ms against ~10ms warm**, which overran the
  plan's 30s harness bound. The harness now reports a dead child immediately via
  `try_wait()` and waits 120s.
- **`swift test --filter <no match>` exits 0 having run nothing.** Guarded now, in
  `scripts/build-arca-engine.sh`.

## Traps that will cost you if you learn them the hard way

### Added 2026-08-16 (late), from milestone 3's task 6. Every one was measured.

**A BOUND ON THE WRONG AWAIT TELLS YOU NOTHING, AND IT COST TWO TEN-MINUTE HANGS.** `exec.rs`'s
`drain` had a 60-second bound and the test still sat for ten minutes with **no output at all**,
twice, because the block was upstream of it — in `backend.exec()`, which had no bound. The first
run's only diagnostic was the engine's own log going quiet. **Bound every await in a live test, not
just the one you expect to hang**, and make the panic message name which await it was. The second
run, with every `send` bounded too, is what proved the block was in `exec()` itself and pointed
straight at the response headers.

**THE RPC THAT WORKS IS THE ONE THAT HAPPENS TO SPEAK FIRST.** grpc-swift accepts an RPC implicitly
on the first response message, and tonic's bidirectional call does not hand its caller a stream
until the response headers arrive. So a streaming handler that reads before it writes deadlocks its
client, and **a handler that writes first hides it completely**. The first live exec —
`sh -c 'echo out; echo err 1>&2; exit 3'` — passed with a correct exit status against the broken
engine. `await context.acceptRPC(headers: [:])` is the fix, and any future bidirectional method
needs it. **Nothing in Arca can see this**: `swift test --filter ArcaEngineTests` is 221 passing
with and without.

**A `grep | grep | awk` PIPELINE OVER A TEST LOG DROPPED TWO THIRDS OF ITS LINES.** Tallying the
workspace suite, `grep "test result:" log | grep "0 filtered out" | awk ...` reported **24 targets
and 543 passed** against a 74/1436 baseline — which reads as a catastrophic regression. Counting the
same two greps separately gives **74**. This is the same instrument failure this file already
records for `git diff | grep | grep | grep` and for `ps aux`, and it has now bitten in a third
shape. **Write each stage to a file and read the file.** Doing that gives 74 / 1435 / 1 / 41, which
reconciles exactly.

**A REVIEW MUTATION CAN FAIL A TEST FOR THE WRONG ASSERTION, AND THAT DECIDES HOW THE TEST MUST BE
WRITTEN.** Replacing `Exec`'s argv `String(data:encoding:)` guard with the lossy
`String(decoding:as:)` failed exactly one test — but the **code** assertion still passed, because
the mangled argv is refused further down by `createExec`, which answers `invalid_state` too. Only
the message assertion caught it. **A refusal test that asserts the code alone can be green against
the mutation it exists for**; assert something the wrong refusal cannot produce.

**IF YOU WRITE "MEASURED" INTO A COMMENT, RUN IT FIRST.** Four mutation results were written into
this task's doc comments from reasoning and then run. Three matched. The fourth — the one above —
was wrong in a way that mattered, and it was wrong in the direction that flatters the test. The
comment now records what actually happened, including that the code assertion survived.

### Added 2026-08-16, from milestone 3's tasks 3-5. Every one was measured.

**A TEST CLASS ANYWHERE IN `ArcaTests` EXCEPT `NetworkPruneGateTests` IS NOT RUN BY THE RELEASE
GATE.** `scripts/build-arca-engine.sh:226-227,250-253` filters on `^ArcaEngineTests\.` and
`^ArcaTests\.NetworkPruneGateTests/` and nothing else. **Measured: a task's acceptance suite sitting
in `ArcaTests` gave a gate run of 175 tests with none of the new ones in it; moved to
`ArcaEngineTests`, 180.** The acceptance test for that task would not have gated a release. **Put
Swift tests in `ArcaEngineTests`.** Note also that **`swift test list --filter` ignores its filter**
and will mislead you — use real runs.

**`grep "^    public func \|^    private func "` IS BLIND TO `package func`, AND THIS REPOSITORY USES
`package` PRECISELY FOR TEST REACHABILITY.** Measured on `ContainerManager.swift`: 61 functions found,
63 with the corrected scan, exactly **2** `package func`s and **both invisible** — including
`loadPersistedState()`, whose own comment says it is `package` so tests can drive it. **A reachability
question answered with that grep gets the opposite answer.** It produced a false premise that survived
into a source comment, a commit message and a report.

**`ContainerBridge`'s LOG WRITER HAS CONSUMERS IN TWO OTHER MODULES AND NONE OF THEM IS IN THE GATE'S
FILTER.** `DockerAPI/Handlers/ContainerHandlers.swift` and `ArcaDaemon/DockerRawStreamUpgrader.swift`
parse what `LogWriter.swift` writes. **Two fixes in two review rounds each cost `docker logs`
something** — one dropped U+2028/U+2029/U+0085 from container output, the other silently lost a
restore case. Both were green in the gate. **Changing that file is a cross-module change wearing a
single-file diff.**

**`ssh-add -l` ANSWERING IS NOT EVIDENCE THAT SIGNING WILL WORK, AND THERE IS A ONE-COMMAND PROBE.**
A locked 1Password enumerates its keys happily and refuses to sign — listing needs no authorisation,
using one does. **Check before attempting a commit, not after:**

```bash
echo test | ssh-keygen -Y sign -n git -f <(git config --get user.signingkey)
```

It reproduces the failure **without creating a commit object**. The failure is
`Couldn't sign message (signer): communication with agent failed`, and it is **not** the
`env -u SSH_AUTH_SOCK` trap — that is Gas Can's rule, and Arca's key needs the agent.

**A DERIVATION LABELLED AS A DERIVATION CAN STILL BE WRONG.** An implementer carefully marked a
peak-memory bound `O(readWindow + chunkByteLimit)` as a derivation rather than a measurement — the
right instinct and this project's own rule — **and it was measurably false for one input class.**
Labelling a claim correctly does not make it true; it makes the failure honest. **A derivation still
has to be checked.**

**A TEST THAT SIZES ITS FIXTURE FROM THE CONSTANT IT TESTS PINS NOTHING.** It moves with the thing it
is supposed to catch. Measured: restoring the circular fixture and mutating the constant to 7 gives
`("0") is less than ("2")` — vacuously green. **Size fixtures from literals.**

**A CLAIM CAN BE OVERTAKEN BY ITS OWN FIX, AND THE CORRECTION CAN OVERSHOOT.** One task produced
over-claim → under-claim → over-claim in three rounds, all from *editing a sentence to match a change
rather than re-deriving what was true after it*. The instruction now written into that source is
**`Re-derive. Do not edit.`**

**A DEFENCE IN DEPTH THAT NO MUTATION CAN FALSIFY IS A CLAIM, NOT A DEFENCE.** An implementer
declined to add a second belt-and-braces guard on the grounds that with the real fix in place the
second guard would be unfalsifiable, and this project does not ship those. **That reasoning was
reviewed and upheld.**

**MAKING AN ERROR UNREPRESENTABLE BEATS DETECTING IT.** Task 2's defect was closed by removing the
parameter that allowed it, so the mutation became a **compile error** rather than a failing test.
Reviewed and upheld as strictly stronger.

### Added 2026-08-15, from milestone 3's first two tasks.

**A SUBAGENT RUNNING `swift test` IN ARCA SILENTLY BREAKS THE LIVE TIER FOR WHOEVER IS RUNNING IT.**
`swift test` re-links `arca-engine` and re-signs it ad-hoc, **stripping the entitlements**, so a tier
run in flight starts failing with `vmnet_return_t(rawValue: 1002)` — and this file's own trap says
1002 means an unsigned binary, which is correct and which makes the diagnosis land on the wrong
suspect. It cost two tier runs. **Two agents in one checkout collide over BUILD ARTIFACTS, not only
over source**: `git status` was clean throughout, neither agent wrote a file the other touched, and
the failure surfaced hundreds of engines away.
**The rule is not "do not dispatch during a tier run" — that was tried and was too narrow. It is:
send NOTHING to any agent while the tier runs, because a message wakes an idle agent and an awake
agent builds. An idle agent is not a quiescent one.** Re-sign **unconditionally** before every tier
run, **assert the entitlement is present** rather than trusting `codesign` to have exited 0, and
capture the binary's mtime before and after so the cause identifies itself.

**A NONDETERMINISM IS SETTLED BY FORCING IT, AND THIS IS THE CHEAPEST EXAMPLE THIS PROJECT HAS.**
A 2-in-440 exit-143 flake in the live tier was turned into **12/12 vs 0/12** in about a minute by
spawning `arca-engine` directly and varying only when SIGTERM was sent — no cargo, no VM, no test
edit, both arms interleaved in one process against one binary. **Reach for the forced version before
the bigger sweep.** A clean 440-engine re-run would have been weak evidence at that rate; the forced
version is decisive.

**CHECK THE INSTRUMENT BEFORE THE SUBJECT — TWICE MORE, BOTH NEARLY PRODUCING FALSE CONCLUSIONS.**
A `git diff | grep | grep | grep` pipeline returned **empty** for a commit that provably changed
three lines, which would have read as "no code changed at all"; redirecting the diff to a file and
grepping that found them immediately, the same fix this file already records for `ps aux`. And a
spike script used `status` as a shell variable — **read-only in zsh** — so it captured no exit codes
at all and printed a tidy `0/12` and `0/12`, which read as *disconfirming* the hypothesis it was
built to test. **A green figure you cannot account for is not a pass, and that applies to a figure of
zero.**

**A CLAIM CAN BE OVERTAKEN BY ITS OWN FIX.** A comment saying "this file is the only cover for X" was
true when written and false within the same fix round, because that round closed the gap elsewhere.
The correction then went one step too far and gave away a property nothing else had, and the
re-correction over-claimed in a third direction. **All five instances came from editing a sentence to
match a change rather than re-deriving what was true after it.** The instruction now written into the
source is `Re-derive. Do not edit.`

**A CITATION WHOSE RANGE STOPS MID-CLAIM UNDERSTATES RATHER THAN MISSTATES, WHICH IS WHY IT SURVIVES
REVIEW.** Twice in one milestone: `runtime.rs:893-918` for a claim that ends at `:922`, and
`NetworkManager.swift:297-338` for a function spanning `:297-358` whose third relevant line is at
`:349`. The reader who follows a short range finds support and stops looking. **Check where the claim
ends, not where the function starts.**


### Added 2026-08-14 late, from the shutdown fix.

**A CORRELATE WRITTEN DOWN AS A CONDITION SENDS THE NEXT SESSION TO THE WRONG PLACE, AND THIS FILE DID
IT.** "It only happens once containers have been created" was an honest observation — every crash
anyone had seen came from a run that made one. Recorded as a *condition*, it produced a map of the
engine's seven event-loop groups, six of them container-scoped, and named a `TCPProxy` line as "the
single most promising lead". **`TCPProxy` is not on the path at all.** The cheapest experiment in the
world — run it *without* the thing — had never been run, and it took ten seconds: an engine that
created no container crashes 1 time in 96. **Before building a map from a correlate, try removing it.**

**A THING THAT ALWAYS HAPPENS UNDER TEST IS NOT THEREFORE A THING THAT HAPPENS.** The same section
said an engine that created no container "exits cleanly", stated flatly. It was a summary of runs
nobody had counted. **A negative claim needs a denominator as much as a positive one does** — and once
this one had one (96), it inverted.

**MAKING A WAIT REAL MAKES EVERY TIMEOUT AROUND IT LOAD-BEARING FOR THE FIRST TIME.** The engine's
shutdown handler carried a paragraph explaining why the second signal existed — "a graceful shutdown
waits for in-flight RPCs" — and the code had never waited for anything: it watched the listening socket,
which closes synchronously. Fixing that turned a decorative escalation path into one that had to work,
and exposed a second defect underneath (a drain that grpc-swift cannot always finish). **When you make
a comment true for the first time, everything it justified has to be re-checked, because none of it was
ever exercised.**

**A COMMENT DESCRIBING BEHAVIOUR IS A CLAIM, AND IT ROTS THE SAME WAY A REPORT DOES.** Both false claims
above lived in doc comments, which are the one place this project had not been mutating. The rule
already standing — *ask what mutation would falsify it* — applies to prose about behaviour, not just to
commit messages and reports.

### Added 2026-08-13 late, from Task 13's first hour.

**A MEASUREMENT'S SCOPE IS WHATEVER IT COULD REACH, NOT WHAT ITS SENTENCE SAYS.** This file recorded
"the isolation probes came back empty, three separate times, cross-checked" — true, careful, and
**taken when no container had ever been created**. It read as "the engine is isolated". It meant "the
engine's *startup* is isolated". Creating one container broke it in two places at once. **When you
write a measurement down, write what it could not have observed** — the three-times cross-check made
the claim feel stronger while the gap was in what was never exercised at all, and repetition cannot
close that kind of gap.

**A GUARD THE SUITE CANNOT FALSIFY IS WORTH SHIPPING ONLY IF IT SAYS SO IN ITS OWN NAME.** Task 13a's
call site needs a kernel and a VM, so its guards read the source as text. One was named
`...NeverNamesApplesSharedStore` — and swapping `manager.imageStore.path` for `ImageStore.default.path`
restores the defect **exactly** while leaving `Executed 149 tests, with 0 failures`. The assertion was
fine; the **name** was the overclaim, and a name is what the next person greps. Renamed to
`...CarriesNoLiteralPathToApplesSharedStore`, with both evasion vectors written into the doc comment
beside their measured results.

**WHEN A REPOSITORY STRUCTURALLY CANNOT TEST SOMETHING, SAY WHICH REPOSITORY CAN — AND RUN IT.** The
same mutation that leaves Arca green at 149 drives Gas Can's live `Create` to `NSPOSIXErrorDomain
Code=2`, empties the engine's own `containers/`, and grows Apple's shared store by one. Both halves are
now recorded on both sides. **An honest "this proves nothing about runtime" is only half the work; the
other half is running the thing that does.**

### Added 2026-08-13 evening. These five are new, and four cost real time the day they were found.

**A GATE THAT TWO PLACES ENFORCE IS A GATE NO TEST MEASURES.** Task 12's implementer wrote its
sandbox-id validation into *both* `lifecycleAck` and `lifecycleRefusal`, one belt-and-braces call apart.
**Deleting either copy left `Executed 147 tests, with 0 failures`, because the other caught it.**
Reading could never have found this — both copies were correct. It was found by mutating one and
getting a green suite. The fix was to put it in exactly one place, before the store read; the reasoning
is recorded at `EngineLifecycle.swift:83-91`. **When a guard is duplicated "for safety", the duplicate
does not add safety, it removes the instrument.**

**N IDENTICAL FIXTURES ARE NOT N SAMPLES. VARY WHAT THE ORDER DEPENDS ON.** This ledger recommended
"twenty independent stores, because the failure is per-process" from round 2 of Task 11 onward, and it
was written into two doc comments and several dispatch briefs. **It is wrong as stated.** Rebuilding an
identical store twenty times in one process gives *one* enumeration order, because the order is a
function of the data. Measured under a controlled experiment — twenty iterations and a fresh store per
iteration held constant, only the tag-name salt varied: **unsalted 60% catch rate, salted 100%.** Round
3's reviewer had measured the symptom (39 of 40, where truly independent samples would miss ~1.6 times
in a million, so the iterations had to be correlated); round 4 found the cause.

**A-THEN-B ON A DRIFTING MACHINE IS NOT A CONTROLLED COMPARISON.** Three separate agents were misled by
this in one day. The worst: Task 12's implementer saw five consecutive `SIGBUS` runs, bisected, watched
unmodified HEAD go green twice, and concluded its own change was the cause — **the identical tree then
ran 8/8 green once load fell from 5.58 to 3.74.** The technique that works is round 3's: it needed to
compare two resolver algorithms, so it **transplanted the old one beside the new one as
`probeResolveLegacy` and drove both against one store in one process, interleaved read by read.** When
a before/after would be confounded by environment, put both behaviours in one binary or interleave them.

**A RED FIGURE YOU CANNOT ACCOUNT FOR IS NOT A FAILURE, EITHER — CHECK THE INSTRUMENT BEFORE THE
SUBJECT.** The standing rule is "a green figure you cannot account for is not a pass". Its mirror bit on
2026-08-13: a bad `awk` field offset summed the workspace suite as **542 passed across 24 targets**
against a 1435/74 baseline, which read as a serious regression. The suite was fine; the arithmetic was
not. Re-extracted with an explicit `sed -E 's/.*ok\. ([0-9]+) passed; .../'` it is exact. **The
per-target baseline is what made the mismatch visible in seconds — keep recording it.**

**SUBAGENTS DIE WITH A FRAGMENT RETURN AND LEAVE MUTATIONS IN THE WORKING TREE. FOUR OF SIX DID ON
2026-08-13.** This is a different failure from the previously-recorded "idle with work committed": the
return message is a half-sentence, the report file may be complete or absent, and **the tree is dirty
with a reviewer's mutation that must not be mistaken for a fix.** Two left the tree dirty mid-mutation.
**Always, before anything else: `git status --short --untracked-files=all`, read any diff you find, and
`git stash push -m` it rather than `git checkout --` it** — you may need to know what it was. Two such
stashes exist in `~/code/arca` right now (`stash@{0}` five simultaneous Task-12 review mutations,
`stash@{1}` a Task-11 precedence inversion); both are discardable. **When an agent dies mid-review,
finishing its work directly is often faster than re-dispatching** — the last two verdicts of milestone 2
are controller adjudications for exactly this reason, and both are labelled as such in their files.

**THE GUEST CONSOLE LOG IS THE INSTRUMENT FOR ANYTHING INSIDE THE VM, AND IT IS EASY TO MISS.**
Apple's `ContainerManager` points every container's boot log at
`<state-root>/images/containers/<id>/bootlog.log` (`Containerization/ContainerManager.swift:317`), and
`CHVirtualMachineInstance.swift:617` captures `hvc0` into it — so it carries the kernel's device
enumeration, `vminitd`'s own logging and `vmexec`'s mount-by-mount trace. On 2026-08-14 it overturned
**two** confident source readings in ten minutes, one of them the controller's own, and it is what
turned "the guest mounts none of them" into "the guest mounts all three, read-only, as overlay
lowerdirs".

**Two obstacles, both solved, and the fix is in `stash@{0}` in `~/code/gascan`:**

1. The live harness puts its state root in a `tempfile::TempDir` that is removed when the test ends,
   taking the log with it. The stash makes the state root persist under `/tmp/gascan-spike-state-*`.
2. `Remove` empties the container directory, so even a persisted state root loses the log at teardown.
   The stash snapshots it to `/tmp/gascan-spike-bootlogs/` **before** teardown.

It is marked `SPIKE PATCH -- NOT FOR COMMIT`. **`common/mod.rs`'s half still applies; `mounts.rs`'s
half does not**, because that file was rewritten — take the harness half with
`git checkout stash@{0} -- crates/gascan-arca/tests/live/common/mod.rs` and re-add the snapshot by hand
(the natural place is `start_and_read`, which both mount tests go through). **Making this a permanent,
committed affordance of the live tier is worth doing and has not been done.**

**WHEN A SUBAGENT GOES QUIET — THE PROCEDURE, because the warning alone did not work.** This file
already said "an empty agent roster is not proof of death" and "check file mtimes before re-dispatching".
The controller read that, checked mtimes, found both trees clean and an empty roster **three minutes
after dispatch**, concluded the agent had died, and re-dispatched. It had not died; it had not yet
written anything. Two agents then edited one working tree, and the second one's first file duplicated a
type name and mismatched an enum case's arity — a build break that would have read as the first agent's
own bug. It was caught only because that agent noticed and stopped after one file.

**The check is not "is there work on disk" but "is there work on disk NOW and also several minutes from
now".** An agent that has produced nothing is indistinguishable from a dead one at any single instant.
Before re-dispatching: sample `git status --short --untracked-files=all` in every repo **twice, at least
five minutes apart**, and only conclude death if both are empty AND the roster is still empty. There are
no worktrees here, so two agents in one tree is always a collision.

**AND DO NOT SPLIT A TASK BY FILE TO LET TWO AGENTS RUN.** It was considered and correctly rejected on
2026-08-14: the Gas Can half of the volume work looks file-disjoint from the Arca half, but the new
test can only be seen to fail and then pass against a rebuilt engine and vminit — the contended
resources. **Split on the resource the measurement depends on, not on files.** Two agents rebuilding
`~/.arca/vminit` produce two green runs that were never measuring the same thing.

**A STASH DOES NOT KEEP APPLYING AFTER THE FILE IT PATCHES IS REWRITTEN.** The bootlog spike stash was
applied against a `mounts.rs` its hunks no longer fitted; the apply failed and left the file **in a
conflicted `UU` state**, which is a mutation in a tree that otherwise held staged work. Recovered with
`git checkout HEAD -- <file>`. Take one file at a time with
`git checkout stash@{N} -- <path>` when the stash is partly stale, and check `git status` after every
stash operation rather than assuming a failed apply changed nothing.

**THE MACHINE'S MEMORY IS THE HIDDEN VARIABLE.** 24 GB installed, and on 2026-08-13 it was carrying
~8 GB of swap and 7.8 GB in the compressor, with Firefox alone at 11.25 GB across 38 processes.
`swift test` died with `SIGSEGV`/`SIGBUS` intermittently and one run was OOM-killed with exit 137.
**None of it was code.**

**CORRECTED 2026-08-14 — THE INSTRUMENTS NAMED HERE ARE BOTH WRONG.** This paragraph used to end
"check `sysctl vm.swapusage` and `top -l 1 -o mem -n 20` ... and note that `ps -A` output is filtered
under this harness — `top` and `ps aux` are not." Following that produced two confident false
conclusions in one hour. **Use `memory_pressure` for pressure**, not swap or `top`'s "unused" — swap
and the compressor are a high-water mark that macOS never proactively reclaims, so they describe the
worst moment of the session rather than the current one. **And `ps aux` is filtered here too** —
redirect it to a file. Both corrections are written up with their measurements in the vmnet section
above.

---

**THE DEFECT'S NEXT FORM IS A CLAIM THAT OUTRUNS THE CODE, AND TASK 11 PRODUCED SIX.** Round 1 of its
review found three: the commit message and report each said offline-plus-ports was refused (it was
accepted, and a test *pinned* the acceptance), that every refusal was asserted by exact string
equality (collapsing seven messages to the literal `"unsupported"` left all 123 tests green), and
that a destructive `docker rmi` change was "Docker's own semantics" (it is not). Round 2 added three
more: a tie-break sort pinned by no test — **deleting it entirely left 137 tests green** — a claim
that resolution no longer depends on enumeration order which was **false for two of the four resolver
arms**, and a measured rate borrowed from a different fixture.

**Every one was caught by a reviewer running a mutation. None was caught by reading.**

**The rule that would have caught all six, now standing for this project:** before writing a claim
into a commit message, a source comment, or a report, ask **what mutation would falsify it, and
whether a test already fails under that mutation.** If none does, write the test or write the weaker
claim. A commit message asserting a property the suite cannot demonstrate is worse than silence,
because the next person greps for it and stops looking.

**REPORT A MUTATION BY COMPOSITION — WHICH TESTS SURVIVE, BY NAME — NEVER BY COUNT.** Task 10's
round-1 report read a *rising* failure count as evidence its new test was load-bearing. The count had
risen for an unrelated reason and that test was the one test in the file proving nothing; two
reports asserted it before a third measured which tests actually survived.

**A NONDETERMINISM IS NOT FIXED BY RUNNING IT MORE TIMES — RESTRUCTURE SO THE FAILURE IS FORCED.**
Task 11 shipped an `rmi` guard that threw on 2 of 5 runs, and in one run `getImage` and `deleteImage`
resolved the same string to different rows *inside one process*. Three of five runs looked fine. The
fix was to remove the order dependence at its root (one pass per arm, not one pass per row) and to
prove it with a test that builds 20 independent stores per run and reads one store 25 times: 10
consecutive runs, 10 GREEN, and 0 GREEN / 10 RED with the old arrangement restored. **Looping fresh
fixtures inside the test is what converts an N-in-5 flake into a deterministic failure.**

**A SUBAGENT WILL GO IDLE WITH ITS WORK STAGED AND UNCOMMITTED.** It happened once with **1755 lines
in the index**, alongside SourceKit diagnostics showing real-looking compile errors — a combination
that reads as an agent stopped mid-edit with broken code. Measuring said the opposite: `swift build`
exit 0, `Executed 123 tests, with 0 failures`. **The diagnostics were stale editor state.** Check
`git status --short --untracked-files=all`, `git diff --cached --stat`, and then actually build,
before concluding anything. Six subagents this session went idle with committed work and only the
return message missing.

**SOURCEKIT DIAGNOSTICS IN THIS REPO ARE ROUTINELY STALE AND SOMETIMES NAME FILES THAT NO LONGER
EXIST.** Reviewers write transient `ZZ*Probe*.swift` files and delete them; a diagnostic captured
mid-life outlives the file. Four of those appeared this session and all four were already gone.
**Check with `/usr/bin/find` and `git status --untracked-files=all` rather than trusting or
dismissing a diagnostic** — and note `find` is intercepted by the rtk hook for compound predicates,
so use the absolute path.

**SENDING A DECISION IS NOT THE SAME AS THE DECISION ARRIVING.** A maintainer approval crossed with a
subagent's messages **twice**; it reported itself blocked while the approval sat unread in its
mailbox, and about an hour was lost. Recording an approval in the ledger is not evidence the agent
received it. **For anything blocking, confirm the recipient acted on it.**

**THE DEFECT THIS MILESTONE FOUND EIGHT TIMES IS A TEST THAT PASSES WHILE PROVING NOTHING.**
Every task in landings 1-2 shipped one on the first attempt, and every one was caught by a
reviewer's mutation rather than by reading the diff. In order of increasing subtlety:

1. an assertion that was a **tautology** — `containerizationRoot()` returned the value the test
   handed the initializer;
2. a **one-sided** assertion — `XCTAssertTrue(hidden.isEmpty)` could not distinguish "hid the
   internal container" from "hid **every** container";
3. a **stub-driven** test that stayed green when only the production default was dropped;
4. a pair pinning the **failure path** while leaving the gate's **normal path** unpinned —
   "the read failed → don't delete" was proved, "the read said in-use → don't delete" was not;
5. six well-formed tests proving a **function** and never that `run()` **called** it;
6. an assertion on `--kernel-path` in stderr satisfied by **ArgumentParser's usage line**, printed
   on any parse error before `run()` is entered;
7. a conjunct **implied by its sibling** — `contains("arca-vminit:latest") && contains("vminit:latest")`,
   where the second string is a substring of the first, so the "which was found" half asserted
   nothing;
8. a signing step whose only proof would have been `codesign -d` output — which proves the command
   ran, not that the binary works.

**The two rules that catch these: mutate the PRODUCTION DEFAULT, not the seam; and mutate the CALL
SITE, not only the function.** A test that drives an injected stub proves the stub. In Task 3 a
reviewer dropped only the production default while leaving the stub path intact — the stub-driven
test stayed green and only the test that installed nothing caught it.

**A REVIEWER THAT CANNOT MAKE A FIX FAIL SHOULD SAY SO, INCLUDING AGAINST ITSELF.** Task 6b's
reviewer filed an Important finding backed by a real measurement, the fix landed, and it then
retracted the finding with a second measurement — its first had run `swift build --build-tests`
directly, which strips signatures, where `make test` never does. The fix was kept as defence in
depth and the commit subject asserting a relink that does not happen was corrected by a following
commit. **A fix you cannot make fail is either unnecessary or unprovable, and both deserve saying.**

**`arca-engine` CANNOT START A CONTAINER UNSIGNED, AND THE ERROR LIES.** `initialize()` constructs
`Containerization.VmnetNetwork()`, which needs `com.apple.security.virtualization`. Unentitled it
throws `vmnet_return_t(rawValue: 1002)` — the SDK header labels that `VMNET_MEM_FAILURE`; the
cause is the entitlement, not memory. The process exits and **never creates a socket**. Task 6b
signs it in Arca's `Makefile` and in `scripts/build-arca-engine.sh` (ad-hoc `--sign -`, which needs
no certificate). **Ad-hoc is sufficient for the gate and the live tier and NOT for a shipped
`.pkg`** — Developer ID signing is milestone 4's.

**LINE ANCHORS IN `SandboxEngineService.swift`, `ContainerManager.swift` AND
`NetworkManager.swift` MOVED UNDER EVERY SINGLE TASK.** `getNetworkAttachments` was cited at four
different lines across one landing. The `printf` in `build-arca-engine.sh` moved four times, twice
inside commits whose own comments say "re-derive rather than trusting the number, it has gone stale
twice". **Re-derive every anchor immediately before editing**, and re-derive again after your own
edits if you cite them.

**A SUBAGENT WILL FLIP THE SHARED TASK TRACKER TO `completed` BEFORE ANY REVIEW EXISTS.** It
happened twice. The tracker is an instruction surface, not a status board — the ledger is
authoritative. **And an idle notification is not a result:** three times a subagent went idle with
its work committed and only its return message missing. **Check `git log`, the working tree, and
the report file before re-dispatching anything.**

**SIGNING IS INVERTED BETWEEN THE TWO REPOSITORIES.** Gas Can's `user.signingkey` is a file
PATH (`~/.ssh/gascan-signing`), so commit with `env -u SSH_AUTH_SOCK git commit`. **Arca's
key lives in 1Password**, so it needs the agent and `env -u SSH_AUTH_SOCK` breaks every
commit with `unable to sign`. One rule for both aborts everything in one of them.
**NEVER `--no-gpg-sign`**, never a lightweight tag. Verify `%G?` is `G`. No co-author
trailer and no AI-tool mention in any commit message.

**1PASSWORD ANSWERS `ssh-add -l` WITHOUT APPROVAL BUT REFUSES TO SIGN WITHOUT IT.** "The
agent lists the key" is not evidence that signing will work. Signing in Arca needs a human
at the keyboard; if it fails, ask rather than working around it.

**THE SOURCE TREE IS NOT A RELIABLE PREDICTOR OF THE PINNED BUILD.** `~/code/arca` resolves
grpc-swift-2 2.4.2 successfully; an identical clean clone of the same revision does not,
and the mechanism was never found. Verify against a clean clone. Related: an executable
product build reaches dependency validation that library-target builds skip, which is how
a broken graph hid until the pinned build first produced a binary.

**AN IDLE NOTIFICATION IS NOT DEATH, AND AN EMPTY AGENT ROSTER IS NOT PROOF OF DEATH.** A
subagent went idle mid-work with output uncommitted, `ListAgents` reported nothing
reachable, a replacement was dispatched, and the two collided in the same files. **Check
file mtimes before re-dispatching a task whose work is already on disk.**

**A SUBAGENT CANNOT SUSTAIN A MULTI-MINUTE BACKGROUND BUILD.** Its session pauses and takes
the build with it — twice, the second time leaving `scripts/build-arca-engine.sh`'s `mkdir`
lock held. That lock fails closed by design, so every later run exits 75 until it is
cleared. Long builds belong in the controller session.

**WRITE PLANS THAT SAY WHERE YOU ARE GUESSING.** Nine blocks of this plan's Swift and shell
were wrong. Every one was marked "a best reading, not verified" with the command to confirm
it, and every one surfaced as a directed correction rather than a fix round. The worst
would have been silent: ContainerBridge reports container names with a **leading slash**,
which Gas Can compares against the bare sandbox id — every owned container would have
looked unrelated to its sandbox and drift detection would have seen nothing.

**THE INSTRUMENT KEEPS BEING NARROWER THAN THE CLAIM.** This is the defect this project pays
for over and over. This session alone: a permissions assertion that mis-parsed because `&`
binds tighter than `??`, so it masked a literal zero; and a stale-socket test whose fixture
created a regular file, which the code under test correctly refuses to unlink — it would
have failed against the very property it existed to prove. **Check a fix as hard as you
checked the defect, and prefer reading an artifact to grepping it.**

**COUNTING `test result:` LINES OVERCOUNTS THE WORKSPACE.** Some come from child processes
re-executing a test binary with a filter. Sum only the lines reporting `0 filtered out`,
and check that their count equals the target count.

**NEVER RUN THE WORKSPACE SUITE WHILE ANY OTHER CARGO IS RUNNING — NOT JUST A SUBAGENT'S.**
Run it alone, after `pgrep -fl "cargo test"` comes back empty. Concurrent suites against one
target directory produced **rc=101 with 59 failures**, none of them real: those tiers spawn
daemons and bind sockets, so they starve each other. Run alone it takes **93 seconds**.

**It is not only subagents, and not only this repository.** On 2026-08-12 another Claude
session was looping full `cargo test --workspace` cycles in an unrelated repo
(`capsule-os-worktrees/worker`) on the same machine, and this repo's suite failed three
times in a row while it ran. `pgrep` ancestry named the owner every time. **Read the
ancestry before assuming a stray cargo is yours, and never `pkill` it.**

**The failure count scales with the load, which is how you recognise it.** Measured that
day in this repo, same tree, same commit `351a646`: 2 concurrent cargo processes → **21
failures / 2 targets**; 3 processes → **37 / 5**; 3 processes with the run stretched
longer → **41 / 9**. Then every one of those 9 targets run *alone under the same
contention* → **318 passed, 0 failed, rc=0 for all nine**
(`gascan-apple/backend_fake_runner`, `gascan-e2e/{apple_apply,autostart,doctor}`,
`gascand/{daemon_idle,doctor_state,lifecycle,reconcile,ssh_config}`).

**`-- --test-threads=N` DOES NOT HELP; IT MAKES IT WORSE.** Bounding per-binary
parallelism to survive a loaded machine stretches the run, which overlaps *more* of the
neighbour's load — that is the 41-failure row above. Wait for a quiet machine, or verify
by isolation. Do not tune your way out of it.

**AND IT HAPPENED AGAIN, AT 92 FAILURES, AND WAS NEARLY BELIEVED.** Task 10 raised a
blocker on `rc=101, 92 failures across 12 targets`, every one of them a tier that spawns a
daemon or a helper. It was the same artifact. Settled by measurement rather than argument:
`cargo test -p gascand --test daemon_idle` is `running 11 tests`, 11 passed, exit 0 on
*both* the merge-base `9665107` and the branch tip under the same contention, and the full
suite run alone is exit 0 with **1435 passed / 0 failed / 28 ignored**. **A report claiming
"it reproduces on a quiet machine" is not evidence the machine was quiet** — check
`pgrep -fl "cargo test"` yourself and record the output.

**`git checkout <path>` IS NOT A PERSONAL UNDO IN A SHARED TREE.** It discards every
uncommitted change to that path, including another agent's in-flight work. Check
`git status` for a concurrent writer first, and undo your own edits with a targeted edit.

**NEVER PUT CONTROLLER STEPS IN A TASK OWNED BY A SUBAGENT.** The task tracker is an
instruction surface, not a status board.

**A STAGED BRIEF IS A CACHE AND IT GOES STALE.** Re-extract every brief immediately before
dispatch.

**A GREEN FIGURE YOU CANNOT ACCOUNT FOR IS NOT A PASS.** Account for every increment against
a per-target table you can re-derive by reading `running N tests` lines.

**`RUSTUP_TOOLCHAIN=1.95.0` is exported** and overrides `rust-toolchain.toml` — prefix every
cargo command with `env -u RUSTUP_TOOLCHAIN`. Use `--no-fail-fast`. Confirm the `running N
tests` line, because a bare test name silently runs zero and exits 0. `cargo clippy --fix`
is prohibited here. `ls` is aliased to something that rejects trailing-slash paths — use
`find` or `git ls-files`.

**A DOCS-ONLY CI RUN SKIPS `rust` AND `engine` ENTIRELY** (VERIFIED, run `31262534703`), so a
green docs run is not evidence about anything in Rust.

## CI: what to expect, so you do not spend a session on it

**`ci / gate` is NOT a required check and does not block merging.** VERIFIED 2026-08-12:
ruleset `20492137` carries `deletion`, `non_fast_forward`, `required_signatures` and
`pull_request`, and **zero** `required_status_checks`; PR #69 read
`mergeable=MERGEABLE, mergeStateStatus=UNSTABLE` and merged. `allowed_merge_methods` is
`["merge"]` — merge commits only, never squash.

**The `rust` job fails about 38% of the time on `main`, on a different test each time.**
Measured by reading the `rust` conclusion of every run in
`gh run list --workflow=ci.yml --branch main --limit 15`: of the 13 that completed, 8
green and 5 red, and the five reds were five distinct tests. PR #69's `rust` job then
failed **four consecutive times** — `pty_resize_driver_drains_chatty_child_without_
backpressure_timeout` (twice, missing a hard 2s wall-clock bound by 100ms and by **2.3ms**),
`concurrent_clients_converge_on_one_private_daemon` (D7, above), and
`same_image_apply_recreates_explicit_ssh_as_automatic`.

**One failure mode reproduced verbatim across branches, five days apart**, which is the
proof it is not a given branch's doing: `main`'s `31203816056` and PR #69's third attempt
both died as `KeygenRejected(KeygenRejection { outcome: Code(255), message:
KeygenMessage("/dev/fd/<N>: Bad file descriptor"), descriptor: Intact })`, fd 18 and fd 24,
both ending `error: test failed, to rerun pass \`-p gascand --test apply_setup\``.

**How to decide whether a red `rust` is yours.** Do not argue from probability — check the
diff. `git diff <merge-base>..HEAD -- crates/<the failing crate>/` empty means your branch
cannot have caused it, and that is a proof rather than an estimate. It was empty for
`crates/gascan-e2e/` throughout P5.1.

**The standing rule: a green local `cargo test --workspace` is the bar. CI reports but
must not gate, and flake-chasing waits** until someone is asked to do it. There are at
least three distinct root causes to fix when that day comes — the PTY wall-clock bound,
D7's `0200` window, and the keygen `/dev/fd` descriptor.

**Arca has NO CI AT ALL.** `gh pr checks 56` reported "no checks reported on the
'feat/sandbox-engine' branch", and `.github/workflows` does not exist in that repository.
Gas Can's `engine` job — which builds Arca from the signed tag and runs its 30 tests in a
clean checkout — is the only automated thing that ever exercises Arca. Any earlier
statement in this file that "CI is green on both" was wrong.

## D7 has fired, and the retry is now justified — write it

**The first `0200` occurrences landed on 2026-08-12, and there were two.** The instrument
did exactly what it was built for: it named which state fired. Verbatim, the local one,
from `cargo test --workspace --no-fail-fast`:

```
---- daemon_stderr_sink_survives_the_launching_cli stdout ----
daemon start failed: stdout=, stderr=Error: started daemon did not become healthy and
current (state Unsafe): protected runtime file is unsafe: mode is 0200 and the file has
content: written but never published (mode 0200, size 375, links 1, uid 501, expected
uid 501)
```

And in CI, run `31621889316`'s second `rust` attempt, a different test with the same
fault — `concurrent_clients_converge_on_one_private_daemon`, **size 382**, which is two
clients racing to autostart one daemon.

Read the message, not the test name. Of the two `0200` states
`crates/gascan/src/daemon.rs:3077-3079` distinguishes, both occurrences were **"has
content: written but never published"**.

**CORRECTION, recorded because it reverses the first reading of this evidence.** The doc
comment at `:3057-3064` calls that state "a daemon that wrote its record and died before
publishing, which never becomes 0600 on its own" — a corpse. On that basis this file
briefly said the evidence argued *against* the retry. **The code says otherwise, and the
code wins:**

- `is_interrupted_tombstone` (`:2633-2639`) is *defined* as 0200 with `st_size > 0`. It is
  a named, expected state the supervisor knows how to handle, not an unrecoverable one.
- `retire_held_record` (`:1372-1375`) resolves it — and **produces it transiently itself**:
  it `fchmod`s to `INSTANCE_TOMBSTONE_MODE` *first* and `ftruncate`s to 0 *second*, so
  between those two syscalls the file is 0200-with-content on disk.

So 0200-with-content is reachable as a publication in flight, `validate_file_stat` rejects
it as a hard `PermissionDenied`, and a client that reads during that window fails its
autostart. That is a race worth waiting out — exactly what the narrowed retry is for.
**The condition this file set ("stays unwritten until a run names which of the two `0200`
states fired") is met, and it points toward the retry.** Maintainer's ruling 2026-08-12:
write it in its own PR, not folded into unrelated work.

**D7'S RATE HAS GONE UP SHARPLY, AND THAT IS THE STRONGEST ARGUMENT YET FOR WRITING THE RETRY.** This
section was written after **two** occurrences on 2026-08-12. **2026-08-15 added three more**, all the
same `written but never published` state, each in a different test:

| run | test |
|---|---|
| during task 17 | `daemon_start_identity_is_stable_across_caller_locale_and_timezone` |
| after the re-review fixes | `environment_teardown_terminates_its_exact_live_daemon` |
| on merged `main` | `durable_controller_state_survives_daemon_replacement` |

**Read the tests as a set rather than individually: three different ones, one state.** That is the
signature of a race in the runtime record, not of three flaky tests. Each target passed alone
immediately afterwards (28/28, 16/16, 28/28) and a re-run of the whole suite was exit 0 with zero
occurrences, so isolation still exonerates the branch every time — but the cost of doing that
exoneration is now paid on roughly every second workspace run. Load averages were 3.2-5.9 throughout,
which is the condition this file records these scaling with.

**The keygen fault fired once the same day too** —
`KeygenMessage("/dev/fd/22: Bad file descriptor")` in `gascand --test apply_setup`, the third of the
three known root causes. All three have now been seen on this machine.

Two cautions that remain true:

- **It is load-dependent and does not reproduce on demand.** Both occurrences happened
  under contention (local: another repo's suite, pid 4969 recorded by `pgrep`; CI: a
  hosted runner). Quiet re-runs report **0 occurrences of `mode is 0200`**.
- **It predates the engine work.** Nothing in P5.1 touches `gascand`, the runtime record,
  or `crates/gascan`.

## P5.2 is done

`crates/gascan-arca` implements `RuntimeBackend` over Arca's contract behind an
`EngineTransport` seam, merged as `bd412b4`. `ChannelTransport` ships with no tests by
explicit ruling — the compiler checking it against `EngineTransport` was the stated
assurance, and **Tasks 9 and 10 of the current plan are what finally test it against a real
engine.** Do not add a test double for it.

The `sandbox_id`-claim rule is still duplicated verbatim between
`gascan-arca/src/translate.rs` and `gascan-apple/src/inspect.rs`, each with its own test and
a comment warning they must not diverge. Sharing it belongs to P5.3.
