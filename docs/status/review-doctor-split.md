<!--
Committed verbatim as written by the reviewer. Reviewed synchronously over
`c0679c6..fb7d4b0` before either pull request left draft; the fixes are
`de14a94`, whose message lists what was addressed and what was not.

Scope of this file: the doctor host/runtime split (fb7d4b0).
-->

# Review — `fb7d4b0` "gascan doctor answers without a daemon, by splitting the facts by who can measure them"

Scope: the single commit `fb7d4b0` on `feat/milestone-4-product-wiring`, read against the
working tree (the commit is HEAD's parent chain tip for these files; the files on disk
match the commit).

Verification I ran: `cargo test -p gascan-core doctor` → **10 passed, 137 filtered out,
1.10s**. I did not run the workspace suite or the e2e tier; every other statement below is
read off the source at the file:line given.

---

## Critical

### C1. `runtime.cli` is not "equal by construction" — the CLI's `GASCAN_ENGINE_BIN` overwrites the daemon's answer about the daemon's own engine

- `crates/gascan/src/cli.rs:419-420` — the CLI reads `GASCAN_ENGINE_BIN` **from its own
  process environment** and builds `HostFacts` from it.
- `crates/gascan/src/cli.rs:450` — `host_facts.apply(&mut facts)` runs **unconditionally**,
  after the daemon's capabilities have been ingested.
- `crates/gascan-core/src/doctor/host.rs:183-185` — `apply` overwrites `facts.cli` whenever
  the backend is Arca.
- `crates/gascand/src/main.rs:361-362` — the daemon's engine executable comes from
  `required(gascand::ENGINE_BIN_ENV, …)` read at **the daemon's** startup, and
  `crates/gascand/src/main.rs:389-392` captures that exact path
  (`let engine_binary = launch.executable.clone()`) into the doctor closure. That path is
  the one the running engine was actually spawned from.

The doc comments assert these cannot differ:

- `crates/gascan-core/src/doctor/host.rs:175-179` — "the values are equal by construction --
  same functions, same host, same account".
- `crates/gascan/src/cli.rs:410-413` — "They are equal by construction … applying them in
  both paths is what keeps that a fact rather than an assumption."

Same functions, same host, same account — but **not the same environment**. `engine_binary`
is a parameter sourced from a per-process, mutable environment variable, and it is the one
input to `engine_binary_fact` (`host.rs:122-141`). Architecture, macOS and the artifact
digest check take no such parameter (`ArtifactPaths::for_user()` →
`account::effective_account_home()` → passwd db keyed by euid, and
`crates/gascan/src/client.rs:501-503` refuses any daemon whose socket peer uid differs from
`geteuid()`, so the account really is shared). `runtime.cli` is the one fact in the set that
is process-scoped, and it is the one that is silently overwritten.

`BackendMismatch` does **not** close this: it compares only the backend selection
(`crates/gascan/src/daemon.rs:2073-2091`), not `GASCAN_ENGINE_BIN`.

**Failure scenario A — a false failure over a healthy daemon.**

1. `GASCAN_ARCA_BACKEND=1 GASCAN_ENGINE_BIN=/opt/arca/bin/arca-engine GASCAN_ENGINE_SOCKET=… gascan up`
   — daemon starts, engine runs, sandboxes are live.
2. Later, from a shell where only `GASCAN_ARCA_BACKEND=1` is exported (the engine path is
   set by a launcher script, not the profile): `gascan doctor`.
3. Backend matches, so the connection is handed over and the daemon answers `runtime.cli
   pass: engine executable present at /opt/arca/bin/arca-engine`.
4. `host_facts.apply` overwrites it with `fail: GASCAN_ARCA_BACKEND selects the Arca engine
   backend, so GASCAN_ENGINE_BIN must name the engine executable` (`host.rs:123-129`).
5. Exit code flips from 0 to `EXIT_RUNTIME` (`cli.rs:482-490`). The user is told their
   engine executable is missing while their sandboxes are running on it.

**Failure scenario B — a real failure masked (worse).**

1. Daemon running with `GASCAN_ENGINE_BIN=/old/build/arca-engine`; the developer rebuilds
   and deletes `/old/build/`.
2. `GASCAN_ENGINE_BIN=/new/build/arca-engine gascan doctor` — the daemon honestly reports
   `runtime.cli fail: engine executable unavailable at /old/build/arca-engine`, which is
   exactly the condition `host.rs:118-120` says the check exists to catch ("A running engine
   proves an engine ran; it does not prove that the variable still names one, and the next
   daemon start is what discovers that it does not").
3. The CLI overwrites it with `pass: engine executable present at /new/build/arca-engine`,
   and the doctor exits 0. The next daemon start is still the thing that will discover the
   truth — the check that was built to pre-empt it now hides it.

No test can see this: `crates/gascan-e2e/tests/arca_common/mod.rs:324-330` sets
`ENGINE_BIN_ENV` on the CLI command and `TokioDaemonSpawner` forwards it to the daemon
(comment at `mod.rs:300-302`), so CLI and daemon always share one value in the harness.

**Suggested fix.** Split `HostFacts::apply` by provenance, not by backend: apply
architecture, macOS and the artifact digest unconditionally (all three are parameterless and
account-scoped), and apply the engine-executable fact **only on the no-daemon path**, where
there is no better authority. Where a daemon answered, its `runtime.cli` is the authoritative
statement about the engine it is actually running. If the CLI's own reading is considered
worth surfacing, the honest form is a comparison — have the daemon report its engine path
(it already knows it) and raise a distinct check when the CLI's `GASCAN_ENGINE_BIN` names a
different file, rather than silently substituting one for the other.

---

## Major

### M2. Three no-daemon-answered supervisor failures still raise with no report at all — including a missing `gascand`

`crates/gascan/src/cli.rs:1241-1247` folds only `DaemonStartup` and `Readiness`. Variant by
variant, on the `connect_current_or_recover_observing` path:

| Variant | Classification | Judgement |
|---|---|---|
| `DaemonStartup` | reported | **Correct** — the daemon named a cause and exited. |
| `Readiness` | reported | **Correct** — came up, never became healthy. |
| `GracefulTimeout` | raised | **Correct** — daemon alive, holding work, `--force` is the one action; `suggestion()` at `daemon.rs:219` is the only variant that carries a suggestion, and `doctor_recovery_does_not_force_a_held_durable_operation` pins it. |
| `ExitTimeout` | raised | **Correct** — daemon alive after a forced stop; actionable and specific. |
| `IdentityChanged` | raised | **Correct** — a shutdown-time race; retry is the remedy. |
| `TombstoneChanged` | raised | **Correct** — concurrent recovery; retry. |
| `Outdated` | raised | **Correct** — a live daemon of another version; recovery already tried and declined (`daemon.rs:1228-1236`). |
| `BackendMismatch` | raised | **Defensible** (see m6) — its `Display` carries its own remedy, and the running daemon is alive and healthy. |
| `TombstoneBusy` | raised | **Borderline** — `daemon.rs:1380-1434`: the endpoint is held by something that is not a usable daemon. No `suggestion()`, no remedy. The user is in "why can't I talk to the daemon" territory, which is the report's territory. |
| `InvalidState { state, .. }` | raised | **Wrong for `Unsafe` / `Unreachable` / `Unhealthy`** — `daemon.rs:1238-1244` is reached when a daemon exists but is unusable and cannot be recovered. Nothing came up to answer, no remedy is attached, and the user gets one error line and no host facts. |
| `Client(ClientError)` | raised | **Borderline** — escapes from probe/attestation failures; means the endpoint could not be talked to. |
| `Io(io::Error)` | raised | **Wrong** — see below. |

`Io` is the serious one. It is produced by:

- `daemon.rs:2043` `DaemonPaths::for_user()?` — an unsafe or unusable runtime directory;
- `daemon.rs:2044` `crate::client::daemon_path()?` (`client.rs:473-480`) and `2052-2056`
  the canonicalize;
- `daemon.rs:2249` `paths.prepare_directory()?`;
- `daemon.rs:1245` `spawner.spawn(&launch)?` — **the daemon executable itself failing to
  spawn.**

**Failure scenario.** A partially-installed or partially-uninstalled `.pkg` leaves `gascan`
in `PATH` but no `gascand` beside it. `client.rs:477-479` derives the sibling path,
`supervisor_context` tolerates the `NotFound` canonicalize (`daemon.rs:2054`), and `spawn`
then fails `ENOENT`. `gascan doctor` prints
`Error: daemon supervisor I/O error: No such file or directory (os error 2)` on stderr, no
JSON on stdout, and no host facts — in a state where architecture, macOS version and the
engine artifact digest were all measurable in-process and where the artifact check's
`run gascan engine fetch` remedy might well be the answer. This is the same class of defect
the commit exists to fix, in a different variant. The same applies to an unsafe runtime
directory mode via `prepare_directory`.

Note also that the justification comment at `crates/gascan/src/cli.rs:1230-1231` — "Every
other supervisor failure carries its own remedy and must reach the user as an error" — is
**false as written**: `daemon.rs:217-231` gives a `suggestion()` to `GracefulTimeout` only,
and `Io`, `Client`, `InvalidState`, `TombstoneBusy`, `TombstoneChanged`, `IdentityChanged`
and `ExitTimeout` all return `None`. Only `BackendMismatch` embeds remedy prose in its
`Display`. The rule may still be the right rule; the reason given for it is not a fact.

**Suggested fix.** Extend `doctor_reports_rather_than_raises` to the states that mean *no
daemon answered and none can be brought up*: `Io`, `InvalidState { state: Unsafe |
Unreachable | Unhealthy }`, and `TombstoneBusy`. The discriminator the code is reaching for
is not "does this carry a remedy" but "is there a live daemon whose work I would be talking
past" — `GracefulTimeout` / `ExitTimeout` / `Outdated` / `BackendMismatch` /
`IdentityChanged` are exactly the live-daemon set, and everything else is the report's case.

### M3. `workspace.access` and `ssh.client` are host-measurable, and the report now states something false about them

`crates/gascan-core/src/doctor.rs:214-247` (`runtime_unreachable`) marks **all twenty-one**
checks `Fail` with the daemon's startup cause, and `HostFacts::apply` restores at most four
of them. So on the Arca no-daemon path the report contains, verbatim:

```
workspace.access  fail  engine_environment_incomplete: … GASCAN_ENGINE_BIN must name the engine executable
ssh.client        fail  engine_environment_incomplete: … GASCAN_ENGINE_BIN must name the engine executable
```

Both are false statements, and both are avoidable under the commit's own principle:

- `crates/gascand/src/doctor.rs:262-272` — `workspace_fact` is `canonicalize` + `metadata`
  + `is_dir` **on the path the CLI itself sent in the request** (`cli.rs:423`,
  `crates/gascand/src/api.rs:1902-1909`). It touches no runtime, no engine, no store. The
  CLI can compute it with strictly more authority than the daemon: it is the CLI's own cwd.
- `crates/gascand/src/doctor.rs:275-278, 288` — `ssh_client_fact(Path::new("/usr/bin/ssh"))`
  is a stat of a fixed absolute path. `ssh.identity` and `ssh.config` genuinely need the
  store; `ssh.client` does not.

The new e2e test asserts the daemon's cause reaches `runtime.version`, `runtime.service`,
`runtime.schema`, `storage.state`, `storage.images`
(`crates/gascan-e2e/tests/arca_startup.rs:245-260`) — all correct choices — but the
implementation applies it to `workspace.access` and `ssh.client` too, which no test
examines. The commit message's own framing ("The facts are split by who can measure them")
is not fully carried out, and the miss is user-visible: two of the report's failing lines
blame the engine environment for a directory stat and for `/usr/bin/ssh`.

**Suggested fix.** Move `workspace_fact` and `ssh_client_fact` into
`gascan_core::doctor::host` alongside the other four (both are already
dependency-free helpers), have both `gascand` and the CLI call them, and extend
`HostFacts` / `apply` to cover them. That also removes the daemon-side asymmetry where the
workspace fact is computed in the RPC handler and patched into the report after the fact
(`crates/gascand/src/api.rs:1922-1929`).

### M4. Nothing tests `DoctorCheckId::from_name`, and it is the CLI's ingest path — the round-trip test does not cover it

`crates/gascan/src/cli.rs:441-443` drops any capability whose name `from_name` does not
recognise, silently (`else { continue }`). `from_name`
(`crates/gascan-core/src/doctor.rs:109-134`) is a **fourth** hand-written table over the
same twenty-one variants, alongside `as_str`, `ALL` and `field_mut` — and
`git grep from_name` returns exactly three hits repo-wide (the definition,
`runtime_readiness_failure` at `doctor.rs:602`, and the CLI call). It has no test.

`every_check_id_round_trips_through_a_fact` (`doctor.rs:620-651`) cannot catch a `from_name`
defect: it goes `field_mut` → `into_report` → `report.check(id.as_str())`, so it exercises
`as_str` on both sides of the comparison and never calls `from_name` at all.

**Failure scenario.** Someone renames a check, updates `as_str` to `"runtime.named-volumes"`
and forgets the `from_name` arm (or typos it). Every unit test still passes — `as_str` is
self-consistent. At run time the daemon serialises `name: "runtime.named-volumes"`
(`api.rs:1933`), the CLI's `from_name` returns `None`, the answer is dropped, and the check
renders as `unknown: the daemon did not report this check` (`cli.rs:439`) — the daemon
measured it correctly and the CLI threw it away, which is precisely the failure mode the
round-trip test's doc comment (`doctor.rs:612-618`) claims to have closed.

**Suggested fix.** Add to the same test, inside the existing loop:
`assert_eq!(DoctorCheckId::from_name(id.as_str()), Some(id))`. One line, and it pins the
third and fourth tables to the first two.

---

## Minor

### m5. `DoctorCheckId::ALL` is not pinned to the enum — the round-trip test misses one variant-drop case

Asked directly: **does the test catch a variant missing from `ALL`?** Partly.

- Missing from `ALL`, present in `into_report`: **caught.** `doctor.rs:645-649` asserts
  `report.checks.len() == DoctorCheckId::ALL.len()`, so 21 vs 20 fails.
- Two ids mapped to the same field in `field_mut`: **caught**, via the `detail` assertion at
  `doctor.rs:640-644` (the second id reads back `"not collected"`).
- Missing from **both** `ALL` and `into_report`: **not caught.** Adding a variant forces only
  that `field_mut`'s match stay exhaustive (`doctor.rs:250-274`) — and a new variant may map
  to an *existing* `DoctorFacts` field, so it does not force a new struct field, which is the
  thing `into_report` would notice. `ALL.len()` and `checks.len()` both stay at 21 and the
  test is green while the check simply does not exist.

The gap is narrow (`DoctorRemedies`' exhaustive matches, `doctor.rs:437+`, do force each
backend to name a new variant, so it will not be invisible for long) but the doc comment at
`doctor.rs:52-58` overstates: `ALL` is "paid for by" a test that only checks it against
`into_report`, not against the enum. `std::mem::variant_count` is unstable; the practical
pin is `assert_eq!(DoctorCheckId::ALL.len(), 21)` next to a comment, or a match in a test
that destructures every variant.

### m6. `BackendMismatch` produces no report, when a report is exactly what the user wants

`cli.rs:1241` raises it. The message (`daemon.rs:208-211`) is good and actionable, but the
user typed `doctor` and gets zero facts about a host they were trying to diagnose. Since the
CLI cannot trust the running daemon's runtime facts here (they describe a different backend),
the right shape is a report whose runtime half carries the mismatch as its cause — which is
what `runtime_unreachable` already does. Low urgency: not a regression, the old code failed
here too.

Separately, and pre-existing (not touched by this commit): `daemon.rs:409` contains 18
literal spaces mid-message — `"…expects {expected};                  stop it with…"` — which
reaches the user as-is. Verified with `sed … | cat -A`.

### m7. `plist` is still a production dependency of `gascand`, used only by a `#[cfg(test)]` module

After the move, `git grep plist -- crates/gascand/src/` returns only
`crates/gascand/src/main.rs:1648-1657` (inside `mod doctor_tests`), yet
`crates/gascand/Cargo.toml:30` still lists `plist.workspace = true` under `[dependencies]`.
It should be a dev-dependency, or the test should move with the code (see m8).

### m8. The macOS-version threshold is now tested only from the crate that no longer implements it

`crates/gascan-core/src/doctor/host.rs:204-208` tests only the unreadable-plist case. The
test that a `ProductVersion` of `25.9` **fails** lives at
`crates/gascand/src/main.rs:1647-1663` (`plist_product_version_is_structured_and_requires_26`),
and `host_architecture_mismatch_fails` at `main.rs:1640-1645` likewise. Both still test what
they name — they were rewired to `host::…` and still assert real behaviour — but the
threshold logic now has its only coverage in a downstream crate, so `cargo test -p
gascan-core` passes with a broken `>= 26` comparison. Move both into `host.rs`'s test module
(which also resolves m7).

### m9. `execute_doctor` re-implements `DoctorReport::is_ready`

`cli.rs:483-485` writes `all(|c| c.status == Pass || c.status == Warning)` by hand;
`DoctorReport::is_ready` (`doctor.rs:595-597`) is `all(|c| c.status.is_available())` and
`is_available` (`doctor.rs:15-17`) is exactly `Pass | Warning`. Two expressions of one rule,
in a codebase whose `DoctorRemedies` doc comment argues at length against exactly that.
Use `report.is_ready()`.

### m10. The unreachable `Command::Doctor` arm returns success

`cli.rs:760` — `Command::Doctor { .. } => Ok(0)`. Correct today because of the early return
at `cli.rs:621`, but if that early return is ever moved or made conditional, doctor exits 0
having printed nothing. `unreachable!("doctor returns before the daemon connection")` states
the invariant and fails loudly instead. (The neighbouring `Command::Engine` arm at
`cli.rs:759` has the same shape, so this is consistent with existing style — flagging it as
a shared, pre-existing weakness rather than something introduced here.)

### m11. `the_engine_artifact_check_is_answered_without_a_daemon` reads the developer's real account

`crates/gascan-e2e/tests/arca_startup.rs:295-320` asserts the detail contains
`"engine artifacts"`. `engine_artifact_fact` → `ArtifactPaths::for_user()`
(`crates/gascan-core/src/engine_artifacts.rs:220-225`) → `effective_account_home()`, i.e.
the passwd home, **not** the harness's `GASCAN_E2E_ACCOUNT_HOME` (which
`crates/gascand/src/main.rs:594` and `710` redirect for controller state and SSH only). So
the test reads `~/Library/Application Support/dev.gascan/engine` on the machine running it.
Both common states satisfy it — absent → `"engine artifacts are not installed under …"`
(`host.rs:105-108`), valid → `"engine artifacts under … match …"` (`host.rs:99-103`) — but
the third branch, `host.rs:111`, returns `error.to_string()` for a digest mismatch or a
truncated file, and that string need not contain `"engine artifacts"`. A developer with a
half-fetched artifact tree sees this test fail for a reason unrelated to the change. Assert
on `runtime.kernel`'s *remedy* naming `gascan engine fetch`, or on the absence of the daemon
cause plus a passing/failing status, rather than on prose that only two of three branches
produce.

---

## Verified and found correct

- **The e2e `GASCAN_E2E_ACCOUNT_HOME` divergence you asked about does not exist.** The
  daemon honours it only in `controller_state_paths` (`gascand/src/main.rs:592-604`) and
  `e2e_ssh_paths` (`gascand/src/main.rs:708-717`); the CLI's `configure_e2e_account_home`
  (`gascan/src/ssh_config.rs:285-294`) only seeds `E2E_ACCOUNT_HOME` for SSH path
  resolution. Neither feeds a host fact. `engine_artifact_fact` uses
  `effective_account_home()` in both processes.
- **uid/euid cannot differ between the CLI and a daemon that answered.**
  `crates/gascan/src/client.rs:501-506` rejects any socket peer whose uid is not
  `geteuid()`, so `effective_account_home()` (`crates/gascan-core/src/account.rs:10-14`,
  keyed on `Uid::effective()`) resolves identically. `HOME` is never consulted on this path.
  So `runtime.kernel` (the artifact digest check) really is equal by construction, as
  claimed — it is only `runtime.cli` that is not (C1).
- **Status round-trip through the wire is complete and correct.** The daemon serialises
  `"status": check.status` (`crates/gascand/src/api.rs:1936-1941`) with
  `#[serde(rename_all = "snake_case")]` on `DoctorStatus` (`doctor.rs:5-12`), producing
  `pass` / `warning` / `fail` / `unknown`; `capability_fact` (`cli.rs:510-517`) maps all
  four explicitly and falls back to `capability.available` only when the field is absent or
  unparseable — which matches the old code's fallback exactly. `warning` and `unknown` both
  survive, where the old code passed the string through untyped.
- **Remedies survive the wire rather than being re-derived.** `into_report` fills every
  check's remedy from the producing process's `DoctorRemedies` (`doctor.rs:377-379`), so the
  daemon's per-fact remedy (`engine_artifact_fact`'s "run `gascan engine fetch`") arrives
  non-empty and `capability_fact` (`cli.rs:523-528`) keeps it. The CLI's own
  `remedies_for(backend)` only fills checks the daemon did not answer or that the host half
  overwrote.
- **`remedies_for` cannot hand out the wrong backend's prose on the connected path.**
  `require_matching_backend` is called from `connected_outcome`
  (`daemon.rs:2111`), which the comment at `2105-2110` identifies as the single funnel
  through which any caller receives a connection — verified: it is the only call site. So a
  daemon on backend X is never handed to a CLI whose environment selects Y. The residual
  case is a mismatch, which raises (m6).
- **Exit code is behaviourally identical to the old rule, given the same facts.** Old:
  `doctor.findings.is_empty()`, where `findings` is `Fail | Unknown`
  (`crates/gascand/src/api.rs:1944-1948`). New: `all(Pass | Warning)`. Same partition of the
  four statuses, and the daemon computes `findings` *after* patching the workspace check
  (`api.rs:1922-1929`), so no ordering difference either. The README's "Warning-only reports
  remain ready and `gascan doctor` exits successfully" was true before and is true now. The
  only way the two rules differ on the same daemon is when the host overlay changes a status
  — i.e. C1 — which is a consequence of C1, not an independent defect.
- **`controller_state_errors_survive_all_daemon_start_paths` is not weaker.**
  `crates/gascan-e2e/tests/autostart.rs:429-470`: for `doctor` it *adds*
  `assert!(stderr.is_empty())` and then makes the identical two content assertions against
  stdout; `start` and `restart` are unchanged. Same code, same actionable phrases, same
  `!contains("backend_unavailable")`, same startup-error-file assertion — one channel moved,
  one assertion gained. The new sibling
  `doctor_keeps_its_host_facts_when_the_controller_store_is_unsafe`
  (`autostart.rs:474-501`) covers the half the move made possible.
- **`doctor_recovery_does_not_force_a_held_durable_operation` still proves what it did.**
  `crates/gascan-e2e/tests/doctor.rs:600-622` is untouched and still asserts non-zero exit,
  **empty stdout** (no partial report), `--force` on stderr, the same pid, and the fixture
  still running. `GracefulTimeout` is excluded from `doctor_reports_rather_than_raises`, so
  all five assertions still hold — the empty-stdout one in particular is what pins the
  classification.
- **The refactor left no orphan in `gascand/src/main.rs`.** `architecture_fact`,
  `macos_fact`, `macos_fact_at` and `engine_artifact_fact` are gone and both call sites go
  through `host::HostFacts::collect(...).apply(...)`
  (`gascand/src/main.rs:722-725` Apple, `765` Arca). `DoctorFact` is still used
  (`main.rs:764`), and `storage_fact`, `apply_cli_error`, `service_error_fact` are unrelated
  and still exercised. The only leftover is the `plist` dependency (m7).
- **The doctor early return sits in the established place**, immediately after
  `Command::Engine`'s (`cli.rs:600-622`), and skips nothing the doctor needs — the
  `Configure` preflight below it is command-specific.
- **`HostFacts::collect` correctly declines Apple's `runtime.cli` / `runtime.kernel`**
  (`host.rs:163-171`, pinned by `only_the_engine_backend_contributes_engine_facts` at
  `host.rs:225-233`), and `apply` leaves every runtime field alone
  (`apply_leaves_every_runtime_fact_alone`, `host.rs:237-246`). Those are the two facts a
  host cannot measure and they are not invented.
- **The commit message's MEASURED block is consistent with the code.** With
  `GASCAN_ARCA_BACKEND` set and `GASCAN_ENGINE_BIN` unset, `runtime.cli` fails naming the
  variable (`host.rs:123-129`), `runtime.kernel` is the artifact check independent of the
  daemon (`host.rs:169`), and `runtime.service` carries
  `engine_environment_incomplete` from the daemon's diagnostic
  (`gascand/src/main.rs:361-362` → `DaemonStartup` → `runtime_unreachable`). I did not
  re-run the command; the mechanism checks out.
- **No progress-line residue on the reported-failure path.** `observer.finish()` is skipped
  when `connect_with_recovery_progress_reporting` returns `Err` (`cli.rs:1222-1224`), but
  the observer is a local and `OperationProgress`'s `Drop` calls `clear()`
  (`crates/gascan/src/presentation.rs:664-668`), and in `--json` mode the observer is
  `Suppressed` outright (`cli.rs:1141-1145`). I checked this specifically because the new
  e2e test asserts `stderr.is_empty()`.

---

## Severities with no findings

None at any severity were omitted for space. There is **one Critical** (C1), **three
Majors** (M2, M3, M4), and seven Minors. I found no correctness defect in `capability_fact`'s
status mapping, in the exit-code rule itself, in `remedies_for`'s backend mapping, or in
either of the two behaviour-changed tests.
