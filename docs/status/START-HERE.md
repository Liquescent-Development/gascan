# START HERE

This file is the session entry point. It is written to be read cold, and it is
addressed to you, the agent. Follow it as instructions — there is nothing to paste.

Written 2026-08-11 for two branches in flight; rewritten 2026-08-12, after both merged.

---

## Where the work is

**P5.1 milestone 1 is DONE and MERGED in both repositories. There is nothing in
flight.** Start a new branch for whatever comes next.

| | |
|---|---|
| Gas Can | PR #69 merged as `58f26aa`, 2026-08-12 |
| Arca | PR #56 merged as `cc316b65`, 2026-08-12 |
| Design | `docs/superpowers/specs/2026-08-10-p5-1-engine-service-and-wiring-design.md` |
| Plan | `docs/superpowers/plans/2026-08-10-p5-1-milestone-1-engine-skeleton.md` |
| Detail | `docs/status/arca-integration-handoff.md`, from `## Session of 2026-08-10/11` |

Both are real merge commits, not squashes — `git rev-list --parents -n 1` reports two
parents for each — so every commit message below is still reachable by SHA. VERIFIED
with `git merge-base --is-ancestor`: `b3390b8` is an ancestor of Arca's `origin/main`
and `351a646` of Gas Can's.

**Read the design document before touching anything.** Every decision taken — offline
reported `PROVEN` the way `gascan-apple` earns it, image ingress as an engine subcommand,
the engine as a launchd job `gascand` dials rather than supervises, a mid-exec reset
meaning cancellation — is recorded there with its reasoning.

**P5.1 is both halves.** The roadmap and the older handoff say "implement the engine
service"; an earlier START-HERE said "wire the backend to the daemon". It is both, and
the design document supersedes the narrower readings.

### What shipped

**Arca** (`~/code/arca`, now on `main`) — seven task commits `bc03394..e74aff0`, a
dependency fix `8fc1ca5`, a comment fix `f5fde96`, and two answering the adversarial
review: `16abeec` (Inspect and ListResources) and `b3390b8` (the socket path and the
shutdown path). **30 tests pass** (`swift test --filter ArcaEngineTests`, exit 0); it was
27 before the review added three to `EngineServerTests`.

**Gas Can** (`~/code/gascan`, now on `main`) — design and plan (`33d37f9`, `4981b39`,
`77ff591`, `b36d18f`), Tasks 8-11, then the review wave `140b274..351a646`.

| Task | Commit | What |
|---|---|---|
| 8 | `f75d069`, `ddb4f6a` | `scripts/build-arca-engine.sh` builds the engine product, runs its tests in the verified clean checkout, and prints the binary path as a second stdout line |
| 9 | `cb81024` | the live harness — a real engine on a real socket, and the `connect` error paths |
| 10 | `2fe3711` | live coverage of the read RPCs — since replaced, see below |
| 11 | `c0e0cc8`, `aebf558`, `fb50d4c` | `tests/release/engine-targets-check.sh` — neither `arca-engine` nor `ArcaEngine` reaches `DockerAPI` or `ArcaDaemon` |

**1435 tests pass, 0 fail, 26 ignored** across 74 targets reporting `0 filtered out`
(`cargo test --workspace --no-fail-fast`, exit 0). It was 28 ignored: the live tier's
`Inspect` and `ListResources` tests folded into one that covers all ten unimplemented
methods, so the tier went from 8 tests / 6 ignored to 6 / 4, and nothing else moved.

### What the engine actually does, which is less than it sounds

**`Capabilities` is the ONE implemented method. The other ten answer
`unsupported_capability`.** The engine runs — VERIFIED by running it: `arca-engine
--socket-path … --state-root …` logs `engine listening` and creates the socket
`srw-------`, and Gas Can's live tier drives all eleven over a real socket.

`Inspect` and `ListResources` were counted as real until 2026-08-12 and are not. The
process calls `initialize()` on no manager, so each could return exactly one answer under
every input — `absent`, and an empty list. Answering `absent` without having looked is
what makes a reconciler create a duplicate of a running sandbox; an empty `ResourceList`
is a confident report of a clean host, which is the report that hides a leak. Both now
refuse instead. The reasoning is on each method in `SandboxEngineService.swift` and in
`ArcaEngineCommand.run()`.

## What to do next

Three things are open, in the order I would take them.

1. **Plan milestone 2.** It is outlined at the end of the plan and was deliberately left
   unplanned until milestone 1 landed, because its tasks depend on what milestone 1
   found. It now has a concrete first task it did not have before: give `ContainerBridge`
   a read-only load path that neither starts a VM nor writes, and restore `Inspect` and
   `ListResources` on top of it. Arca's I3, I4 and I5 (below) are the known defects that
   path must fix before either method reports anything.
2. **The Minors** — 6 in Gas Can, 8 in Arca. Cheap, well-specified, and each carries its
   own reproduction in the review reports.
3. **D7's narrowed retry**, which is now unblocked by evidence. See its section below.
   The maintainer's ruling on 2026-08-12 was: separate PR, after the merge — not folded
   into unrelated work.

### The adversarial reviews

**Every Critical and Important from both is fixed.** The Minors are not, and are item 2
above.

| | |
|---|---|
| Gas Can PR | https://github.com/Liquescent-Development/gascan/pull/69 (merged) |
| Arca PR | https://github.com/Vas-Solutus/arca/pull/56 (merged) |
| Findings, Gas Can | `docs/status/adversarial-review-gascan-pr69.md` — Critical 1, Important 5, Minor 6 |
| Findings, Arca | `docs/status/adversarial-review-arca-pr56.md` — Critical 1, Important 6, Minor 8 |

Both report files are left exactly as written, recording what was observed at `39be145`
and `f5fde96`; each carries a status header saying what has since been fixed. They hold
file:line, a reproduced failure scenario, and a fix for each finding, plus a section on
what was attacked and *held* — which is as load-bearing as the findings, because it says
what not to re-litigate. **Read the "attacked and could not break" sections too.**

**What the two Criticals turned out to be, because they change what you believe:**

1. **Gas Can — the signed-pin gate could verify a different object than the one it
   compiled.** `verify-tag "$tag"` unqualified beside `refs/tags/${tag}` qualified: git
   tries `refs/<name>` before `refs/tags/<name>`, so the signature gate and the identity
   gate could land on different objects. Fixed by qualifying every tag name in both
   `build-arca-engine.sh` and `sync-arca-proto.sh`, and by constraining `.tag` in the pin
   schema to `^[A-Za-z0-9._-]+$`. **The old pin was never exploited** — `gascan-engine-m1`
   has no slash and was verified independently.
   `tests/release/engine-pin-contract.sh` now carries both halves of the attack as
   negative cases, and **both were confirmed to catch it**: against the unfixed script
   `slash-tag` exits 0 (the attacker's commit is compiled) and so does `shadowed-ref`.
2. **Arca — `Inspect` and `ListResources` could never report anything.** Resolved by
   option (b): both now answer `unsupported_capability`. Calling `initialize()` was
   rejected for this milestone on evidence the review had not reached — its restore loop
   *writes*, marking every persisted `running` container exited with code 137
   (`ContainerManager.swift:316-338`), so an engine pointed at a live `ArcaDaemon`'s state
   root would declare that daemon's containers dead; and `NetworkManager.initialize()`
   ends in `createDefaultNetworks()`, which creates a vmnet network. **This dissolved
   Arca's I3, I4 and I5** — all three are properties of answers that no longer exist.
   Milestone 2 gives ContainerBridge a read-only load path that neither starts a VM nor
   writes, and restores both methods.

**Which Minors are left.** Two Gas Can Minors were taken along the way because they were
load-bearing for an Important — M1 (the `runtime-probe` comment orphaned onto `gate`) and
M2 (the EXIT trap that collapsed every documented exit code to 1, which the new
pin-contract cases assert exactly). M3 (the `/tmp` socket-root leak), M4 (the product
check being narrower than its comment — the comment now says so, rather than the check
being widened), M5 and M6 remain, as do all eight Arca Minors.

## The pin is real, and now on Arca's main

**`engine/arca-pin.json` names the signed annotated tag `gascan-engine-m1.1` at
`b3390b80528f425be0109298d6a95dd863747c5d` on `https://github.com/Vas-Solutus/arca.git`.**
This resolves the blocker earlier versions of this file recorded, which said the pin named
`gascan-engine-proto-v1` at `77b293e` — a revision with no engine in it, against which
`swift build --product arca-engine` exits 1, so CI's `engine` job *failed* rather than
building something old. It does not fail any more. Do not reintroduce the old wording.

**VERIFIED end to end against this pin**, not merely resolved: `./scripts/build-arca-engine.sh`
exits 0 in 6m00s from a cold clone — signature verified against `engine/allowed-signers`,
tag target matched, clean checkout, `Executed 30 tests, with 0 failures` — and prints the
checkout and binary paths. The live tier then passes 4/4 against that binary. CI's
`engine` job did the same twice on a hosted runner in run `31621889316` (14m42s the
second time), which is the only automated verification Arca has at all — see the CI
section below.

Since PR #56 merged, `b3390b8` is an ancestor of Arca's `main`, so the pinned revision no
longer depends on a tag alone to stay reachable. The older `gascan-engine-m1` at
`f5fde96` is still pushed and still valid; it was left where it was rather than moved,
because moving a pushed signed tag rewrites what an already-verified pin resolved to.

The engine build step is `.github/workflows/ci.yml:108` — re-derive that anchor with
`grep -n 'Build the pinned Arca engine' .github/workflows/ci.yml` rather than trusting it.
It has drifted on every single pass over this file so far; assume it has drifted again.
The `printf` anchor inside `ci.yml`'s own comments had drifted from `:179` to `:208` and
was corrected the same way — `grep -n "printf '%s" scripts/build-arca-engine.sh`.

`.artifacts/arca-dev-pin.json` still exists as a *development* pin naming a
`file:///Users/kiener/code/arca` URL and a local `gascan-engine-dev` tag. `.artifacts/` is
gitignored and worthless on any other machine — it is a convenience, no longer a
substitute. It now trails Arca HEAD by three commits, so **any rebuild from it must first
move the local tag** (`git tag -f -s`) and update its revision, or
`build-arca-engine.sh`'s tag-target assertion rejects it — correctly.

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

**NEVER RUN THE WORKSPACE SUITE WHILE ANY OTHER CARGO IS RUNNING — NOT JUST A SUBAGENT'S.**
Run it alone, after `pgrep -fl "cargo test"` comes back empty. Concurrent suites against one
target directory produced **rc=101 with 59 failures**, none of them real: those tiers spawn
daemons and bind sockets, so they starve each other. Run alone it takes **93 seconds**.

**It is not only subagents, and not only this repository.** On 2026-08-12 another Claude
session was looping full `cargo test --workspace` cycles in an unrelated repo
(`capsule-os-worktrees/worker`) on the same machine, and this repo's suite failed three
times in a row while it ran. `pgrep` ancestry named the owner every time. **Read the
ancestry before assuming a stray cargo is yours, and never `pkill` it.**

**The failure count scales with the load, which is how you recognise it.** Measured that
day in this repo, same tree, same commit `351a646`: 2 concurrent cargo processes → **21
failures / 2 targets**; 3 processes → **37 / 5**; 3 processes with the run stretched
longer → **41 / 9**. Then every one of those 9 targets run *alone under the same
contention* → **318 passed, 0 failed, rc=0 for all nine**
(`gascan-apple/backend_fake_runner`, `gascan-e2e/{apple_apply,autostart,doctor}`,
`gascand/{daemon_idle,doctor_state,lifecycle,reconcile,ssh_config}`).

**`-- --test-threads=N` DOES NOT HELP; IT MAKES IT WORSE.** Bounding per-binary
parallelism to survive a loaded machine stretches the run, which overlaps *more* of the
neighbour's load — that is the 41-failure row above. Wait for a quiet machine, or verify
by isolation. Do not tune your way out of it.

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

## CI: what to expect, so you do not spend a session on it

**`ci / gate` is NOT a required check and does not block merging.** VERIFIED 2026-08-12:
ruleset `20492137` carries `deletion`, `non_fast_forward`, `required_signatures` and
`pull_request`, and **zero** `required_status_checks`; PR #69 read
`mergeable=MERGEABLE, mergeStateStatus=UNSTABLE` and merged. `allowed_merge_methods` is
`["merge"]` — merge commits only, never squash.

**The `rust` job fails about 38% of the time on `main`, on a different test each time.**
Measured by reading the `rust` conclusion of every run in
`gh run list --workflow=ci.yml --branch main --limit 15`: of the 13 that completed, 8
green and 5 red, and the five reds were five distinct tests. PR #69's `rust` job then
failed **four consecutive times** — `pty_resize_driver_drains_chatty_child_without_
backpressure_timeout` (twice, missing a hard 2s wall-clock bound by 100ms and by **2.3ms**),
`concurrent_clients_converge_on_one_private_daemon` (D7, above), and
`same_image_apply_recreates_explicit_ssh_as_automatic`.

**One failure mode reproduced verbatim across branches, five days apart**, which is the
proof it is not a given branch's doing: `main`'s `31203816056` and PR #69's third attempt
both died as `KeygenRejected(KeygenRejection { outcome: Code(255), message:
KeygenMessage("/dev/fd/<N>: Bad file descriptor"), descriptor: Intact })`, fd 18 and fd 24,
both ending `error: test failed, to rerun pass \`-p gascand --test apply_setup\``.

**How to decide whether a red `rust` is yours.** Do not argue from probability — check the
diff. `git diff <merge-base>..HEAD -- crates/<the failing crate>/` empty means your branch
cannot have caused it, and that is a proof rather than an estimate. It was empty for
`crates/gascan-e2e/` throughout P5.1.

**The standing rule: a green local `cargo test --workspace` is the bar. CI reports but
must not gate, and flake-chasing waits** until someone is asked to do it. There are at
least three distinct root causes to fix when that day comes — the PTY wall-clock bound,
D7's `0200` window, and the keygen `/dev/fd` descriptor.

**Arca has NO CI AT ALL.** `gh pr checks 56` reported "no checks reported on the
'feat/sandbox-engine' branch", and `.github/workflows` does not exist in that repository.
Gas Can's `engine` job — which builds Arca from the signed tag and runs its 30 tests in a
clean checkout — is the only automated thing that ever exercises Arca. Any earlier
statement in this file that "CI is green on both" was wrong.

## D7 has fired, and the retry is now justified — write it

**The first `0200` occurrences landed on 2026-08-12, and there were two.** The instrument
did exactly what it was built for: it named which state fired. Verbatim, the local one,
from `cargo test --workspace --no-fail-fast`:

```
---- daemon_stderr_sink_survives_the_launching_cli stdout ----
daemon start failed: stdout=, stderr=Error: started daemon did not become healthy and
current (state Unsafe): protected runtime file is unsafe: mode is 0200 and the file has
content: written but never published (mode 0200, size 375, links 1, uid 501, expected
uid 501)
```

And in CI, run `31621889316`'s second `rust` attempt, a different test with the same
fault — `concurrent_clients_converge_on_one_private_daemon`, **size 382**, which is two
clients racing to autostart one daemon.

Read the message, not the test name. Of the two `0200` states
`crates/gascan/src/daemon.rs:3077-3079` distinguishes, both occurrences were **"has
content: written but never published"**.

**CORRECTION, recorded because it reverses the first reading of this evidence.** The doc
comment at `:3057-3064` calls that state "a daemon that wrote its record and died before
publishing, which never becomes 0600 on its own" — a corpse. On that basis this file
briefly said the evidence argued *against* the retry. **The code says otherwise, and the
code wins:**

- `is_interrupted_tombstone` (`:2633-2639`) is *defined* as 0200 with `st_size > 0`. It is
  a named, expected state the supervisor knows how to handle, not an unrecoverable one.
- `retire_held_record` (`:1372-1375`) resolves it — and **produces it transiently itself**:
  it `fchmod`s to `INSTANCE_TOMBSTONE_MODE` *first* and `ftruncate`s to 0 *second*, so
  between those two syscalls the file is 0200-with-content on disk.

So 0200-with-content is reachable as a publication in flight, `validate_file_stat` rejects
it as a hard `PermissionDenied`, and a client that reads during that window fails its
autostart. That is a race worth waiting out — exactly what the narrowed retry is for.
**The condition this file set ("stays unwritten until a run names which of the two `0200`
states fired") is met, and it points toward the retry.** Maintainer's ruling 2026-08-12:
write it in its own PR, not folded into unrelated work.

Two cautions that remain true:

- **It is load-dependent and does not reproduce on demand.** Both occurrences happened
  under contention (local: another repo's suite, pid 4969 recorded by `pgrep`; CI: a
  hosted runner). Quiet re-runs report **0 occurrences of `mode is 0200`**.
- **It predates the engine work.** Nothing in P5.1 touches `gascand`, the runtime record,
  or `crates/gascan`.

## P5.2 is done

`crates/gascan-arca` implements `RuntimeBackend` over Arca's contract behind an
`EngineTransport` seam, merged as `bd412b4`. `ChannelTransport` ships with no tests by
explicit ruling — the compiler checking it against `EngineTransport` was the stated
assurance, and **Tasks 9 and 10 of the current plan are what finally test it against a real
engine.** Do not add a test double for it.

The `sandbox_id`-claim rule is still duplicated verbatim between
`gascan-arca/src/translate.rs` and `gascan-apple/src/inspect.rs`, each with its own test and
a comment warning they must not diverge. Sharing it belongs to P5.3.
