# Review — `3306491` "publish and retire the daemon instance record by rename, not by chmod"

Range reviewed: `a8c37a3..3306491` (branch `fix/daemon-instance-publish-race`), read at
`3306491`. Read in full: `crates/gascand/src/socket.rs` (1107 lines at head),
`crates/gascan/src/daemon.rs` regions 1350–1600, 2590–2900, 3180–3235, 4830–4890,
`crates/gascand/src/api.rs:485–510`, `docs/status/START-HERE.md` (110–160, 245–256,
680–700), `docs/status/next-session-kickoff.md` header + 478–487,
`docs/status/arca-integration-handoff.md` (D7 rows).

**I did not run `cargo`.** ~28 other agents are active in this session and this project's
workspace suite is documented as load-sensitive; I could not confirm no other cargo job was
running, so every claim below is from reading, and where I could not verify something from
the tree I say so.

---

## Strengths (specific)

- **The core design is right, and it is the design that was already in the file.**
  `SocketPaths::bind` (`crates/gascand/src/socket.rs:73–107`) publishes the socket by
  staging + `renameat_with(NOREPLACE)` + `StagingGuard`. Publication
  (`socket.rs:332–402`) now has exactly that shape, so there is one publish idiom in the
  module rather than two. Reusing a proven shape beats inventing a second one.

- **`NOREPLACE` is a strictly stronger guard than what it replaced.** The old code proved the
  destination safe by opening it and re-`stat`ing (`a8c37a3:crates/gascand/src/socket.rs`,
  the `identity_at(&directory, name)? != identity` check); the new code proves it with a
  kernel-atomic flag at `socket.rs:383–390`. Two concurrent publishers now both refuse
  deterministically instead of both writing into one inode.

- **The destination is still validated, and validated in the right order.**
  `clear_inert_destination` (`socket.rs:404–436`) refuses anything that is not exactly the
  inert tombstone, and removes it through the identity-checked quarantine dance
  (`remove_named_identity`) rather than a bare `unlinkat`. The refusal is the same refusal
  the predecessor made, and the doc comment at `socket.rs:404–410` says exactly that. The
  invariant "a published record, an interrupted record, or a foreign file is never touched"
  survives the rewrite intact.

- **Generalising `remove_named_identity` with `kind` does not weaken the socket path.** All
  five socket call sites pass `FileType::Socket` (`socket.rs:562`, `658`, `713`, `78`, and
  the two tests at `790`/`1090`), the equality test at `socket.rs:594` is still exact-kind,
  and the "put it back if it is not what we expected" branch at `socket.rs:596–603` is
  unchanged. The only new caller passes `FileType::RegularFile`. This is a widening of
  applicability with no loosening of the check.

- **Two complementary tests, and the mutation testing is the right kind.** The hook test
  (`socket.rs:940`) pins the single instant of commit; the observer test (`socket.rs:876`)
  covers retirement, which has no hook. The commit message records that reverting each half
  separately failed exactly one test each — that is disjointness, not just redness, and it is
  the evidence that matters.

- **Both rewritten tests preserve intent.** I compared them against `a8c37a3`:
  - `..._never_publishes_over_a_destination_that_appeared` (`socket.rs:980–1011`) still
    asserts all three of the original's claims (publication errors, the interloper's content
    survives byte-for-byte, no file in the runtime dir contains the managed bytes). Only the
    hook's setup changed, because unlinking first is now unnecessary. If anything it is
    stronger: the loss is now enforced by `RENAME_EXCL` rather than by a descriptor recheck.
  - `..._leaves_one_inert_tombstone_a_successor_replaces` (`socket.rs:1043–1065`) keeps every
    original assertion (tombstone mode, tombstone size 0, successor's content, and
    `read_dir(&root).count() == 1` proving no staging litter) and inverts only the inode
    assertion, which is the behaviour that actually changed. Not weakened to pass.

- **The commit message's verifiable citations check out.** `api.rs:489` is
  `config.paths.bind()?` and `api.rs:506` is `write_daemon_instance_record(...)`, so "binds
  its socket before it writes this record" is exact. `recover_interrupted_tombstone`
  (`crates/gascan/src/daemon.rs:1357–1372`) does loop `0..2` with
  `prove_endpoint_absent_or_inert` inside, so "proves the endpoint absent twice" is exact.
  The retraction of the earlier reclaim framing is correct and was worth making.

---

## Issues

### Critical (Must Fix)

**None.** I found no defect that breaks the protocol or that makes the tree worse than
`a8c37a3`. The change is a net improvement on every axis I checked.

### Important (Should Fix)

**1. `crates/gascan/src/daemon.rs:3205–3206` — the corrected comment states a reachability
set that is false, and omits the one producer that is still in the tree.**

> "it is reachable only from a daemon that died mid-publish or from one older than that
> change."

The first disjunct is not true of the code this same commit ships. Under the new publisher
the content never exists at the destination: a daemon that dies mid-publish leaves the
destination *absent* (it was cleared at `socket.rs:355`) and a `.token`-named staging file
in the runtime directory. `gascand` can no longer produce 0200-with-content at
`daemon-instance.json` by any death.

What *can* still produce it is `retire_held_record` in this very file,
`crates/gascan/src/daemon.rs:1453–1457`:

```rust
rustix::fs::fchmod(&record.file, Mode::from_raw_mode(INSTANCE_TOMBSTONE_MODE))?;
rustix::fs::ftruncate(&record.file, 0)?;
```

Reached from `recover_stale_published_record` (`daemon.rs:1409`), where `record.file` is a
*published* 0600 record still linked at the destination — `validate_held_published_record`
proves the path still names it immediately before. So the CLI's own reclaim walks the
destination through 0200-with-content, exactly the two-step edit this commit removed from
`gascand`'s retirement. Why it matters: the comment is the map the next reader will use, and
it points away from the only remaining in-tree producer. Fix: replace the disjunct with
"from `retire_held_record` above, which still chmods then truncates in place, or from a
binary older than this change", and cite `daemon.rs:1453`.

Calibration: the *residual defect* at `daemon.rs:1453` is much milder than the one fixed —
the record there has been proven dead (process absent, endpoint absent, twice), so a
concurrent reader that samples 0200-with-content forms a verdict that is unflattering but
**not false**. I am not asking for it to be fixed in this commit. I am asking for the comment
not to claim it does not exist.

**2. `crates/gascand/src/socket.rs:255` — retirement commits with a plain `renameat`, the one
commit in this module that is not identity-guarded.**

```rust
if identity_at(directory, name).is_ok_and(|current| current == expected) {   // :242
    ...
    rustix::fs::renameat(directory, staging.as_str(), directory, name)?;     // :255
```

Between the identity check at `:242` and the rename at `:255` the destination could be
replaced, and a plain `renameat` clobbers whatever is there. Every other commit in this file
uses `RenameFlags::NOREPLACE` (`:97`, `:389`, `:584`), and `remove_named_identity` goes to
real trouble (`:565–604`) to never unlink a node it has not proven. Retirement must replace,
so `NOREPLACE` genuinely cannot be used here — but the asymmetry deserves either a fix or a
written reason. No in-tree actor can hit this window (`gascan`'s reclaim edits in place and
never renames), so it is a tamper-resistance gap, not a functional bug — which is exactly
what the rest of this module is built to close. See Recommendation 1 for a fix that removes
the window entirely.

**3. `crates/gascand/src/socket.rs:243` — retirement can now fail for resource reasons that
the old retirement could not, and the failure is swallowed in `Drop`.**

The old retirement was `fchmod` + `ftruncate` + `sync_all` on a held descriptor: essentially
unfailable. The new one creates a file first (`stage_inert_instance_file`), so retirement can
now fail on `ENOSPC`, `EDQUOT`, or `EMFILE`. `OwnedInstanceRecord::drop`
(`socket.rs:200–209`) discards the error with `let _ =`. On a full filesystem the daemon now
exits leaving a **complete, 0600, live-looking record** at the destination, where the old
code would still have degraded it to a tombstone. This is a real behaviour change, it is not
in the commit message's list of four, and it trades a rare-but-total failure for a
never-happens one. Fix: Recommendation 1 (unlink-based retirement) removes the allocation
entirely; failing that, the `Drop` should at minimum not be silent.

**4. `crates/gascand/src/socket.rs:876–928` — the new observer test adds an unbounded
spin loop to a suite this project documents as load-sensitive.**

The observer thread (`socket.rs:888–905`) calls `symlink_metadata` in a `while !stop` loop
with no yield and no backoff, and runs for the full duration of 64 publish-and-retire cycles
(each with several `fsync`s). Under `cargo test --workspace` — which runs `ncpu` test threads
— this burns one core for the length of the test. The flakiness this whole commit exists to
remove is documented in `docs/status/START-HERE.md:694` as scaling with load average ("Load
averages were 3.3-4.9 throughout, which is the condition this file records these scaling
with"). Adding a spin loop to that suite is worth a deliberate decision.

There is a real tension, and I want to state it rather than hand-wave it: `yield_now()` would
cut the load, but the liveness assertion at `socket.rs:923–926` needs the observer to sample
the *published* state, and in the loop at `:907–909` the record is published and dropped
immediately, so that window may be microseconds. A yielding observer could miss it 64 times
and flake in the other direction. The clean fix is to make both cheap: yield in the observer
**and** give the published state a bounded, observable lifetime in the main loop. **I did not
run this test, so I am flagging a design concern, not an observed failure.**

**5. Reader-side windows in `crates/gascan/src/daemon.rs` still turn legitimate concurrent
transitions into terminal verdicts.** The task asked; here they are, with provenance:

- **`daemon.rs:2792–2796` → `validate_instance_tombstone` (`daemon.rs:2842–2880`),
  PermissionDenied — pre-existing, not fixed, narrowed.** The reader `stat`s the path, sees
  the tombstone, then re-opens it *by name* at `:2852`. If a successor's publication lands in
  between, the open returns the new 0600 record, `is_instance_tombstone` is false, and
  `:2866–2869` returns a terminal `PermissionDenied`, which `inspect_with` turns into
  `DaemonState::Unsafe` at `daemon.rs:1072–1088`. This was reachable before the change too
  (the in-place chmod also broke the tombstone's identity), and the window is now far
  narrower — a single rename instead of an `fsync` — but it is the same defect family and it
  is still open.
- **`daemon.rs:2852` can now return `ENOENT` where it previously could not — introduced
  here.** `clear_inert_destination` (`socket.rs:411`) unlinks the tombstone before publishing;
  the old publisher opened it in place and never unlinked it. So the tombstone name can now
  vanish between the reader's `stat` and its `openat`. Blast radius today is small:
  `read_instance_record_for_inspection` (`daemon.rs:2602–2609`) maps `NotFound` to
  `Ok(None)`, so `inspect_with` is unaffected; only `read_attested_instance`
  (`daemon.rs:937`) propagates it, and that function has no non-test callers yet (the file
  carries `#![allow(dead_code, reason = "Task 5 management entry points are consumed by the
  Task 6 CLI commands")]`). It will matter when Task 6 wires it.
- **`open_published_record` (`daemon.rs:2611–2661`) — a legitimate daemon *stop* is reported
  as `Unsafe`, and this commit changes which message it gets.** After reading, `:2643`
  re-`stat`s and calls `validate_file_stat`, and `:2648–2661` compares identity. A concurrent
  retirement now installs a *different inode*, so the failure is "daemon instance path
  changed while binding its descriptor" where it used to be "mode is 0200 and the file is
  empty: not yet published". Either way `DaemonState::Unsafe` (`daemon.rs:1089–1104`). The
  false-verdict-on-a-live-transition shape is pre-existing; the message is new.

The structural point behind all three: the reader has **no retryable verdict**. Every
disagreement between two observations is terminal. As long as that is true, every narrow
window is a terminal verdict waiting for a loaded machine. That is a design decision worth
making explicitly rather than inheriting — see Recommendation 4.

**6. `docs/status/START-HERE.md` was not updated, and it is this project's handoff
contract.** Details and exact wording in the Documentation section below. Listing it as
Important because the file currently instructs the next session to do work that is done.

### Minor (Nice to Have)

**7. `crates/gascan/src/daemon.rs:3197–3198` — an unanchored past-tense claim.**
"It cost five workspace runs and one CI run before it was named." That is an event, and the
project's rule requires an inline anchor. The anchor exists in the tree —
`docs/status/START-HERE.md:145–149` records the five runs, the date, and the fifth one on
merged `main` at `8417070` — it just is not cited. Either cite it or cut the sentence; it
does not help a reader of `validate_file_stat` either way.

**8. `crates/gascan/src/daemon.rs:3196–3197` and `:3202–3203`, and
`crates/gascand/src/socket.rs:213–218` and `:868–874` — the measurement numbers cannot be
reproduced from anything in this tree.** 12,131,645 / 6,812 / 47,124,057 / "2000 cycles" all
come from a probe the commit message says was "since replaced by the bounded test below". The
committed test runs **64** cycles, not 2000, so `socket.rs:871` ("MEASURED on 2026-08-18 with
2000 cycles under this same observer") asserts an identity between the removed probe and the
committed test that a reader cannot check. The claims are plausible and the commit message is
honest that the probe is gone — this is not dishonesty, it is an unreproducible anchor. The
cheap fix that makes the number real: land the 2000-cycle variant as an `#[ignore]`d test.
The cost is one line in the ignored-test census (`scripts/ci-check-ignored-tests.sh`, baseline
49 → 50), which the commit message shows is already tracked. Failing that, say "measured with
a probe that is not in this tree" so the reader stops looking for it.

**9. `crates/gascand/src/socket.rs:261` — the `fchmod` in the renamed branch of retirement no
longer does anything.** After the rename at `:255` the old inode is unreachable by name, and
an already-open descriptor keeps its access mode regardless of the mode bits. The comment at
`:257–259` correctly names `ftruncate` as the thing that carries the guarantee ("emptying it
is what keeps a descriptor that outlives this process from reading one back") and then chmods
anyway. Harmless and symmetrical with the other branch; either drop it or say it is for
`fstat` observers.

**10. `stage_inert_instance_file` has two different cleanup paths for the same file.** Its own
failure path unlinks with a bare `rustix::fs::unlinkat` (`socket.rs:310`), while the caller's
`StagingGuard` unlinks through the identity-checked `remove_named_identity`. Both are
defensible (the bare unlink is on a name only this process knows, created `O_EXCL`, in a 0700
directory) but a reader has to work out why they differ. Pick one, or say why.

**11. `crates/gascand/src/socket.rs:719–726` — `random_name`'s `purpose` argument is
discarded (`let _ = purpose;`), and this commit adds a second producer of `.token` staging
files.** Bind staging and instance staging are now indistinguishable by name, so a leaked
staging file in the runtime directory cannot be attributed to either. Pre-existing smell, but
the change makes it matter more. Using `purpose` in the name costs nothing.

**12. Stale hook name.** `write_instance_record_with_commit_hook`'s parameter is still
`before_descriptor_commit` (`socket.rs:334`) and it no longer runs before a descriptor commit
— the commit is a rename now, and the hook moved to `socket.rs:381`, after the `fchmod` and
validation. `before_rename_commit` would say what it does. (`retire_instance_record_with_hook`'s
`after_descriptor_identity` is still accurate.)

**13. Three error-message changes are unlisted in the commit message.**
`socket.rs:311` ("daemon instance tombstone changed while opening it" → "…staging file…"),
`socket.rs:377` ("destination changed while publishing it" → "staging file changed…"), and
`socket.rs:602` ("socket changed during cleanup" → "runtime node changed during cleanup").
I grepped the workspace: no test or doc asserts on any of the old strings, so nothing breaks.
The third one is user-visible diagnostic text on a shared helper, and this project treats
diagnostics as load-bearing.

**14. `crates/gascand/src/socket.rs:951` — a now-unreachable arm in an assertion.**
`matches!(observed, None | Some((super::INSTANCE_TOMBSTONE_MODE, 0)))`. Because
`clear_inert_destination` runs first, the destination is *always* absent at commit time, so
the `reused` iteration of the loop observes `None` exactly like `fresh`. The assertion is
correct and the second iteration still earns its keep (it proves a pre-existing tombstone
does not block publication), but the tombstone arm can no longer fire and the reader will
spend time working out when it could.

**15. `crates/gascan/src/daemon.rs:4873–4875` — a derived claim written as an observation.**
"Chmod-ing last still showed content at the published path for the length of an `fsync`." True
and checkable from `a8c37a3:crates/gascand/src/socket.rs`, but written in the past tense
without the anchor. One SHA would settle it. (The pre-existing "MEASURED: this test failed
exactly that way on a `macos-26` runner" at `:4863–4866` is unchanged by this commit.)

---

## The duplicated constant (question 3)

`INSTANCE_TOMBSTONE_MODE = 0o200` at `crates/gascand/src/socket.rs:16` and
`crates/gascan/src/daemon.rs:18` is not the whole of it. **Six** protocol values are declared
independently in both crates:

| value | gascand | gascan |
|---|---|---|
| `DIRECTORY_MODE` 0o700 | `socket.rs:14` | `daemon.rs:16` |
| 0o600 | `SOCKET_MODE` `socket.rs:15` | `FILE_MODE` `daemon.rs:17` |
| `INSTANCE_TOMBSTONE_MODE` 0o200 | `socket.rs:16` | `daemon.rs:18` |
| `SOCKET_NAME` | `socket.rs:17` | `daemon.rs:19` |
| `INSTANCE_NAME` | `socket.rs:18` | `daemon.rs:20` |
| `LIFECYCLE_LOCK_NAME` | `socket.rs:19` | `daemon.rs:21` |

Note that 0o600 does not even share a *name* across the two crates, which is how a duplicate
survives review: nothing greps it up.

**Does this change make the drift risk better or worse? Worse, mildly.** Before, the coupling
was "these two crates agree on some mode bits". Now there is an additional, stronger, and
entirely unwritten-down rule: *the instance path shows exactly three faces — absent,
(0200, 0), and (0600, len>0)*. That rule is asserted by a test in `gascand`
(`socket.rs:913–914` hardcodes `0o600` and `super::INSTANCE_TOMBSTONE_MODE`) and consumed by
a classifier in `gascan` (`daemon.rs:2834`, `2759`, `3210`). Nothing mechanically connects
them. If someone changed `gascan`'s `INSTANCE_TOMBSTONE_MODE`, `gascand`'s test would still
pass and the classification would silently break — which is precisely the failure this commit
spent five workspace runs diagnosing.

**Recommendation (not in scope for this commit):** `crates/gascan-core` is already a path
dependency of *both* crates (`crates/gascan/Cargo.toml:15`, `crates/gascand/Cargo.toml:10`),
so there is a shared home and adding it costs no new crate. Move the six values into a
`gascan-core::instance_protocol` module — with the three-face rule as its module doc — and
have both crates import them. That converts a convention into a compile-time fact. It should
be its own commit; folding it into this one would bury a good fix under a cross-crate move.

---

## Scope (question 4)

**Fixing retirement was in scope and correctly justified.** The assignment was "the
publication race"; the defect is "the instance path shows a state that `gascan` reads as a
corpse". The implementer measured that publication-only left 6,812 occurrences and that all
of them came from `Drop`. Shipping publication alone would have left the defect reachable on
every daemon stop, which is not a fixed defect — it is a halved one. The bound is also right:
the change stops at the two functions that write the path and does not touch the reader, the
reclaim path, or the socket protocol.

**What is left half-done, plainly:** the reader-side windows in Issue 5, and
`retire_held_record` (`crates/gascan/src/daemon.rs:1453`) in Issue 1. All pre-existing except
the new `ENOENT` variant, all in this defect's family, none of them things I would hold this
commit for — but they should be *named* in `START-HERE.md` rather than left for the next
session to rediscover, because the current text implies the family is closed once open item 1
is done.

---

## Design alternatives, argued (question 1)

**`RENAME_EXCHANGE` / `RENAME_SWAP` for retirement — no.** It buys atomicity that plain
`renameat` already has, requires the destination to exist (needing a fallback path for the
case where it does not, which this project's rules forbid), and gives no identity guarantee
at all — the swap window in Issue 2 would be identical. Strictly worse than what shipped.

**Keeping the tombstone's inode — no.** That is what the old code did, and it is the whole
defect. Any scheme that edits the file at the destination in more than one step is the bug.

**Unlink-based retirement — better than what shipped, and I would take it.** See
Recommendation 1.

---

## Recommendations

**1. Retire by identity-checked unlink instead of by staged rename.** Replace the whole
`if identity_at(...) == expected` branch at `socket.rs:242–263` with
`remove_named_identity(directory, name, expected, FileType::RegularFile, "retired")`. This:

- closes the clobber window in Issue 2 (`remove_named_identity` renames to a quarantine name
  with `NOREPLACE` and re-proves identity before unlinking — it is the guarantee the plain
  `renameat` lacks);
- removes the new allocation-failure mode in Issue 3;
- deletes the staging file, the second `StagingGuard`, and the "rejected-retirement" path;
- is DRY — it reuses the helper this commit just generalised, and that generalisation
  (`kind: FileType`) was made for exactly this shape of caller.

The cost is that the path goes *absent* instead of showing a tombstone. **The reader already
treats those identically**: `daemon.rs:2789–2791` returns `Ok(None)` for `ENOENT` and
`daemon.rs:2792–2796` returns `Ok(None)` for the tombstone, and I grepped `crates/gascan-e2e`
— nothing there depends on the instance tombstone. So the only consumers of the tombstone's
existence are `gascand`'s own test (`socket.rs:1043`) and a human reading the runtime
directory. **Caveat I could not settle from the tree:** I did not find the original rationale
for the tombstone, and it may carry a purpose I have not seen. If it is kept, its remaining
purpose should be written into `socket.rs:213–218` — because with rename-based publication
the reason it *used* to exist (being the inode the publisher wrote into) is gone, and a
guarantee nobody can state is a guarantee nobody can maintain.

**2. Make `stage_inert_instance_file` return a self-guarding value.** Today it returns
`(File, String, Identity)` and every caller must remember to build a `StagingGuard`
immediately — the invariant "a staged file is always guarded" is convention, not structure,
and `StagingGuard`'s `name: &'a str` forces the caller to keep the `String` alive
specifically to satisfy it. A `StagedFile` owning the file, name, identity and armed flag,
with `commit(dest)` and `Drop`, would make the invariant unforgeable and remove the
`disarm(); drop(guard);` dance from both call sites. This is the leaky-abstraction answer to
question 2; everything else in the factoring is clean, and sharing the helper between
publication and retirement is right.

**3. Fix the comment in Issue 1 before merge.** It is the one place where the record is wrong
rather than merely unanchored, and this project's whole review culture rests on the comments
being true.

**4. Decide, and write down, whether the reader gets a retryable verdict.** Issue 5's three
windows all have the same root: `crates/gascan/src/daemon.rs` has exactly two outcomes for a
changed observation — terminal `PermissionDenied`, or `Unsafe`. That was defensible when
"changed" meant "tampered". It is less defensible now that "changed" also means "a daemon
started or stopped between two of my syscalls". Either add a bounded re-`stat`-and-reclassify
for the tombstone branch, or record in `START-HERE.md` that these windows are known,
narrowed, and deliberately terminal. Do not leave it implicit.

**5. Land the 2000-cycle observer as an `#[ignore]`d test** (Issue 8), so the numbers in four
different comments have a reproducible anchor.

---

## Documentation (question 7)

**`docs/status/START-HERE.md` must change; it is the only doc that must.**

- **Lines 125–132, "THE ONE THING TO DO NEXT"** — currently "Fix the daemon instance record's
  publish race — open item 1 below." This is now done and it is the *first* thing the next
  session reads. It must name whatever is actually next, or say the branch is open for review.
- **Lines 136–160, open item 1** — should move to the "what was fixed" record with: the SHA
  `3306491`; that **both** publication and retirement were fixed, and why retirement was in
  scope (6,812 occurrences with publication alone); that the fix is stage-and-rename, matching
  `SocketPaths::bind`; that `validate_file_stat`'s false premise at `daemon.rs:3187` is
  corrected in place; and the mutation evidence. It should **not** claim the family is closed
  — it should carry forward, as new open items or as a note: the reader-side windows at
  `daemon.rs:2842` and `daemon.rs:2611`, and `retire_held_record` at `daemon.rs:1453` as the
  last in-tree producer of 0200-with-content.
- **Lines 684–689, the four-run flake table** — two of the four now have an attributed and
  fixed cause, and saying so is the point of the table. Run 1 (`mode is 0200 … written but
  never published`) is the defect directly. Run 2 (`interrupted daemon instance descriptor
  changed while opening it`, which is `daemon.rs:2732`) is reachable **only** when the path is
  0200-with-content, so it is the same defect seen through `open_interrupted_tombstone` and it
  is closed by making that state unreachable. Runs 3 and 4 remain unattributed — say so; do
  not let this fix absorb credit it has not earned.
- **Line 254** — "**The production publisher has the same shape** — see open item 2". Two
  errors: the shape is no longer the same, and the cross-reference points at item 2 when the
  publish race is item 1. Fix both.

**Nothing else in `docs/` needs editing.** `docs/status/next-session-kickoff.md` is stamped
"SUPERSEDED 2026-08-11. DO NOT FOLLOW THIS DOCUMENT'S INSTRUCTIONS." at line 3, so its D7
paragraph at :483 is inert. `docs/status/arca-integration-handoff.md` is a dated
append-only session log (header "Date: 2026-08-04") whose D7 rows at :1910 and :2117 are
records of what was known then and should not be rewritten — though if the convention is to
append, that ledger's D7 entry ("remedy is a design choice") is the one this commit resolves.
`docs/release/macos-checklist.md` and `docs/status/review-whole-diff.md` contain no `0200`
references.

---

## Assessment

**Ready to merge? With fixes.**

The design is right, it reuses a shape already proven in the same file, `NOREPLACE` is a
stronger guarantee than the descriptor recheck it replaced, the `kind: FileType`
generalisation does not weaken the socket path, and neither rewritten test was softened to
pass. Nothing here is Critical. The two things I would fix before merge are cheap and both are
about the record rather than the code: the false reachability claim at
`crates/gascan/src/daemon.rs:3205–3206`, which points the next reader away from
`retire_held_record` at `daemon.rs:1453`, and `docs/status/START-HERE.md`, which still tells
the next session to do this work and still calls the family open. The retirement-by-unlink
simplification (Recommendation 1) is the one design change I would genuinely argue for, and it
can follow.
