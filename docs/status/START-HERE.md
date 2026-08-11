# START HERE

This file is the session entry point. It is written to be read cold, and it is
addressed to you, the agent. Follow it as instructions — there is nothing to paste.

Written 2026-08-11, describing two branches in flight.

---

## Where the work is

**P5.1 milestone 1 is in progress across two repositories, on two unmerged branches.**

| | |
|---|---|
| Design | `docs/superpowers/specs/2026-08-10-p5-1-engine-service-and-wiring-design.md` |
| Plan | `docs/superpowers/plans/2026-08-10-p5-1-milestone-1-engine-skeleton.md` |
| Detail | `docs/status/arca-integration-handoff.md`, from `## Session of 2026-08-10/11` |

**Read the design document before touching anything.** Every decision taken — offline
reported `PROVEN` the way `gascan-apple` earns it, image ingress as an engine subcommand,
the engine as a launchd job `gascand` dials rather than supervises, a mid-exec reset
meaning cancellation — is recorded there with its reasoning.

**P5.1 is both halves.** The roadmap and the older handoff say "implement the engine
service"; an earlier START-HERE said "wire the backend to the daemon". It is both, and
the design document supersedes the narrower readings.

### Arca — `~/code/arca`, branch `feat/sandbox-engine`, NOT pushed, no PR

Seven task commits `bc03394..e74aff0` plus a dependency fix `8fc1ca5`. All reviewed. 27
tests pass.

**The engine genuinely runs** — VERIFIED by running it: `swift run arca-engine
--socket-path … --state-root …` logs `engine listening` and creates the socket
`srw-------`. `Capabilities`, `Inspect` and `ListResources` are real; the other eight
methods answer `unsupported_capability` until later milestones.

### Gas Can — `~/code/gascan`, branch `docs/p5-1-engine-design`

Design, plan, and corrections committed (`33d37f9`, `4981b39`, `77ff591`, `b36d18f`).
Task 8 (the build script) was mid-flight at handoff. **Tasks 9, 10 and 11 are untouched**
— the live-test harness, live coverage of the three implemented RPCs, and the contract
test asserting `ArcaEngine` reaches neither `DockerAPI` nor `ArcaDaemon`.

## What to do next

1. Finish and review Task 8 if it is not committed. Check `git status` and
   `git log --oneline -3` before assuming either way.
2. Tasks 9, 10, 11 from the plan. Use `superpowers:subagent-driven-development`.
3. Then milestone 2, which is outlined at the end of the plan and is to be *planned* when
   milestone 1 lands — not before, because its tasks depend on what milestone 1 found.

## THE THING THAT MUST NOT BE FORGOTTEN

**`engine/arca-pin.json` still names `gascan-engine-proto-v1` at `77b293e` — a revision
with no engine in it.** Everything built this session used a *development* pin at
`.artifacts/arca-dev-pin.json`, naming a `file:///Users/kiener/code/arca` URL and a local
`gascan-engine-dev` tag. `.artifacts/` is gitignored: none of it is committed, and it is
worthless on any other machine.

Before anything merges: push `feat/sandbox-engine` to `Vas-Solutus/arca`, create a real
signed tag there, and bump `engine/arca-pin.json`. Until then CI
(`.github/workflows/ci.yml:91`) builds the pre-engine revision.

## Traps that will cost you if you learn them the hard way

**SIGNING IS INVERTED BETWEEN THE TWO REPOSITORIES.** Gas Can's `user.signingkey` is a file
PATH (`~/.ssh/gascan-signing`), so commit with `env -u SSH_AUTH_SOCK git commit`. **Arca's
key lives in 1Password**, so it needs the agent and `env -u SSH_AUTH_SOCK` breaks every
commit with `unable to sign`. One rule for both aborts everything in one of them.
**NEVER `--no-gpg-sign`**, never a lightweight tag. Verify `%G?` is `G`. No co-author
trailer and no AI-tool mention in any commit message.

**1PASSWORD ANSWERS `ssh-add -l` WITHOUT APPROVAL BUT REFUSES TO SIGN WITHOUT IT.** "The
agent lists the key" is not evidence that signing will work. Signing in Arca needs a human
at the keyboard; if it fails, ask rather than working around it.

**THE SOURCE TREE IS NOT A RELIABLE PREDICTOR OF THE PINNED BUILD.** `~/code/arca` resolves
grpc-swift-2 2.4.2 successfully; an identical clean clone of the same revision does not,
and the mechanism was never found. Verify against a clean clone. Related: an executable
product build reaches dependency validation that library-target builds skip, which is how
a broken graph hid until the pinned build first produced a binary.

**AN IDLE NOTIFICATION IS NOT DEATH, AND AN EMPTY AGENT ROSTER IS NOT PROOF OF DEATH.** A
subagent went idle mid-work with output uncommitted, `ListAgents` reported nothing
reachable, a replacement was dispatched, and the two collided in the same files. **Check
file mtimes before re-dispatching a task whose work is already on disk.**

**A SUBAGENT CANNOT SUSTAIN A MULTI-MINUTE BACKGROUND BUILD.** Its session pauses and takes
the build with it — twice, the second time leaving `scripts/build-arca-engine.sh`'s `mkdir`
lock held. That lock fails closed by design, so every later run exits 75 until it is
cleared. Long builds belong in the controller session.

**WRITE PLANS THAT SAY WHERE YOU ARE GUESSING.** Nine blocks of this plan's Swift and shell
were wrong. Every one was marked "a best reading, not verified" with the command to confirm
it, and every one surfaced as a directed correction rather than a fix round. The worst
would have been silent: ContainerBridge reports container names with a **leading slash**,
which Gas Can compares against the bare sandbox id — every owned container would have
looked unrelated to its sandbox and drift detection would have seen nothing.

**THE INSTRUMENT KEEPS BEING NARROWER THAN THE CLAIM.** This is the defect this project pays
for over and over. This session alone: a permissions assertion that mis-parsed because `&`
binds tighter than `??`, so it masked a literal zero; and a stale-socket test whose fixture
created a regular file, which the code under test correctly refuses to unlink — it would
have failed against the very property it existed to prove. **Check a fix as hard as you
checked the defect, and prefer reading an artifact to grepping it.**

**COUNTING `test result:` LINES OVERCOUNTS THE WORKSPACE.** Some come from child processes
re-executing a test binary with a filter. Sum only the lines reporting `0 filtered out`,
and check that their count equals the target count.

**NEVER RUN THE WORKSPACE SUITE WHILE ANY SUBAGENT IS RUNNING CARGO.** Run it alone, after
`pgrep -fl "cargo test"` comes back empty. Concurrent suites against one target directory
produced **rc=101 with 59 failures**, none of them real: those tiers spawn daemons and bind
sockets, so they starve each other. Run alone it takes **93 seconds**.

**`git checkout <path>` IS NOT A PERSONAL UNDO IN A SHARED TREE.** It discards every
uncommitted change to that path, including another agent's in-flight work. Check
`git status` for a concurrent writer first, and undo your own edits with a targeted edit.

**NEVER PUT CONTROLLER STEPS IN A TASK OWNED BY A SUBAGENT.** The task tracker is an
instruction surface, not a status board.

**A STAGED BRIEF IS A CACHE AND IT GOES STALE.** Re-extract every brief immediately before
dispatch.

**A GREEN FIGURE YOU CANNOT ACCOUNT FOR IS NOT A PASS.** Account for every increment against
a per-target table you can re-derive by reading `running N tests` lines.

**`RUSTUP_TOOLCHAIN=1.95.0` is exported** and overrides `rust-toolchain.toml` — prefix every
cargo command with `env -u RUSTUP_TOOLCHAIN`. Use `--no-fail-fast`. Confirm the `running N
tests` line, because a bare test name silently runs zero and exits 0. `cargo clippy --fix`
is prohibited here. `ls` is aliased to something that rejects trailing-slash paths — use
`find` or `git ls-files`.

**A DOCS-ONLY CI RUN SKIPS `rust` AND `engine` ENTIRELY** (VERIFIED, run `31262534703`), so a
green docs run is not evidence about anything in Rust.

## Do not write D7's narrowed retry

No `0200` occurrence has fired since the instrument landed. The retry is approved in
principle and stays unwritten until a run names which of the two `0200` states fired. A
failing run containing the D7 test's *name* is not a D7 occurrence — check the message
(`mode 0200 …`), never the test name.

## P5.2 is done

`crates/gascan-arca` implements `RuntimeBackend` over Arca's contract behind an
`EngineTransport` seam, merged as `bd412b4`. `ChannelTransport` ships with no tests by
explicit ruling — the compiler checking it against `EngineTransport` was the stated
assurance, and **Tasks 9 and 10 of the current plan are what finally test it against a real
engine.** Do not add a test double for it.

The `sandbox_id`-claim rule is still duplicated verbatim between
`gascan-arca/src/translate.rs` and `gascan-apple/src/inspect.rs`, each with its own test and
a comment warning they must not diverge. Sharing it belongs to P5.3.
