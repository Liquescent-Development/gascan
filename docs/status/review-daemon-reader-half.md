# Review — `bf107a1..8613f22` on `fix/daemon-reader-retryable-verdict`

Reviewer: `code-reviewer` subagent. Repo `/Users/kiener/code/gascan`, worktree clean at start and at finish (`git status --short` empty both times).

**Verdict: request_changes.** The classification work in the diff is sound — I could not break the fail-closed default at any of the new sites, and every classification decision I mutated was caught by a test. But the branch does not achieve the thing it says it achieves. **On the production `gascan status` path, an ordinary stop transition still produces terminal `Unsafe` verdicts, at roughly half of all `Unsafe` observations, from a window this diff left unclassified** — and the docs commit replaced the honest residue list with a "THE READER HALF IS COMPLETE" claim that omits it.

Everything below is measured unless marked otherwise. All measurements are on this machine, `cargo test -p gascan --lib`, tree at `8613f22`.

---

## Baseline

- `cargo test -p gascan --lib` → **323 passed, 0 failed** (5.32s).
- `cargo fmt --all --check` → exit 0.
- `cargo clippy -p gascan --all-targets -- -D warnings` → no output, exit 0.

---

## Critical

### C1. `open_published_record`'s `openat` is unclassified, so the reader still returns a terminal `Unsafe` for a plain stop — on the production path

**Location:** `crates/gascan/src/daemon.rs:3263-3273` (the `openat` in `open_published_record`), consumed at `crates/gascan/src/daemon.rs:1322-1372` (`observe_once_with_hook`'s `published_record` error arms).

**What is wrong.** `observe_once_with_hook` calls `open_published_record` whenever the record read returned `Ok(Some(record))` (`daemon.rs:1231-1234`). `open_published_record` opens the instance name `O_RDWR`. A retirement committing between the record read finishing and that `openat` puts a 0200 inert tombstone at the name, and `O_RDWR` against 0200 is refused `EACCES`. That error is `.map_err(errno)?` — **unclassified**. It flows to every one of the three `published_record` error arms, each of which builds `DaemonState::Unsafe` with `raced: race_marker(&error)` — `None` — so `retry_while_raced` returns it **immediately, on the first observation, with no retry at all**.

The unlink variant is the same: if the record is removed instead, that `openat` returns `ENOENT` → `io::ErrorKind::NotFound` → `race_marker` `None` → terminal `Unsafe`.

This is exactly the failure the branch exists to eliminate, on exactly the transition the branch names, and it is not covered by any test in the diff because the loop test `every_reader_failure_across_a_real_stop_transition_is_marked_raced` (`daemon.rs:7016`) exercises only `read_instance_record_for_inspection` — a strict subset of what one production observation does.

**Failure scenario.** `gascan status` (→ `daemon::inspect` at `daemon.rs:2592` → `inspect_with` → `observe::inspect_with_hook` → `observe_once_with_hook`) samples the record while `gascand` is retiring. Record read succeeds. Retirement's `renameat` commits. `open_published_record`'s `openat` → `EACCES`. Verdict: `DaemonState::Unsafe`, detail `"Permission denied (os error 13)"`. A healthy stop is reported to the user as the same state class as a symlink attack, with a detail that names neither the file nor the mode.

**Measurement.** I drove `observe_once_with_hook` — the production observation, not the record read — against a real publish/retire producer, with `MutableEndpoint::new(EndpointProbe::AbsentOrInert)` and `MutableInspector::new(None)`, 20 000 observations. Temporary probe, not retained; tree restored and verified clean.

Run 1 (unmodified tree):

```
STATES: {"Stopped": 10990, "Unsafe": 9010}
MARKED UNSAFE: 4554
UNMARKED UNSAFE: { "Permission denied (os error 13)": 4456 }
```

Run 2, with `open_published_record`'s `openat` tagged `.map_err(|error| io::Error::other(format!("OPR-OPENAT {}", errno(error))))` to attribute it:

```
STATES: {"Stopped": 9128, "Unsafe": 10872}
MARKED UNSAFE: 4895
UNMARKED UNSAFE: { "OPR-OPENAT Permission denied (os error 13)": 5977 }
```

**100% of the unmarked terminal `Unsafe` verdicts come from that one `openat`**, and they are ~55% of all `Unsafe` observations in the run.

Two targeted probes confirm both errnos in isolation:

- publish a record, commit a tombstone over it, then `open_published_record` → `kind=PermissionDenied raced=false msg=Permission denied (os error 13)`
- publish a record, `remove_file`, then `open_published_record` → `kind=NotFound raced=false msg=No such file or directory (os error 2)`

**On "is this pre-existing".** The `openat` line itself is untouched by the diff, and the removed `KNOWINGLY LEFT` block in `docs/status/START-HERE.md` did note that `open_published_record`'s marks "sit *after* its `openat`". But the diff replaces that honest residue with `THE READER HALF IS COMPLETE AS OF ae03597` and a `WHAT IS STILL NOT COVERED` list that contains only three unreachable defensive branches. The record of the gap was deleted and the gap was not closed. That makes the state materially worse than `bf107a1`, and it is squarely inside this diff's declared scope.

**Fix.** Classify that `openat` with the same evidence-based helper the record read uses. `classify_unreadable_instance_record(error, &directory, name, paths.expected_uid)` already handles `EACCES`-then-restat and is directly reusable; extend it (or add a sibling) to also split `ENOENT` for this call site, since a name that resolved during the record read and no longer resolves here is a successor's commit between two of this reader's own looks — the identical argument `file_identity_at`'s new `ENOENT` split already makes at `daemon.rs:3985-4004`. Then add a loop test that drives `observe_once_with_hook` (not `read_instance_record_for_inspection`) against the publish/retire producer and asserts every `Unsafe` carries a marker; that test is what would have caught this, and the probe above shows it fires within a few hundred iterations.

**Confidence: high.** Direct measurement on the production observation function, attributed to a single line.

---

## Important

### I1. `inspect_with`'s doc comment now states the opposite of what the code does

**Location:** `crates/gascan/src/daemon.rs:1017-1023`.

Unchanged by the diff, and now false in every clause:

> So a race-shaped failure is looked at again rather than believed -- **five of them**: the three in `validate_instance_tombstone` and the two in `open_published_record`. The other "changed while ..." failures in this file **are still terminal**, as are the **`validate_file_stat` faults** and the **`EACCES` that a 0200 path returns** -- which is what the common published-to-inert transition reaches instead of `open_published_record`'s two marks.

After this diff: there are far more than five; the record read's two "changed while …" failures are `raced()` (`daemon.rs:3498`, `daemon.rs:3527`); `open_interrupted_tombstone`'s two are `raced()` (`daemon.rs:3386`, `daemon.rs:3399`); `validate_open_file`'s is `raced()` for `InstanceRecord` (`daemon.rs:3972`); two `validate_file_stat` faults are retryable (`daemon.rs:4186-4189`); and the `EACCES` is split (`daemon.rs:3559-3579`).

This is the doc comment on `inspect_with` — the crate-visible entry point, and the first thing anyone reasoning about the retry semantics reads. It also points at `docs/status/START-HERE.md` "records that residue", which no longer records it.

**Fix.** Rewrite the paragraph against `StatFault::is_transitional_for` and the `raced(` call sites, or delete it and point at `GuardedFile` / `StatFault`. **Confidence: high** (read against the code).

### I2. `docs/status/START-HERE.md` claims a test holds the staging-name fix; measured false, and the same section contradicts itself

**Location:** `docs/status/START-HERE.md`, the replacement block (visible in the docs diff as the `The other four minors are fixed…` paragraph and the `WHAT IS STILL NOT COVERED BY A DRIVING TEST` paragraph).

The first says:

> **The other four minors are fixed, each held by a test that fails when the fix is reverted:** `stage_inert_reclaim_file` now performs the staging-name check …

The second, ~10 lines later, lists that same check as one of three things **not** covered by a driving test.

**Measured:** deleting the `raw_identity_at` comparison at `daemon.rs:1817-1823` → `cargo test -p gascan --lib` → **322 passed, 0 failed**. Nothing holds it. The "each held by a test" claim is wrong for one of the four.

For contrast, the other three minors *are* held, each by exactly one test (each mutation run once, tree restored after each):

| mutation | tests that failed |
|---|---|
| `raced: race_marker(&error).map(\|_\| composed)` → `race_marker(&error)` | `a_terminal_endpoint_fault_survives_into_the_race_marker` |
| readiness loop passes `DEFAULT_POLL` instead of `timeouts.poll` | `the_retry_waits_the_caller_s_poll_rather_than_the_constant` |
| `inspect_with` calls `observe::observe_once_with_hook` directly | `cargo build -p gascan` → **E0425** (see M1) |

**Fix.** Drop the staging-name check from the "held by a test" list; it already appears correctly in the uncovered list. Under the repo's own rule that durable past-tense claims carry an anchor, a claim measured false is worse than a claim left bare. **Confidence: high** (measured).

### I3. All three loop tests can pass while observing zero failures

**Locations:** `daemon.rs:7016` (`every_reader_failure_across_a_real_stop_transition_is_marked_raced`), `daemon.rs:7180` (`every_interrupted_tombstone_failure_across_a_concurrent_reclaim_is_marked_raced`), `daemon.rs:7378` (`every_tombstone_failure_across_a_concurrent_unlink_is_marked_raced`).

Each collects unmarked failures into a `BTreeSet` and asserts it is empty. Each then adds a coverage guard — but the guards count *successes*, not failures:

- `published > 0 && stopped > 0`
- `found > 0 && absent > 0`
- `absent > 0`

A reader that crosses the transition cleanly every time — one that never tears — satisfies every guard and asserts an empty set against an empty sample. The docstrings say the guard "is what stops it passing vacuously", but it only rules out the reader never running; it does not establish that the classification was exercised at all.

Today this is latent, not live: I ran the classification mutations (below) and each was caught. But the tests are threaded races with `yield_now`, and the branch's own history records this workspace's suite wandering under load. A CI runner that starves the reader thread turns these from a proof into a green tick.

**Measured, each mutation run once, tree restored after each:**

| mutation | tests that failed |
|---|---|
| `is_transitional_for` drops `Unlinked` | `every_reader_failure_across_a_real_stop_transition_is_marked_raced`, `the_instance_record_treats_only_its_two_transitional_faces_as_races` |
| `is_transitional_for` drops `InertTombstone` | same two |
| record read's `openat` → `.map_err(errno)` (drops `classify_unreadable_instance_record`) | `a_record_retired_between_resolving_its_identity_and_opening_it_is_raced`, `every_reader_failure_across_a_real_stop_transition_is_marked_raced` |
| `file_identity_at` drops the `ENOENT` split | `every_tombstone_failure_across_a_concurrent_unlink_is_marked_raced` |
| `validate_instance_tombstone` recheck drops the `ENOENT` split | `every_tombstone_failure_across_a_concurrent_unlink_is_marked_raced` |

**Fix.** Count marked races alongside the successes and assert `marked > 0` in each of the three. One line per test, and it converts the guard from "the reader ran" to "the reader tore, and every tear was classified". **Confidence: high** (structural, read from the assertions).

---

## Minor

### M1. `observe::sealed` is enforced only by a non-`cfg(test)` compilation

**Location:** `daemon.rs:1067-1074`.

`#[cfg(test)] pub(super) use sealed::{observe_once_with_hook, retry_while_raced};` makes the bypass nameable inside `crate::daemon` whenever `cfg(test)` is on. **Measured:** rewriting `inspect_with`'s body to `observe::observe_once_with_hook(paths, expected_executable, endpoint, inspector, || Ok(())).await`:

- `cargo build -p gascan` → `error[E0425]: cannot find function 'observe_once_with_hook' in module 'observe'`
- `cargo test -p gascan --lib` → **322 passed, 0 failed** — the bypass compiles and the whole suite is green under the test harness.

So the seal is real but is a *build* property, not a *test* property. CI does catch it: `.github/workflows/ci.yml:54` runs `cargo clippy --workspace --all-targets -- -D warnings`, which compiles the lib target without `cfg(test)`. Worth knowing that a developer iterating with `cargo test` alone will not see it, and that the doc's "the bypass … is now a name that does not resolve" is true only outside the test build. No change required; a sentence in the module doc would close the gap between the claim and the mechanism. **Confidence: high** (measured).

### M2. Three positionally-distinguished no-op closures in one signature

**Location:** `daemon.rs:3446-3457` (`read_instance_record_with_hook_and_directory_mode`), call sites at `daemon.rs:3235`, `daemon.rs:3250-3256`, `daemon.rs:3428-3435`.

The function now takes `between_identity_and_open`, `between_read_and_recheck`, `before_tombstone_validation` and a bare `bool`. Production call sites read `(paths, || Ok(()), || Ok(()), before_tombstone_validation, false)` — three visually identical closures whose *position* is the entire semantics.

The concrete hazard: `a_record_republished_before_the_reader_opens_it_is_raced` (`daemon.rs:7113`) and `a_record_republished_before_the_reader_rechecks_it_is_raced` (`daemon.rs:7137`) differ only in which of the two positions holds `republish` and which holds `|| Ok(())`. Swapping them in either test still yields a `raced()` error — window 1 trips the `actual != expected` comparison at `daemon.rs:3496-3499`, window 2 trips the `file_identity_at` comparison at `daemon.rs:3515-3527`, both `raced()` — so the swap compiles and both tests still pass while testing the same window twice. The comment at `daemon.rs:3508-3513` argues the two windows cannot share a seam; nothing enforces that a test aims at the seam it names.

**Fix.** Replace the closure triple with a single `struct ReadHooks<'a> { between_identity_and_open, between_read_and_recheck, before_tombstone_validation }` (or an injection-point enum plus one closure), so a call site names the window it is injecting into. **Confidence: high** (structural).

### M3. The terminal `EACCES` path returns the least actionable message in the file

**Location:** `daemon.rs:3577` (`_ => errno(error)` in `classify_unreadable_instance_record`).

This is the arm the doc calls "the tampering shape" — a record chmod-ed to 0200 with content still in it, or left in some other state the reader cannot explain. It reaches the user as `Unsafe` with detail `"Permission denied (os error 13)"`: no path, no mode, no size, no uid. Compare `validate_file_stat`'s `"protected runtime file is unsafe: … (mode 0600, size 128, links 1, uid 501, expected uid 501)"`, whose doc at `daemon.rs:4040-4066` explains that a missing size field once made a CI failure unattributable. The same argument applies here with more force, because this is the arm that survives to a human.

Same for the `ENOENT` half of C1 once that is classified — do not let it fall through as a bare `errno`.

**Fix.** Build the terminal arm's error with the re-stat's fields when the `statat` succeeded (`mode`, `size`, `uid`), and say the name and that the open was refused. **Confidence: high.**

### M4. The new staging check's failure path unlinks by bare name

**Location:** `daemon.rs:1817-1830`.

The new check fires precisely when `staging` no longer resolves to the inode `openat` created. The `Err` arm at `daemon.rs:1826-1829` then does `unlinkat(directory, staging.as_str(), AtFlags::empty())` — removing whatever is at that name, which by hypothesis is not the file this function staged. `ReclaimStagingGuard` (`daemon.rs:1848-1864`) exists for exactly this and its doc says it "removes the *staging* name and only while that name still resolves to the inode it staged". The new error path takes the weaker route the guard was written to avoid.

Reachability is remote — the name comes from `reclaim_staging_name()` and the directory is 0700 — which is also why nothing tests it (measured in I2). But if the check is worth adding, its cleanup should be at least as careful as the check. **Fix:** on the identity-mismatch branch, skip the unlink (there is nothing of ours to remove) or route it through the guard's inode-checked unlink. **Confidence: medium** (reasoned; unreachable by construction on every path I traced).

---

## Things I checked and found correct

Recording these so a later reader does not redo them.

1. **No newly-`raced()` failure is a tampering signal that now escapes.** `raced` is consumed in exactly two places — `race_marker` (`daemon.rs:3783`) and `retry_while_raced`'s `raced_detail()` (`daemon.rs:1154`), plus `ensure_started_locked_with_hook`'s `inspected.raced.is_some()` arm (`daemon.rs:1637`). No safety decision short-circuits on it. `retry_while_raced` always terminates in `DaemonState::Unsafe` after `OBSERVATIONS = 3`, and the readiness arm is bounded by `timeouts.readiness`. So the widening's worst case is a bounded delay before the same `Unsafe`, not a wrong verdict — provided the state stays illegal, which is the tampering case by definition.
2. **`O_NOFOLLOW` reasoning holds at each new site.** All three new `ENOENT`/`EACCES` splits sit behind `statat(..., SYMLINK_NOFOLLOW)` or `openat(..., O_NOFOLLOW)`. A symlink at the name makes `statat` *succeed* (returning the link's own stat, which fails `FileType::RegularFile`) and makes `openat` return `ELOOP`, not `EACCES` — so neither split can be reached by substituting a symlink. `ELOOP` stays terminal at every site.
3. **`StatFault::is_transitional_for` (`daemon.rs:4186-4189`) is right, and the exclusions are implemented as reasoned.** `Unlinked` is `st_nlink == 0`, tested before `st_nlink != 1`, so `ExtraLinks` is `nlink >= 2` and stays terminal — correct, a second name is not the daemon's doing. `UnpublishedRecord` (0200 with content) stays terminal, which is the load-bearing line. `NotRegularFile`, `ForeignOwner`, `WrongMode` terminal everywhere. Both retryable faults are terminal for `LifecycleLock`. `every_stat_face` (`daemon.rs:9319`) builds the fixtures from real kernel-produced files including a genuinely unlinked held inode, and writes the `transitional` column independently of the production predicate — that is the right shape for this table.
4. **`classify_unreadable_instance_record` is TOCTOU-sound as a classifier.** It only widens on two pieces of positive evidence (`is_instance_tombstone` — which requires `RegularFile && uid match && nlink == 1 && 0200 && size 0` — or `ENOENT`); every other outcome, including a re-stat that itself fails with a non-`ENOENT` errno and a re-stat showing 0200-with-content, keeps the kernel's terminal verdict. An attacker who wins the re-stat race by leaving an inert tombstone has produced the "stopped" face, which a same-uid attacker could write directly anyway. No new admission.
5. **Every `GuardedFile` call site is labelled correctly.** Lock sites `daemon.rs:132`, `daemon.rs:168`, `daemon.rs:3945` → `LifecycleLock`. Record sites `daemon.rs:2100`, `2125`, `3281`, `3300`, `3479`, `3496`, `3521` → `InstanceRecord`. `validate_held_published_record` still discards via `.is_err()` at `daemon.rs:2097-2105` and `daemon.rs:2118-2126`, so its label is inert; it is nonetheless the correct label if that code is ever changed to propagate.
6. **No unintended control-flow change from `raced()`'s `PermissionDenied` kind.** Only two sites changed kind (`file_identity_at`'s `ENOENT`, `validate_instance_tombstone`'s recheck `ENOENT`); every other newly-`raced()` site was already `PermissionDenied`. The one caller that branches on `NotFound` is `read_instance_record_for_inspection_with_hook` (`daemon.rs:3258`), and the two `NotFound`s that legitimately mean "stopped" — the runtime directory being absent, and the initial `statat` returning `ENOENT` (which `return Ok(None)`s directly at `daemon.rs:3462`, never becoming an error) — are untouched. `read_attested_instance` has **no non-test callers** (`grep -rn read_attested_instance crates/ --include=*.rs` outside `daemon.rs` → exit 1), so its `NotFound` construction is unaffected in production. The remaining `NotFound` matches at `daemon.rs:2695` (`canonicalize`) and `daemon.rs:2801-2802` (signal path) are on unrelated errors.
7. **Both `start_paused` timing arguments are valid.** `a_never_settled_inspection_is_polled_against_the_readiness_deadline`: readiness 1s, poll 10ms → one inspection is 3 observations + 2×10ms, so ~50 inspections fit; `> 3` is robust. `the_retry_waits_the_caller_s_poll_rather_than_the_constant`: readiness 1s, poll 10s → the virtual clock advances to the `timeout_at` at +1s inside the first sleep, so exactly 1 observation; `<= 3` cannot be met at 25ms, where the same second buys ~40. Both verified by mutation (I2 table).
8. **`a_terminal_endpoint_fault_survives_into_the_race_marker` is not meaningfully load-dependent.** Its `composed > 0` guard needs a *raced* `open_published_record` failure under an `Unsafe` probe; my probe measured ~4 900 marked races in 20 000 observations of the same producer (≈25%), so 4 096 iterations gives ~1 000 expected hits. It also fails loudly rather than silently if the producer thread dies early.
9. **The `poll` parameter is a no-op in production today.** `SupervisorTimeouts::for_environment` (`daemon.rs:672-680`) overrides only `readiness`; `poll` is always `DEFAULT_POLL` (25ms). The plumbing is correct and the test holds it, but nothing in production changes. Worth knowing before anyone treats the fix as behavioural.
