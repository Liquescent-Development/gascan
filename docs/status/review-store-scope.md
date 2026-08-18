<!--
Committed verbatim as written by the reviewer. Reviewed synchronously over
`c0679c6..fb7d4b0` before either pull request left draft; the fixes are
`de14a94`, whose message lists what was addressed and what was not.

Scope of this file: the backend-scoped controller store (ae75595).
-->

# Review — `ae75595` "the controller store is scoped by backend, and Apple keeps the unscoped path"

Repo `/Users/kiener/code/gascan`, branch `feat/milestone-4-product-wiring`.
Reviewed against the commit as landed. Note that `crates/gascand/src/controller_state.rs`
has changed by only 12/-4 lines since (`git diff --stat ae75595 HEAD --
crates/gascand/src/controller_state.rs`, error-code constants), so line numbers below
are from the working tree and match the commit.

The design is sound and the central claim holds: two backends no longer share a
database, Apple keeps the path its existing records live at, and a scoped store
never reads the legacy runtime location. The findings below are one real data-loss
path the change opens outside the daemon, three smaller defects, and two
commit-message numbers that do not reproduce.

---

## Critical

**None found.**

---

## Major

### M1. `uninstall.sh --remove-data` destroys one backend's sandboxes and then deletes every backend's store

`packaging/macos/uninstall.sh:212-247` (removal block), with
`gascan_uninstall_remove_controller_data` at `packaging/macos/uninstall.sh:123-133`.

**What is wrong.** The `--remove-data` path is a two-stage contract: first destroy
every owned sandbox through `gascan` (`uninstall.sh:212-239`), then delete the
controller directory (`uninstall.sh:246`). Stage one now reads *one* backend's store
— `gascan list --json` and `gascan list --all --json` go to a daemon, and the daemon's
store is the one this commit scoped to the backend its environment selects. Stage two
still deletes the whole tree: `gascan_uninstall_remove_controller_data` passes
`controller` to `gascan_uninstall_remove_absolute_private_child`, which ends in
`/bin/rm -rf -- "./controller"` (`uninstall.sh:119`), taking `controller/arca/` and
`controller/fake/` with it.

The commit message states "removes the whole `controller` directory, so the children
were already covered". That is true of the *deletion* and false of the *contract the
deletion is guarded by*: the gate at `uninstall.sh:228-239` ("owned sandbox inventory
did not reach the destroyed state") now passes while another backend's inventory has
never been consulted.

**Failure scenario.** A user runs some work under `GASCAN_ARCA_BACKEND=1`, leaving three
sandboxes recorded in `~/Library/Application Support/dev.gascan/controller/arca/state.sqlite3`
and three live VMs in the engine's state root. In a plain shell (no backend variable),
they run `./packaging/macos/uninstall.sh --remove-data`:

1. `gascan list --json` autostarts an **Apple** daemon → Apple's records only.
2. Those are destroyed; `gascan list --all --json` reports all `absent`. Gate passes.
3. `rm -rf .../controller` deletes `controller/arca/state.sqlite3`.
4. The three Arca VMs survive — `GASCAN_ENGINE_STATE_ROOT` is undefaulted and Arca's
   to choose (`crates/gascan-core/src/backend.rs:62-77`), and
   `gascan_user_engine_root` (`packaging/macos/release-common.sh:13-15`) is the
   *fetched boot artifacts* directory, not the engine state root, so nothing in
   `gascan_uninstall_remove_engine_data` touches them.

Result: records deleted while the runtime keeps the sandbox alive, unreferenced. That
is precisely the harm the commit message opens with, reintroduced at uninstall time —
and made deterministic rather than accidental, since before scoping a single shared
store at least *listed* the other backend's records.

**Suggested fix.** Make stage one per-backend, or refuse rather than delete blind:
enumerate `"$controller_root"/*/state.sqlite3` (the loop the preserve message already
has at `uninstall.sh:201-204`) and, for each child, either re-run the destroy stage with
that backend's environment variable set, or abort with a message naming the stores
whose sandboxes could not be enumerated. The seeded scoped store already in
`tests/release/installer-contract.sh:250-255` gives the contract test a place to assert
whichever behaviour is chosen; today it asserts only that the file is gone
(`installer-contract.sh:310`).

---

## Minor

### m1. Two of the three `MEASURED` mutation counts in the commit message do not reproduce

The commit message states:

```
- un-scoping every backend fails 3 of the 4 new unit tests and both e2e tests
- scoping Apple as well fails 19 of the 29 `controller_state` tests
- letting a scoped store claim the legacy database fails 3 of the 4
```

I re-ran each mutation in a detached worktree (`git worktree add --detach`, isolated
`CARGO_TARGET_DIR`, worktree since removed). Baseline first:

- `cargo test -p gascand --test controller_state` → **29 passed; 0 failed**
- `cargo test -p gascand --lib -- apple_keeps_the_unscoped_path every_other_backend_is_scoped a_scoped_store_leaves_the_legacy two_backends_on_one_account` → **4 passed; 0 failed**

Mutation A — un-scope every backend (`scope_directory`'s non-Apple arm returns `None`):
**3 failed, 1 passed** (`apple_keeps_…` passes). ✔ Claim holds.

Mutation B — scope Apple as well (delete the `BackendSelection::Apple => None` arm, so
`scope_directory` is `other => Some(other.as_str())`): `cargo test -p gascand --test
controller_state` → **8 passed; 21 failed**. The message says 19.

Mutation C — let a scoped store claim the legacy database (`ControllerScope::Backend`
carries `runtime_directory.join(DATABASE_NAME)`, `legacy_database()` returns `Some` for
it): **2 passed; 2 failed**. The message says 3 of the 4.

For C the passing test is `two_backends_on_one_account_do_not_see_each_others_records`
(`crates/gascand/src/controller_state.rs:3469`), and it passes for a structural reason
rather than a flaky one: `scoped_paths` creates `root/runtime` empty
(`controller_state.rs:3354-3374`), so a legacy-claiming scoped store finds no legacy
database to adopt and behaves identically. Failures were
`every_other_backend_is_scoped_under_its_own_instance_record_name` (line 3423) and
`a_scoped_store_leaves_the_legacy_database_untouched` (line 3454).

**Why it matters.** The counts are the commit's evidence that each new test is
load-bearing, and they are in a durable record. Three of the four tests *are*
mutation-proven (verified above); the numbers as written are not reproducible from the
mutations they name. Either restate them from a re-run, or drop the counts and keep the
qualitative claim.

**Suggested fix.** Replace with the measured figures, or state the exact mutation
alongside each count so a reader can reproduce it.

### m2. Creating a scope directory fires `NOTE_LINK` on the watched `controller` descriptor, failing a concurrent store open

`crates/gascand/src/controller_state.rs:2594-2602` (the `mkdir`),
`2881-2891` (`identity_events` includes `FilterFlag::NOTE_LINK`, registered on every
descriptor including ancestors), `2923-2930` (`ensure_unchanged`).

**What is wrong.** `controller/` is now watched — as `descriptor` for the shared store,
as the last `ancestors` entry for a scoped one — and `ensure_private_child_directory`
creating `controller/<backend>` is a `mkdirat` *inside* it. On macOS that raises
`NOTE_LINK` on the parent directory's vnode, which is in `identity_events`, so any
in-flight `open_controller_store` in another process sees an event and returns
`ControllerStateError::Unsafe("controller database identity changed while opening the
store")`.

Measured, not assumed. A 40-line kqueue probe (registering
`NOTE_DELETE|NOTE_RENAME|NOTE_LINK|NOTE_REVOKE` on an open directory fd, then `mkdir`ing
a child) printed, on both a scratch volume and under `$HOME`:

```
before nlink=2
events=1
  ident=3 fflags=0x12 (LINK=1 WRITE=1)
after nlink=3
```

`0x12` = `NOTE_WRITE|NOTE_LINK`; `NOTE_LINK` is registered on ancestors, `NOTE_WRITE` is
not. Before this commit nothing ever created a directory under `controller/` except the
opening process's own quarantines, and the monitor is built *after*
`open_controller_directory` (`controller_state.rs:1740-1742`), so no process trips its
own `mkdir`.

**Failure scenario.** Backend B's daemon starts for the first time ever while backend A's
daemon is inside `Store::open_no_follow`. B's `mkdir controller/b` raises `NOTE_LINK` on
A's `controller` watch; A's `ensure_unchanged()` at `controller_state.rs:1752` fails and A
refuses to start with an unsafe-substitution error naming nothing the user did.

**Assessment.** Narrow — the two backends share one daemon socket, and the window exists
only on a backend's first-ever start — and fail-closed rather than corrupting. Retrying
succeeds because the directory then exists. Worth knowing about because the error text
will send whoever hits it hunting for an attacker.

**Suggested fix.** Either accept it and note it in the `ControllerDirectory` doc comment,
or narrow the ancestor watch: `NOTE_LINK` on `controller` guards against a substitution
that `NOTE_DELETE|NOTE_RENAME` plus the `open_existing_controller_directory` re-walk in
`validate_database_binding` (`controller_state.rs:3240-3251`) already catches.

### m3. The new intermediate directory has no security test

`crates/gascand/tests/controller_state.rs` contains the suite that pins the safety
contract — `open_rejects_symlinked_managed_components` (line 150),
`open_rejects_foreign_expected_owner` (176),
`open_rejects_unsafe_managed_directory_and_database_modes` (192),
`open_rejects_special_bits_on_managed_paths` (246). **Every one of the 29 tests in that
file constructs its paths with `BackendSelection::Apple`**, which by design never creates
a scope child. So the `controller/<backend>` directory this commit introduces is covered
by no test for symlink substitution, foreign ownership, a non-0700 mode, or set-uid/sticky
bits.

The code itself is correct — I traced it (see "Verified correct", V3) — but the property
that keeps it correct is currently unpinned, and the file's whole point is pinning it.

**Suggested fix.** Parameterise the four tests over `[Apple, Arca]`, or add one scoped
variant that plants a symlink at `controller/arca` and asserts
`controller_state_unsafe`.

### m4. `README.md` still documents the unscoped path as *the* durable database, and a test enforces that

`README.md:64-66` reads "Gas Can keeps its per-user controller inventory … at
`~/Library/Application Support/dev.gascan/controller/state.sqlite3`" with no mention of
scoping, and `scripts/tests/macos_release_smoke.rs:143-148`
(`readme_documents_durable_controller_recovery_contract`) asserts that exact string is
present. `docs/release/macos-checklist.md:210-220` *was* updated by this commit; the
README was not, so the two now disagree about the same fact.

**Suggested fix.** Mirror the checklist's wording into `README.md`; the smoke test's
assertion is a `contains`, so adding the per-backend sentence beside the existing path
keeps it green.

### m5. One vacuous assertion in `installer-contract.sh`

`tests/release/installer-contract.sh:310`:

```sh
[[ ! -e $fixture_controller_root && ! -L $fixture_controller_root ]]   # line 309
[[ ! -e $fixture_scoped_controller_root && ! -L $fixture_scoped_controller_root ]]  # line 310
```

`$fixture_scoped_controller_root` is `$fixture_controller_root/arca`. Line 309 already
establishes the parent does not exist, so line 310 cannot fail unless 309 has. It would
pass under an implementation that removed nothing but the parent name, and under one that
removed everything — it distinguishes no behaviours.

The other three new shell assertions are load-bearing: the preserve-message `grep -Fqx`
at line 282 fails if the loop at `uninstall.sh:201-204` is deleted, and `[[ -f
$fixture_scoped_controller_root/state.sqlite3 ]]` at line 286 fails if the no-argument
path deletes anything.

**Suggested fix.** Drop line 310, or make it meaningful by asserting removal ordering /
that the parent's other children went too.

### m6. `AmbiguousBackend` is now the first startup failure, and it is the one that bypasses the startup diagnostic

`crates/gascand/src/main.rs:255`.

Moving `backend_from_environment()?` above the store open is correct and the commit
argues it well. The side effect: `?` here propagates out of `run()` to stderr, which a
production daemon has as `Stdio::null()` — it is the only startup failure in `run()` that
does not go through `report_startup_error` / `controller_startup_error`
(`main.rs:341, 520-566`), including after `f081e61` ("every Arca startup failure reaches
the user by name"), which I checked at HEAD.

This is not a regression — the call bypassed the channel before the move too — but the
reorder changes which error a user sees when a daemon has both a broken controller store
and both backend variables set: previously the diagnosed controller error, now the
undiagnosed ambiguity error, which reaches the user as a generic readiness timeout.

**Suggested fix.** Route it through `startup_error` with an accepted code, the way the
Arca arm's `required(...)` failures are.

---

## Verified correct

**V1 — path/directory handling (review item 1).** The `ancestors().nth(5)` derivation was
the only positional assumption, and it is gone: `grep -rn 'ancestors()' crates/` returns
only the doc comment at `controller_state.rs:124` and two unrelated test *names*. Every
other derivation is scope-agnostic:

- `controller_path` (`controller_state.rs:2268-2274`) uses `durable_database().parent()`,
  which is the directory the database lives in for either scope — correct for the
  migration temps, snapshots, backups and quarantines it names.
- `open_state_ancestor_directories` (`2666-2691`) now reads `paths.home` directly.
  `home` and `durable_database` are both built from the same `home` argument inside
  `for_home_and_runtime` (`145-176`), the field is private, and there is no setter, so
  they cannot disagree.
- `validate_absolute_normal_path` (`2513-2536`) still rejects `/`, relative paths, and any
  non-`Normal` component, so the stored `home` cannot smuggle `..` or a trailing `.`.
- The scope child is `BackendSelection::as_str()` (`crates/gascan-core/src/backend.rs:137-146`),
  a `const fn` over a closed enum returning `"apple" | "arca" | "fake"` — no path
  separator or traversal component is constructible.
- `open_existing_controller_directory` re-derives the same chain from `paths.scope_child()`,
  so the ABA re-walk in `validate_database_binding` (`3240-3251`) goes through the scope
  child too.

**V2 — the fd chain (review item 2).** Nothing was dropped and the scope directory *is*
watched. For the shared store `descriptors()` yields exactly the previous five
(`home, Library, Application Support, dev.gascan, controller`, `2585-2603`). For a scoped
store it yields six, with `controller` appended to `ancestors` and `controller/<backend>`
as `descriptor`. `NOTE_WRITE`, which `new_for_controller_family` and `new_for_snapshot_input`
put on `directory.descriptor` (`2803`, `2825-2828`), consequently lands on the directory
that actually holds the database in both shapes — writes into `controller/` by another
backend correctly do not count as writes to a scoped store's directory. The only
consequence of the extra ancestor is m2 above.

**V3 — security/validation (review item 3).** The scoped path is defended by the same
primitives as the unscoped one, not by weaker ones:

- Creation goes through `ensure_private_child_directory` (`2731-2764`) — `statat` with
  `SYMLINK_NOFOLLOW`, `openat` with `O_NOFOLLOW|O_DIRECTORY` via
  `open_existing_child_directory` (`2717-2729`) so a symlink is `ELOOP`, `fchmod 0700` on
  create, then `validate_directory(.., private_mode = true)` (`3188-3208`) which requires
  mode exactly `0700`, `st_uid == expected_uid`, and a real directory. Special bits are
  caught because the comparison is against the full `& 0o7777`.
- Re-opening goes through `open_existing_child_directory` + `validate_directory(.., true, ..)`
  (`2645-2655`) — identical to how `controller` itself is handled two lines above.
- ABA of the new intermediate directory is covered twice: the kqueue watch on the
  `controller` ancestor (`NOTE_DELETE|NOTE_RENAME|NOTE_REVOKE`) and the full re-walk in
  `validate_database_binding`, which re-`statat`s `DATABASE_NAME` through a freshly opened
  chain and compares `DatabaseIdentity`.

Only the test coverage is missing (m3).

**V4 — the legacy split (review item 4).** `legacy_database()` is `Some` for every
`ControllerScope::Shared`, unconditionally (`183-190`), so the shared/Apple store's
behaviour is byte-identical to before — `git diff` shows every shared-path change is a
`&Path` → `Option<&Path>` unwrap and nothing else. All four `legacy_database_required()`
production call sites (`428`, `542`, `2454`, plus the `Conflict` construction) sit inside
code reached only with a `LegacyState` in hand, which `open_legacy_state` returns `None`
from for a scoped store before any of them run — so the `Invalid` branch is genuinely
unreachable rather than a silent fallback.

Skipping `recover_legacy_archive_transactions` for a scoped store leaves nothing undone.
It scans the *legacy runtime directory* for `ARCHIVE_QUARANTINE_PREFIX` children
(`1280-1341`), and those are created only by `archive_legacy_state`, which only a
shared-scope migration reaches. A prepared quarantine left by a crashed Apple daemon
survives an Arca daemon's start untouched and is recovered by the next Apple start.

The date argument is true: `git log -1 --format='%h %ad' --date=short` gives `9c6933e
2026-08-04` and `7f9e8e6 2026-08-17`, and `git merge-base --is-ancestor 9c6933e 7f9e8e6`
exits 0.

**V5 — ordering (review item 5).** The backend that scopes the store and the backend the
daemon records cannot diverge: one `selection` binding (`main.rs:255`) feeds both
`controller_state_paths(&paths, selection)` (`269`) and `DaemonRuntime { backend: selection }`
(`279-283`), and the `match selection` arms follow. The debug-only e2e branch in
`controller_state_paths` (`588-604`) threads the same value. The only way to detach the
store from the backend is the pre-existing `GASCAN_STATE_PATH` override (`266-267`), which
bypasses `ControllerStatePaths` entirely and is what the e2e "legacy" fixture uses
deliberately.

The commit message's account of the original defect checks out:
`crates/gascand/src/main.rs:664` is still `let _ = service.reconcile().await?;`, so the
reconcile report is indeed discarded (this commit does not claim to fix that, and does
not).

**V6 — tests (review item 6).** The four new unit tests are load-bearing, and I confirmed
it by mutation rather than by reading (see m1 for the runs). `assert_eq!(paths.scope_child(),
Some(backend.as_str()))` at `controller_state.rs:3413` looked tautological but is not — it
fails under mutation A. The e2e test
`a_scoped_daemon_neither_adopts_nor_deletes_another_backends_store`
(`crates/gascan-e2e/tests/fake_backend.rs:744-812`) is well-constructed: the legacy
database is seeded by a real daemon pointed at it through `GASCAN_STATE_PATH`
(`fake_backend.rs:104-108`), so the bytes compared at line 798 are bytes a store wrote,
and its three load-bearing assertions each die under a different mutation — `durable_database()
.is_file()` under un-scoping, `!shared_database().exists()` under un-scoping, and the
`legacy_bytes` equality under legacy-claiming (the file would not exist and `fs::read` would
error). The `autostart.rs` change (`default_database()` → `controller/fake/state.sqlite3`,
`autostart.rs:97-100`) is the right fix and its doc comment names the exact reason the old
path would have made the assertion pass for the wrong reason.

The only vacuous new assertion I found in the whole change is `installer-contract.sh:310`
(m5).

**V7 — `uninstall.sh` glob (review item 7).** The glob itself is correct.
`for gascan_preserved_scoped in "$gascan_preserved_controller_root"/*/state.sqlite3`
(`uninstall.sh:201`): the variable half is quoted so spaces in `$HOME` survive, and glob
results are not word-split in bash regardless. With `nullglob` off (the script sets only
`set -euo pipefail`, line 2) a no-match iteration yields the literal pattern, which
`[[ -e $gascan_preserved_scoped ]] || continue` at line 202 discards — and `[[ ]]` does not
word-split either. `set -u` is satisfied because the loop variable is always assigned. The
depth is exactly one, matching the layout. The removal path does cover the children
(`rm -rf` at line 119). The only defect in this file is M1, which is about *what the
removal is gated on*, not about the glob.

---

## Not findings, recorded so the next reviewer does not re-derive them

- `ensure_private_child_directory` recurses on `EEXIST` (`controller_state.rs:2751-2753`),
  which is unbounded in principle. Pre-existing, untouched by this commit, and requires an
  adversary winning an unbounded number of consecutive races.
- The `--remove-data` gate reading only one backend's store (M1) was *also* wrong before
  this commit, in the other direction: a shared store made an Apple daemon try to destroy
  Arca sandboxes. Scoping does not create the class, it makes the outcome deterministic
  and silent.
