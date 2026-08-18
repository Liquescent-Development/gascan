<!--
Committed verbatim as written by the reviewer. Reviewed synchronously over
`c0679c6..fb7d4b0` before either pull request left draft; the fixes are
`de14a94`, whose message lists what was addressed and what was not.

Scope of this file: the whole diff c0679c6..fb7d4b0, adversarial.
-->

# Whole-diff adversarial review — `c0679c6..fb7d4b0`

Range: `ae75595` (backend-scoped controller store), `f081e61` (startup diagnostic channel),
`fb7d4b0` (doctor early return + host/runtime split). Reviewed together for cross-commit
interactions, claim truth, scope, coverage and doc truth. Per-commit correctness audits are
covered by other reviewers and are not repeated here.

**Critical: none found.**

---

## Major

### M1. `HostFacts::apply` overwrites a live daemon's `runtime.cli` with the CLI's own environment, and the stated justification is false

`crates/gascan-core/src/doctor/host.rs:184` (`facts.cli = engine_binary;`), reached from
`crates/gascan/src/cli.rs:450` (`host_facts.apply(&mut facts);`), which runs on **both** the
daemon-answered and the daemon-dead path.

The doc comment on `HostFacts::apply` (host.rs:168-173) and the matching comment on
`execute_doctor` (cli.rs:407-410) both justify applying host facts over a live daemon's report
this way:

> When one did, the values are equal by construction -- same functions, same host, same
> account -- and applying them unconditionally is what keeps that true rather than assumed.

Three of the four facts are genuinely host-derived (`architecture` = this process's compile
target; `macos` = a plist on disk; `engine_artifacts` = a digest check over this account's
files). The fourth is not. `engine_binary_fact` (host.rs:118-137) is a function of
`GASCAN_ENGINE_BIN` **as read from the calling process's environment**
(cli.rs:419: `std::env::var_os(gascan_core::backend::ENGINE_BIN_ENV)`), while the daemon's
answer came from the value that daemon was *launched* with
(`main.rs:766`, `HostFacts::collect(Arca, Some(&engine_binary))` where `engine_binary` is
`launch.executable`). "Same host, same account" does not imply "same environment block" —
these are two processes started at different times from different contexts.

`GASCAN_ENGINE_BIN` is set by no packaging or script in the repo
(`grep -rn "GASCAN_ENGINE_BIN\|ENGINE_BIN_ENV" packaging/ scripts/` returns nothing), so its
value is entirely user/launcher supplied — exactly the kind of value that skews between a
long-lived daemon and a later CLI invocation.

**Failure scenario.** Terminal A: `export GASCAN_ARCA_BACKEND=1 GASCAN_ENGINE_BIN=/opt/arca/engine`,
`gascan up .` — daemon starts, healthy, `runtime.cli` = `pass: engine executable present at
/opt/arca/engine`. Later, from a launcher or shell that exports `GASCAN_ARCA_BACKEND` but not
`GASCAN_ENGINE_BIN` (a wrapper script, a different profile, a `sudo -u` shell, or simply a
profile edited between the two), the user runs `gascan doctor`. The daemon answers correctly;
`apply` then overwrites `runtime.cli` with
`fail: GASCAN_ARCA_BACKEND selects the Arca engine backend, so GASCAN_ENGINE_BIN must name the
engine executable`, and `execute_doctor` exits `EXIT_RUNTIME` (cli.rs:481-489, non-zero iff any
check is not Pass/Warning). The user is told to set a variable the running daemon already has,
about a daemon that is perfectly alive — the same category of wrong-remedy defect that
`DoctorRemedies` and the `no Apple prose on Arca` assertion exist to close.

`host.rs:238` (`apply_leaves_every_runtime_fact_alone`) does not catch this: it asserts `cli`
stays `Unknown` on the **Apple** backend, where `engine_binary` is `None`. Nothing asserts the
Arca overwrite is safe.

**Suggested fix.** Apply the engine-binary fact only on the no-daemon path (it is the path that
needs it — it is what makes the report say why the daemon could not start), or have the daemon
report the executable path it was launched with and have the CLI measure *that* path rather
than its own variable. The artifact fact can stay unconditional; it really is host-derived.
Either way the "equal by construction" comment must be narrowed to the facts for which it is
true.

**Secondary, same call site:** `engine_artifact_fact()` runs a full digest verification over the
installed kernel + vminit (~27 MB on this machine) in the CLI on *every* `gascan doctor`, even
when the daemon already answered `runtime.kernel` from the identical check. That is the same
work done twice per invocation. Cheap to avoid with the same fix.

### M2. `Command::Doctor { .. } => Ok(0)` is a silent-success dead arm

`crates/gascan/src/cli.rs:760`.

`fb7d4b0` adds an early return at cli.rs:621 and leaves the match arm as `Ok(0)` with the
comment `// Returned above, before the daemon connection.` If that early return is ever moved,
guarded, or reordered — e.g. behind a future flag, or when the next command gets the same
treatment — `gascan doctor` prints nothing and exits 0, reporting a healthy host it never
measured. That is the worst available failure mode for a diagnostic command and it violates
"fail fast, never silence errors".

The file already has the correct idiom 530 lines later, for exactly this shape of invariant:

```rust
crate::daemon::SupervisorError::DaemonStartup { .. } => {
    unreachable!("daemon startup diagnostics returned above")
}
```
(cli.rs:1293)

**Suggested fix.** `unreachable!("Command::Doctor returned before the daemon connection")`, or
an explicit `CliError::Runtime` if a panic is unacceptable in this crate. Not `Ok(0)`.

### M3. `ae75595`'s coverage-narrowing admission is incomplete, and the "26 tests" replacement is not a replacement

Commit `ae75595`, the `KNOWN COVERAGE NARROWING` block:

> The migration's process-level instrument is now `crates/gascand/tests/controller_state.rs`,
> which drives the same `open_controller_store` the daemon calls, over 26 tests. What replaced
> the leg is stronger for the property that actually changed […]

Two things are wrong.

**The number.** That file has 29 `#[test]`s, not 26 — and had 29 before the change:

```
c0679c6 integration=29   fb7d4b0 integration=29
```
(`git show <rev>:crates/gascand/tests/controller_state.rs | grep -c '#\[test\]'`)

A `diff` of the sorted test-name lists between `c0679c6` and `fb7d4b0` is **empty**: no test was
added, removed or renamed. The file's 57 added / 57 removed lines are entirely mechanical —
`fixture.paths.legacy_database()` → `fixture.legacy_database()` and a
`BackendSelection::Apple` argument threaded into `for_home_and_runtime`. Every one of the 29
still runs Apple.

**Therefore it did not replace anything.** It covered the migration before this commit and it
covers exactly the same thing after. What was deleted from
`crates/gascan-e2e/tests/fake_backend.rs` — the leg where a **real daemon process** performs the
legacy→durable migration and removes the legacy file:

```rust
assert!(env.durable_database().is_file());
assert!(!env.legacy_database().exists());
```

— is now asserted by no test that runs a daemon at all. The new e2e test that took its place,
`a_scoped_daemon_neither_adopts_nor_deletes_another_backends_store`, asserts the *opposite*
property (no adoption, no deletion), which is the right test for the new behaviour but is not
coverage of the migration. `crates/gascand/tests/controller_state.rs` calls
`open_controller_store` in-process; calling it "the process-level instrument" for a leg that was
specifically about a daemon process is a category slip.

**Failure this leaves uncovered.** A regression in `main.rs`'s Apple path — wrong
`ControllerStatePaths`, a store opened before the daemon's own directory preparation, a
migration that runs in-library but not under the daemon's uid/umask — would be caught by no
test in the suite. The library-level tests would stay green.

**Suggested fix.** Either restore the process-level migration leg with an Apple-scoped fixture
(the fake tier can no longer stand in for Apple, but `crates/gascand/tests/` could drive the
daemon binary), or rewrite the admission to say plainly that the process-level migration
coverage was dropped and nothing replaced it. Correct "26" to 29 either way.

### M4. `ae75595`'s mutation result "letting a scoped store claim the legacy database fails 3 of the 4" is not supported by the tests

Commit `ae75595`, `MEASURED, and each new test proven by mutation` block.

The four new unit tests are in `crates/gascand/src/controller_state.rs`:

1. `apple_keeps_the_unscoped_path_and_owns_the_legacy_database`
2. `every_other_backend_is_scoped_under_its_own_instance_record_name`
3. `a_scoped_store_leaves_the_legacy_database_untouched`
4. `two_backends_on_one_account_do_not_see_each_others_records`

Under the stated mutation (a scoped store's `legacy_database()` returns `Some`):

- (1) asserts only Apple's `durable_database`, `legacy_database == Some(...)` and
  `scope_child() == None`. Unaffected — **passes**.
- (2) asserts `legacy_database() == None` for Arca and Fake — **fails**.
- (3) seeds an Apple legacy store and a stray `-wal`, opens Arca, asserts both untouched —
  **fails**.
- (4) seeds nothing at the legacy path: it calls `open_controller_store(&apple)` (which creates
  the *durable* Apple store) and then seeds `apple.durable_database()`. A scoped store that
  claimed the legacy path would find no file there to claim — **passes**.

That is 2 of 4, not 3. The companion claim in the same block, "un-scoping every backend fails
3 of the 4 new unit tests", *is* consistent with the bodies ((2), (3), (4) fail; (1) passes), so
the 3-of-4 figure looks copied from the neighbouring line.

I did **not** re-run the mutation — the lead's instruction forbids contended cargo runs and this
repository has measured that they fail differently every run. This is reasoning from the test
bodies, not an executed contradiction.

**Suggested fix.** Re-run that mutation and record the real number, or drop the line. As it
stands a durable record claims a measurement that the code cannot produce.

---

## Minor

### m1. `README.md:64-66` still states the unscoped path as *the* durable controller database

`ae75595` corrected `docs/release/macos-checklist.md:210-221` and `packaging/macos/uninstall.sh`
but left README's "Controller-state recovery and upgrades" section:

> Gas Can keeps its per-user controller inventory, operation history, and destroyed-sandbox
> tombstones at `~/Library/Application Support/dev.gascan/controller/state.sqlite3`.

True for Apple only after this diff. Worse, the sentence is pinned by a test —
`scripts/tests/macos_release_smoke.rs:145` (`readme_documents_durable_controller_recovery_contract`)
asserts the README contains that exact path string — so the now-partial statement is
test-enforced and will not decay into being noticed.

**Fix.** Mirror the checklist wording into README §Controller-state recovery, and extend the
smoke assertion to require the per-backend sentence too.

README:89 (`removes the private runtime root and dev.gascan/controller directory`) and
README:517 (the new doctor row) are both still true. No other doc statement was found to be
falsified by this diff.

### m2. `backend_from_environment()?` is the one failure inside the held-descriptor window that does not report

`crates/gascand/src/main.rs:255`.

`ae75595` moved this call to before the store opens (correctly — the store is scoped by
backend). It now sits inside the window where the startup diagnostic descriptor is held, and it
is the only fallible call in that window routed through a bare `?` rather than
`report_startup_error`. `AmbiguousBackend` (both `GASCAN_TEST_FAKE_BACKEND` and
`GASCAN_ARCA_BACKEND` set) goes to the `Stdio::null()` stderr the CLI gives a production daemon
and reaches the user as a 150s readiness timeout — the exact failure mode `f081e61` exists to
remove. There is also no whitelist code for it in `gascan_core::startup_diagnostic`.

Reachable only in debug builds (`backend_selection` cannot return `Err` when `fake_requested` is
hard-`false`, backend.rs:172-180), which is why this is Minor rather than Major. It is still the
first thing a developer or e2e run can hit.

`SocketPaths::for_user()?` (main.rs:244) and `paths.prepare_directory()?` (main.rs:245) are in
the same window and equally unreported; both predate this diff.

### m3. `f081e61`'s "nothing reportable happens after it" is not accurate

`crates/gascand/src/main.rs:639` — `run_daemon` takes the diagnostic by value and drops it as its
first statement. The structural argument for that is sound and I agree with it (a fourth arm
cannot forget to release the descriptor, and
`successful_daemon_closes_inherited_startup_diagnostic_descriptor` requires the release).

The accompanying factual claim is not:

> every backend reaches that function and nothing reportable happens after it

After the drop, `SandboxService::new_with_doctor_state_for_image(...)?` (main.rs:648),
`let _ = service.reconcile().await?` (main.rs:665), `configure_e2e_daemon(config)?` and
`Daemon::serve(config, api).await?` all propagate errors out of a daemon that has not yet
served. A `reconcile()` failure on either backend — `recover_pending()` erroring, an inspection
that cannot run — is a startup failure that still reaches the user as a readiness timeout.

I am not asking for the structure to change; I am asking for the sentence to. Something like
"nothing after it is reported through this channel today, and widening that is a separate
change" would be true.

### m4. Unknown check ids from the daemon are now silently dropped

`crates/gascan/src/cli.rs:436-441`: `let Some(id) = DoctorCheckId::from_name(&capability.name)
else { continue };`. The previous code rendered `capability.name` verbatim, so any check the
daemon reported reached the user. Now anything outside the 21-variant enum disappears from the
report — and, because `DoctorFacts` is a fixed shape, it does not even appear as `unknown`.

`every_check_id_round_trips_through_a_fact` guards `field_mut` against `into_report`; it does
not guard `from_name` against a daemon that knows a name the CLI does not. Low severity given
the two ship together and backwards compatibility is not a project requirement, but it is a
silent discard sitting inside a change whose stated theme is closing silent discards.

### m5. `plist` is now a production dependency of `gascand` for a test-only use

`crates/gascand/Cargo.toml:30` keeps `plist.workspace = true` under `[dependencies]`. After
`macos_fact_at` moved to `gascan-core`, the only remaining use in `gascand` is
`crates/gascand/src/main.rs:1652-1657`, inside `#[cfg(test)] fn
plist_product_version_is_structured_and_requires_26`. The released daemon now links a plist
parser it never calls. Move it to `[dev-dependencies]`.

### m6. Stale comment in `crates/gascand/src/main.rs:348`

> `engine_artifact_fact()` below reports on the same two

`engine_artifact_fact` is no longer in this file — it is `gascan_core::doctor::host`. The
comment's point (one source for the kernel/vminit layout) still holds; the location word does
not.

### m7. `doctor_reports_real_host_facts_and_names_the_runtime_cause` makes the every-push tier host-dependent

`crates/gascan-e2e/tests/arca_startup.rs:227-236` asserts `host.macos` is `pass`. The comment is
explicit that this means "this machine really is an aarch64 host on macOS 26+". That is a new
environmental precondition for a tier the same commit advertises as running on every push. It is
defensible (the product requires macOS 26+), but a host below that will now fail this test with
a message about the doctor rather than about the host. Worth an explicit skip-or-explain if the
push tier ever runs anywhere other than a 26+ arm64 runner.

---

## Verified and found correct

**Startup ordering in `run()` (main.rs:229-470).** Read top to bottom. The order is
`backend_from_environment` → `controller_state_paths`/`open_controller_store` → `e2e_ssh_paths`
→ backend arm → `run_daemon`. Backend resolution genuinely must precede the store now, and the
comment at main.rs:246-254 states why correctly. I found **no half-initialised state on any error
path**: every arm either returns before constructing a service or reaches `run_daemon`, and the
store is opened before anything that could leave a container or engine running.

**No double reporting, and no failure under the wrong code.** Every diagnostic write goes through
`report_startup_error` (main.rs:706-738), which is called exactly once per failure by
`startup_error`/`controller_startup_error`, each of which returns a `StartupError` carrying the
same code that then propagates out of `run()` and `main()` to a `Stdio::null()` stderr in
production. The `eprint!` inside `report_startup_error` is the same line, not a second report.
`EngineError::code()` (engine.rs:109-126) maps each variant to its own constant rather than one
bucket, and every constant it returns is in `ACCEPTED_CODES`. The `debug_assert!` at main.rs:715
covers a code that is not on the whitelist; all current call sites pass `&'static str`
constants from the shared module, so a release-build silent drop is not reachable today.

**Descriptor lifetime.** `startup_diagnostic` is borrowed mutably by the `reported` closure in
the Arca arm and then moved into `run_daemon`; NLL makes this sound, and `run_daemon` drops it
before anything long-running. The contract
`successful_daemon_closes_inherited_startup_diagnostic_descriptor` is satisfied by construction.

**Exit-code semantics are preserved across the doctor rewrite.** Old:
`Ok(if doctor.findings.is_empty() { 0 } else { EXIT_RUNTIME })`. Daemon-side `findings` is
`checks.filter(|c| matches!(c.status, Fail | Unknown))` (`crates/gascand/src/api.rs:1944-1947`).
New: `0` iff every check is `Pass` or `Warning` (cli.rs:481-489). Identical predicate. README's
"Warning-only reports remain ready and `gascan doctor` exits successfully" (README:17-18)
remains true.

**`git merge-base --is-ancestor 9c6933e 7f9e8e6` → exit 0.** Ran it.
`9c6933e` = 2026-08-04 07:42:59 -0700, "Preserve controller state across upgrades (#40)".
`7f9e8e6` = 2026-08-17 23:42:31 -0700, "feat(daemon): the Arca backend is selectable, and …".
Both dates and the ordering are as `ae75595` states.

**`ancestors().nth(5)` defect claim.** Verified: `git show
c0679c6:crates/gascand/src/controller_state.rs` line 2511 is
`let home = paths.durable_database.ancestors().nth(5).ok_or_else(|| {`. The described symptom
follows.

**"Seven engine codes join the four controller ones."** `ACCEPTED_CODES` is `[&str; 11]` with
4 `controller_state_*` and 7 `engine_*`. Correct, and `the_accepted_codes_are_distinct` /
`an_unlisted_code_is_not_accepted` are real tests of the closedness rather than of membership.

**"150 seconds."** `ENGINE_BACKED_DAEMON_READINESS` is `Duration::from_secs(150)`
(`crates/gascan-core/src/backend.rs:122`). The before-message
`started daemon did not become healthy and current (state Stopped)` is the readiness bound
expiring, as claimed. The "1.2s" after-figure is not independently verifiable but is consistent
with a daemon that reports and exits before dialling anything.

**`fb7d4b0`'s claimed doctor output.** Not reproduced end-to-end — running it would spawn a real
daemon on this machine. Every component I could check read-only agrees:
`/usr/libexec/PlistBuddy -c "Print :ProductVersion" /System/Library/CoreServices/SystemVersion.plist`
→ `26.6.1`, matching `host.macos pass SystemVersion.plist ProductVersion is 26.6.1`; `uname -m`
→ `arm64` (Rust `std::env::consts::ARCH` = `aarch64`), matching `host.architecture`; the pin tag
`gascan-engine-m4` exists and the engine artifacts are installed under
`~/Library/Application Support/dev.gascan/engine`, so `runtime.kernel pass` is reachable. The
report shape matches what `arca_startup.rs:200-294` asserts.

**Test-count arithmetic across all three commits is exactly right**, which is a good sign for the
`cargo test --workspace` lines I could not re-run:
- `ae75595` 1469 → `f081e61` 1473: +4 = 2 in `arca_startup.rs` + 2 in `startup_diagnostic.rs`.
- `f081e61` 1473 → `fb7d4b0` 1483: +10 = 2 in `arca_startup.rs` + 1 in `autostart.rs`
  (`doctor_keeps_its_host_facts_when_the_controller_store_is_unsafe`) + 2 in `doctor.rs`
  (`report_shape_tests`) + 5 in `doctor/host.rs`.

**"3 of the 4" for the un-scoping mutation** is consistent with the test bodies (see M4).
**"19 of the 29 `controller_state` tests"** — 29 matches the integration file exactly, and since
all 29 use `BackendSelection::Apple` and 12 of them touch
`legacy_database_required()`/`legacy_database()`, a mutation that scopes Apple would fail a large
majority. 19 is plausible; not re-run.

**`scripts/ci-check-ignored-tests.sh` baseline.** `tests/ci/expected-ignored-tests.txt` has 49
lines and is untouched by this diff — so "49 matching the baseline" is consistent for all three
commits without needing the cargo run.

**"15 contracts."** `ls tests/release/*-contract.sh tests/ci/*-contract.sh` → 15, which is the
exact glob `scripts/ci-run-release-contracts.sh:12` iterates.

**`cargo fmt --all --check` → exit 0** at `fb7d4b0` with a clean worktree. Ran it.

**`crates/gascan-e2e/tests/arca_startup.rs` contains no `#[ignore]`.** The single `grep` hit is
prose in the module doc referring to `arca_engine.rs`. Confirmed.

**Host facts were moved, not copied.** `architecture_fact`, `macos_fact`, `macos_fact_at` and
`engine_artifact_fact` are deleted from `gascand/src/main.rs` and exist only in
`gascan-core/src/doctor/host.rs`. `main.rs` keeps its 16 unit tests, unchanged in name, now
calling `host::architecture_fact` / `host::macos_fact_at`. No test coverage was lost in the
move, and `production_doctor_report` (main.rs:726) and `arca_doctor_report` (main.rs:766) both
call the shared collector — so the "one implementation, two call sites" claim holds.

**`doctor_recovery_does_not_force_a_held_durable_operation` was not weakened.**
`crates/gascan-e2e/tests/doctor.rs` is absent from the diffstat entirely; the test at doctor.rs:600
is untouched. `fb7d4b0`'s claim that this test failed and the *code* changed rather than the test
is therefore true. The one test whose assertions did change —
`controller_state_errors_survive_all_daemon_start_paths`, autostart.rs:419-470 — did not weaken:
it still requires a non-zero exit and the same code and actionable substring on all three paths,
merely reading doctor's from stdout, and additionally asserts doctor's stderr is *empty*. The new
sibling `doctor_keeps_its_host_facts_when_the_controller_store_is_unsafe` covers the other half.
`autostart.rs:120-130`'s `default_database()` retarget to `controller/fake/state.sqlite3` is
correct and its comment names the wrong-reason-pass it avoids.

**Scope discipline: clean.** Every file in the 19-file diff maps to one of the three authorised
items. `crates/gascand/src/engine.rs` (+21) is `EngineError::code()`, item (b). The
`ControllerStartup` → `DaemonStartup` rename across `crates/gascan/src/daemon.rs` is item (b)
and is justified in the type's own doc. The `plist` dependency and `Cargo.lock` line are item
(c) (`macos_fact_at` moving into `gascan-core`). `DoctorCheckId::ALL`, `field_mut`,
`runtime_unreachable` and `remedies_for` are all consumed by the CLI's new report assembly,
item (c). `ControllerStatePaths` holding the account home and `ControllerDirectory` holding its
ancestors as a list are the fix for the defect scoping introduced, item (a). I found **no
gratuitous refactor and no silently widened scope.**

---

## Not checked

Per the lead's instruction (build-directory contention; this repository has measured that
contended cargo runs fail differently every run), I did **not** run `cargo test --workspace`,
`cargo clippy --workspace --all-targets`, `scripts/ci-check-ignored-tests.sh`,
`scripts/ci-run-release-contracts.sh`, or any of the mutations. The pass/fail counts, the clippy
line and the contract-status line in all three "Every CI step run locally" blocks are therefore
**unverified** — though the arithmetic behind the test counts, the ignored baseline and the
contract count all check out statically, which is as much support as they can get without
running.

M4 is reasoning from test bodies, not an executed contradiction; it should be re-run before the
number is corrected or defended.
