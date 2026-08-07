# Kickoff — next session

Paste the block below to start the next agent. It is written to be read cold.

---

Continue the Arca integration. Work happens in `~/code/gascan` and `~/code/arca`.

Read `~/code/gascan/docs/status/arca-integration-handoff.md`, starting at
"## Session of 2026-08-07 (later)" and reading to the end. Then read
`docs/superpowers/specs/2026-08-07-arca-engine-proto-design.md` in full — it is the
design for the file you will be generating from, and §9's type mapping exists
specifically so P5.2 does not re-derive it.

**THE GOVERNING DECISION, still binding:**

  A green `cargo test --workspace` ON THIS MACHINE counts as a pass. CI still runs
  and still reports, but it DOES NOT GATE. Do not re-enable a required status check.
  Do not spend this session on CI stability. We make CI stable after the product
  works with Arca as a backend, not before.

  Last VERIFIED green at `6f88e79`: 1377 passed, 0 failed, 22 ignored, rc=0. The
  2026-08-07 (later) session was documentation-only and did not re-run it, so that
  figure is carried, not re-measured. Re-measure before relying on it.

**STATE:**

  gascan  `main` clean, ZERO open PRs. **Confirm the SHA with `git log --oneline -1`**
          rather than trusting this line: the last content-bearing merge was `920c225`
          (#61), and a documentation commit that records a SHA is invalidated by its
          own merge. Do not chase it; verify it.
          Ruleset 20492137: deletion, non_fast_forward, required_signatures,
          pull_request with allowed_merge_methods ["merge"], bypass
          OrganizationAdmin. NO required_status_checks — keep it that way.
  arca    `main` `a974f17`, clean. **No longer untouched** — the engine proto
          landed there 2026-08-07 (#52).
          Pin: `engine/arca-pin.json` -> tag `gascan-engine-ip-internal`,
          revision `d66c320c09e1dfc4f37aafa1fb27e36aa5cabe5d`. The annotated tag
          OBJECT is `dfdf8b9`; the commit is `d66c320c`. Do not confuse them.
  Open:   Vas-Solutus/arca#50 (broadcast-allocation defect, deliberately unfixed).

**WHAT LANDED LAST SESSION: P3.1, and U4 is RESOLVED.**

  `arca/proto/arca/engine/v1/engine.proto` — package `arca.engine.v1`, service
  `SandboxEngine`, 11 RPCs, on Arca `main`. **P4 and P5 are now unblocked**; the
  roadmap's fan-out point is open. Nothing implements the proto and codegen is
  wired into neither build.

  **The proto is a PUBLISHED CONTRACT and it is now on `main`.** Its compatibility
  burden started at that commit. Field numbers are never reused; removals become
  `reserved`; the major version is the package path. Changing a shipped field is
  not a fix, it is a new package.

**YOUR TASK: P3.2** — codegen wired both sides, Swift server and Rust client.

  Both generators are VERIFIED to run against the file already (design doc §12), so
  this is wiring, not discovery. What is NOT yet known is how Gas Can's build
  reaches the file, which is the trap below.

  P3's exit is "proto exists, both sides generate, nothing implements it yet." Two
  of three are done.

**THE FIRST THING THAT WILL BLOCK YOU, VERIFIED 2026-08-07:**

  **The proto is ABSENT at the pinned Arca revision.** The pin resolves to
  `d66c320c`; the proto landed later, at `89916f5`/`a974f17`.

    git cat-file -e d66c320c…:proto/arca/engine/v1/engine.proto   ->   fails

  So Gas Can's build cannot see `engine.proto` until Arca is re-tagged and
  `engine/arca-pin.json` is bumped. That tag must be an ANNOTATED, SIGNED tag —
  never lightweight — which means the 1Password SSH agent must be unlocked. See the
  signing trap below. Decide the tag name before you start; the existing one
  (`gascan-engine-ip-internal`) describes the change that earned it, so a new name
  describing this one is the established pattern.

**DO NOT:**
  - Re-enable a required status check, or work on CI stability.
  - Redesign the proto. It is published. If you find a genuine defect in it, say so
    and treat it as a contract change with a cost, not an edit.
  - Resolve U5 or U6 — genuine spec gaps that belong to P5.4 and P6.3.
  - "Finish" P1. It is `partial by necessity`; its binary half is booked against
    P5.1 and P4.3.
  - Squash- or rebase-merge either repo. Commit to main. Both are forbidden.

**STILL OPEN, none of it this session's task:**
  - D4 — delete `runtime-probe` from `ci.yml`. Spec §7.2 says the job is temporary
    and "then deleted"; §11.5 recorded its VERIFIED answer. It is the ONLY failing
    check on a clean docs PR (VERIFIED on #59) and makes every workflow run report
    `conclusion: failure`. Real, but CI work.
  - D5 — `stash@{0}` `f6356f9` holds EXACTLY ONE file, `.superpowers/sdd/progress.md`,
    which is gitignored. No tracked content. Dropping it is the maintainer's call.
  - D7 — how the health check should treat mode `0200`. `validate_file_stat` reports
    the daemon's own not-yet-published record (created inert at 0200, published by
    chmod to 0600) as "unsafe". Mechanism VERIFIED 2026-08-07; the remedy is a design
    choice: retry, treat-as-absent, or wait. NEEDS A MAINTAINER DECISION.
  - The `ssh-keygen` descriptor defect. NOT FIXED, deliberately deferred. Cause class
    VERIFIED (`Bad file descriptor`, parent descriptor intact at fork); mechanism
    unknown. `crates/gascand/tests/ssh_identity_concurrency.rs` reproduces it under
    load and will announce a recurrence. **Its reproduction rate on a healthy machine
    is now UNKNOWN** — see the Spotlight trap.
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

**TRAPS, all paid for already:**
  - **GENERATE A PROTO BEFORE YOU COMMIT IT; DO NOT REVIEW IT BY READING.** A `oneof`
    named `result` emits seven `pub enum Result` types in Rust that shadow
    `std::result::Result` at every use site. Caught 2026-08-07 only by running
    `tonic_build` and grepping the output. A `oneof`'s NAME is not on the wire, so the
    rename was free before publication and would be a breaking change after it.
  - **A generator that emits an empty module also exits 0.** Grep the output for the
    symbols you expect, then COMPILE it. `cargo build` on the generated module is the
    real witness; `protoc` returning 0 is not.
  - **`protoc-gen-grpc-swift` must be 1.27.0** to match Arca's `grpc-swift` dependency
    — `arca/scripts/generate-grpc.sh:39` enforces this. The installed one is 1.27.0
    (VERIFIED 2026-08-07), alongside `protoc` 35.1 and `protoc-gen-swift` 1.38.1.
    Gas Can's Rust side uses vendored protoc via `protoc_bin_vendored`
    (`crates/gascan-proto/build.rs:4`), so the two sides do NOT share a protoc.
  - **The proto size gate is DECLARATION LINES, not raw lines** (restated 2026-08-07).
    Raw count compares commenting style: the engine proto is 483 raw but 275
    declaration lines against `gascan.proto`'s 240/200, with 11 RPCs against 14.
    Threshold stays 400.
  - **THE LATENCY PROBE IN OLDER NOTES IS WRONG. DO NOT TIME A SHELL SCRIPT.** A
    `#!/bin/sh` script measured 0.005s while a freshly built Rust binary measured
    32.8s AT THE SAME INSTANT. The correct probe copies a built test binary to a
    BRAND-NEW path and times that — a new path matters, an already-evaluated binary
    is cheaper. Two lines, and the only form that sees the effect:
      `cp target/debug/deps/store-* /tmp/probe && time /tmp/probe --list`
  - `~/code` IS EXCLUDED FROM SPOTLIGHT INDEXING, and that is what took a workspace
    run from 37 failures to 0. If measurements suddenly go strange, verify it is still
    excluded: `mdfind -onlyin /Users/kiener/code -name Cargo.toml` must return NOTHING.
  - **The `ssh-keygen` defect's "load dependence" was characterised BEFORE that fix.**
    That load was partly Spotlight. Re-measure before assuming it behaves as recorded.
  - `cargo test --workspace` STOPS after the first failing binary — later binaries
    never run, so one unrelated flake hides everything after it. **Use
    `--no-fail-fast`.**
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
    This bit once more on 2026-08-07 — `${PIPESTATUS[0]}` after an `if` block reads
    empty.
  - `RUSTUP_TOOLCHAIN=1.95.0` is exported in this environment and overrides
    `rust-toolchain.toml`. Prefix `env -u RUSTUP_TOOLCHAIN` to be certain.
  - `cargo clippy --fix` is NOT safe here — it emitted invalid Rust at service.rs:2694
    and dropped needed parentheses in an SSH host-key check at ssh/manager.rs:647.
  - `crates/gascand` denies `clippy::panic`, `clippy::expect_used`, `clippy::unwrap_used`
    **including in its own tests**, and forbids `unsafe_code`. Write test helpers that
    return `Result`, not ones that `panic!`/`expect`.
  - In `gascan-e2e`, assert child success with `gascan_e2e::succeeded(output)` —
    a bare `assert!(x.status.success())` reports nothing.
  - Repository rulesets update with **PUT**. `PATCH` on
    `/repos/{owner}/{repo}/rulesets/{id}` returns 404. PUT replaces the rules array
    wholesale, so restate every rule you are keeping.
  - A required check's context is the BARE JOB NAME (`gate`), not the UI's `ci / gate`.
  - `mergeStateStatus` reports branch policy and is VIEWER-INDEPENDENT. It stays
    BLOCKED even for an actor who can bypass. `current_user_can_bypass` answers
    "may I override this". A docs-only PR reads `UNSTABLE` because of D4 alone.
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
  - Signing goes through the 1Password SSH agent. "communication with agent failed"
    means locked — ASK THE USER. `ssh-add -l` succeeding proves nothing. Never fall
    back to `--no-gpg-sign` or a lightweight tag. **This will matter this session:**
    the pin bump needs a signed annotated tag.
  - The permission classifier refuses `gh pr merge` and repo-admin `gh api` calls. Ask
    the user to run them with `!`. NEVER route around a refusal with a different tool
    performing the same irreversible action.

**CONVENTIONS THAT ARE LOAD-BEARING:**
  - Mark every claim VERIFIED or PLAN. Never promote a PLAN without running something.
  - Past-tense claims carry their anchor inline — command, SHA, file:line, exit code —
    or they come out. Rules may ship bare; events may not.
  - Record corrections in place, struck through with a pointer. Do not quietly edit a
    superseded conclusion away. Several are still visible on purpose — two more were
    added 2026-08-07, in the backend spec and the contract.
  - When something is hard to diagnose, MAKE IT SAY MORE BEFORE GUESSING BETTER. This
    is the highest-yield habit in this project.
  - **Verify the instrument, not only the result.** Two measuring instruments were
    confidently wrong on 2026-08-07, and the size gate turned out to be a third — it
    measured commenting style rather than surface. Instruments drift silently; the
    anchor a gate was calibrated against grew 28% while the quoted figure did not.
  - Before attributing an intermittent failure to the product, CHECK WHAT THE MACHINE
    WAS DOING. A Spotlight setting accounted for 37 of 38 failures.
  - Prefer an A/B you can run over an argument you find convincing.
  - `docs/superpowers/` is TRACKED and committed. `.superpowers/` (dot-prefixed) is
    gitignored scaffolding. Two different paths; do not conflate them.
