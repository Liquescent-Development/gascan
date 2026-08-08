# START HERE

This file is the session entry point. It is written to be read cold, and it is
addressed to you, the agent. Follow it as instructions — there is nothing to paste.

Written 2026-08-08 at commit `149fa41`, branch `feat/gascan-arca`.

---

Continue the Arca integration. Work happens in `~/code/gascan` and `~/code/arca`.

You are RESUMING an in-flight execution, not starting one. P5.2 is half done on
branch `feat/gascan-arca`, pushed, at `149fa41`. Do not start from `main`, do not
re-plan, and do not re-dispatch completed tasks.

Read, in this order:

1. `~/code/gascan/.superpowers/sdd/2026-08-08-gascan-arca/progress.md` — the SDD
   ledger. Start at `=== ROTATION POINT` and read the CORRECTION entry after it.
   This file is the AUTHORITY wherever any other document disagrees with it: it
   names every commit, every ruling, and every deferred finding. It is gitignored,
   so it exists only on this machine.
2. `docs/status/next-session-kickoff.md` — read the whole thing, but the
   **TRAPS ADDED 2026-08-08** block and the **CLOSING THOUGHTS** are the parts that
   were paid for.
3. `docs/status/arca-integration-handoff.md`, from `## Session of 2026-08-08`.
4. `docs/superpowers/plans/2026-08-08-gascan-arca.md` — the plan you are executing.
   Its Global Constraints bind every task.
5. `docs/superpowers/specs/2026-08-08-gascan-arca-backend-design.md` — the design.
   Read §4.6 and §5 before touching mapping code, and heed the stale-line-number
   warning in its header.

Then re-enter `superpowers:subagent-driven-development` against the EXISTING ledger.

**STATE:** Tasks 1-4 complete with clean reviews. Task 5 implemented and reviewed but
NOT complete — one open Important finding. Tasks 6-10 pending, all briefs staged and
already audited.

## Do these two things first

**1. Close Task 5.** Dispatch it as fix round 1/5. Its field-placement test in
`crates/gascan-arca/src/error.rs` asserts rendered text for only 4 of the 12 accepted
error codes, so a `resource`↔`message` transposition in any of the other eight
(`ownership_mismatch`, `foreign_resource_refused`, `not_found`, `command_io`,
`invalid_output`, `invalid_state`, `resource_conflict`, `helper_error`) passes all
five tests, because `code()` does not depend on which string fills which field. All
eight were hand-verified correct, so this is a coverage gap and not a live bug.
Extend the test to all twelve variants, and require a flip: transpose `resource` and
`message` in the fixture for at least two newly covered variants, confirm the test
fails, then restore.

**2. Re-measure the workspace suite.** The last verified figure (1382 passed, 22
ignored at `5ad7ea9`) PREDATES this branch. A two-minute attempt timed out and a
later background run reached 329 passed / 0 failed without completing — neither is a
result. Run it with a long timeout and capture the exit code directly, never through
a pipe. Task 10 owns the authoritative measurement.

## Six things that will cost you if you learn them the hard way

**THE PLAN HAS BEEN THE DEFECT SOURCE, NOT THE IMPLEMENTERS.** All six defects last
session came from plan or spec text; none from implementation code. Three cost a
review round each; three cost nothing because the brief was audited before dispatch.
**AUDIT EVERY BRIEF BEFORE YOU DISPATCH IT.** It is the highest-yield habit in this
work, and it is why Tasks 6-10's briefs already carry fixes.

**THE INSTRUMENT KEEPS BEING NARROWER THAN THE CLAIM** — six times last session, most
recently when the FIX for a narrow test was itself narrow (4 of 12). Check a fix as
hard as you checked the defect. Prefer reading an artifact to grepping it: one grep
nearly produced a false accusation against an implementer that had done the work
correctly.

**`cargo clippy -- -D warnings` CANNOT PASS for `gascan-arca` until Task 8**, because
`translate.rs` arrives before its consumer and rustc's `dead_code` fires (17 lib, 13
lib-test, verified). Use `-D warnings -A dead_code` for Tasks 5, 6 and 7, **ON THE
COMMAND LINE ONLY**. Never add `#[allow(dead_code)]` to the source — an attribute
outlives the condition that justified it. Task 8 restores the plain gate; Task 10's
workspace gate is unconditional. This lives in the plan's Global Constraints, which
the brief extraction does NOT include, so **you must state it in each dispatch**.

**EVERY `crates/gascan-core/src/runtime.rs` LINE NUMBER in the 2026-08-08 design spec
is stale by ~50-57 lines**: Task 1 added 58 lines. `from_resources` 944→1001,
`discovered` 503→554, `RuntimeError::code` 1056→1113, `trait RuntimeBackend`
990→1047. Cite symbols, not lines. When a citation and the code disagree, the code
wins.

**THE TWO BACKENDS MUST AGREE ON `sandbox_id` FOR A `Mismatched` RESOURCE.** Both
report the id it claims: `owner.managed_by == MANAGED_BY ? parsed : None`. The rule
`match ownership { GasCanOwned => parsed, _ => None }` is WRONG and was reverted
twice — once after shipping in `gascan-apple`, once caught in Task 4's brief before
shipping. `gascand/src/service.rs:3001-3012` finds a mismatched container BY that
claim with no ownership filter, so withholding it silently drops an
`OwnershipMismatch` finding. A parity test defends it; do not "simplify" it back.

**SUBAGENTS GO IDLE WITHOUT REPORTING** — three times last session, including an
implementer that had done nothing beyond an uncommitted edit. **NEVER read silence as
success:** check `git log` and grep for the expected symbol. Do not take a subagent's
self-description at face value either; one claimed a nudge was stale when direct
verification showed the work had not landed.

## Environment

`RUSTUP_TOOLCHAIN=1.95.0` is exported and overrides `rust-toolchain.toml` — prefix
every cargo command with `env -u RUSTUP_TOOLCHAIN`. Use `--no-fail-fast`. Confirm the
`running N tests` line, because a bare test name silently runs zero and exits 0.
`cargo clippy --fix` is prohibited here; `cargo fmt` is fine. `ls` is aliased to
something that rejects trailing-slash paths — use `find` or `git ls-files`.

**SIGNING AND TRANSPORT NO LONGER USE 1PASSWORD ON THIS REPO.** `user.signingkey` is
`~/.ssh/gascan-signing`, a file PATH, which is why no agent is needed; transport is
HTTPS via `gh`'s credential helper. All `--local`; global config untouched. Commit
with `env -u SSH_AUTH_SOCK git commit`. **NEVER `--no-gpg-sign`** and never a
lightweight tag. Verify `%G?` is `G`. No co-author trailer and no AI-tool mention in
any commit message.

## Rulings already made — do not re-litigate

**Task 9 ships with NO tests.** The only thing that could answer it is a Rust server,
which is forbidden; the compiler checking it against `EngineTransport` is the stated
assurance, and the plan says exactly that rather than dressing it up as coverage.

**Three shapes are plan-mandated**, so tell reviewers up front instead of looping on
them: Task 6's temporary `exec`/`logs` stubs, Task 10 committing nothing, and Task 3's
deliberately non-compiling first file.

## Two items deliberately left open — rule on them, do not inherit them silently

**Task 7** has no test for a mid-stream `TransportError` (as distinct from an engine
error chunk), nor for a `LogsChunk` with an unset `outcome`. Both paths exist in the
specified code. They were found, neither was fixed, and both were recorded so the
decision is yours.

**Five deferred minors** are listed in the ledger for the final whole-branch review to
triage. One is arguably under-rated: `runtime_sandbox`'s unparseable-owner-label
branch refuses correctly but has no test, and nothing later in the plan covers it —
the same class an earlier review rated Important.

## Do not write D7's narrowed retry

No `0200` occurrence has fired since the instrument landed. A docs-only CI run skips
the `rust` and `engine` jobs entirely (VERIFIED, run `31262534703`), so a green docs
run proves nothing about D7. The retry is approved in principle and stays unwritten
until a run names which of the two `0200` states fired.

## After Task 10

Final whole-branch review on the most capable model, pointed at the ledger's
deferred-minor and parked lines, then `superpowers:finishing-a-development-branch`.
**Merge only — never squash, never rebase, and only via a PR. Never commit to
`main`.**
