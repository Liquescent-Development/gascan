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

### Arca — `~/code/arca`, branch `feat/sandbox-engine`, **pushed, but no PR and not merged**

Seven task commits `bc03394..e74aff0`, a dependency fix `8fc1ca5`, and a comment fix
`f5fde96`. All reviewed. 27 tests pass.

The branch is now on `Vas-Solutus/arca` and carries the signed annotated tag
**`gascan-engine-m1` at `f5fde96`** — the tag Gas Can's `engine/arca-pin.json` names.
**There is still no PR and nothing is merged**, so `main` in Arca has no engine in it; the
tag is the only thing holding that revision reachable.

**The engine genuinely runs** — VERIFIED by running it: `swift run arca-engine
--socket-path … --state-root …` logs `engine listening` and creates the socket
`srw-------`. `Capabilities`, `Inspect` and `ListResources` are real; the other eight
methods answer `unsupported_capability` until later milestones.

### Gas Can — `~/code/gascan`, branch `docs/p5-1-engine-design`

Design, plan, and corrections committed (`33d37f9`, `4981b39`, `77ff591`, `b36d18f`), then
**Tasks 8 through 11 and the whole-branch fix wave. All four tasks are complete and all
four were independently reviewed**, including Task 8, which was the one task that had
landed without a review.

| Task | Commit | What |
|---|---|---|
| 8 | `f75d069`, `ddb4f6a` | `scripts/build-arca-engine.sh` builds the engine product, runs its tests in the verified clean checkout, and prints the binary path as a second stdout line |
| 9 | `cb81024` | the live harness — a real engine on a real socket, and the `connect` error paths |
| 10 | `2fe3711` | live coverage of `Capabilities`, `Inspect` and `ListResources` |
| 11 | `c0e0cc8`, `aebf558`, `fb50d4c` | `tests/release/engine-targets-check.sh` — neither `arca-engine` nor `ArcaEngine` reaches `DockerAPI` or `ArcaDaemon` |

**1435 tests pass, 0 fail, 28 ignored** (`cargo test --workspace`, run alone, exit 0).

## What to do next

**Both PRs are open, CI is green on both, and NEITHER MAY MERGE.** An independent adversarial
review of each found one Critical apiece, both reproduced end to end rather than argued. The
maintainer has asked for **every Critical and Important** to be fixed.

| | |
|---|---|
| Gas Can PR | https://github.com/Liquescent-Development/gascan/pull/69 |
| Arca PR | https://github.com/Vas-Solutus/arca/pull/56 |
| Findings, Gas Can | `docs/status/adversarial-review-gascan-pr69.md` — Critical 1, Important 5, Minor 6 |
| Findings, Arca | `docs/status/adversarial-review-arca-pr56.md` — Critical 1, Important 6, Minor 8 |

Read both reports. They carry file:line, a reproduced failure scenario, and a fix for each
finding, plus a section on what was attacked and *held* — which is as load-bearing as the
findings, because it says what not to re-litigate.

**The two Criticals, because they are the ones that change what you believe:**

1. **Gas Can — the signed-pin gate can verify a different object than the one it compiles.**
   `scripts/build-arca-engine.sh:64` calls `verify-tag "$tag"` unqualified while `:70`
   resolves `refs/tags/${tag}` qualified. Git tries `refs/<name>` before `refs/tags/<name>`,
   so the signature gate and the identity gate can resolve different objects. REPRODUCED:
   with `refs/tags/foo` (annotated, good) and `refs/tags/tags/foo` (lightweight, evil),
   unqualified `tags/foo^{commit}` resolves to the good commit while
   `refs/tags/tags/foo^{}` resolves to the evil one — all three gates pass and the evil
   commit is compiled. The one-word fix is `verify-tag "refs/tags/${tag}"`; also constrain
   `.tag` in the pin schema. `scripts/sync-arca-proto.sh:82` has the same pattern and is
   immune only by accident. **The current pin is not exploited** — `gascan-engine-m1` has no
   slash and was verified independently — but fix this before trusting the gate again.
2. **Arca — `Inspect` and `ListResources` can never report anything, for the life of the
   process.** Not restart-scoped, and not only containers. `initialize()` is never called
   (`Sources/arca-engine/ArcaEngineCommand.swift:40-56`, comments only); the sole insertion
   sites for `ContainerManager.containers` are `:382` (inside `initialize()`) and `:1883`
   (inside `createContainer`, which answers `unsupported_capability`). Volumes and networks
   are equally blind. PROVEN LIVE: a state root seeded from a real `~/.arca/state.db`
   (1 container, 2 networks) still yields `ListResources = Ok([])`. The danger is semantic —
   `Absent` means "it is not there", so a reconciler creates a duplicate sandbox.
   **This is a design decision for the maintainer, not a mechanical fix:** either call
   `initialize()` this milestone, or make these two answer `unsupported_capability` like the
   other eight rather than answer a confident falsehood. Do not leave it as is. Whichever
   way it goes, the PR body and this file must stop calling those three methods implemented.

**Three findings correct things this repository currently asserts.** Fix the records with the
code: the "hardcoded forbidden-name pair" in `engine-targets-check.sh` is not cosmetic (a
rename of `DockerAPI` upstream makes the check print PASS while the edge exists — executed);
`ci-classify-paths.sh` routes the live tier on its *test files* but not on its *subject*, so
editing `crates/gascan-arca/src/channel.rs` skips the engine job entirely; and
`ci-classify-paths.sh:35` says "four of its six tests" where the truth is 8 tests / 6 ignored,
contradicting `ci.yml:137-139`.

Then milestone 2, which is outlined at the end of the plan and is to be *planned* when
milestone 1 lands — not before, because its tasks depend on what milestone 1 found. Note that
Arca Critical 1 is exactly the `ContainerManager.initialize()` question milestone 2 was
already expected to revisit; it has arrived early and with teeth.

## The pin is real now

**`engine/arca-pin.json` names the signed annotated tag `gascan-engine-m1` at
`f5fde96224937e4617b8dac9ae5eeea837089420` on `https://github.com/Vas-Solutus/arca.git`.**
This resolves the blocker earlier versions of this file recorded, which said the pin named
`gascan-engine-proto-v1` at `77b293e` — a revision with no engine in it, against which
`swift build --product arca-engine` exits 1, so CI's `engine` job *failed* rather than
building something old. It does not fail any more. Do not reintroduce the old wording.

The engine build step is `.github/workflows/ci.yml:108` — re-derive that anchor with
`grep -n 'Build the pinned Arca engine' .github/workflows/ci.yml` rather than trusting it.
It has drifted on every single pass over this file so far; assume it has drifted again.

`.artifacts/arca-dev-pin.json` still exists as a *development* pin naming a
`file:///Users/kiener/code/arca` URL and a local `gascan-engine-dev` tag. `.artifacts/` is
gitignored and worthless on any other machine — it is a convenience, no longer a
substitute. It trails Arca HEAD by one commit (`8fc1ca5` against `f5fde96`), so **any
rebuild from it must first move the local tag** (`git tag -f -s`) and update its revision,
or `build-arca-engine.sh`'s tag-target assertion rejects it — correctly.

## What milestone 1 answered

These were unverified before this branch. Each is now an observation, and the anchors are
in `docs/status/arca-integration-handoff.md`.

- **A real engine accepts the client's placeholder authority `http://[::]:50051`.**
- **A missing socket and a non-socket render differently, and both name the path.**
  Missing: `No such file or directory (os error 2)`. A regular file: `Socket operation on
  non-socket (os error 38)`. Both carry the io cause past tonic's opaque `transport error`,
  so `source_chain` (`crates/gascan-arca/src/channel.rs:62-78`) does what it claims.
- **The engine claimed nothing it had not earned** — every capability flag came back
  `false` and `offline` came back `Unverified`.
- **The measured socket path is 41 bytes of `sun_path`'s 103.** Headroom, but the harness
  asserts the length before binding rather than meeting the cap as a mystery bind failure.
- **The engine's first-ever execution costs ~997ms against ~10ms warm**, which overran the
  plan's 30s harness bound. The harness now reports a dead child immediately via
  `try_wait()` and waits 120s.
- **`swift test --filter <no match>` exits 0 having run nothing.** Guarded now, in
  `scripts/build-arca-engine.sh`.

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

**AND IT HAPPENED AGAIN, AT 92 FAILURES, AND WAS NEARLY BELIEVED.** Task 10 raised a
blocker on `rc=101, 92 failures across 12 targets`, every one of them a tier that spawns a
daemon or a helper. It was the same artifact. Settled by measurement rather than argument:
`cargo test -p gascand --test daemon_idle` is `running 11 tests`, 11 passed, exit 0 on
*both* the merge-base `9665107` and the branch tip under the same contention, and the full
suite run alone is exit 0 with **1435 passed / 0 failed / 28 ignored**. **A report claiming
"it reproduces on a quiet machine" is not evidence the machine was quiet** — check
`pgrep -fl "cargo test"` yourself and record the output.

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
