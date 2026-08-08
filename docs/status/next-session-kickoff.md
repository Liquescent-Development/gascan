# Kickoff — next session

> **The session entry point is now `docs/status/START-HERE.md`.** Point a new agent at
> that file and tell it to follow the instructions; it needs nothing pasted. This
> document is what START-HERE sends it to read — the traps, the still-open items, and
> the conventions. Both are current as of `149fa41`; if they ever disagree, the SDD
> ledger at `.superpowers/sdd/2026-08-08-gascan-arca/progress.md` settles it.

The block below is kept for reference and is still accurate. It is written to be read
cold.

---

Continue the Arca integration. Work happens in `~/code/gascan` and `~/code/arca`.

Read `~/code/gascan/docs/status/arca-integration-handoff.md`, starting at
"## Session of 2026-08-08" and reading to the end — that is the session you are
continuing, and it is the shortest path to the current state. Then read
**`.superpowers/sdd/2026-08-08-gascan-arca/progress.md`**, the SDD ledger: it names
every commit, every deferred finding and every ruling, and it is the authority where
this document and the plan disagree with it.

Then read `docs/superpowers/plans/2026-08-08-gascan-arca.md` — the plan you are
executing — and `docs/superpowers/specs/2026-08-08-gascan-arca-backend-design.md`,
its design. Read the design's §4.6 and §5 before touching any mapping code.

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

  **THAT FIGURE PREDATES THE `feat/gascan-arca` BRANCH AND HAS NOT BEEN RE-MEASURED
  ON IT.** A 2-minute attempt on 2026-08-08 timed out — a tooling limit, not a
  failure. Per-crate figures WERE measured and are green: `gascan-arca --lib` **21
  passed rc=0**, and `gascan-apple` plus `gascan-core` full suites clean. Task 10 of
  the plan owns the workspace measurement. **Its accounting total is a convenience,
  not an authority** — reviews added tests four separate times and moved it from 39
  to 47. Recount from the ledger.

**STATE:**

  gascan  **`feat/gascan-arca`, NOT `main`** — P5.2 is mid-flight on it and it is
          pushed. **Confirm the SHA and the branch with `git log --oneline -1` and
          `git rev-parse --abbrev-ref HEAD`** rather than trusting any SHA written in
          the docs: a documentation commit that records a tip is invalidated by its
          own merge. This has cost two sessions a round already. Do not chase it;
          verify it.
          **Signing and transport on this repo no longer use 1Password** — see the
          struck-through trap below for the mechanism and its verification. Commit
          with `env -u SSH_AUTH_SOCK git commit` if the agent is unreachable.
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

**WHAT LANDED 2026-08-08: HALF OF P5.2. FIVE OF TEN TASKS.**

  Branch **`feat/gascan-arca`**, pushed, tree clean. `crates/gascan-arca` exists with
  the transport seam, both halves of the mapping, and the error table.

  **Tasks 1-4 are complete with clean reviews. TASK 5 IS NOT — it is implemented and
  reviewed, but carries one OPEN IMPORTANT FINDING.** Close it before Task 6.
  Its field-placement test asserts rendered text for only **4 of the 12** accepted
  error codes, so a `resource`↔`message` transposition in any of the other eight
  (`ownership_mismatch`, `foreign_resource_refused`, `not_found`, `command_io`,
  `invalid_output`, `invalid_state`, `resource_conflict`, `helper_error`) passes all
  five tests — `code()` does not depend on which string fills which field. All eight
  were hand-verified correct, so it is a coverage gap, not a live bug. Extend the test
  to all twelve and flip two of the newly covered ones to prove it catches a swap.

  **Tasks 6-10 remain and every brief is staged and audited** in
  `.superpowers/sdd/2026-08-08-gascan-arca/`.

  Commits: `c980c7b`+`34eba01` (Task 1), `fd02093` (2), `09050c3`+`0140511` (3),
  `a82e110` (4), `15251ae` (5). Interleaved `docs:` commits are plan corrections and
  are listed in the handoff.

  **Resume by running the SDD skill against the existing ledger.** It records which
  tasks are done; do not re-dispatch them.

**WHAT LANDED 2026-08-07: P3.2. P3 IS COMPLETE.**

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

**YOUR TASK: finish P5.2. Tasks 6-10 of the plan, in order.**

  This is not a fan-out any more — it is an execution queue with five items and a
  written plan. Continue with `superpowers:subagent-driven-development` against the
  existing ledger.

  - **Task 6** is the largest: `ArcaBackend`, the fake transport, the sealed-request
    `PolicyCompiler` fixture, 11 tests. `CreateRequest` cannot be constructed outside
    `gascan-core` — the fixture builds one through `PolicyCompiler::compile`, copying
    `gascan-apple/tests/backend_fake_runner.rs`.
  - **Task 7** `logs`, **Task 8** `exec` (including the drop-cancellation test that
    nothing else can detect), **Task 9** the `tonic` arm, **Task 10** mutation flips
    plus the workspace gate.
  - **Task 9 ships with NO tests, by the maintainer's explicit ruling.** The only
    thing that could answer it is a Rust server, which is forbidden. Do not
    re-litigate it; the compiler checking it against `EngineTransport` is the stated
    assurance and the plan says so rather than dressing it up as coverage.
  - **The maintainer pre-ruled three shapes as plan-mandated**, so tell reviewers
    rather than looping on them: Task 6's temporary `exec`/`logs` stubs, Task 10
    committing nothing, and Task 3's deliberately non-compiling first file.

  After Task 10: final whole-branch review, then
  `superpowers:finishing-a-development-branch`. **Merge only, never squash or rebase,
  and only via a PR.**

**AFTER P5.2, the fan-out reopens:**

  - **P5.3 — extract the conformance suite** from `fake_runtime.rs` and run it against
    the fake, apple and arca backends. This is the natural successor: P5.2 leaves
    `gascan-arca` tested against a fake transport only, and P5.3 is what makes
    "both backends behave the same" systematic instead of a matter of having read the
    right file. Several parity properties are currently defended by single tests that
    P5.3 should absorb.
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

**TRAPS ADDED 2026-08-08, all paid for:**
  - **THE PLAN WAS THE DEFECT SOURCE, NOT THE IMPLEMENTERS.** All six defects that
    session came from plan or spec text; none came from an implementer's code. Three
    cost review rounds; the other three cost nothing because the brief was audited
    **before** dispatch. **Audit every brief before you dispatch it.** Minutes against
    a dispatch-plus-fix-plus-re-review.
  - **`cargo clippy -- -D warnings` CANNOT PASS for `gascan-arca` until Task 8.**
    `translate.rs` arrives before its consumer, so rustc's `dead_code` fires — 17 lib,
    13 lib-test, VERIFIED. The gate for Tasks 3-7 is `-D warnings -A dead_code`,
    **on the command line only. NEVER add `#[allow(dead_code)]` to the source** — an
    attribute outlives the condition that justified it and hides a genuinely dead
    function later. Task 8 restores the plain gate; Task 10's workspace gate is
    unconditional.
  - **EVERY `crates/gascan-core/src/runtime.rs` LINE NUMBER IN THE 2026-08-08 DESIGN
    SPEC IS STALE.** Task 1 added 58 lines. `from_resources` 944→**1001**,
    `discovered` 503→**554**, `RuntimeError::code` 1056→**1113**, `trait
    RuntimeBackend` 990→**1047**. The old numbers are kept on purpose (correct against
    `cf13a74`) with a table beside them. **Cite symbols, not lines; when a citation
    and the code disagree, the code wins.**
  - **THE TWO BACKENDS MUST AGREE ON `sandbox_id` FOR A `Mismatched` RESOURCE.** Both
    report the id it claims: `owner.managed_by == MANAGED_BY ? parsed : None`. The
    rule `match ownership { GasCanOwned => parsed, _ => None }` is WRONG and was
    reverted twice — once after shipping in `gascan-apple`, once caught in Task 4's
    brief before shipping. The reconciler at `gascand/src/service.rs:3001-3012` finds
    a mismatched container **by that claim with no ownership filter**, so withholding
    it silently drops an `OwnershipMismatch` finding. A parity test defends it.
  - **A DOCS-ONLY CI RUN SKIPS `rust` AND `engine` ENTIRELY** (VERIFIED, run
    `31262534703`). So a green docs run is **not evidence about D7**, or about
    anything in Rust. Check which jobs actually ran before drawing a conclusion.
  - **SUBAGENTS GO IDLE WITHOUT REPORTING.** Three times in one session — an
    implementer mid-fix-round and two reviewers. **Never read silence as success.**
    `git log` and a grep for the expected symbol establish the real state; once, an
    implementer had done nothing beyond an uncommitted edit. Do not accept a report's
    self-description either: one claimed a nudge was "stale" when direct verification
    showed the work had not landed.
  - **`ls` IS ALIASED** to something that rejects a trailing-slash path argument
    (`invalid value ... for '--icons'`). Use `find` or `git ls-files` in scripted
    checks rather than `ls`.

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

**CLOSING THOUGHTS FROM THE 2026-08-08 SESSION — read once, then get to work.**

**The plan was the defect source. Not one of the six defects came from an
implementer's code; all six came from text I wrote.** Three were caught by the loop
and cost a review round each. The other three cost nothing, because I audited the
brief before dispatching it. If you take one habit from this session, take that one:
**read the brief you are about to hand over, as an adversary, before you hand it
over.** The most alarming find was Task 4's brief still instructing an implementer to
write the exact rule that had been reverted as a regression two tasks earlier — a
defect that had already been "fixed" once, waiting in a document to be reintroduced
in the other backend.

**The instrument was narrower than the claim, five times, in five costumes.** A
`grep | head -10` that returned exactly ten and read as complete. A test asserting
`code()` while appearing to verify a whole mapping. A test *named*
`start_stop_and_prepare_image_report_an_ack` that never called `stop`. Test-count
figures that went stale as reviews added tests. And a grep for flip evidence whose
pattern could not match the format the evidence was in, which nearly had me accuse an
implementer of skipping a verification it had performed correctly. This project's
standing lesson is "verify the instrument, not only the result." It keeps arriving in
new clothing. **Prefer reading the artifact to pattern-matching it.**

**Silence is not success.** Three subagents went idle mid-job without reporting. Every
time, `git log` and a grep for the expected symbol told the truth, and once the truth
was that nothing had landed beyond an uncommitted edit. A subagent's self-report is
evidence, not a finding — one of them told me my nudge was stale when verification at
the time showed otherwise.

**What is deliberately unfinished, and why it is not a loose end.** Task 7 has no test
for a mid-stream `TransportError` or an unset `LogsChunk.outcome`. I found both, fixed
neither, and wrote them into the ledger — because a gap you can see is worth more than
a gap you patched without deciding whether it belongs to this task. Rule on it; do not
inherit it silently. The same applies to the five deferred minors: one of them,
`runtime_sandbox`'s untested unparseable-label branch, is the same class a reviewer
rated *Important* earlier in the session, and nothing later in the plan covers it.

**D7 is still the one thing you must not do early.** No `0200` occurrence has fired
since the instrument landed, and a docs-only run tells you nothing because it skips
`rust` entirely. The retry is approved in principle and remains unwritten. If the
reasoning feels sound enough to skip the wait, that is the moment this document exists
to interrupt.
