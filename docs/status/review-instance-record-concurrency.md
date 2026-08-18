# Review — `fix: publish and retire the daemon instance record by rename, not by chmod`

Range: `a8c37a3..3306491`. Files read in full: `crates/gascand/src/socket.rs` (at `3306491`),
`crates/gascand/src/api.rs:40-110,469-515`, and the reader-side of `crates/gascan/src/daemon.rs`
(`978-1200`, `1205-1245`, `1350-1600`, `1885-1990`, `2309-2440`, `2600-2900`, `3155-3260`,
`4840-4890`). CI config `.github/workflows/ci.yml:14-60`.

**I did not run `cargo test` / `cargo build`.** This session has ~27 other agents addressable and I
have no reliable way to prove no other cargo job is running (`ps` under-enumerates on this machine),
so per the read-only constraint every conclusion below is derived from the tree, not from a run. The
commit's own CI numbers are therefore unverified by me.

---

## Strengths

- **The core substitution is correct and minimal.** `crates/gascand/src/socket.rs:368-393` builds
  the whole record in a private file and commits it with one `renameat_with(..., NOREPLACE)`
  (`:384-391`). A rename is atomic with respect to `statat`, so the reader's mode+size classifier
  cannot observe a half-state at the destination. This is the right primitive, and it is the same
  one `SocketPaths::bind` already used at `:92-99` — verifying the commit message's "that is what
  `SocketPaths::bind` in this same file already did".

- **Fixing retirement too was necessary, not gold-plating.** `Drop` at `:202-212` →
  `retire_instance_record_with_hook:245-263` now stages an inert tombstone and renames it over the
  record. Had only publication been fixed, `fchmod(0200)` followed by `ftruncate(0)` on a *linked*
  record would still walk the destination through 0200-with-content on every clean stop. The
  measurement in the commit message (6,812 residual samples with publication alone fixed) is
  consistent with the code it describes.

- **`stage_inert_instance_file:277-323` is a genuinely good factoring.** One function owns "a private
  0200 empty regular file, proven private by `fchmod` *after* `openat` because the mode argument is
  umask-masked" (`:288-290`), proven by `fstat` (`:291-302`) and by a name-to-inode recheck
  (`:308-313`), and it unlinks itself on every error path (`:316-322`). Both callers get the same
  guarantee. DRY, and the invariant is stated once.

- **`remove_named_identity` generalisation is the right shape.** `:565-605` now takes the expected
  `FileType` (`:594`) instead of hard-coding `Socket`, so the tombstone and the staging file get the
  same rename-to-quarantine-then-verify-identity treatment the socket had. The "put it back if it is
  not what we expected" branch (`:596-604`) is preserved.

- **Guards are armed over every fallible region.** Publication: guard constructed at `:361-367`
  immediately after staging, and the entire fallible closure (`:368-393`) runs armed; `publication?`
  at `:394` propagates with the guard live. Retirement: guard at `:247-253`, then `sync_all()?`
  (`:254`) and `renameat(...)?` (`:255`) both armed, disarmed only after the rename succeeds
  (`:256`). I found no fallible statement outside guard coverage in either path.

- **Recoverability from a mid-publish crash actually improves.** Before, a daemon killed during
  `write_all`/`sync_all` left an interrupted tombstone at the well-known path, and `gascand` could
  not start again until the CLI reclaimed it. Now the destination is untouched (or absent), so the
  successor starts cleanly. That is a real, unclaimed win.

- **The comment corrections are done in the project's style** — struck through in place with the
  correcting measurement (`crates/gascan/src/daemon.rs:3187-3206`, `:4870-4877`), not deleted.
  Several load-bearing claims check out against the tree (see "Claims verified" below).

- **`instance_record_commit_never_publishes_over_a_destination_that_appeared`
  (`socket.rs:980-1011`) is a real test, not a control.** The hook creates a file at the destination
  with `O_EXCL`, `NOREPLACE` makes publication lose, and the test asserts both that the winner's
  content survives and that no directory entry contains the loser's bytes. That second assertion is
  what makes it more than a smoke test.

---

## Issues

### Critical (Must Fix)

**None.** I could not construct an interleaving that ends with a wrong record at the destination, a
clobbered foreign file, or a lost publication. The two commit primitives (`NOREPLACE` for
publication, identity-checked plain rename for retirement) are each sound for their case, and the
failure modes I did find are litter and stale claims, not corruption.

On question 2 specifically — **the retirement check-then-act is sound in production, and here is the
argument.** `retire_instance_record_with_hook:245` checks `identity_at(directory, name) == expected`
and then plain-renames at `:255`. For that to clobber something legitimate, some actor must *create
or move a node onto the name* between those two lines. The candidates:

- A competing `gascand` publisher cannot *create* at the name: its commit is
  `renameat_with(NOREPLACE)` (`socket.rs:384-391`), which fails while our record occupies the name.
- It cannot reach that rename anyway: `clear_inert_destination:411-427` refuses anything that is not
  0200-and-empty, and our record is 0600-with-content.
- Nor can it get there via an earlier stat: to have statted a *tombstone* at the name it would have
  to have looked before we published, but we could only have published by first clearing that same
  tombstone ourselves.
- The CLI never takes the name. `retire_held_record` (`gascan/src/daemon.rs:1453-1460`) only
  `fchmod`s and `ftruncate`s a descriptor it already holds; nothing in `gascan` creates or renames a
  node at the instance path. I grepped the reader module for this and found no writer.
- And two concurrent publishers are already excluded upstream: `api.rs:489` binds the socket before
  `api.rs:506-507` writes the record, and `prepare_socket:526-531` fails with `AddrInUse` against a
  live socket.

So the counterexample does not exist in production. It *does* exist in a two-daemon test harness —
see Minor #3.

### Important (Should Fix)

**I-1. The commit's headline claim is false about the path, because `gascan`'s own reclaim still
produces 0200-with-content at the destination.**
`crates/gascan/src/daemon.rs:3200-3202` states: *"`crates/gascand/src/socket.rs` now builds both the
record and the tombstone under a private name and renames them into place, so this path shows only
three faces: absent, the inert tombstone, and the whole record."* The commit message repeats it
("The instance path now shows an observer exactly three faces").

That is true of `gascand`. It is not true of the path. `retire_held_record`
(`crates/gascan/src/daemon.rs:1453-1457`) does `fchmod(0200)` then `ftruncate(0)` on a held
descriptor, and `validate_held_published_record:1501-1545` has just proven that descriptor is *still
linked at the destination name* (`path_identity != published_record.identity` → abort). So on the
`recover_stale_published_record` path (`:1375`, reached from `ensure_started_locked:1231`) the
destination goes **0600-with-content → 0200-with-content → 0200-empty**. That middle state is
exactly `is_interrupted_tombstone` (`:2759-2765`).

Why it matters: this is not a rare path. It is the ordinary "previous daemon was SIGKILLed, next
`gascan start` cleans up its record" path. And the observing reader is not excluded by the lifecycle
lock — `inspect()` at `crates/gascan/src/daemon.rs:1966-1975` calls `inspect_with` with **no**
`lock_async`, while `start_with:1171` holds it. So a concurrent `gascan status` can still land in
`DaemonState::Unsafe`, "daemon record publication was interrupted" — the very verdict this commit
set out to make impossible. The window is two syscalls rather than an `fsync`, so it is orders of
magnitude narrower than the bug that was fixed, which is why this is Important and not Critical.

Fix, in order of preference:
1. Make `retire_held_record` stage-and-rename too. `InterruptedTombstone` already carries
   `directory` and `name` (`:2655-2662`), so the mechanics exist. **But note the coupling**:
   `validate_retired_tombstone:1548-1578` requires the held fd's inode to still be *at the name*
   (`path_identity != tombstone.identity` → `TombstoneChanged`) and requires `st_nlink == 1` via
   `is_instance_tombstone`. A rename unlinks the held inode, so that validation must be rewritten
   against the new tombstone, not the old descriptor. This is real work and arguably out of this
   commit's scope.
2. At minimum, **correct the claim now** and open the follow-up. Rewrite `:3200-3202` to say
   `gascand` shows only three faces and name `retire_held_record` as the remaining producer. In a
   codebase that treats these comments as the durable record, leaving a claim this specific and this
   wrong is worse than leaving the defect.

**I-2. The same comment's reachability claim is now wrong in the other direction.**
`crates/gascan/src/daemon.rs:3205-3206`: *"it is reachable only from a daemon that died mid-publish
or from one older than that change."* Under the new writer, a daemon that dies mid-publish leaves
**nothing** at the destination — its half-written record is at the staging name, and the destination
is absent or holds the tombstone it cleared. `is_interrupted_tombstone` is unreachable from a
mid-publish death by construction. The two producers that actually remain are (a) a pre-change
daemon, and (b) `retire_held_record` per I-1. Both halves of the sentence need replacing.

**I-3. A crash between staging and the rename now strands the record — with its `owner_token` — under
a name nothing ever sweeps.** (Question 4.)
`socket.rs:369-370` writes and `fsync`s the record into `.{random}`, and `:371` chmods it to 0600,
before the commit at `:384`. A `SIGKILL` anywhere in that span leaves a **0600 file containing the
serialized `DaemonInstanceRecord`, including `owner_token`** (`api.rs:88-98`), at a name derived from
`random_name:719-725` — `.{10 base64 chars}`, with the `purpose` argument discarded at `:723`, so the
file is not even self-describing.

Before the change, the same crash left the token at the *well-known* path as an interrupted
tombstone, where `open_interrupted_tombstone:2690` → `recover_interrupted_tombstone:1357` →
`retire_held_record:1453` truncated it on the next lifecycle command. Now nothing does. I grepped
`crates/gascand/src` and `crates/gascan/src` for any enumeration of the runtime directory outside
test modules and found none — there is no sweeper, and `SocketPaths::bind` does not clean staging
litter either. So these accumulate one per crash, indefinitely.

Calibration: the directory is 0700 and owned by the user, and `owner_token` is a per-launch
provenance nonce (generated at `api.rs:86`, matched at `gascan/src/daemon.rs:1295`) with no standing
authority once its launch is over — so this is a hygiene and secret-at-rest regression, not an
escalation. Note also that the commit message's "A failed publication now leaves nothing at the
destination" is *literally* accurate (it is scoped to the destination and to a returned error); the
crash case simply is not covered by any claim, and is a behaviour change that the "Behaviour changes"
list should have named.

Fix: sweep the runtime directory for `.`-prefixed regular files at `bind()` time (they can only be
this process's own abandoned staging, since the directory is 0700 and single-user), or give staging
names a stable recognisable prefix — `random_name`'s `purpose` parameter already exists and is thrown
away at `:723` — so a sweeper can identify them. Either way, this belongs in the same change as the
rename, because the rename is what created the class.

**I-4. `clear_inert_destination` runs an `fsync` too early, widening the absent window for no
benefit.** `socket.rs:359` clears the destination, and the record is not renamed into place until
`:384` — with `write_all` (`:369`) and `sync_all` (`:370`) in between. So the destination is
**absent for the full duration of an fsync**, on every daemon start that follows a clean stop.

I traced the reader and this window is benign: `read_instance_record_with_hook_and_directory_mode`
returns `Ok(None)` on `ENOENT` (`:2790-2792`), `inspect_with` then reaches `classify_connected` with
`record: None` (`:2309`), which skips the `record_matches_endpoint` check at `:2349-2357` and can
still return `DaemonState::Current` — and the socket is already bound (`api.rs:489` before `:506`),
so the endpoint answers. Retryable, not terminal. So this is not a correctness bug.

It is still worth fixing, because the whole point of the commit is that fsync-length windows at this
path are exactly what bite. Reorder to: stage → write → sync → chmod → attempt
`renameat_with(NOREPLACE)` → on `EEXIST`, run `clear_inert_destination` and retry the rename once.
That shrinks the absent window from an `fsync` to two adjacent syscalls, and it keeps every existing
refusal (a non-inert destination still fails `clear_inert_destination`, and publication still loses
to anything that appeared). It also closes the secondary window inside
`clear_inert_destination` → `remove_named_identity` described in Minor #3.

**I-5. `publication_never_shows_an_interrupted_tombstone_at_the_destination` (`socket.rs:939-960`) is
mostly encoding where the hook is called.** (Question 6, second half — and yes, I think the concern
is correct.)

The hook now fires at `:380`, immediately before the rename, where the destination has already been
cleared at `:359`. So `observed` is `None` in both the fresh and the reused case, and the
`Some((INSTANCE_TOMBSTONE_MODE, 0))` arm of the `matches!` at `:951` is dead against the current
implementation. The test's discriminating power comes entirely from the hook's *position*, which is
implementation, not contract: move `before_descriptor_commit` back above the `fchmod` and the test
starts failing on correct code; move it to any other point and it starts passing on incorrect code.
It also cannot see any window that opens *between* the hook and the rename.

The commit's mutation evidence is honest about the mechanism (it grafted the pre-fix publisher under
these tests and got `Some((128, 9))`) — but that only shows the test discriminates *when the hook sits
where the pre-fix code put it*. Suggest either strengthening the doc comment on the test to say
plainly that it pins one instant and that
`no_reader_ever_sees_an_illegal_state_across_start_and_stop` is the load-bearing one, or dropping it
in favour of the continuous observer. The test module's own comment at `:868-870` already concedes
this; the concession should be in the test that is weak, not only in the test that is strong.

### Minor (Nice to Have)

**M-1. `assert_ne!(fs::metadata(&path)?.ino(), inode)` (`socket.rs:1060`) is not a sound assertion —
it asserts the absence of inode reuse.** Sequence: `first` publishes inode I₁ (`:1049`); `drop(first)`
allocates I₂ for the tombstone and unlinks I₁ (I₁ is freed when `OwnedInstanceRecord::_file` drops);
`write_instance_record(b"second")` clears I₂ (freeing it) and allocates a fresh inode. On a
filesystem that reuses freed inode numbers — ext4 preferentially allocates the lowest free inode in
the parent's block group — that fresh allocation can legitimately be I₁, and the test fails on
correct code.

This is not a live flake here: `.github/workflows/ci.yml:39` puts the only `cargo test --workspace`
job on `macos-26`, and APFS allocates monotonically increasing object IDs. But it is a latent
portability trap the moment a Linux test job appears, and the property being asserted (the inode
changed) is a *consequence* of the design, not a guarantee anyone depends on — the assertions that
matter (`:1061` content, `:1063` no litter) are already there. Recommend deleting the `assert_ne!`
and keeping the doc comment's explanation.

**M-2. `no_reader_ever_sees_an_illegal_state_across_start_and_stop` (`socket.rs:875-928`) can false-PASS
in principle, and it hot-spins.** The observer (`:889-905`) is an unthrottled `symlink_metadata` loop
accumulating into a `BTreeSet`; it detects the illegal state only if it samples during the window. On
correct code there is no window, so the risk is the reverse case: a *regression* whose window is
narrow enough to be missed across 64 cycles. Two things bound this well — the guard assertion at
`:923-926` fails the test if the observer never saw both a published record and a tombstone (so a
dead observer cannot pass), and the regressions this is guarding against are `fsync`-length, which at
64 cycles is essentially certain to be sampled. The commit's mutation run (retirement reverted →
only this test failed, with `Some((128, 9))`) is the right evidence and I have no reason to doubt it.

Residual concerns, both cheap to address: (a) the loop has no `std::hint::spin_loop()` or yield and
will saturate a core for the length of 64 publish-retire cycles, and this project already has a
recorded problem with the workspace suite wandering under load; (b) 64 is unanchored — nothing in the
tree says why 64 rather than 8 or 2000. A sentence naming the smallest cycle count at which the
mutation was still caught would make the number a measurement rather than a guess.

**M-3. One two-daemon interleaving strands a file; it is unreachable in production but reachable in
tests.** (Question 1 / question 3.) Publisher P statts the tombstone at `clear_inert_destination:412`;
retirer R publishes and then begins retirement; P's `remove_named_identity:579-590` quarantine-rename
lands between R's identity check (`:245`) and R's plain rename (`:255`). P moves **R's published
record** to `.retired-<pid>-<ino>-<seq>`; R's rename then creates R's tombstone at the now-free name;
P's restore at `:597-599` is skipped because `identity_at(source)` now succeeds, so P returns
`AlreadyExists` and R's record — containing `owner_token` — is stranded under the quarantine name.

Two mitigations make this Minor rather than Important: R's own descriptor immediately truncates that
inode via the "somebody else's file" branch at `:265-270` (which is why that branch's comment is
correct and worth keeping), so the secret is erased in the ordinary continuation; and concurrent
publishers are excluded in production by the socket bind (see the Critical section's argument). The
fix in I-4 (clear immediately before the rename) also collapses this window to near-zero.

**M-4. `remove_named_identity` treats a vanished source as a hard error.** `:588` returns `ENOENT`
verbatim if the quarantine rename finds the source gone. For `clear_inert_destination`'s caller that
is the wrong polarity: "the tombstone I wanted removed is already gone" is exactly the state the
caller wants, and it now fails publication with `NotFound`. Reachable only from the two-publisher race
of M-3, so low priority, but the asymmetry is worth a comment or an `ENOENT → Ok(())` arm at the
`clear_inert_destination` call site.

**M-5. `stage_inert_instance_file` has no retry on name collision.** `:279-285` uses `O_EXCL` and
propagates `EEXIST` as a hard error, where `bind_staging:627` loops 64 times over fresh names. With 56
bits from `random_name:720`, a collision is not a practical concern — but the two staging paths in the
same file now handle the same failure two different ways, which is the sort of inconsistency that
reads as an oversight later. One line of comment saying the collision is deliberately fatal here would
settle it.

**M-6. `random_name`'s `purpose` parameter is dead** (`socket.rs:723`, `let _ = purpose;` —
pre-existing, not introduced here). It matters more now that there are two kinds of staging node in
the directory and, per I-3, a reason to want to identify abandoned ones.

**M-7. Neither publication nor retirement `fsync`s the directory after the rename.** Noting it only to
close the question: the instance record describes a running process and is meaningless after a crash
that loses it, and the runtime directory is tmpfs (`/tmp`, or `XDG_RUNTIME_DIR`). No action needed.

---

## Claims audit (question 7)

**Verified against the tree:**

| Claim | Anchor | Verdict |
|---|---|---|
| "`gascand` binds its socket (api.rs:489) before it writes this record (api.rs:506)" | `api.rs:489` `config.paths.bind()?`; `api.rs:506-507` `write_daemon_instance_record(...)` | ✅ exact |
| "`recover_interrupted_tombstone` proves the endpoint absent twice before `retire_held_record`" | `daemon.rs:1357-1373`, `for probe_index in 0..2` around `prove_endpoint_absent_or_inert` | ✅ |
| "That is what `SocketPaths::bind` in this same file already did for the socket" | `socket.rs:92-99` | ✅ |
| "…and what `DelayedPublicationSpawner` was fixed to do for the fixture" | `daemon.rs:4878-4881`, stage/write/chmod/rename; the diff touches only its comment, so it predates this commit | ✅ |
| "A failed publication now leaves nothing at the destination, where before it left an inert file there" | old error path chmod+truncated the destination file; new path unlinks staging via `StagingGuard` | ✅ (see I-3 for the crash case it does not cover) |
| "`OwnedInstanceRecord` carries the directory fd and the file name, because retirement renames" | `socket.rs:194-200` | ✅ |
| "1498 is the 1496 baseline at `d65801d` plus the two tests added here" | diff adds `no_reader_ever_sees_…` and `publication_never_shows_…`, renames two others → net +2 | ✅ arithmetic consistent (I did not run the suite) |

**Cannot verify from the tree — flag:**

1. **`socket.rs:217-220` and `socket.rs:871-874`**: the 2000-cycle / 12,131,645 / 6,812 / 47,124,057
   numbers. The commit is candid that the probe was "temporary … since replaced", so the anchor is an
   artifact that no longer exists. The test that replaced it runs **64** cycles (`socket.rs:907`), not
   2000, so `socket.rs:871-874` documents an experiment the code underneath it does not perform. That
   is the drift hazard the anchoring rule exists to prevent. Suggest: keep the numbers, but say
   explicitly in both places that they come from a probe not retained in the tree, and that the
   retained test is a 64-cycle bounded version of it.
2. **`crates/gascan/src/daemon.rs:3197-3198`**: *"It cost five workspace runs and one CI run before it
   was named."* Past-tense, no anchor of any kind — no run IDs, no SHAs, nothing re-derivable. Under
   the project's own rule this either carries its anchor or comes out. The rule it supports ("size is
   reported in every case") stands on its own without it.
3. **`crates/gascan/src/daemon.rs:3200-3206`**: two claims that are not merely unanchored but
   contradicted by the tree — see I-1 and I-2.

---

## Answers to the specific questions

1. **TOCTOU windows.** Every state the destination passes through is safe for the reader: absent
   (→ `Ok(None)`, retryable), inert tombstone (→ `Ok(None)`), whole record (→ the record). The absent
   window is longer than it needs to be (I-4) but benign — I traced it through `classify_connected`
   with `record: None`, which still yields `Current` against a live endpoint. Two concurrent
   publishers: the loser gets `EEXIST` from `NOREPLACE` and its guard unlinks its staging, no clobber,
   no loss. Publish vs retire: one interleaving strands a file (M-3), no interleaving corrupts.
2. **Retirement's identity check.** The claim holds. Full argument in the Critical section: no
   legitimate production actor can create at that name in the window, because publication's only
   commit is `NOREPLACE`, `clear_inert_destination` refuses a 0600 record, and the CLI never names
   the path. A `RENAME_EXCHANGE`-based retirement would remove the check-then-act entirely and is
   worth considering, but it is hardening, not a fix.
3. **Leaks and litter.** The guard is armed over every fallible region in both paths — I checked
   statement by statement. `remove_named_identity`'s restore-or-strand branch can strand a
   secret-bearing file in exactly one interleaving (M-3), and the retiring daemon's own descriptor
   erases it immediately afterwards. The real leak is the crash case (I-3), which no guard can cover
   and which nothing sweeps.
4. **The secret.** Yes — longer than before, in the crash-during-publish case only. See I-3.
5. **Reader-side compatibility.** No reader path depends on inode continuity across a `gascand`
   publish or retire. Every reader comparison is "identity must be *unchanged* or abort"
   (`validate_held_published_record:1522`, `validate_held_interrupted_tombstone:1478`,
   `open_published_record:2640`), so a changed inode fires the abort, which is the correct outcome —
   and `st_nlink == 1` in `is_interrupted_tombstone:2762` catches the unlinked held inode first. The
   one place that *does* require continuity is the CLI's own in-place retire
   (`validate_retired_tombstone:1548-1578`), which the writer change does not touch. Remaining
   reader-side terminal-verdict-on-transient windows: `validate_instance_tombstone:2842` can still
   return `PermissionDenied` if a publisher replaces the tombstone mid-validation, but that window
   existed identically before (the old writer mutated the same inode under it), and the new
   `ENOENT` variant is handled better — `read_instance_record_for_inspection:2602-2607` maps
   `NotFound` to `Ok(None)`. Net: neutral-to-better. The one genuine regression on this axis is I-1,
   and its producer is in `gascan`, not `gascand`.
6. **Test validity.** `no_reader_ever_sees_an_illegal_state_across_start_and_stop` tests the subject
   and is the load-bearing test; false-PASS risk is low and bounded by the `seen.contains` guard
   (M-2). `publication_never_shows_an_interrupted_tombstone_at_the_destination` largely encodes hook
   placement (I-5). `instance_record_commit_never_publishes_over_a_destination_that_appeared` is a
   good test. `instance_cleanup_leaves_one_inert_tombstone_a_successor_replaces` has one unsound
   assertion (M-1).
7. **Unanchored claims.** Three, listed above; two of them are actively wrong, not merely unanchored.

---

## Recommendations

1. Correct `crates/gascan/src/daemon.rs:3200-3206` before merge (I-1, I-2). This is a two-sentence
   edit and it is the only thing here I would actually block on, because it writes a false statement
   into the durable record — the same failure mode as the `2026-08-07` comment this commit exists to
   correct.
2. Fix the staging-litter class (I-3): sweep `.`-prefixed regular files at `bind()`, and give
   `random_name` back its `purpose` so they are identifiable. Add the crash case to the commit
   message's "Behaviour changes" list either way.
3. Reorder publication to clear-on-`EEXIST` rather than clear-first (I-4). Shrinks the absent window
   from an `fsync` to two syscalls and collapses M-3 and M-4 with it.
4. Drop `assert_ne!` on the inode (M-1); add `spin_loop()` and a sentence justifying 64 (M-2).
5. Open a tracked follow-up for `retire_held_record` (I-1 fix #1), noting the
   `validate_retired_tombstone` coupling so the next session does not discover it the hard way.
6. Re-run `cargo test --workspace` alone before merge. I could not, and this commit's whole subject is
   a race the suite is known to be sensitive to.

---

## Assessment

**Ready to merge? With fixes.**

The concurrency work is correct: the rename substitution is the right primitive, both halves needed
fixing, the guards cover every fallible region, and I could not construct an interleaving that loses
or corrupts a record. What holds it back is not the mechanism but the record it leaves — the doc
comment at `crates/gascan/src/daemon.rs:3200-3206` asserts a property of the instance path that the
CLI's own `retire_held_record` still violates on a common path, and asserts a reachability that the
new writer has made impossible, both verifiable from the tree. Fix those two sentences and the
staging-litter regression (I-3) and this is a good merge; I-4, I-5 and the Minors can follow.
