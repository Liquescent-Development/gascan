# Review — `8613f22..93c77fe` (`fix/daemon-reader-retryable-verdict`)

Scope: `crates/gascan/src/daemon.rs` + `docs/status/START-HERE.md`, one commit. Commits after
`93c77fe` (`cfcfb62`, `ec47492`, `0943cd0`) are docs-only and out of scope; `daemon.rs` is
byte-identical between `93c77fe` and the current `HEAD` (`diff <(git show 93c77fe:…) …` empty), so
everything below was measured against the reviewed code.

**Verdict: request changes.** The Critical is genuinely fixed and genuinely held — reverting
`open_published_record`'s `openat` to `.map_err(errno)` fails
`every_unsafe_observation_across_a_real_stop_transition_is_marked_raced` **3/3**. The widened
predicate is not too wide in any way I could construct that costs a correct verdict, and nothing
retries forever. What is wrong is smaller and specific: one of the five fixes does not do what its
own rationale says it does (measured), the classifier's doc comment now contradicts the classifier
in three places, the raced arms throw away the evidence the commit argues is load-bearing, and a
branch this commit *added* is untested in both directions and missing from the doc's uncovered
list.

**No Critical findings.** I attacked the final predicate, the `ENOENT` split, the hook refactor and
the staging cleanup, and none of them admits a wrong verdict.

---

## Important

### I1 — The `ReadHooks` refactor does not close the gap it was made for. MEASURED.

**Where:** `crates/gascan/src/daemon.rs:3533-3537` (the destructure), with
`crates/gascan/src/daemon.rs:7273-7302` and `crates/gascan/src/daemon.rs:7313-7341` (the two window
tests).

**What is wrong:** the commit message says "*A test aiming at the wrong window still compiled and
still passed … Fields cannot be swapped silently.*" That is true only of the **call site**. The
injection points are still assigned positionally one layer in, at the destructure:

```rust
let ReadHooks {
    between_identity_and_open,
    between_read_and_recheck,
    before_tombstone_validation,
} = hooks;
```

and the two tests that own those windows both assert nothing but `is_raced(&error)`, with the
*identical* message string (`"a publication committing inside the reader's window is a race, not a
fault: {error}"`). Neither pins which window it fired in.

**Failure scenario, run rather than reasoned:** swapping the two field bindings in that destructure
— i.e. relocating both injection points so that
`a_record_republished_before_the_reader_opens_it` actually exercises the read/recheck window and
vice versa — leaves `cargo test -p gascan --lib` at **324 passed, 0 failed, 3 runs out of 3**.
(A fourth run failed on `every_interrupted_tombstone_failure_across_a_concurrent_reclaim_is_marked_raced`,
which is the unrelated flake in M-Minor-2 below; it does not reproduce.) The precondition the brief
named — "a silently relocated hook would make several existing tests test the wrong window while
still passing" — is still true after this commit.

**Fix (two lines, no new test):** the two windows already produce distinct details —
`"daemon instance record changed while opening it"` (daemon.rs:3577) and
`"daemon instance record changed while reading it"` (daemon.rs:3603). Have each test assert its own
string instead of only `is_raced`. That makes a relocated injection point fail loudly, which is the
whole claim.

**Confidence: high** (measured, 3/3).

---

### I2 — `classify_unreadable_instance_record`'s doc comment now contradicts its body, three ways.

**Where:** `crates/gascan/src/daemon.rs:3622-3640`, against the body at
`crates/gascan/src/daemon.rs:3654` and `:3660-3688`, and against the caller at
`crates/gascan/src/daemon.rs:3320-3331`.

**What is wrong:**

1. **`:3638` — "Only `EACCES` is split. Every other errno stays terminal, which is the direction
   that fails closed."** False as of this commit. `ENOENT` is split at `:3654`, unconditionally,
   *before* the `EACCES` test. The new table test's own comment says the opposite of the doc
   ("*Only the two errnos a commit in flight produces are split*", daemon.rs:7801), so the file now
   states both rules.
2. **`:3623` — "this read opens `O_RDONLY`".** One of the two callers does; the other,
   `open_published_record`, opens `OFlags::RDWR` (`:3325`). The whole `EACCES`-against-0200 argument
   is unchanged by that, but the sentence is now wrong about the function's own callers, which is
   the fact a reader would use to reason about which errnos are reachable.
3. **`:3630-3632` — "if it now names an inert tombstone … and if it has gone entirely".** That is
   the *pre-fix* admission set. The predicate now admits every face `validate_file_stat` calls `Ok`
   (any 0600 / uid-matching / `nlink == 1` regular file, **of any size**) plus every fault
   `StatFault::is_transitional_for` calls a transition (`Unlinked` — `nlink == 0` at *any* mode and
   *any* size — and `InertTombstone`).

**Consequence:** this is the fail-closed boundary of the reader, and the docblock above it is the
first thing anyone auditing that boundary reads. It currently describes a strictly narrower rule
than the code holds. It is also the same defect class this commit fixed one screen up — the commit
replaced `inspect_with`'s stale "five of them" list precisely because a list in a doc comment goes
stale — and re-introduced it in the function that list was pointing at.

**Fix:** rewrite `:3628-3640` to state the predicate the code holds, and delete or correct
"Only `EACCES` is split" / "`O_RDONLY`".

**Confidence: high** (read directly against the body).

---

### I3 — The raced arms drop the evidence, and they are the arms that survive to a human.

**Where:** `crates/gascan/src/daemon.rs:3661-3671` (both `raced(...)` arms), against
`crates/gascan/src/daemon.rs:3672-3681` (the terminal arm) and
`crates/gascan/src/daemon.rs:1180-1183` (`retry_while_raced`'s give-up detail).

**What is wrong:** the commit message argues *"The terminal arms of the classifier now carry mode,
size, links and uid, for the reason `validate_file_stat`'s doc already gives: a bare 'Permission
denied' made a CI failure unattributable, and this is the arm that survives to a human."* That is
backwards for the case that actually reaches a human. A race that settles produces no message at
all; the message an operator sees is built by `retry_while_raced` when the path *never* settles, and
it is built **from the raced detail**:

```rust
let status_detail = format!(
    "the daemon record was still changing after {observations} observations: {detail}"
);
```

Both raced arms carry no errno and no stat evidence. The `Ok(())` arm carries only
`"a publication committed over the daemon instance record between resolving it and opening it"`.

**Failure scenario, reproduced on this machine (darwin 25.6.0):** a persistent `EACCES` on a file
whose `lstat` is a legal published record. `chmod +a "$(whoami) deny read,write"` on a 0600 file:

```
open failed: errno 13 Permission denied
mode 600 size 6 nlink 1 uid 501
```

`validate_file_stat` returns `Ok(())` for that stat, so `classify_unreadable_instance_record`
returns `raced()`. The reader then burns all three observations plus 2 × `DEFAULT_POLL` (25 ms) and
reports:

> the daemon record was still changing after 3 observations: a publication committed over the daemon
> instance record between resolving it and opening it

— naming a transition that never happened and discarding the `EACCES` entirely. Before this commit
the same state produced a terminal `Unsafe` carrying `Permission denied (os error 13)` on the first
look. The verdict class is unchanged (`Unsafe` either way, and it is bounded — see "checked and
sound" below), so this is a diagnosability regression, not a correctness one. After a spawn it also
costs time: `ensure_started_locked_with_hook`'s `raced.is_some()` arm
(`crates/gascan/src/daemon.rs:1666-1674`) makes the readiness loop poll until the full readiness
deadline before returning the same misleading detail, where it previously failed on the first look.

The trigger is exotic (a same-uid deny-ACL on macOS, an LSM denial on Linux) and same-uid is not a
privilege boundary in this threat model — which is why this is Important, not Critical. But the fix
is four lines and it restores the property the commit is arguing for.

**Fix:** format both raced details the way the terminal arm is formatted — include `errno(error)`
and the mode/size/links/uid the `Err(fault)` arm already carries.

**Confidence: high on the mechanism** (the `EACCES` + legal-stat state is reproduced above);
**medium on how often it is hit in the field.**

---

## Minor

### M1 — The staging cleanup guard added by this commit is untested in *both* directions, and is not on the doc's uncovered list.

**Where:** `crates/gascan/src/daemon.rs:1846-1859`; `docs/status/START-HERE.md:506`.

The logic itself is **correct**: `is_ok_and` is the right way round (unlink only when the name still
resolves to our inode), the no-unlink direction is the safe one, and the only leak it introduces is
an empty `.reclaim-` file that `sweep_abandoned_staging` collects — which the comment says and which
holds.

What is wrong is that nothing holds it. Two mutations, each `cargo test -p gascan --lib`:

- **Delete the guard entirely**, restoring the bare `unlinkat(directory, staging.as_str(), …)` that
  finding 4 exists to remove: **324 passed, 0 failed.**
- **Invert it** to `!raw_identity_at(...).is_ok_and(|at_name| at_name == ours)` — unlink precisely
  when the name is a *stranger's* file and leak our own: **324 passed, 0 failed.**

So the defect the commit fixed can be reintroduced, and the guard can be flipped into the exact
behaviour it was written to prevent, with the suite green. `docs/status/START-HERE.md:506` says
"**it is three defensive branches**" and lists `stage_inert_reclaim_file`'s staging-name comparison
among them, but not the cleanup guard this commit added beside it. The count is four (five, with M3).

I did separately confirm the doc's own MEASURED claim about the staging-name check: deleting the
`raw_identity_at(...)? != identity` comparison at `:1836-1841` leaves `cargo test -p gascan --lib`
green (324 passed). That claim holds.

**Confidence: high** (measured, both directions).

---

### M2 — `cargo test -p gascan --lib` is not reliably green at `93c77fe`; the commit's five-green-runs claim did not reproduce. Not attributable to this diff.

**Where:** `crates/gascan/src/daemon.rs:6060`,
`start_readiness_waits_for_its_own_connected_publication_to_finish`.

The commit message states *"`cargo test -p gascan --lib` run five times consecutively, 324 passed
each."* Running exactly that command here: **2 failures in 6 runs**, both this test, both
`test result: FAILED. 323 passed; 1 failed`. It failed twice more during unrelated mutation runs on
an unmutated tree.

The mechanism is a wall-clock bound, same class as the eighth mechanism recorded in `0943cd0`: the
test gives `start_with` `readiness: Duration::from_millis(200)` with `poll: 1ms`
(`crates/gascan/src/daemon.rs:6084-6088`), so a loaded machine misses the budget and `outcome?`
returns `SupervisorError::Readiness`. It is not currently in `docs/` — `grep -rn
start_readiness_waits_for_its_own_connected_publication_to_finish docs/` is empty — so it is a ninth
mechanism, and it lives in `gascan --lib`, which `0943cd0` calls "the signal that means something on
this branch right now".

**I could not attribute it to this diff, and I tried.** At the parent `8613f22` in a throwaway
worktree the same command failed **1/6** (`inherited_startup_diagnostic_survives_path_replacement`),
and an interleaved paired run of 8 HEAD / 8 parent under four spinning `yes` processes gave
**HEAD 0/8, parent 1/8** on this same test. A later unloaded 10-run sweep at HEAD gave **0/10**. The
rate is load-dependent and present on both sides. Report it as a pre-existing flake; the finding is
only that "324 passed each of five runs" should not be carried into a durable doc without the load
conditions attached.

**Confidence: high on the observation, high that it is not a regression from this diff.**

---

### M3 — `open_published_record`'s recheck-`statat` `ENOENT` split is a fifth uncovered defensive branch.

**Where:** `crates/gascan/src/daemon.rs:3353-3359`.

Reverting it to `.map_err(errno)?` leaves the suite green (2/3 runs; the one failure was M2's flake,
which does not reproduce on that mutation). This is the same class as the three the doc already
lists — the window is a rename wide and no producer in the suite lands in it — but it was added by
this commit and is not recorded. Add it to the uncovered list beside M1's guard rather than leaving
the doc's count at three.

**Confidence: high** (measured).

---

### M4 — `classify_unreadable_instance_record` hardcodes `GuardedFile::InstanceRecord` where the file's own convention is to take it as a parameter.

**Where:** `crates/gascan/src/daemon.rs:3641-3646` and `:3660`, against
`crates/gascan/src/daemon.rs:4131-4136`.

`file_identity_at` performs the *identical* `ENOENT`-to-`raced` split but gates it on
`matches!(guarded, GuardedFile::InstanceRecord)`, with a comment saying why: nothing renames the
lifecycle lock, so the lock must keep the terminal verdict. The new split at `:3654` has no such
gate, and the `validate_file_stat` call at `:3660` hardcodes `GuardedFile::InstanceRecord`. Nothing
is wrong today — both call sites are instance-record opens — but a lock-guarding caller added later
inherits the record's retry classification silently, which is exactly the accidental widening the
`GuardedFile` parameter's own doc comment (`:4235-4244`) says the parameter exists to prevent.
Threading `guarded` through costs two lines and makes the fail-closed default a compile error to
skip.

**Confidence: medium** (forward-looking; no present defect).

---

## Checked and found sound

Stated so the review is legible about what was actually attacked.

- **The Critical is fixed and held.** Reverting `open_published_record`'s `openat` to
  `.map_err(errno)?` fails `every_unsafe_observation_across_a_real_stop_transition_is_marked_raced`
  **3 runs out of 3**, at the committed 4096 observations. The commit's claim here reproduces.
- **Every decision branch of the widened classifier is held by the table test.** Six mutations, each
  failing `the_unreadable_record_classifier_admits_only_faces_in_motion`: drop the `ENOENT` early
  return; drop the non-`EACCES` passthrough; `Ok(())` → terminal; `Err(transitional)` → terminal;
  `Err(fault)` → `raced` (the dangerous widening); recheck-`ENOENT` → terminal. Only the
  `Err(other)` arm (recheck `statat` fails with something other than `ENOENT`) is unheld, and it is
  defensive.
- **The final predicate is not too wide in any way that costs a verdict.** `validate_file_stat`
  returns `Ok(())` only for a regular file, `st_uid == expected_uid`, `st_nlink == 1`,
  `mode == 0600`; size is unchecked but size is not a legal-face discriminator at 0600. The illegal
  fourth face (0200 with content) stays terminal, and the table test pins it. The one state that
  reads as transient while being permanent is I3's, and it is bounded.
- **Nothing retries forever.** `retry_while_raced` is capped at `OBSERVATIONS = 3`
  (`crates/gascan/src/daemon.rs:1131`), and the readiness loop wraps each inspection in
  `tokio::time::timeout_at(deadline, …)` (`:1566`), so both the extra retries and the extra polls
  are bounded. Worst-case added latency for a `gascan status` is 2 × 25 ms.
- **No caller's `ErrorKind` control flow moved unintentionally.** The only arm at risk is
  `read_instance_record_for_inspection`'s `NotFound => Ok(None)` (`:3304`), and moving it is the
  intended fix (a mid-transition record stops reading as a confident "stopped"). The only other
  consumer of the record read is `read_attested_instance` (`:965`), which has no non-test production
  callers. `observe_once_with_hook` never matches on `ErrorKind`. The paths that legitimately mean
  "no daemon" — the directory being absent (`:3541`) and the initial `statat` returning `ENOENT`
  (`:3543`) — are untouched and still return `Ok(None)` directly.
- **The new observation test's premise holds.** With `EndpointProbe::AbsentOrInert` and
  `MutableInspector::new(None)`, both legitimate outcomes route to
  `classify_unreachable` → `DaemonState::Stopped` (`:3093` for `record: None`, `:3105` for
  `Ok(None)` from the inspector). `is_interrupted_tombstone` requires `st_size > 0` (`:3480`), so
  the producer's inert 0200 tombstone never enters the interrupted-tombstone branch. Empirically:
  raising the loop to 40,000 observations gives **3 clean runs out of 3**, which reproduces the
  commit's sweep claim.
- **No hook injection point moved.** The destructure order at `:3533-3537` and the three call sites
  (`:3572` `between_identity_and_open`, `:3598` `between_read_and_recheck`, `:3552`
  `before_tombstone_validation`) are identical to the pre-refactor positional bodies, and every
  migrated test call site maps position→field correctly. (That the suite cannot *detect* a move is
  I1.)
- `is_raced` is type-based (`error.get_ref().is::<RacedObservation>()`, `:3903`), not a string
  match, so the nested `raced(&format!("…: {moving}"))` in the transitional arm cannot leak a marker
  into a terminal message.
- `cargo fmt --all --check` exit 0 and `cargo clippy -p gascan --all-targets -- -D warnings` clean
  at the reviewed tree.

## Tree state

All mutations were applied from a pristine copy and reverted. `git status --porcelain` is empty; the
throwaway worktree at the parent commit was removed and `git worktree prune` run.
