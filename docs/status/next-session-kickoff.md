# Kickoff — next session

Paste the block below to start the next agent. It is written to be read cold.

---

Continue the Arca integration. Work happens in `~/code/gascan` and `~/code/arca`.

Read `~/code/gascan/docs/status/arca-integration-handoff.md`, starting at
"## Session close, 2026-08-07" and reading to the end. Then read the two sections
above it — "### The exec-latency probe is invalid" and "### The publish-window race
is no longer a hypothesis". Both correct things that were believed for several
sessions, and the reasoning matters more than the conclusions.

**THE GOVERNING DECISION, still binding:**

  A green `cargo test --workspace` ON THIS MACHINE counts as a pass. CI still runs
  and still reports, but it DOES NOT GATE. Do not re-enable a required status check.
  Do not spend this session on CI stability. We make CI stable after the product
  works with Arca as a backend, not before.

  As of 2026-08-07 the suite is genuinely green — 1377 passed, 0 failed, 22 ignored,
  rc=0 — so this is a real gate, not an aspiration. Keep it that way.

**STATE:**

  gascan  `main` 6f88e79, clean, ZERO open PRs.
          Suite VERIFIED green: `cargo test --workspace --no-fail-fast` rc=0,
          1377 passed / 0 failed / 22 ignored.
          Ruleset 20492137: deletion, non_fast_forward, required_signatures,
          pull_request with allowed_merge_methods ["merge"], bypass
          OrganizationAdmin. NO required_status_checks — keep it that way.
  arca    `main` 7da8f77, clean, UNTOUCHED FOR FIVE SESSIONS.
          Pin: `engine/arca-pin.json` -> tag `gascan-engine-ip-internal`,
          revision `d66c320c09e1dfc4f37aafa1fb27e36aa5cabe5d`. The annotated tag
          OBJECT is `dfdf8b9`; the commit is `d66c320c`. Do not confuse them.
          ARCA MAIN IS DELIBERATELY AHEAD OF THE PIN; the pin resolves via the tag.
  Open:   Vas-Solutus/arca#50 (broadcast-allocation defect, deliberately unfixed).

**YOUR TASK: P3.1**, in `docs/superpowers/plans/2026-08-04-arca-integration-roadmap.md`.

  P3 is the fan-out point — P4 and P5 both depend on it, and nothing else is
  unblocked. P1 is `partial by necessity` and stays that way: its binary half is
  booked against P5.1 and P4.3. Do not "finish" P1.

  P3.1 is: define the engine proto, derived from `RuntimeBackend`, constrained by
  contract §4 (what must be INEXPRESSIBLE) and §5 (what must be EXPRESSIBLE), and
  resolve **U4**. P3's exit is deliberately modest — "proto exists, both sides
  generate, nothing implements it yet."

  Two things make this heavier than transcription:
   - Per the 2026-08-05 weight increase, the proto is a REAL PUBLISHED CONTRACT with
     more than one consumer over time (Gas Can now, a Docker-compatible Arca later),
     so its compatibility burden is real from the first commit.
   - It LIVES IN ARCA ("Arca owns the wire protocol", P3.3). This session is the
     first to write in `~/code/arca` in five sessions. Read its conventions before
     assuming gascan's apply.

  This is design work. Consider brainstorming/planning before writing proto files.

**DO NOT:**
  - Re-enable a required status check, or work on CI stability.
  - Start P6, or resolve U5/U6 — genuine spec gaps that belong to P5.4 and P6.3.
  - "Finish" P1.
  - Squash- or rebase-merge either repo. Commit to main. Both are forbidden.

**STILL OPEN, none of it this session's task:**
  - D4 — delete `runtime-probe` from `ci.yml`. Spec §7.2 says the job is temporary
    and "then deleted"; §11.5 recorded its VERIFIED answer. It makes every workflow
    run report `conclusion: failure`. Real, but CI work.
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
    is now UNKNOWN** — see the trap below.
  - `autostart.rs`'s symlink test (`daemon_attest_rejects_a_symlink_…`) still fails
    OPEN: both reader timeouts yield an empty buffer and the assertion is that the
    buffer is empty. The fix is mechanical (wait on process exit — the test already
    calls `.output()`, which waits — rather than a 1s wall clock). No design needed.
    A good warm-up if you want one.
  - `syspolicyd`/Gatekeeper. The Spotlight exclusion does not address it; it sat at
    33.7% after the suite went green. If timing flakes return WITHOUT `corespotlightd`
    being hot, that is where to look.

**TRAPS, all paid for already:**
  - **THE LATENCY PROBE IN OLDER NOTES IS WRONG. DO NOT TIME A SHELL SCRIPT.** A
    `#!/bin/sh` script measured 0.005s while a freshly built Rust binary measured
    32.8s AT THE SAME INSTANT. The correct probe copies a built test binary to a
    BRAND-NEW path and times that — a new path matters, an already-evaluated binary
    is cheaper. Two lines, and the only form that sees the effect:
      `cp target/debug/deps/store-* /tmp/probe && time /tmp/probe --list`
  - `~/code` IS NOW EXCLUDED FROM SPOTLIGHT INDEXING, and that is what took a
    workspace run from 37 failures to 0. If measurements suddenly go strange, verify
    it is still excluded: `mdfind -onlyin /Users/kiener/code -name Cargo.toml` must
    return NOTHING.
  - **The `ssh-keygen` defect's "load dependence" was characterised BEFORE that fix.**
    That load was partly Spotlight. Re-measure before assuming it behaves as recorded.
  - `cargo test --workspace` STOPS after the first failing binary — later binaries
    never run, so one unrelated flake hides everything after it. **Use
    `--no-fail-fast`.** This cost a full hunt run on 2026-08-07.
  - `cargo test <name>` without the full module path silently runs ZERO tests and
    exits 0. Always confirm the "running N tests" line.
  - A mutation test that does not flip is not a test. Forcing the predicate false MUST
    make it fail, with a useful message. One "verified" assertion this session was
    vacuous because the condition it asserted happened to hold either way.
  - `ps -o pcpu` on macOS is a lifetime-weighted average, NOT current CPU. Use
    `top -l 2` when the claim is about now.
  - NEVER pattern-kill processes. Capture PIDs when you spawn (`pid=$!`) and kill
    those. `pkill -f` once destroyed five of the user's login shells. If you spawn
    load generators, a command timeout kills your cleanup line too — kill by captured
    PID afterwards and verify with `ps -p`.
  - The maintainer often has OTHER cargo builds running for other projects. Machine
    state is not yours alone; check before claiming two runs are comparable.
  - Capture exit codes directly, never through a pipe: `if cmd; then rc=0; else rc=$?; fi`.
  - `RUSTUP_TOOLCHAIN=1.95.0` is exported in this environment and overrides
    `rust-toolchain.toml`. Prefix `env -u RUSTUP_TOOLCHAIN` to be certain.
  - `cargo clippy --fix` is NOT safe here — it emitted invalid Rust at service.rs:2694
    and dropped needed parentheses in an SSH host-key check at ssh/manager.rs:647.
  - `crates/gascand` denies `clippy::panic`, `clippy::expect_used`, `clippy::unwrap_used`
    **including in its own tests**, and forbids `unsafe_code`. Write test helpers that
    return `Result`, not ones that `panic!`/`expect`.
  - In `gascan-e2e`, assert child success with `gascan_e2e::succeeded(output)` —
    a bare `assert!(x.status.success())` reports nothing. Fifty such sites were fixed
    on 2026-08-07; do not add the fifty-first.
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
  - Signing goes through the 1Password SSH agent. "communication with agent failed"
    means locked — ASK THE USER. `ssh-add -l` succeeding proves nothing. Never fall
    back to `--no-gpg-sign` or a lightweight tag.
  - The permission classifier refuses `gh pr merge` and repo-admin `gh api` calls. Ask
    the user to run them with `!`. NEVER route around a refusal with a different tool
    performing the same irreversible action.

**CONVENTIONS THAT ARE LOAD-BEARING:**
  - Mark every claim VERIFIED or PLAN. Never promote a PLAN without running something.
  - Past-tense claims carry their anchor inline — command, SHA, file:line, exit code —
    or they come out. Rules may ship bare; events may not.
  - Record corrections in place, struck through with a pointer. Do not quietly edit a
    superseded conclusion away. Several are still visible on purpose.
  - When something is hard to diagnose, MAKE IT SAY MORE BEFORE GUESSING BETTER. This
    is the highest-yield habit in this project and it has now paid off across sessions:
    instrumentation added for a race seen exactly once fired months of work later and
    turned it into a mechanism.
  - **Verify the instrument, not only the result.** Two measuring instruments were
    confidently wrong on 2026-08-07 and neither was caught by reasoning about them.
  - Before attributing an intermittent failure to the product, CHECK WHAT THE MACHINE
    WAS DOING. A Spotlight setting accounted for 37 of 38 failures.
  - Prefer an A/B you can run over an argument you find convincing. A well-reasoned
    fix lost 6/28 to 0/28 this session; the measurement cost one iteration.
  - `docs/superpowers/` is TRACKED and committed. `.superpowers/` (dot-prefixed) is
    gitignored scaffolding. Two different paths; do not conflate them.
