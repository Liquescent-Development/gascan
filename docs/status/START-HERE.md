# START HERE

This file is the session entry point. It is written to be read cold, and it is
addressed to you, the agent. Follow it as instructions — there is nothing to paste.

Written 2026-08-08 at commit `a9cb67c`, branch `feat/gascan-arca`.

---

**P5.2 IS COMPLETE.** All ten tasks landed, the final whole-branch review returned
`request_changes`, both must-fix findings were fixed with mutation proofs, and the fix
wave's re-review returned **merge**. The branch is pushed and ready.

**VERIFIED at `a9cb67c`, measured alone on an otherwise idle machine:**

| Gate | Result |
|---|---|
| `cargo test --workspace --no-fail-fast` | **rc=0 — 1433 passed, 0 failed, 22 ignored** |
| `cargo clippy --workspace --all-targets -- -D warnings` | rc=0, **nothing allowed** |
| `cargo fmt --all --check` | rc=0 |
| `./scripts/ci-run-release-contracts.sh` | rc=0, **15/15** |

1433 accounts for itself: 1432 measured at `10821dd`, plus the one test the final fix
wave added. **Count only `test result:` lines reporting `0 filtered out`** — see the
instrument trap below.

## If the branch is not yet merged, that is your first task

Use `superpowers:finishing-a-development-branch`. **Merge only — never squash, never
rebase, and only via a PR. Never commit to `main`.** Both repositories forbid squash-
and rebase-merge. The permission classifier refuses `gh pr merge`; ask the maintainer to
run it with `!`.

## What P5.2 built

`crates/gascan-arca` implements `gascan_core::runtime::RuntimeBackend` over Arca's
published gRPC engine contract, behind an `EngineTransport` seam stated in **wire types**,
so every mapping is tested without a live engine. It also extracted the shared ownership
classifier into `gascan-core`, which `gascan-apple` now uses too.

`ChannelTransport` (the real `tonic` arm) **ships with no tests, by explicit ruling** —
the only thing that could answer it is a live engine or a Rust server, and a Rust server
is forbidden here because a test double would make a wrong client look correct. The
compiler checking it against `EngineTransport` is the stated assurance. Do not
re-litigate this and do not "fix" it by adding a double.

## What to do next

**Nothing in `gascan-arca` is wired to anything yet.** Nothing outside the crate
references `gascan_arca` or `ChannelTransport::connect`. That is expected — wiring is
P5.1 — but do not assume it is done.

- **P5.1 — wire the backend to the daemon.** This is where every unverifiable claim in
  Task 9 gets answered. Read "What P5.1 will discover" below **before** you start.
- **P5.3 — extract the conformance suite** from `fake_runtime.rs` and run it against the
  fake, apple and arca backends. Several parity properties are defended by single tests
  that P5.3 should absorb — including the one below.
- **P4** — Docker removal in Arca. **P3.3** — publish and version the proto (`buf
  breaking`); still inert because `buf` is not installed and Arca has no CI (P2.3).

### The one piece of cohesion work this branch left half done

**The `sandbox_id`-claim rule is duplicated verbatim in both backends** —
`gascan-arca/src/translate.rs` (`runtime_resource`) and `gascan-apple/src/inspect.rs` —
each with a long comment warning that the two must not diverge, and each with its own
test. The classifier was extracted to `gascan-core`; **the rule this branch exists to
protect was not.** It has been reverted twice as a regression. Sharing it — a core
function returning `(Option<SandboxId>, ResourceOwnership)` — would replace two comments
and two tests with one definition. P5.3 is the natural home.

### What P5.1 will discover, recorded so it is not discovered the hard way

- **The client half-close may arrive as `RST_STREAM`, not a clean half-close.** Dropping
  `ExecStream` drops the sender, which ends the request stream — but dropping `streaming`
  *resets* the h2 stream, so a real engine may see a reset where the fake sees a clean
  close. Same outcome, different wire event. **Whether Arca treats a mid-exec reset as
  cancellation or as an error is unanswered**, and the exec cancellation test pins the
  outcome against the fake and structurally cannot see the difference.
- **Exec teardown is engine-paced where it is not bounded.** Both relay tasks are now
  bounded on `Sender::closed()`, but a real engine still decides when its own half ends.
- **Everything on the wire is unverified**: that Arca accepts this client's `Exec` framing
  frame for frame, that `LogsChunk` ordering and end-of-stream behave as the contract
  describes, that a real server ignores the placeholder authority, and every error path
  through `connect` — no socket was ever dialed.

## Traps that will cost you if you learn them the hard way

**COUNTING `test result:` LINES OVERCOUNTS THE WORKSPACE.** The log has 76 such lines but
only 73 targets. Three come from **child processes** that re-exec a test binary with a
filter, and they are identifiable because a plain `cargo test --workspace` applies no
filter — so every genuine target reports `0 filtered out` while a child re-run reports a
non-zero count. Sum only the `0 filtered out` lines, and check that the count of them
equals the target count.

**NEVER RUN THE WORKSPACE SUITE WHILE ANY SUBAGENT IS RUNNING CARGO.** Run it alone, after
`pgrep -fl "cargo test"` comes back empty, writing to a path nothing else shares. Three
concurrent suites against one target directory produced **rc=101 with 59 failures** across
ten `gascand`/`gascan-e2e` binaries, none of them real: those tiers spawn daemons and bind
sockets, so they starve each other (`AddrInUse`, one `autostart` at 342s). Run alone it
takes **93 seconds**. Partial reads at 302 and 329 passed looked reassuring only because
the failures came later.

**`git checkout <path>` IS NOT A PERSONAL UNDO IN A SHARED TREE.** It discards every
uncommitted change to that path, including another agent's in-flight work. Two agents
collided in this tree on 2026-08-08 doing exactly this. Check `git status` for a
concurrent writer first, and undo your own edits with a targeted edit.

**NEVER PUT CONTROLLER STEPS IN A TASK OWNED BY A SUBAGENT.** The task tracker is an
instruction surface, not a status board. A task reading "dispatch the final reviewer…"
while owned by the final reviewer caused it to start work already assigned elsewhere.

**A STAGED BRIEF IS A CACHE AND IT GOES STALE.** Re-extract every brief immediately before
dispatch. A brief staged one session earlier was missing the drop-cancellation test its own
audit had added — dispatching it would have silently dropped the single most important test
in that task.

**THE INSTRUMENT KEEPS BEING NARROWER THAN THE CLAIM.** This is the defect this project
pays for over and over; it cost six fix rounds across four tasks in one session. Every
instance looked convincing until someone checked: a test asserting `code()` while appearing
to verify a whole mapping; a fixture alternating `T,F,T,F,T,F` that still could not see half
its transpositions; a test *named* `start_stop_…` that never called `stop`; a
drop-cancellation test whose every exit was cancellation-independent, so no mutation of the
wiring could fail it; and a report whose prose outran its own numbers, twice. **Check a fix
as hard as you checked the defect, and prefer reading an artifact to grepping it.**

**A GREEN FIGURE YOU CANNOT ACCOUNT FOR IS NOT A PASS.** Account for every increment against
a per-target table you can re-derive by reading `running N tests` lines. A per-task estimate
went stale three times in one session.

**SUBAGENTS GO IDLE WITHOUT REPORTING** — nine times in one session, twice after committing
but before writing the report. **Never read silence as success:** `git log` and a grep for
the expected symbol establish the real state. A nudge has always retrieved the work intact.

**`RUSTUP_TOOLCHAIN=1.95.0` is exported** and overrides `rust-toolchain.toml` — prefix every
cargo command with `env -u RUSTUP_TOOLCHAIN`. Use `--no-fail-fast`. Confirm the `running N
tests` line, because a bare test name silently runs zero and exits 0. `cargo clippy --fix`
is prohibited here. `ls` is aliased to something that rejects trailing-slash paths — use
`find` or `git ls-files`.

**SIGNING AND TRANSPORT DO NOT USE 1PASSWORD ON THIS REPO.** `user.signingkey` is
`~/.ssh/gascan-signing`, a file PATH, which is why no agent is needed; transport is HTTPS via
`gh`'s credential helper. All `--local`. Commit with `env -u SSH_AUTH_SOCK git commit`.
**NEVER `--no-gpg-sign`** and never a lightweight tag. Verify `%G?` is `G`. No co-author
trailer and no AI-tool mention in any commit message.

**A DOCS-ONLY CI RUN SKIPS `rust` AND `engine` ENTIRELY** (VERIFIED, run `31262534703`), so a
green docs run is not evidence about anything in Rust.

## Do not write D7's narrowed retry

No `0200` occurrence has fired since the instrument landed. The retry is approved in
principle and stays unwritten until a run names which of the two `0200` states fired. A
failing run containing the D7 test's *name* is not a D7 occurrence — check the message
(`mode 0200 …`), never the test name.

## Where the detail lives

`docs/status/arca-integration-handoff.md`, from `## Session of 2026-08-08 (second)`, and
`docs/status/next-session-kickoff.md` for the full trap list. The SDD ledger at
`.superpowers/sdd/2026-08-08-gascan-arca/progress.md` recorded every ruling and deviation
during execution; it is deleted once the branch merges, because git history is the record
from then on.
