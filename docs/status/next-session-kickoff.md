# Kickoff — next session

Paste the block below to start the next agent. It is written to be read cold.

---

Continue the Arca integration. Work happens in `~/code/gascan` and `~/code/arca`.

Read `~/code/gascan/docs/status/arca-integration-handoff.md`, starting at
"## Session of 2026-08-07 (later still)" and reading to the end. Then read
`docs/superpowers/specs/2026-08-07-arca-engine-codegen-design.md` — it describes the
build machinery you will be extending, and its §3 explains why the Rust build reaches
Arca the way it does, which is not obvious from the code alone.

**THE GOVERNING DECISION, still binding:**

  A green `cargo test --workspace` ON THIS MACHINE counts as a pass. CI still runs
  and still reports, but it DOES NOT GATE. Do not re-enable a required status check.
  Do not spend this session on CI stability. We make CI stable after the product
  works with Arca as a backend, not before.

  Last VERIFIED green: **1382 passed, 0 failed, 22 ignored, rc=0**, measured
  2026-08-07 with `--no-fail-fast` at `5ad7ea9`. That is the long-carried 1377, plus
  `gascan-engine-proto`'s 4 surface tests, plus 1 for D7's tombstone-report test — so
  every increment is accounted for rather than merely larger. `scripts/ci-run-release-contracts.sh`
  was **15/15, rc=0** at the same point. Re-measure before relying on either.

**STATE:**

  gascan  `main`. **Confirm the SHA with `git log --oneline -1`** rather than
          trusting any SHA written in the docs: a documentation commit that records
          a tip is invalidated by its own merge. This has cost two sessions a round
          already. Do not chase it; verify it.
          Ruleset 20492137: deletion, non_fast_forward, required_signatures,
          pull_request with allowed_merge_methods ["merge"], bypass
          OrganizationAdmin. NO required_status_checks — keep it that way.
  arca    `main`. Also verify.
  Pin:    `engine/arca-pin.json` -> tag `gascan-engine-proto-v1`, revision
          `77b293edd1369c60100045183245ae091f71c39e`. The annotated tag OBJECT is
          `35d2196`; the commit is `77b293e`. Do not confuse them.
          **The tag is the stable anchor** — signed, and asserted by two scripts to
          equal the pin's revision.
  Open:   Vas-Solutus/arca#50 (broadcast-allocation defect, deliberately unfixed).

**WHAT LANDED LAST SESSION: P3.2. P3 IS COMPLETE.**

  Its exit — *proto exists, both sides generate, nothing implements it yet* — now
  holds in all three clauses.

  - Arca has `Sources/SandboxEngineProto/`, a target of generated server code that
    `swift build` compiles. Nothing conforms to it.
  - Gas Can has `crates/gascan-engine-proto`, a generated client. Nothing calls it.
  - `scripts/sync-arca-proto.sh` reaches the proto across the signed pin at build
    time. **The proto is NOT vendored into Gas Can and must not become vendored** —
    a second copy of a published contract is a copy that drifts.

  **The pin moved for the first time.** If you bump it again: the tag must be
  ANNOTATED and SIGNED, never lightweight, which means the 1Password SSH agent must
  be unlocked EARLY rather than forty minutes in. Probe it with an actual signature
  (`ssh-keygen -Y sign`); `ssh-add -l` succeeding proves nothing.

**YOUR TASK: the fan-out is open. P4 and P5 both run from here.**

  P3 was the fan-out point and it is now behind you. Pick with the maintainer:

  - **P5.2 — `gascan-arca`**, the client that implements `RuntimeBackend` over the
    generated stubs. The type mapping is ALREADY DERIVED, in
    `docs/superpowers/specs/2026-08-07-arca-engine-proto-design.md` §9. It exists
    specifically so this step does not re-derive it. Read it before writing a line.
  - **P4 — Docker removal** in Arca.
  - **P3.3 — publish and version the proto**, which carries the `buf breaking`
    check. Still inert: `buf` is not installed and Arca has no CI (P2.3 open). What
    it needs is recorded rather than pretended — a checked-in `FileDescriptorSet`
    and a `buf breaking` invocation in Arca's CI. **Note `gascan-engine-proto`
    already exposes `FILE_DESCRIPTOR_SET`**, so half of that exists.

**DO NOT:**
  - Re-enable a required status check, or work on CI stability.
  - Redesign the proto. It is published and now pinned by tag. A genuine defect in
    it is a contract change with a cost, not an edit.
  - Vendor the proto into Gas Can, or add a second parser of `arca-pin.json`.
    `sync-arca-proto.sh` owns what "the pinned contract" means; `build.rs`
    deliberately reads only its stdout.
  - Generate a Rust **server**. Arca serves this contract. The first thing to
    implement a Rust server would be a test double that made a wrong client look
    correct.
  - Resolve U5 or U6 — genuine spec gaps belonging to P5.4 and P6.3.
  - "Finish" P1. It is `partial by necessity`; its binary half is booked against
    P5.1 and P4.3.
  - Squash- or rebase-merge either repo. Commit to main. Both are forbidden.

**STILL OPEN, none of it the next task:**
  - D7 — **half done.** The instrument landed 2026-08-07; the remedy did not.
    `validate_file_stat` (`daemon.rs:3057`) now names which of the two `0200`
    states it saw and reports size in every case. **`0200` is TWO states and only
    one of them ever resolves** — this module already split on it:

      is_instance_tombstone      (daemon.rs:2708)  mode 0200, size == 0
        an inert placeholder; publication is in flight and WILL complete

      is_interrupted_tombstone   (daemon.rs:2633)  mode 0200, size > 0
        a daemon wrote the record and died before chmod to 0600. NEVER resolves.

    **What remains: the narrowed retry**, which the maintainer approved in
    principle. It must be scoped to the exact inert shape (regular file, uid
    matches, nlink 1, mode 0200, **size 0**), with every other deviation still
    failing immediately. A blanket retry turns the interrupted case from an
    immediate diagnosable failure into a timeout — you burn the health window and
    then report "did not become healthy" instead of "a previous daemon died
    mid-publication".

    The next CI occurrence will now say which state fired. Run `31209969877`
    passed this test and run `31209575561` failed it (`autostart.rs:396`, job
    `92969028088`) — neither log recorded size, which is exactly what the
    instrument fixes.
  - **`scripts/generate-grpc.sh` rewrites six `.pb.go` files inside the
    `containerization` submodule**, which is a SEPARATE REPOSITORY. The Go arms
    were removed from Arca's script 2026-08-07 (option A), so this is CLOSED for
    the collateral damage. **What remains is the root cause**: nothing notices when
    Arca's checked-in Swift falls behind a submodule proto bump, which is how it
    came to be missing four RPCs. The remedy is a "generated code is current" check
    — run the generator, `git diff --exit-code` — and it is **booked against P2.3**
    because Arca has no CI and a check added now would be inert.
  - **`scripts/generate-grpc.sh` rewrites six `.pb.go` files inside the
    `containerization` submodule**, which is a SEPARATE REPOSITORY. arca#54 was
    scoped to Swift and reverted them. Whether this script should regenerate another
    repository's output at all is unanswered and unscheduled.
  - The `ssh-keygen` descriptor defect. NOT FIXED, deliberately deferred. Cause class
    VERIFIED (`Bad file descriptor`, parent descriptor intact at fork); mechanism
    unknown. `crates/gascand/tests/ssh_identity_concurrency.rs` reproduces it under
    load and will announce a recurrence. **Its reproduction rate on a healthy machine
    is UNKNOWN** — see the Spotlight trap.
  - `autostart.rs`'s symlink test (`daemon_attest_rejects_a_symlink_…`) still fails
    OPEN: both reader timeouts yield an empty buffer and the assertion is that the
    buffer is empty. The fix is mechanical (wait on process exit — the test already
    calls `.output()`, which waits — rather than a 1s wall clock). No design needed.
    A good warm-up if you want one.
  - `syspolicyd`/Gatekeeper. The Spotlight exclusion does not address it; it sat at
    33.7% after the suite went green. If timing flakes return WITHOUT `corespotlightd`
    being hot, that is where to look.
  - `arca/Documentation/SANDBOX_ENGINE_PIVOT.md` predates the 2026-08-05 reversal and
    still says `Sources/DockerAPI/` is deleted (`:57-66`, `:199`). The reversal negates
    that. Real work; nobody has scheduled it.

**CLOSED 2026-08-07, do not reopen:**
  - **D4 — `runtime-probe` is DELETED** from `ci.yml`. It asked a settled question
    (hosted `macos-26` has no `container` binary, exit 127) on every pull request and
    failed every time. `scripts/apple-test-preflight.sh` is KEPT — it is the local
    Apple preflight, referenced by six plan documents, and is not CI-specific.
  - **D5 — the stash is DROPPED**, commit `01db66d`, on the maintainer's explicit
    ruling. **The old description of it was WRONG**: it claimed "EXACTLY ONE file,
    `.superpowers/sdd/progress.md`, no tracked content". It actually held THREE files
    — `progress.md`, `task-2-report.md`, `task-4-report.md` — all marked `M`, so all
    tracked when it was made. The content was SDD scaffolding for work that shipped
    long ago (PR #13, release 0.1.20), which is why dropping it was still right. The
    commit stays reachable from the reflog until gc.

**TRAPS, all paid for already:**
  - **`gate` DOES NOT MEAN "everything passed".** Its `needs` is
    `[changes, rust, contracts, engine]`, so any job outside that list can be red
    while `gate` is green — VERIFIED on run `31209969877`, where `runtime-probe`
    failed and `gate` succeeded. The step is named "Require every job to have
    succeeded or been skipped", which is broader than what it checks. If you add a
    job and want it to block, add it to `needs`.
    **Do not go looking for `runtime-probe`** — it was deleted the same day (D4). The
    run is cited because it is the evidence; the lesson is about `needs`, not about
    that job.
  - **THE PIN NOW DRIVES THE RUST BUILD.** `engine/arca-pin.json` decides which
    revision `gascan-engine-proto` generates from. `scripts/ci-classify-paths.sh`
    maps `engine/*` to `rust=true` AND `engine=true` for exactly this reason. It
    mapped to `engine` alone until 2026-08-07, which would have let a pin bump ship
    a client generated from the wrong revision **and report green**.
  - **`mv dir existing-dir` DOES NOT FAIL** — it moves the source inside the target.
    This is why `sync-arca-proto.sh` publishes through an atomic `mkdir` claim. If
    you write another cache, do not reach for a bare `mv`.
  - **A generator that emits an empty module also exits 0.** Grep the output for the
    symbols you expect, then COMPILE it. Both sides now do: `swift build` compiles
    `SandboxEngineProto`, and `gascan-engine-proto`'s tests name types so an empty
    module fails to compile. A descriptor-set check catches the other half — a
    service that lost a method still compiles for callers that never used it.
  - **DO NOT REVIEW A PROTO BY READING IT.** A `oneof` named `result` emits seven
    `pub enum Result` types in Rust that shadow `std::result::Result`. Caught only by
    running the generator. A `oneof`'s NAME is not on the wire, so the rename was
    free before publication and is a breaking change now.
  - **`protoc-gen-grpc-swift` must be 1.27.0** to match Arca's `grpc-swift`
    dependency — `arca/scripts/generate-grpc.sh:39` enforces it. Installed: 1.27.0,
    alongside `protoc` 35.1 and `protoc-gen-swift` 1.38.1 (VERIFIED 2026-08-07). Gas
    Can's Rust side uses vendored protoc via `protoc_bin_vendored`, so the two sides
    do NOT share a protoc and neither inherits the other's constraint.
  - **Arca's checked-in generated Swift WAS stale against its protos**, and not only
    on formatting — `filesystem` was missing 4 RPCs entirely. arca#54 fixed the Swift.
    **The lesson is the one that matters: one file was sampled and four were
    described.** Verify the instrument, not only the result.
  - **The proto size gate is DECLARATION LINES, not raw lines.** The engine proto is
    483 raw but 275 declaration lines against `gascan.proto`'s 240/200, with 11 RPCs
    against 14. Threshold stays 400.
  - **THE LATENCY PROBE IN OLDER NOTES IS WRONG. DO NOT TIME A SHELL SCRIPT.** A
    `#!/bin/sh` script measured 0.005s while a freshly built Rust binary measured
    32.8s AT THE SAME INSTANT. The correct probe copies a built test binary to a
    BRAND-NEW path and times that:
      `cp target/debug/deps/store-* /tmp/probe && time /tmp/probe --list`
  - `~/code` IS EXCLUDED FROM SPOTLIGHT INDEXING, and that took a workspace run from
    37 failures to 0. If measurements go strange, verify it is still excluded:
    `mdfind -onlyin /Users/kiener/code -name Cargo.toml` must return NOTHING.
  - **The `ssh-keygen` defect's "load dependence" was characterised BEFORE that fix.**
    That load was partly Spotlight. Re-measure before assuming it behaves as recorded.
  - `cargo test --workspace` STOPS after the first failing binary — one unrelated
    flake hides everything after it. **Use `--no-fail-fast`.**
  - `cargo test <name>` without the full module path silently runs ZERO tests and
    exits 0. Always confirm the "running N tests" line.
  - A mutation test that does not flip is not a test. Forcing the predicate false MUST
    make it fail, with a useful message.
  - `ps -o pcpu` on macOS is a lifetime-weighted average, NOT current CPU. Use
    `top -l 2` when the claim is about now.
  - NEVER pattern-kill processes. Capture PIDs when you spawn (`pid=$!`) and kill
    those. `pkill -f` once destroyed five of the user's login shells.
  - The maintainer often has OTHER cargo builds running for other projects. Machine
    state is not yours alone; check before claiming two runs are comparable.
  - Capture exit codes directly, never through a pipe: `if cmd; then rc=0; else rc=$?; fi`.
    `${PIPESTATUS[0]}` after an `if` block reads empty.
  - `RUSTUP_TOOLCHAIN=1.95.0` is exported in this environment and overrides
    `rust-toolchain.toml`. Prefix `env -u RUSTUP_TOOLCHAIN` to be certain.
  - `cargo clippy --fix` is NOT safe here — it emitted invalid Rust at service.rs:2694
    and dropped needed parentheses in an SSH host-key check at ssh/manager.rs:647.
  - `crates/gascand` **and `crates/gascan`** deny `clippy::panic`,
    `clippy::expect_used`, `clippy::unwrap_used` **including in their own tests**
    (`gascan/src/lib.rs:2`), and forbid `unsafe_code`. Write test helpers that return
    `Result`, not ones that `panic!`/`expect`. This bit again 2026-08-07: a new test
    used `expect_err` and `cargo clippy --all-targets` rejected it, which `cargo test`
    alone would not have caught.
  - In `gascan-e2e`, assert child success with `gascan_e2e::succeeded(output)` —
    a bare `assert!(x.status.success())` reports nothing.
  - Repository rulesets update with **PUT**. `PATCH` on
    `/repos/{owner}/{repo}/rulesets/{id}` returns 404. PUT replaces the rules array
    wholesale, so restate every rule you are keeping.
  - A required check's context is the BARE JOB NAME (`gate`), not the UI's `ci / gate`.
  - `mergeStateStatus` reports branch policy and is VIEWER-INDEPENDENT. It stays
    BLOCKED even for an actor who can bypass. `current_user_can_bypass` answers
    "may I override this".
  - A workflow-level `paths:` filter cannot back a required check — GitHub leaves it
    Pending forever. A JOB reporting `skipped` satisfies it.
  - `actions/checkout` defaults to `fetch-depth: 1`; release-script-contract.sh
    resolves `HEAD~1`.
  - Hosted `macos-26` has NO `container` binary (exit 127, VERIFIED). The heavy Apple
    e2e tier cannot run there. Do not try again.
  - A bare `swift build` does NOT codesign. `packaging/macos/package.sh:64-69` signs
    WITHOUT `--entitlements` while Arca's `Makefile:62` signs with them. P7.3 must not
    discover this late.
  - `docker run --rm` does not remove, and names collide against a 36-name pool
    (arca#47). The `docker` CLI here points at a stopped Colima socket; Arca is its own
    daemon — start it on `/tmp/arca.sock` and set `DOCKER_HOST` accordingly
    (README.md:130-134, Makefile:123-124).
  - ~~Signing goes through the 1Password SSH agent. "communication with agent failed"
    means locked — ASK THE USER.~~ **CHANGED 2026-08-08 for THIS REPOSITORY ONLY.**
    Gas Can now signs with a repo-local key and pushes over HTTPS, so **neither
    signing nor pushing touches 1Password here**. Every other repository still uses
    the agent, and nothing global was changed.
      `user.signingkey` = `~/.ssh/gascan-signing` — a **file path**, not a literal
      key string. That is the whole trick: `ssh-keygen` signs straight from the
      private key, so no agent is consulted. The global config still holds a literal
      key string, which does need one.
      Transport is `remote.origin.url` = **https**, with
      `credential.https://github.com.helper` = `!gh auth git-credential`. All of it
      is `git config --local`; inspect with `git config --local --list`.
    **VERIFIED 2026-08-08**, four ways, each with the agent explicitly disabled via
    `env -u SSH_AUTH_SOCK`: `ssh-keygen -Y sign` rc=0; `-Y verify` returned
    `Good "git" signature`; a real commit reported `%G?` = `G` with fingerprint
    `SHA256:q6m/eNE…`; and GitHub reported `verified: true, reason: valid` on a
    pushed probe commit (since removed).
    Still true, and still absolute: **never fall back to `--no-gpg-sign` or a
    lightweight tag.** `tag.gpgsign` is now `true` locally so a signed tag is the
    default here.
    The ruleset targets `~DEFAULT_BRANCH` only (VERIFIED via
    `gh api /repos/.../rulesets/20492137`), so `required_signatures` gates `main`,
    not feature branches.
  - The permission classifier refuses `gh pr merge` and repo-admin `gh api` calls. Ask
    the user to run them with `!`. NEVER route around a refusal with a different tool
    performing the same irreversible action.

**CONVENTIONS THAT ARE LOAD-BEARING:**
  - Mark every claim VERIFIED or PLAN. Never promote a PLAN without running something.
  - Past-tense claims carry their anchor inline — command, SHA, file:line, exit code —
    or they come out. Rules may ship bare; events may not.
  - Record corrections in place, struck through with a pointer. Do not quietly edit a
    superseded conclusion away. Several are still visible on purpose — one was added
    2026-08-07 in the codegen spec, where a sample of one file was generalised to four.
  - When something is hard to diagnose, MAKE IT SAY MORE BEFORE GUESSING BETTER. This
    is the highest-yield habit in this project.
  - **Verify the instrument, not only the result.** Three instruments have now been
    confidently wrong here: the exec-latency probe, the proto size gate, and a
    one-file sample of a four-file diff. Instruments drift silently.
  - Before attributing an intermittent failure to the product, CHECK WHAT THE MACHINE
    WAS DOING. A Spotlight setting accounted for 37 of 38 failures.
  - **Prefer an A/B you can run over an argument you find convincing.** P3.2's whole
    design turned on one measurement: 1.3 GB versus 108 KB for the same provenance.
  - `docs/superpowers/` is TRACKED and committed. `.superpowers/` (dot-prefixed) is
    gitignored scaffolding. Two different paths; do not conflate them.
  - **TRUST THIS DOCUMENT'S SHAPE; VERIFY ITS FACTS.** It is right about what matters
    and what to avoid. It has been repeatedly wrong about specifics, because entries
    get written once and then carried. On 2026-08-07 alone, FOUR inherited
    descriptions were wrong: mode `0200` as one state (it is two), the D5 stash as one
    untracked file (three, tracked), the Arca regeneration as annotation churn (it
    added 4 RPCs), and a `gate` that "requires every job" (it requires four). Each was
    plausible, each had survived several sessions, and each would have produced a
    wrong action. **Before acting on an entry here, re-run the check it rests on.**

---

**CLOSING THOUGHTS FROM THE PREVIOUS SESSION — read once, then get to work.**

The wiring in P3.2 was the easy part. What nearly cost correctness, four times, was
believing a confident sentence written by an earlier session. The single habit that
caught all four was the same: **before describing a set, look at every member of it.**
One proto file was read and four were described; one stash entry was quoted and three
files were in it. That is not carelessness about facts — it is generalising from a
sample, and it is the failure this project keeps rediscovering under new names.

The corollary is cheap and worth adopting: when the maintainer asks you to explain an
open item, treat the explaining as the verification. Twice on 2026-08-07 the act of
writing a plain-language summary is what surfaced that the underlying entry was false.

**One thing is deliberately unfinished, and it is not a loose end — it is a decision
point.** D7's narrowed retry is approved in principle and unwritten. The instrument
landed instead, because the evidence could not distinguish the two `0200` states and a
blanket retry would have converted a diagnosable crash into a timeout. The next CI
occurrence will now say which state fired. **Do not write the retry until you have
that.** If you are tempted to skip the wait because the reasoning seems sound, that is
precisely the moment this document exists to interrupt.

**On picking the next task:** P5.2 (`gascan-arca`) is the highest-value step and its
type mapping is already derived — read the P3.1 design §9 before writing a line, since
it exists specifically so you do not re-derive it. P4 and P3.3 are genuine
alternatives. Ask the maintainer rather than assuming; the fan-out means there is no
single obvious next move, and that is by design.
