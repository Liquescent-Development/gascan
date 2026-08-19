# The daemon instance record's reader needs a retryable verdict, and its last in-place producer needs to stop

Date: 2026-08-18
Status: Design, approved in conversation; not yet planned or implemented
Scope: open item 1's residual — the reader's half of the daemon instance record race, and
`retire_held_record`, the last in-tree producer of the illegal fourth face.

Companion documents:

- The publish-race fix this continues: merged as `025b922`, recorded in
  `docs/status/START-HERE.md` under `WHAT IS OPEN` item 1
- The shared file protocol these constants live in:
  `crates/gascan-core/src/daemon_protocol.rs`, merged as `a3fef90` (PR #83)

---

## 1. The defect

Two programs share one file. `gascand` writes the daemon instance record; the `gascan` CLI
reads it and classifies what it finds. The record's path is supposed to show a reader exactly
three faces — absent, inert `(0200, 0)`, and published `(0600, len>0)` — and a fourth,
`(0200, content)`, means "written but never published", which `validate_file_stat` turns into a
terminal `DaemonState::Unsafe`.

**The reader treats every disagreement between two of its observations as terminal.** It makes
several separate observations — read the record, open the published record, probe the endpoint
— and any inconsistency between them becomes `Unsafe` with a detail string. There are nineteen
sites producing that verdict in `crates/gascan/src/daemon.rs` — sixteen constructing
`state: DaemonState::Unsafe` and three returning the variant as an expression, counted
2026-08-18 with `grep -n 'DaemonState::Unsafe'` and excluding one comparison, one doc comment
and four test assertions. Some describe real danger: a
symlink where a regular file belongs, the wrong owner, an oversized record. Others describe
nothing worse than having looked at a moving target:

- `validate_instance_tombstone` returns `PermissionDenied` as
  `"daemon instance tombstone changed while opening it"` and
  `"…while validating it"`
- `open_published_record` returns `"daemon instance record changed while binding its
  descriptor"`, and the identity-and-size mismatch on its recheck

**This is user-visible and it is wrong.** VERIFIED 2026-08-18 by reading the code:
`start_with` takes the lifecycle lock (`crates/gascan/src/daemon.rs:1174`) and `inspect` does
not (`:1969`). So `gascan status` run against a daemon that is legitimately stopping can sample
the record mid-transition and report `Unsafe` — a verdict whose other members are symlink
attacks and foreign ownership — when the honest answer is "you looked while it was moving".

**And the thing it races with is still in the tree.** `retire_held_record`
(`crates/gascan/src/daemon.rs:1456-1460`) does `fchmod(0200)` and then `ftruncate(0)` in place
on a record that `validate_held_published_record` has just proven is still linked at the
destination. Between those two syscalls the destination *is* the illegal fourth face. VERIFIED
2026-08-18: the `fchmod`-then-`ftruncate` order is unchanged. Both of its callers run under the
lifecycle lock — `recover_stale_published_record` is reached from `start_with` at
`crates/gascan/src/daemon.rs:1234`, inside the lock taken at `:1174` — so the only observer that
can ever witness the window is an unlocked reader. Producer and reader are two halves of one
problem, which is why this design fixes both.

## 2. Decisions

### 2.1 The reader retries and reports the settled truth. No new `DaemonState`.

A reader that notices it raced looks again, a bounded number of times, and reports whatever the
daemon settled into. The race is invisible to the user.

The alternative — a new `Transitioning` verdict — was rejected on cost. `DaemonState` is
consumed by the CLI's output, by `start_with`'s decision logic at
`crates/gascan/src/daemon.rs:1211`, and it crosses the wire into `gascan doctor`, which carries
hand-written tables (the hazard that produced the hardening commit `0bf6d75`). A new variant
obliges every one of those to be right about a state that, after this change, is transient by
construction.

### 2.2 Both halves land together

Retrying makes the producer's window survivable; it does not remove it. Fixing only the reader
would leave the illegal state reachable in the tree and leave
`crates/gascan-core/src/daemon_protocol.rs` documenting a three-face rule with a standing
exception — a shared contract that is not true is worse than one that is merely unwritten.

### 2.3 Exhaustion is fail-closed

If the path never settles, the verdict is `Unsafe`, with a detail naming that it kept changing.
A path that will not stop changing is not a race; it is a fault, and this tree's default when
evidence runs out is to refuse rather than to reassure.

### 2.4 `(0200, content)` stays terminal, deliberately

It is the one race-shaped state that must NOT be retried. After §3, nothing in this tree
produces it. The only remaining producer is a `gascand` from an older release, which is genuine
version skew and a real diagnosis. Retrying it would convert that diagnosis into a silent delay
and then an identical-looking `Unsafe` three observations later.

### 2.5 The staging vocabulary becomes shared, which contradicts a comment merged today

§3 makes the CLI a second process that stages files in the daemon's runtime directory. So
`INSTANCE_STAGING_PURPOSE` moves into `gascan_core::daemon_protocol` and the CLI's staging
prefix joins it.

**This contradicts `crates/gascand/src/socket.rs`, merged in `a3fef90` hours before this
design, which says `INSTANCE_STAGING_PURPOSE` "is not part of the shared protocol and must not
join it".** That was true when written — no reader saw a staged file by name because only one
process staged. This design makes it false, and the comment must be corrected rather than left
to be discovered.

It also invalidates the *reasoning* behind an existing safety property. `sweep_abandoned_staging`
argues in its own comment that "a live daemon's staging cannot be caught: publication runs once
per daemon and `prepare_socket` has already refused to start a second one against a live
socket." That argument names one stager. With two, the conclusion may survive via the lifecycle
lock, but the stated reasoning does not, and §3.3 restates it.

## 3. The producer: `retire_held_record`

### 3.1 Two jobs, not one

`retire_held_record` must leave a legal tombstone at the path **and** destroy the dead record's
bytes so that a descriptor outliving the process cannot read the owner token back out of it.
The staging-and-rename trick that fixed the publisher satisfies the first and silently breaks
the second: a rename leaves the old inode alive, unlinked, with its content intact.

### 3.2 The ordering, which is forced

1. Create a fresh file under a private staging name: `O_CREAT|O_EXCL|O_NOFOLLOW|O_CLOEXEC`,
   `fchmod` to `INSTANCE_TOMBSTONE_MODE`, then verify regular file, owner is us,
   `st_nlink == 1`, mode `0200`, size `0`. This is the recipe `stage_inert_instance_file`
   already uses in `crates/gascand/src/socket.rs`.
2. `renameat` it over the destination. The path now shows the inert face; the old inode is
   unlinked and still held.
3. **Then** `ftruncate` the old, now-detached inode.
4. Validate (§3.4).

**Step 3 must follow step 2.** Truncating first would put `(0600, 0)` at the live name, and
`validate_file_stat` (`crates/gascan/src/daemon.rs:3229`, whose mode arm is `:3241` and whose
accepting `else` is `:3243`) accepts that as a published record of size zero — the reader would take it and then fail parsing an empty file. That is a worse
failure than the one being fixed, not a smaller one.

**This is the mirror image of the publish-race fix's lesson, and the mirroring is the point.**
There, `ftruncate` had to precede `fchmod` because `lstat` tears between resolving a name and
reading an inode, so an observer could read `(0200, content)` off an inode already renamed
away. Here the destructive step follows the rename for the same underlying reason: touch an
inode destructively only once it is out of the namespace. The old code did the destructive step
while the inode was still at the name, which is the whole defect.

No `fchmod` of the outgoing inode is needed. It is unlinked; its mode is unreachable and
therefore uninteresting. Dropping it also removes a syscall that can fail between rename and
truncate.

### 3.3 The staging file, and the sweeper

The CLI's staged file is created inert and empty and never receives a record, so an abandoned
one leaks no owner token — unlike `gascand`'s staging, which holds a complete record and is why
the sweeper exists. The tidiness argument for sweeping it still holds; the secrecy argument does
not.

The CLI's staging uses its own purpose name, not `gascand`'s, so that the sweeper can reason
about each separately and so that a stray file says which process left it. Both purpose names
live in `gascan_core::daemon_protocol`; `sweep_abandoned_staging` sweeps both prefixes, and its
safety comment is rewritten to argue from the lifecycle lock rather than from there being a
single stager.

If the rename fails, the staging file is unlinked. `crates/gascand/src/socket.rs` already has
the arm-and-disarm guard shape for exactly this.

### 3.4 `validate_retired_tombstone` is rewritten against two identities

Today it asserts that the inode it holds is still the inode at the name, and that it reached
`(0200, 0)`. A rename unlinks the held inode, so that post-condition is unsatisfiable by
construction under §3.2 — which is precisely why this was not folded into the publish-race fix
and had to become its own design.

The replacement checks two distinct identities:

- the **name** resolves to the freshly staged inode, wearing the inert face
- the **held** inode has `st_size == 0` and `st_nlink == 0`

**Corrected: this section first called that "a strictly stronger post-condition than today's",
and it is not.** It is a trade — stronger in two dimensions, weaker in one.

Stronger: the replacement proves the record is gone from the namespace, *and* that its bytes are
destroyed, *and* that what stands at the name is legal. The old form proved only that one inode
reached `(0200, 0)`; it never proved the bytes were unreachable, because the inode was still
linked at the name when it was checked.

Weaker: the old form proved the inode at the name was the very inode the recovery had validated.
The replacement cannot, and must not try — the rename is not `NOREPLACE`, because replacing is
the whole job, so it overwrites whatever stands at the name at that instant and nothing
downstream can tell that it did.

What restores that dimension is a separate check rather than a post-condition:
`retire_held_record` compares the destination against `record.identity` immediately *before* the
`renameat`, and refuses with `TombstoneChanged` if the name has stopped naming the record this
retirement validated. Post-conditions cannot recover it after the fact; only a check-then-act on
the near side of the rename can.

## 4. The reader: a retryable verdict

### 4.1 Typed, and terminal by default

Race-shaped failures carry a type rather than an `io::ErrorKind::PermissionDenied` and a
message string. Classification **defaults to terminal**: only a failure explicitly constructed
as transient is retryable, so a validator added later that nobody classifies stays `Unsafe`
until a human decides otherwise. Fail-closed on the drift, in the same spirit as
`CERTIFIED_ENGINE_REVISION` staying `None`.

### 4.2 The whole observation sequence retries, not individual validators

The observations are interdependent — the record, the published record opened against that
record, and the endpoint probe. Retrying one against stale others manufactures fresh
disagreements. The retry restarts the sequence from the top.

Three observations total, with `SupervisorTimeouts::poll` between, matching the
`for probe_index in 0..2` shape `recover_interrupted_tombstone` already uses at
`crates/gascan/src/daemon.rs:1366`. Under `start_with` the lifecycle lock makes a race
impossible, so the retry is unreachable there and costs nothing.

### 4.3 What is transient

Transient: the two `validate_instance_tombstone` "changed while…" failures; the
`open_published_record` "changed while binding its descriptor" failure and its identity and
size recheck mismatches; and the `ENOENT` that `gascand`'s `clear_inert_destination`
(`crates/gascand/src/socket.rs:530`) made newly reachable, which surfaces at the `openat` in
`validate_instance_tombstone` (`crates/gascan/src/daemon.rs:2855`).

That last one closes the third piece of open item 1's residual.

**Corrected, and the correction runs the other way from the claim it replaces.** This section
first called that `ENOENT` "reachable but inert in production", reasoning that
`read_instance_record_for_inspection` maps `NotFound` to `Ok(None)` and that only
`read_attested_instance` propagates it, which has no non-test callers. The second half still
holds — every call site sits inside `mod tests`; re-derive it with
`grep -n read_attested_instance crates/gascan/src/daemon.rs` and compare against the line
`mod tests {` opens on, rather than trusting a line number written here. The first half is
backwards: `raced()` builds a `PermissionDenied`, not a `NotFound`, so classifying this failure
is exactly what stops `read_instance_record_for_inspection`'s `NotFound => Ok(None)` arm from
matching, and the failure propagates instead of being swallowed.

MEASURED on 2026-08-19 at `beb05f4` with a temporary probe — not in the tree — calling
`read_instance_record_for_inspection_with_hook` with a `before_tombstone_validation` hook that
unlinks the tombstone: with the classification present, `Err(kind=PermissionDenied, raced=true)`;
with the `raced(...)` arm reverted to `errno(error)`, `Ok(None)`.

So the window changes from a silent "no record" — the reading that yields a `Stopped` verdict
for a daemon that is in fact coming up — into a failure `observe_once` can look at again. That
is a statement about this reader's return value and the arm it feeds. **No end-to-end verdict
flip was tested**, and none is claimed here.

Terminal: everything else, including `(0200, content)` per §2.4.

**Knowingly left, and recorded rather than silently dropped.** The split covers the `openat` in
`validate_instance_tombstone` and not the recheck `statat` further down the same function, whose
`ENOENT` stays unmarked and terminal. The exposure is characterised, not assumed:
`openat`→`fstat` is already covered, because an unlink in that window leaves `st_nlink == 0`,
`is_instance_tombstone` fails, and the existing raced mark fires; what is left is
`fstat`→recheck-`statat`, and within that only the sub-window between a successor's `unlinkat`
and its `renameat_with`. Narrow, real, and fail-closed today — the cost is an unflattering
`Unsafe` where a retry would have settled it, not a wrong action. The durable home for this is
open item 1 in `docs/status/START-HERE.md`; it is repeated here because this is the section
someone extending the split will read.

## 5. What moves to `gascan-core`

`INSTANCE_STAGING_PURPOSE` and the CLI's new staging purpose, joining the six values PR #83
placed in `crates/gascan-core/src/daemon_protocol.rs`. The module's three-face documentation
loses its standing exception — after §3 the rule is true rather than aspirational — and the
`socket.rs` comment quoted in §2.5 is corrected.

The pin test in `crates/gascan-core/tests/daemon_protocol.rs` gains the two new values, for the
reason it already gives: once both crates agree on a value, neither crate's suite notices it
changing.

## 6. Testing

Every claim is proven by mutation rather than by inspection. A test that cannot be made to fail
has proven nothing about the code it covers.

### 6.1 The producer's window is closed

A bounded concurrent-observer test over reclaim cycles, asserting the path shows only the three
legal faces. `no_reader_ever_sees_an_illegal_state_across_start_and_stop` covers publication and
retirement in `gascand`; nothing covers `gascan`'s reclaim path, which is where the remaining
window lives.

**Mutation:** restore the `fchmod`-then-`ftruncate` order and the test must fail.

### 6.2 The reader retries, and stops

- an injected observation sequence that disagrees once and then settles yields the settled
  verdict, not `Unsafe`
- one that never settles yields `Unsafe`, naming that it never settled
- an unclassified failure stays `Unsafe` — the §4.1 default

**Mutation:** make the classification default to transient and the third test must fail.

### 6.3 A green local run is a precondition, not evidence

This tree's own record: 47,124,057 local samples said a state was gone, CI's first run
disagreed, and CI was right — recorded in `docs/status/START-HERE.md` under trap 9 and open
item 1, against `add3c13`. Sampling here is necessary and not sufficient. A large local sample
count must not be written up as proof that the window is closed, and the bounded test must run
in CI before any such claim is made.

Run `cargo test --workspace` alone, never beside another cargo or contract job.

## 7. What this design does not do

- It does not make `inspect` take the lifecycle lock. A read-only status check must not block
  behind a slow start or stop on a lock with a 60-second timeout.
- It does not add a `DaemonState` variant (§2.1), so `gascan doctor`'s tables and the wire are
  untouched.
- It does not address the other two standing CI root causes — the PTY wall-clock bound and the
  `ssh-keygen` `/dev/fd` descriptor failure. Item 1's `0200` window is one of the three; the
  other two are out of scope here.
