<!--
Committed verbatim as written by the reviewer. Reviewed synchronously over
`c0679c6..fb7d4b0` before either pull request left draft; the fixes are
`de14a94`, whose message lists what was addressed and what was not.

Scope of this file: the startup diagnostic channel (f081e61).
-->

# Review — `f081e61` startup diagnostic channel

Repository `/Users/kiener/code/gascan`, branch `feat/milestone-4-product-wiring`, one commit:
`f081e61` "every Arca startup failure reaches the user by name, not as a readiness timeout".

All line references are against the tree at `f081e61`.

**Critical findings: none.** Three Major, eight Minor.

---

## Major

### M1 — The message crossing the channel is unbounded, unsanitized, and printed raw to the terminal

**Where:** `crates/gascan/src/presentation.rs:84`–`106` (`render_error`),
`crates/gascan/src/cli.rs:1274`–`1280`, `crates/gascan/src/daemon.rs:549`–`572`
(the validation site), `crates/gascand/src/main.rs:517`–`546` (the write site).

**What is wrong.** The whitelist bounds the `code`. Nothing bounds the `message`.
`serde_json` escapes control bytes on the way *out* (`main.rs:528-537`), and the
reader decodes them straight back (`daemon.rs:552`), so ESC (`0x1b`), CR, LF, BS
survive the round trip byte-for-byte. `controller_error()` checks the code
against the whitelist, checks the message is non-empty after `trim()`, checks the
owner token — and then hands the message through unaltered. `CliError::message()`
returns it verbatim (`cli.rs:249-270`), and `render_error` writes
`format!("{error_label} {message}\n")` to stderr with no filtering. The only
length control is the 64 KiB file cap, and a message that exceeds it is *dropped
whole* rather than truncated (`main.rs:484`, `main.rs:493-498`) — an over-long
diagnostic silently becomes a readiness timeout again.

**Failure scenario.** A repo `.envrc` (or any wrapper that sets the daemon's
environment) sets

```
GASCAN_ARCA_BACKEND=1
GASCAN_ENGINE_SOCKET=$'/tmp/x\e[2J\e[1;1H  All checks passed.\e[?25l/e.sock'
```

and creates that socket path owned by another uid. `require_own_socket`
(`crates/gascand/src/engine.rs:250-274`) returns `EngineError::ForeignSocket`,
whose `Display` embeds `path.display()` (`engine.rs:89-94`). That is routed
through `reported(error.code(), &error)` at `main.rs:380` as the whitelisted code
`engine_socket_foreign`. The CLI prints it: the ESC sequences clear the user's
screen, reposition the cursor, and paint an attacker-chosen line where the error
should be. `engine_not_listening` gives the same primitive (`engine.rs:95-99`).
Newlines in the message let one diagnostic render as several lines that look like
independent CLI output.

**Is this new?** The class is not — `ControllerStateError::Unsafe(format!(…))`
(`controller_state.rs:418`, `1102`, `1120`) and `Conflict { durable, legacy }`
(`controller_state.rs:51-53`, whose `Display` already contains literal `\n\n`)
embed `PathBuf`s today. But the exposure widened: the writers whose message text
is path- or OS-error-derived went from two to five, and the commit message's
claim that the channel "is already hardened — owner uid, mode, `nlink == 0`, a
size bound, an owner-token match, and a closed whitelist of codes" describes
controls on *provenance and the code*, none of which touch the message. Widening
the whitelist did not weaken it; it enlarged the number of ways to reach an
unguarded rendering path that was already there.

**Fix.** Sanitize once, at `DaemonStartupMonitor::controller_error()`
(`daemon.rs:559`), beside the whitelist check every consumer already goes
through: reject or escape C0/C1 control characters, and cap the message (4 KiB is
generous for every message any current writer produces). Truncate rather than
discard, so an over-long message still names its cause. Doing it at the reader
rather than the writer is the right side — the reader is the one that does not
trust the writer.

---

### M2 — The new descriptor lifetime makes `CLOEXEC` load-bearing, and no test asserts it

**Where:** `crates/gascan-inherited-fd/src/lib.rs:68`–`70` (the flag),
`crates/gascand/src/engine.rs:338`–`348` (`TokioEngineSpawner::spawn`),
`crates/gascand/src/main.rs:373`–`381` (the spawn now happens while the fd is
held).

**What is wrong.** Before this commit the diagnostic descriptor was dropped as
soon as the controller store opened, which was before the backend arm — the
daemon never spawned a child while holding it. Now `ensure_engine` spawns
`GASCAN_ENGINE_BIN` with the descriptor open. `tokio::process::Command` inherits
every non-`CLOEXEC` descriptor and the entire environment; `TokioEngineSpawner`
does no `env_clear` and no fd hygiene. The single thing that keeps the engine
from inheriting a writable descriptor on the diagnostic file is
`rustix::io::fcntl_setfd(&owned, flags | FdFlags::CLOEXEC)` at
`gascan-inherited-fd/src/lib.rs:69`.

I verified that flag *is* set, so this is currently correct. The defect is that
the two tests in that module (`lib.rs:110`, `lib.rs:135`) cover EBADF on a stale
descriptor and rejection of a second claim, and **neither asserts the flag**. A
property that was previously irrelevant is now the boundary, and it is untested
at exactly the moment it became load-bearing.

**Failure scenario.** Someone reorders or removes `lib.rs:69`–`70` (it reads like
tidy-up next to the `from_raw_fd` call), or a refactor introduces a second claim
path. The whole workspace stays green. The engine process then holds fd N open.
`GASCAN_DAEMON_OWNER_TOKEN` is already in its environment — the daemon inherits
it from the CLI (`crates/gascan/src/client.rs:375`) and passes its whole
environment to the engine — so the engine has both halves of the forgery control.
It writes

```
GASCAN_CONTROLLER_STARTUP_ERROR {"code":"engine_exited","message":"<anything>","owner_token":"<from env>"}
```

to fd N, and the CLI prints it as the daemon's own diagnostic, with a code the
newly widened whitelist accepts.

**Fix.** (a) A test in `gascan-inherited-fd` asserting
`fcntl_getfd(&claimed)?.contains(FdFlags::CLOEXEC)` after a successful claim.
(b) Defense in depth in `TokioEngineSpawner::spawn`:
`.env_remove("GASCAN_CONTROLLER_STARTUP_FD").env_remove("GASCAN_DAEMON_OWNER_TOKEN")`
— the engine has no business holding either, and this closes the token half
independently of the fd half.

---

### M3 — `debug_assert!` is the wrong guard, and release loses the diagnostic silently

**Where:** `crates/gascand/src/main.rs:522`–`525`.

**What is wrong.** In a release build the assertion is compiled out. An unlisted
code is then written to the file, passes every provenance check on the read side,
and is dropped by `is_accepted` at `daemon.rs:559` without a trace. The daemon
exits; the CLI waits out the readiness bound and reports
`daemon_readiness_failed`. That is precisely the "written, validated, and then
silently discarded" failure the module doc says the shared table closes — the
shared table closes it for *drift between two lists*; it does not close it for a
typo'd or newly-introduced code, and the only detector for that case is absent
from the binary users run.

**Cost, measured.** I reproduced the unreported state (see Minor 2 for the exact
mutation and command): the CLI took **151.49 s** to print `started daemon did not
become healthy and current (state Stopped)`, versus ~1.2 s for the named cause.

**Fix (preferred).** Make an unlisted code unrepresentable rather than asserted.
In `gascan_core::startup_diagnostic`:

- `#[derive(Clone, Copy, Eq, PartialEq)] pub struct StartupCode(&'static str);`
- eleven `pub const` values, `pub const fn as_str(&self)`, and
  `pub fn from_wire(code: &str) -> Option<Self>` for the reader.
- `report_startup_error` takes `StartupCode`; `ControllerStateError::code()` and
  `EngineError::code()` return `StartupCode`.

The whitelist then holds in every profile by construction, and both the
`debug_assert!` and `is_accepted` disappear. This is the SOLID form of what the
commit already intends — one authority for the set, enforced rather than
documented.

**Fix (minimum).** Promote to a hard `assert!`. A daemon that is already on its
way out of `run()` loses nothing by panicking, and a panic is at least visible in
`GASCAN_DAEMON_STDERR_PATH`.

---

## Minor

### m1 — Commit message: "these strings are what a `--json` consumer branches on" is false

`SupervisorError::DaemonStartup` becomes `CliError::DaemonOperation`
(`cli.rs:1274-1280`), and **no `--json` path serializes a `CliError`.** The two
JSON error envelopes — `render_pre_stream_client_error` (`cli.rs:1686-1701`) and
`json_operation_error` (`cli.rs:1703-1711`) — carry `ClientError` /
`v1::Error` from a *connected* daemon. The commit's own new test says so:
"Not `--json`: that path renders errors a *connected* daemon returned, and a
daemon that never started returned nothing" (`arca_startup.rs:102-107`).

Seven distinct codes is still the right call — they are what the human
`Error: {code}: {message}` line carries, and one bucket would erase the
distinction. But the justification given contradicts the code and the test
shipped in the same commit.

### m2 — Commit message: the mutation evidence says "both fail"; three of four fail

`crates/gascan-e2e/tests/arca_startup.rs` contains four `#[test]` functions, not
two. **MEASURED:** I reproduced the first mutation ("restoring the early
`drop(startup_diagnostic)`") in its behavioural form — the Arca arm's `reported`
closure passing `None` instead of `startup_diagnostic.as_mut()` at
`crates/gascand/src/main.rs:341` — and ran
`cargo test -p gascan-e2e --test arca_startup`:

```
test result: FAILED. 1 passed; 3 failed; 0 ignored; ... finished in 151.49s
```

Failing: `a_missing_engine_variable_reaches_the_user_by_name`,
`an_engine_that_cannot_be_spawned_reaches_the_user_as_an_engine_error`,
`doctor_reports_real_host_facts_and_names_the_runtime_cause` — each reporting
`started daemon did not become healthy and current (state Stopped)`. Passing:
`the_engine_artifact_check_is_answered_without_a_daemon` (it asserts the artifact
check does *not* fall through to the daemon's cause, which is still true when
there is no daemon cause at all). The mutation was reverted with
`git checkout -- crates/gascand/src/main.rs`.

So the mutation proves *more* than the commit claims, and the claim as written
does not match the file. I did **not** run the second mutation (`ACCEPTED_CODES`
back to four): an uncommitted edit to `crates/gascan/src/cli.rs` from another
agent appeared in the shared working tree during my first mutation run, and a
result built on top of a half-finished concurrent edit would not be attributable.

### m3 — The stated reason for dropping in `run_daemon` is not what the CLI does

`crates/gascand/src/main.rs:620`–`628` and the commit message both assert that
`gascan` "reads it still being open as startup still being in progress." In
production `TokioDaemonSpawner` hands over an unlinked **regular file**
(`client.rs:333-347`), and `DaemonStartupMonitor` never observes whether the
child still holds its copy — `controller_error()` (`daemon.rs:521-572`) only
`stat`s and `pread`s the file. The e2e contract detects closure only because it
substitutes a **pipe** (`autostart.rs:578-604`), which is not the production
channel. The companion claim that holding the fd to the end of `run()` also
failed `autostart_waits_for_a_slow_but_healthy_daemon` (`autostart.rs:938-959`)
has no mechanism I can find: that test uses the normal spawner and a regular
file, and asserts only success and a 15 s bound.

The design is right — see V4 — but the reason recorded next to it will mislead
the next reader into believing the CLI has a liveness signal it does not have.

**Fix.** Restate the comment on what is actually true: the descriptor is a
resource the serving daemon has no use for, the pipe-based contract asserts the
release, and the single drop site is what keeps a fourth arm from diverging.

### m4 — `successful_daemon_closes_inherited_startup_diagnostic_descriptor` covers one arm, not three

`Environment::configure_command` sets `GASCAN_TEST_FAKE_BACKEND=1`
(`autostart.rs:248`) and `GASCAN_STATE_PATH` (`autostart.rs:236`). The contract
therefore exercises **only the `Fake` arm**, and not the controller-store path
either. Apple and Arca are uncovered. Structurally that is defensible — the drop
is in the one function all three arms funnel into (`main.rs:638`) — but it is not
what the commit implies, and nothing would catch a future arm that reached
serving without going through `run_daemon`.

**Fix.** Either say so in the test's doc comment, or parameterise the contract
over `BackendSelection` (the Arca arm can be driven to serving only with an
engine, so realistically: document the limit and rely on the single drop site).

### m5 — `reported` duplicates `startup_error`

`main.rs:339-343` and `main.rs:552-560` are the same three lines. `reported`
cannot call `startup_error` only because the latter is `E: std::fmt::Display`
(implicitly `Sized`) while `reported` passes `&dyn Display`. Changing the bound
to `E: std::fmt::Display + ?Sized` lets `reported` be
`|code, error| startup_error(code, error, startup_diagnostic.as_mut())` and
removes the copy.

### m6 — Unrouted `?` outside the Arca arm (scope note)

The commit's title scopes this to Arca and the Arca arm is complete (see Q3
below). Same-class gaps remain elsewhere in `run()`:

- `main.rs:291`, `main.rs:292` — the Apple arm's
  `AppleAttach::configured_from_environment()?` and
  `E2eProcessRunner::configured_from_environment()?` are unrouted and run in
  production. An Apple user with a malformed attach variable still gets
  `daemon_readiness_failed`.
- `main.rs:267` — `Store::open(GASCAN_STATE_PATH)?` is unrouted, and
  `GASCAN_STATE_PATH` is not `cfg`-gated.
- `main.rs:275` — `e2e_ssh_paths()?` is unrouted (e2e-only in practice).
- `main.rs:244-245` — `SocketPaths::for_user()?` / `prepare_directory()?` are
  unrouted but largely masked: the CLI resolves the same paths first
  (`daemon.rs:2249`) and fails identically before spawning.
- `main.rs:255` — `backend_from_environment()?` can fail only when FAKE and ARCA
  are both requested, and `fake_requested` is hard-`false` in release
  (`backend.rs:182-187`), so it is release-unreachable.

**Fix.** Route the two Apple-arm calls through `reported` with a
`daemon_environment_incomplete`-style code (a twelfth table entry), or state in
the arm why they are deliberately left bare.

### m7 — `arca_startup.rs` is coupled to macOS/aarch64 while claimed for "every push"

`tempdir_in("/private/tmp")` (`arca_startup.rs:52`) is macOS-specific, and
`doctor_reports_real_host_facts_and_names_the_runtime_cause` asserts
`host.architecture` and `host.macos` are `"pass"` (`arca_startup.rs:218-226`) —
i.e. it asserts the runner really is an aarch64 macOS 26+ host. The commit places
this file in "the tier that runs on every push". If that tier ever runs anywhere
else, these fail for reasons that have nothing to do with the diagnostic channel.
Worth a one-line note in the module doc, or a `#[cfg(target_os = "macos")]`.

### m8 — The owner token is echoed to the daemon's stderr on every reported failure

`main.rs:541` does `eprint!("{diagnostic}")` — the full JSON line, `owner_token`
included — in addition to writing it to the unlinked file. With
`GASCAN_DAEMON_STDERR_PATH` set (not `cfg`-gated; `daemon.rs:2283`) that lands on
disk and stays there, while `autostart.rs:543-548` asserts the *token-bearing
diagnostic file* must not survive a successful startup. Pre-existing, but this
commit makes it fire on seven more failure paths.

**Fix.** Print `"{code}: {message}"` to stderr and reserve the token-bearing JSON
for the descriptor.

---

## Answers to the specific questions

**Q1 — Did widening the whitelist weaken it?** No new injection class. Every code
a writer can emit is a `codes::` constant or the return of a `const fn` that
returns one, so the code field remains closed (see Q4). What the widening does is
multiply the paths that reach an *unguarded message-rendering* boundary: see M1.
The message is bounded only by the 64 KiB file cap, is not sanitized, and is not
safe to print to a terminal. It cannot forge a second diagnostic *line in the
file* — `serde_json` escapes `\n`, `\r` and all C0 bytes before the single
trailing newline (`main.rs:528-537`), so one write is exactly one parseable line
— but it can forge additional *rendered* lines on the user's terminal, and can
carry ANSI escapes.

**Q2 — Descriptor lifetime.**
(a) Every path that reaches serving drops it. All three arms pass it by value —
`main.rs:307` (Apple), `main.rs:409` (Arca), `main.rs:464` (Fake) — and
`run_daemon`'s first statement is `drop(startup_diagnostic)` (`main.rs:638`).
There is no other route to serving; `main()` calls only `run()`.
(b) No path holds it for the daemon's lifetime. Every early return from `run()`
drops it at scope exit; the longest hold is now across `ensure_engine`, which is
bounded by `EngineReadiness`.
(c) The e2e contract covers **one** arm — Fake — not three. See m4. The two
uncovered arms are Apple and Arca, plus the controller-store path (the test sets
`GASCAN_STATE_PATH`).

**Q3 — The `reported` closure and the later move.** The borrow is correct. NLL
ends the closure's mutable borrow of `startup_diagnostic` at its last use
(`main.rs:383`), before the move at `main.rs:409`; `cargo clippy --workspace
--all-targets -- -D warnings` exits 0 on the tree.

Every fallible operation in the Arca arm, in order:

| site | operation | routed? | code |
|---|---|---|---|
| `main.rs:358-359` | `ArtifactPaths::for_user()` | yes | `engine_artifacts_unavailable` |
| `main.rs:361-362` | `required(ENGINE_BIN_ENV, …)` | yes | `engine_environment_incomplete` |
| `main.rs:363-364` | `required(ENGINE_SOCKET_ENV, …)` | yes | `engine_environment_incomplete` |
| `main.rs:365-366` | `required(ENGINE_STATE_ROOT_ENV, …)` | yes | `engine_environment_incomplete` |
| `main.rs:378-380` | `ensure_engine(…).await` | yes | `EngineError::code()`, all four variants |
| `main.rs:381-383` | `ChannelTransport::connect(…).await` | yes | `engine_transport_unavailable` |
| `main.rs:399-410` | `run_daemon(…).await` | n/a | after the drop, by design |

**No failure path in the Arca arm bypasses the writer.** The unrouted `?`s are
all *before* the arm, in shared `run()` prologue — see m6.

**Q4 — Code table integrity.** Exhaustive, and both directions hold.

*Can a writer emit a code not in the table?* No. `report_startup_error` has
exactly three call sites: `main.rs:270` and `main.rs:272` (via
`controller_startup_error` → `ControllerStateError::code()`, `const fn`, all four
variants → four table entries), and `main.rs:341` (via `reported`, called with
three `codes::` literals and with `EngineError::code()`, `const fn`, all four
variants → seven table entries). 4 + 7 = 11 = `ACCEPTED_CODES.len()`. Every
argument is a table constant. The `debug_assert!` guards a case that today's code
cannot produce — which is exactly why it should be a type, not an assertion (M3).

*Is any table entry dead?* No. All four `ControllerStateError` variants are
constructed (`controller_state.rs:140`, `202`, `247`, `418`, `427`, `541`,
`623`, `1147`, among others). All four `EngineError` variants are reachable from
`ensure_engine`: `Io` at `engine.rs:266` and via `spawner.spawn(launch)?`
(`engine.rs:289`), `ForeignSocket` at `engine.rs:270`, `Exited` at
`engine.rs:322`, `NotListening` at `engine.rs:325`. `engine_artifacts_unavailable`
and `engine_transport_unavailable` are emitted at `main.rs:359` and `main.rs:383`.

One caveat: the four `controller_state_*` codes are reachable only when
`GASCAN_STATE_PATH` is unset (`main.rs:266-274`). With it set, `Store::open` fails
into an unrouted `?` — the failure exists, the code just is not the one used to
report it.

**Q5 — `debug_assert!`.** See M3. Release behaviour: the unlisted code is
written, passes uid/mode/`nlink`/size/token validation on the read side, and is
dropped by `is_accepted` at `daemon.rs:559`; the user gets `daemon_readiness_failed`
after the readiness bound (measured at 151.49 s).

**Q6 — The rename.** Verified complete and behaviour-preserving. The only
surviving occurrence of `ControllerStartup` is inside a doc comment
(`daemon.rs:315`). `DaemonStartup` is handled at `daemon.rs:319` (declaration),
`381` (`Display`, unchanged formatting `"{code}: {message}"`), `424` (the
terminal-failure grouping, same position in the same `matches!`), `cli.rs:1274`
(early return, unchanged body) and `cli.rs:1291` (the `unreachable!` arm), plus
the two unit tests at `daemon.rs:3544` and `daemon.rs:3614` which changed only
the variant name.

**The wire/JSON `code` field did not change.** It is `diagnostic.code`, a string
originating in `gascan_core::startup_diagnostic`, never the Rust variant name; a
consumer reading `stable_code()` sees the same `controller_state_*` strings as
before. What did change user-visibly is the *set* of strings that can appear on
the `Error: {code}: {message}` line — additive, seven new — and, per m1, that line
is stderr text, not a `--json` field.

**Q7 — The new tests.** Load-bearing, with two weak spots.

`startup_failure()` (`arca_startup.rs:108-117`) takes the first stderr line with
the prefix `Error: `. Mildly fragile, not currently wrong: the daemon's stderr is
`Stdio::null()` in these tests (no `GASCAN_DAEMON_STDERR_PATH`), so only the CLI
writes there, and `render_error` emits an ANSI-wrapped label when
`capabilities.color` — which is off under `Command::output()` because stderr is
not a tty. That is an implicit dependency on colour detection, worth a comment.

Would it pass with the wrong code and the right message? **No** —
`failure.starts_with(&format!("{code}: "))` pins the code
(`arca_startup.rs:142`, `172`). Right code, wrong message? **No** —
`contains(variable)` and `contains(expected_fragment)` pin the text
(`arca_startup.rs:146`, `150`, `176`). The mutation in m2 fired all three of
these assertion kinds, so this is verified, not argued.

Weak/near-vacuous assertions, named as asked:

- `arca_startup.rs:279-283` — `!detail.contains(code)` is vacuous in isolation:
  `kernel["detail"].as_str().unwrap_or_default()` makes a *missing* `detail` an
  empty string, which trivially passes. The paired positive at line 284
  (`detail.contains("engine artifacts")`) rescues it. This is also the only test
  in the file that never asserts the exit status, so it passes whether `doctor`
  exited 0 or non-zero.
- `arca_startup.rs:288-296` — the remedy loop uses
  `check_value["remedy"].as_str().unwrap_or_default()`, so a check with no
  `remedy` field passes silently. Weak, not wrong.
- Environment coupling: see m7.

Everything else in the file is load-bearing, and the per-variable loop
(`arca_startup.rs:127-153`) removing one variable at a time is the right shape —
it catches a daemon that reports only the first thing it checks.

---

## Verified and found correct

- **V1 — CLOEXEC is set today.** `gascan-inherited-fd/src/lib.rs:68-70`. The
  engine spawned at `engine.rs:340` does not inherit the diagnostic descriptor.
  (Untested — M2.)
- **V2 — One write is exactly one line.** `serde_json::json!` escapes `\n`, `\r`
  and all C0 bytes in both `code` and `message` before the single trailing
  newline (`main.rs:528-537`), so a message cannot forge a second parseable
  diagnostic line in the file.
- **V3 — The read side's validation chain is intact and correctly ordered.**
  `daemon.rs:526-534` (regular file, owner uid, mode `0600`, `nlink == 0`, size
  bound) before any read; then prefix, JSON shape, whitelist, non-empty trimmed
  message, and owner-token equality (`daemon.rs:549-565`). The token comparison
  is what stops an unrelated same-uid process that somehow obtained the fd.
- **V4 — Descriptor lifetime is structurally correct.** All three arms pass by
  value; `run_daemon` drops as its first statement (`main.rs:638`). A fourth arm
  cannot forget, because it cannot call `run_daemon` without surrendering the
  value. (The *rationale* recorded beside it is wrong — m3 — but the structure is
  right, and it is better than three copies of a drop.)
- **V5 — The `reported` closure's borrow is sound**; last use at `main.rs:383`,
  move at `main.rs:409`.
- **V6 — The whitelist genuinely has one home.** Both `ControllerStateError::code()`
  (`controller_state.rs:69-77`) and the reader (`daemon.rs:559`) now draw on
  `gascan_core::startup_diagnostic`, and `gascan-core` is depended on by both
  `gascan` and `gascand` with no dependency between them. The two unit tests in
  the module (`startup_diagnostic.rs:70-95`) are non-vacuous: the distinctness
  test would catch a rename that left a duplicate, and the negative test would
  catch an `is_accepted` that returned `true` unconditionally.
- **V7 — All four `EngineError` variants are reachable from `ensure_engine`**, so
  the seven new codes are not speculative.

### Commands run (this tree, `f081e61`, working tree clean at the time)

| command | result |
|---|---|
| `cargo fmt --all --check` | exit 0 |
| `cargo clippy --workspace --all-targets -- -D warnings` | exit 0, no issues |
| `cargo test -p gascan-e2e --test arca_startup` | 4 passed, 1.54 s |
| same, with `main.rs:341` mutated to pass `None` | 1 passed / 3 failed, 151.49 s (see m2) |

**Not verified here:** the commit message's `cargo test --workspace` "1473 passed
/ 0 failed / 49 ignored", `scripts/ci-check-ignored-tests.sh`, and
`scripts/ci-run-release-contracts.sh`. I did not run the workspace suite (it is
known to wander under concurrent load, and other agents were building in this
tree during the review).

**Working-tree caution:** an uncommitted edit to `crates/gascan/src/cli.rs` from
another agent appeared in the shared tree during my mutation run. I reverted only
`crates/gascand/src/main.rs` (`git checkout -- crates/gascand/src/main.rs`) and
left that edit untouched. If another agent also edited
`crates/gascand/src/main.rs` in that ~152 s window, my revert would have
discarded it — verify against your own expectations before continuing.
