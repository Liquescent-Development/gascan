# Gas Can CI consolidation — P2.1 design

**Date:** 2026-08-05
**Roadmap step:** P2.1, `docs/superpowers/plans/2026-08-04-arca-integration-roadmap.md:208`
**Resolves:** U3 (`roadmap:435-437`)
**Status of this document:** design, approved in conversation before writing. Every
claim below is marked **VERIFIED** or **PLAN**. A PLAN is never promoted without
running something. Past-tense claims carry their anchor inline — command, SHA,
`file:line`, exit code — or they are not made.

## 1. Scope

One pipeline in Gas Can covering the Rust workspace, the release contract suite,
protobuf codegen and the pinned Swift engine build, with path-based triggers from
the start, and a ruleset that makes the result load-bearing.

Arca gets no CI in this pass. It gets a **named roadmap step** instead — §9.

## 2. Why this phase pays for itself immediately

**VERIFIED.** A test in the hermetic suite has been failing since the moment it was
committed, and nothing noticed because nothing has ever run it.

- `crates/gascan-e2e/tests/fake_backend.rs:589-591` searches the **raw** PTY
  transcript for the literal `"✓ Sandbox is running"`.
- `crates/gascan/src/presentation.rs:636-642` emits that marker as
  `"\u{1b}[32m✓\u{1b}[0m"` when color is enabled, so the bytes are
  `ESC[32m` `✓` `ESC[0m` `" Sandbox is running"`. The raw `find` cannot match them.
- The same file already knows the correct idiom: `fake_backend.rs:606` uses
  `console::strip_ansi_codes(&stderr).ends_with("✓ Sandbox is running\r\n")`.
- Ordering, VERIFIED: `20de03d` "feat: render polished lifecycle progress"
  (2026-07-22 13:27:34 -0700) introduced the colored marker; `6d01465` "test:
  require PTY in-place redraw sequence" (2026-07-22 13:31:53 -0700) introduced the
  raw `find`. `git merge-base --is-ancestor 20de03d 6d01465` exits 0 — the marker
  landed **four minutes before** the test. The test was written against
  already-colored output and has never passed.
- Reproduction is environment-independent: three consecutive reruns each
  `RC=101`; `TERM` was already `xterm-256color` and forcing it again gave
  `TERM_SET_RC=101`; and the maintainer ran it in a real terminal with a
  controlling TTY and it failed identically.

Two weeks of red, invisible. This is the same argument P1.4's gate made when it
caught the uncold-buildable pin on its first run, and it is why P2.1 is worth doing
before more code lands.

## 3. Findings

### 3.1 There is almost no CI to consolidate

**VERIFIED.** Two workflow files exist:

| File | Lines | Automatic trigger |
|---|---|---|
| `.github/workflows/engine-pin.yml` | 31 | `pull_request` filtered to 4 paths (`:4-9`) |
| `.github/workflows/workspace-bundles.yml` | 459 | `push` to branch `feature/provisioning` only (`:3-16`), plus `workflow_dispatch` |

`gh run list -L 100 --json workflowName -q '.[].workflowName' | sort | uniq -c`
returns **9 runs, all `engine-pin`**. `workspace-bundles` has never executed.

So the Rust workspace — 7 crates (`Cargo.toml:2`), ~52k lines under `src/`, and
1,140 `#[test]`/`#[tokio::test]` annotations by grep count — plus the 14
`tests/release/*-contract.sh` scripts have **never** been built or tested in CI.
P2.1 is standing up Gas Can's first real CI, not merging existing pipelines.

### 3.2 Two of P2.1's four named languages are not in Gas Can

**VERIFIED.** `find . -name "*.go"` matches only paths under `.artifacts/`, which is
build output — a clone of the Arca engine. Go lives in Arca's containerization
submodule and is cross-compiled by `scripts/build-vminit.sh:54-80` **in Arca**. The
Swift that Gas Can touches is only the pinned-engine build that `engine-pin.yml`
already performs.

Protobuf codegen needs no pipeline step: `crates/gascan-proto/build.rs:4` uses
`protoc_bin_vendored`, so `cargo build` compiles `proto/gascan/v1/gascan.proto`
with a vendored `protoc` and no external install.

### 3.3 Baseline, measured rather than assumed

All timings are **warm-cache and therefore floors, not cold-runner numbers** — the
same caveat `2026-08-05-arca-engine-pin-design.md:113-117` applied to its Swift
figure.

| Command | Result | Anchor |
|---|---|---|
| `cargo fmt --all --check` | green | `FMT_RC=0`, zero output |
| `cargo clippy --workspace --all-targets -- -D warnings` | ~~green~~ **WRONG TOOLCHAIN — see §11.1** | `CLIPPY_RC=0`, 13.431s, 0 diagnostics, but on 1.95.0 not the pinned 1.85.0 |
| `cargo test --workspace --no-run` | green | `NO_RUN_RC=0`, 1:57.82 |
| `cargo test -p gascan-core -p gascan-proto -p gascan-inherited-fd -p gascan -p gascand` | green | `UNIT_RC=0`, 1:00.07, 43 binaries, 902 passed / 0 failed / 0 ignored |
| `cargo test -p gascan-apple -p gascan-e2e` | **1 red** | `REST_RC=101`, 1:16.63, 464 passed / 1 failed / 22 ignored |
| 14 × `tests/release/*-contract.sh` | green | 14 × `RC=0`, each captured per script |
| `scripts/build-arca-engine.sh` in CI | green | run `31055299650`, `conclusion=success`, `headSha=f562e6e` |

The single failure is the born-red test of §2. Everything else in the proposed gate
passes today.

`rust-toolchain.toml` pins `channel = "1.85.0"` with
`components = ["clippy", "rustfmt"]`, so CI needs no toolchain-installer action.

Engine build wall times from `gh run list`, for U3: green runs `f562e6e`
23:08:12→23:16:50 (8m38s), `efde07d` 23:19:36→23:27:01 (7m25s), `4ef1f16`
23:31:26→23:38:47 (7m21s), `abcc1fa` 23:41:40→23:49:05 (7m25s). The four red runs
failed in ~1m15s each.

### 3.4 The heavy suite is already self-documenting — 22 tests, not 11

**VERIFIED.** `cargo test --workspace -- --ignored --list` exits 0 and emits exactly
**22** lines matching `: test$`, matching the 22 `#[ignore = "…"]` attributes. Split:
11 in `crates/gascan-e2e/tests/apple_*.rs` (8 in `apple_apply.rs`, 1 each in
`apple_lifecycle.rs`, `apple_recovery.rs`, `apple_security.rs`) and 11 in
`crates/gascan-apple/tests/` (`attach`, `backend_contract`, `lifecycle`, `network`,
`resources`, `storage`). Every one carries a reason string naming its preconditions.

> ~~The ignore-set is 11 tests, all in `crates/gascan-e2e/tests/apple_*.rs`.~~
> **Corrected in the same session.** Two counting errors compounded: a first grep for
> `ignore\]` matched nothing at all, because the attribute form is
> `#[ignore = "reason"]`; the follow-up count then covered only
> `crates/gascan-e2e/tests/` and missed `crates/gascan-apple/tests/` entirely. The
> authoritative count comes from the test harness, not from grep.

Preconditions for the heavy set, VERIFIED from the scripts:
`scripts/apple-test-preflight.sh` asserts `uname -s` = Darwin and `uname -m` =
arm64, then runs `container system version --format json`;
`scripts/run-apple-e2e.sh:10-60` requires a digest-pinned candidate image **and** a
predecessor image receipt; the suite is driven by
`cargo test -p gascan-e2e --test "$test_name" -- --ignored --test-threads=1 --nocapture`.

~~**PLAN, explicitly not verified:** that a GitHub-hosted `macos-26` runner cannot run
these, because they need nested virtualization and the `container` tooling. §7.2
settles this with a measurement instead of leaving it as a belief.~~
**Settled — promoted to VERIFIED in §11.5.** The probe failed with
`container: command not found`, exit 127. The reason differs from the guess: the
tooling is simply absent, which is established without needing any claim about
nested virtualization.

### 3.5 The release contracts are hermetic

**VERIFIED.** Four contracts mention signing or notarization
(`distributable-package`, `publish`, `release-script`, `smoke`) and three mention
`sudo` (`cask`, `installer`, `smoke`), but they fake those dependencies rather than
invoking them: `installer-contract.sh:135` and `smoke-contract.sh:182` both
`write_fake sudo`, and `cask-contract.sh:41` merely `awk`s over `uninstall.sh` text.
All 14 exited 0 on a machine with no notarization credentials.

### 3.6 A path filter plus a required check is a trap

**VERIFIED** from GitHub's documentation:

> "If a workflow is skipped due to path filtering, branch filtering or a commit
> message, then checks associated with that workflow will remain in a 'Pending'
> state. A pull request that requires those checks to be successful will be blocked
> from merging."
> — <https://docs.github.com/en/actions/how-tos/manage-workflow-runs/skip-workflow-runs>

> "Required status checks must have a `successful`, `skipped`, or `neutral` status
> before collaborators can make changes to a protected branch."
> — <https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-protected-branches/about-protected-branches>

So a **workflow-level** path filter blocks a required check forever, while a **job**
reporting `skipped` satisfies it. `engine-pin.yml:4-9` filters at the `on:` level,
so making `engine-pin / build-engine` required would permanently block every PR
that does not touch those four paths — a docs-only PR like #45 could never merge.
This single fact decides the topology.

### 3.7 Nothing is enforced today

**VERIFIED** at design time: `gh api repos/Liquescent-Development/gascan/rulesets`
returns `[]`, and the repository reports
`{"squash": true, "rebase": true, "merge": true, "delete_branch_on_merge": false}`.
New checks would therefore block nothing, and the "never squash Gas Can" discipline
that protects every SHA citation in these documents
(`docs/status/arca-integration-handoff.md:818-826`) rests on memory alone.

## 4. Decisions

| # | Decision | Rationale |
|---|---|---|
| D1 | Gas Can only this pass; Arca CI becomes a named roadmap step | Gas Can's pipeline is what P2.1 names and is fully unblocked; Arca's gap is real but is a second repo with a known-red baseline (§9) |
| D2 | One workflow, internal change detection, one aggregate required check | The only topology satisfying both "one pipeline" and "path-based triggers" without §3.6's trap |
| D3 | Change detection in shell, not a marketplace action | A third-party action is a supply-chain surface inside the pipeline that attests releases; `scripts/` is already all `sh` |
| D4 | Hermetic tier on every PR; heavy tier stays out, capability probed | §3.4; the probe replaces a belief with a measurement |
| D5 | Required checks **and** `allowed_merge_methods: ["merge"]` | A check that blocks nothing is advisory; one squash invalidates every SHA these docs cite |
| D6 | Fix the born-red test inside P2.1 | §2; a required gate whose first run is red for an unexplained reason teaches everyone to ignore it |
| D7 | Every build and test job on `macos-26`; no Linux build job | macOS 26 is the product's only supported platform (`handoff:874`); both repos are public (`arca-engine-pin-design.md` §2.8) so the minutes are free. A Linux job would test a portability property that is not claimed. The two coordination jobs (`changes`, `gate`) run on `ubuntu-24.04-arm` because they execute only `git` and shell and never touch the toolchain |
| D8 | **No caching anywhere** | The engine-pin gate's value came entirely from being cold; a warm SwiftPM cache is what hid P1.4 for four sessions. Same class of risk applies to cargo, and §3.3's timings say no cache is needed. Recorded as a decision so it does not read as an omission someone later "fixes" |

## 5. Components

### 5.1 `.github/workflows/ci.yml`

Triggers: `pull_request` with **no** `paths:` filter, and `push: branches: [main]`.
`permissions: contents: read`. `concurrency: group: ci-${{ github.ref }}`,
`cancel-in-progress: true`, matching `engine-pin.yml:14-16`.

| Job | Runner | `if:` | Runs | Timeout |
|---|---|---|---|---|
| `changes` | `ubuntu-24.04-arm` | always | path→area booleans | 5 |
| `rust` | `macos-26` | `needs.changes.outputs.rust == 'true'` | `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace`; then §5.3's ignore-set diff | 30 |
| `contracts` | `macos-26` | `needs.changes.outputs.contracts == 'true'` | the 14 `tests/release/*-contract.sh` | 20 |
| `engine` | `macos-26` | `needs.changes.outputs.engine == 'true'` | `scripts/build-arca-engine.sh`, unchanged | 45 |
| `gate` | `ubuntu-24.04-arm` | `always()` | §5.4 | 5 |

**Five jobs, not six.** The ignore-set guard was originally scoped as its own job.
With no caching (D8), a separate job would recompile every test binary a second time
— `NO_RUN_RC=0` at 1:57.82 warm, more when cold — to produce a list the `rust` job
has already built the binaries for. It is a final step in `rust` instead. Consequence
accepted: if `cargo test` fails, the guard does not report, which is the correct
fail-fast order anyway.

`engine-pin.yml` is **deleted**; its job moves here verbatim. Cost accepted: the
`engine-pin / build-engine` check name retires and its 9 runs become orphaned
history. There is no migration hazard because no ruleset references it (§3.7).

`ci / gate` is the **only** required check, so job names below it can churn as P3,
P4 and P5 add work without touching the ruleset.

### 5.2 `changes`

Checks out with `fetch-depth: 0` and diffs explicit SHAs rather than `HEAD`, because
`actions/checkout` gives pull requests a synthetic merge ref:

```sh
git diff --name-only \
  "${{ github.event.pull_request.base.sha }}...${{ github.event.pull_request.head.sha }}"
```

On `push: main` it skips the diff and forces every boolean true. Filtering therefore
applies **only** to `pull_request`. Deriving a base from `github.event.before` would
need fallback logic for force-pushes and the initial-push `000…0` sentinel, and
fallback logic is what this repository's conventions forbid.

| Area | Paths |
|---|---|
| `rust` | `crates/**`, `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `proto/**` |
| `contracts` | `tests/release/**`, `packaging/**`, `scripts/**`, `docs/**`, `README.md` |
| `engine` | `engine/**`, `scripts/build-arca-engine.sh` |

`proto/**` maps to `rust` because `crates/gascan-proto/build.rs:14` compiles it.
`docs/**` and `README.md` map to `contracts` because `documentation-contract.sh`
asserts against them — so a docs-only PR runs the 14 contracts and skips Rust and
Swift. A change to `.github/workflows/ci.yml` forces all three true: if the pipeline
changes, the pipeline runs.

**The areas overlap deliberately.** `scripts/**` maps to `contracts` while
`scripts/build-arca-engine.sh` also maps to `engine`, so editing that one script
fires both. Overlap is intended: an area boundary that forced a file into exactly one
bucket would be the thing that silently under-tests.

**Unmapped paths force all booleans true and emit `::notice::` naming the path.** A
new top-level directory must not go silently untested. A notice was chosen over
failing the job because failing on an unrecognised file path is hostile, and the
conservative direction is running more, not less. This is a deliberate choice,
recorded as one.

Retires a known trap: "every push to a PR re-triggers the engine-pin gate, including
docs-only commits" (`handoff` traps list). Docs-only pushes now skip `engine`.

### 5.3 The anti-quarantine guard — final step of `rust`

`tests/ci/expected-ignored-tests.txt` holds today's 22 entries, sorted. The step
regenerates the list with `cargo test --workspace -- --ignored --list`, normalizes
lines matching `: test$`, sorts, and diffs against the file. **It fails in both
directions:** a new `#[ignore]` (quarantine creep) and a silently deleted heavy test
both break the build. VERIFIED implementable — the command exits 0 and emits exactly
22 matching lines (§3.4).

### 5.4 `gate` — the single required check

Runs `if: always()`, `needs: [changes, rust, contracts, engine]`, and
fails unless every result is `success` or `skipped`. `changes` is held to `success`
alone, because a skipped `changes` leaves the booleans undefined. `failure` and
`cancelled` both fail the gate as a consequence of the same check, with no special
casing.

Exit codes are captured directly, never through a pipe — `cmd | tail` returns
`tail`'s status, and five false "exit code 0" reports have come from that across
three prior sessions.

## 6. Error handling

- Every job carries `timeout-minutes` (§5.1). Fail fast; no job hangs indefinitely.
- No `continue-on-error` anywhere. A red step reddens its job and the gate.
- The `contracts` job records each script's exit code separately, so a failure names
  the script rather than reporting an aggregate.
- No caching (D8), so no cache-poisoning failure mode and no warm-cache masking.

## 7. Testing

### 7.1 The gate is mutation-tested, not eyeballed

A reviewer that only reads agrees with the code, and that applies to a pipeline. The
implementation must push a commit that deliberately breaks clippy, confirm
`ci / gate` goes **red**, then revert — recording both run URLs. An aggregation bug
in `gate` would make the gate wrong about everything at once, so a green run proves
nothing on its own.

### 7.2 Hosted-runner capability probe

A temporary job runs `scripts/apple-test-preflight.sh` on hosted `macos-26`. Its run
URL and verbatim output are recorded as VERIFIED **whatever the outcome**, replacing
the PLAN in §3.4, and the job is then deleted. If `container system version` works
on a hosted runner, D4 reopens in the project's favour, and that is worth knowing.

### 7.3 Acceptance

- `ci / gate` green on a PR touching only docs, with `rust` and `engine` reported
  `skipped` and `contracts` reported `success`. This is the case §3.6 says a
  workflow-level path filter would have blocked forever, so it is the acceptance test
  for D2 as much as for the mapping.
- `ci / gate` green on a PR touching `crates/**`.
- `ci / gate` **red** under §7.1's deliberate break.
- The §5.3 guard red when an `#[ignore]` is added without updating the expected file.

## 8. Sequencing

1. **PR A — the PTY test fix.** Its own PR and review. systematic-debugging first, so
   the fix follows a diagnosis rather than pattern-matching §2's hypothesis.
2. **PR B — the pipeline.** `ci.yml` added, `engine-pin.yml` deleted,
   `tests/ci/expected-ignored-tests.txt` added, plus §7.1's evidence.
3. **The ruleset, only after B is merged and `ci / gate` has passed on a real PR.**
   Require `ci / gate`; set `allowed_merge_methods: ["merge"]`. **Never require a
   check that has never passed.** The maintainer runs or approves the `gh api` call —
   the permission classifier sometimes refuses these, and routing around it with a
   different tool performing the same irreversible action is not acceptable.
4. **Docs.** U3 resolved with §3.3's numbers; §2 recorded in the handoff with both
   commit anchors; P2.1 marked done; Arca's CI written into the roadmap (§9).

Neither repository is squash-merged. Arca's ruleset enforces merge-only; Gas Can's
does not yet (§3.7), so until step 3 lands the discipline is manual.

## 9. Arca CI — the named follow-on

Arca has no CI: `gh run list -R Vas-Solutus/arca` returns `[]` and there is no
`.github/` in the tree or its history (`handoff:479-480`). That absence has already
distorted one design decision — P4.3 chose a target split over a build flag
"primarily because Arca has no CI" (`roadmap:277-280`).

Written into the roadmap as a step, not left implicit:

- `swift build` and `swift test`, against a **characterized** baseline. Arca has 125
  distinct failing tests on both sides of the P1.4 change (`handoff:716-721`), so a
  gate there means nothing until that baseline is pinned down.
- The Go `arca-services` cross-compile from `scripts/build-vminit.sh:54-80`.
- Needed **before** P4.3 and P5.1 land, so Arca-side changes stop getting their first
  build inside Gas Can's pipeline, in a different repository, at pin-bump time.

## 10. U3 resolved

> **U3 — Consolidated CI wall time.** Determines whether path filters are mandatory
> or merely nice. *Resolve by:* measuring after P1.1. *Blocks:* P2.1 design.
> — `roadmap:435-437`

**Answer: nice, not mandatory.** The Rust half is 1:00.07 for 902 tests and 1:57.82
to compile every test binary, both warm, against 7m21s–8m38s for the Swift engine
build (§3.3). Path filtering earns its keep on exactly one job — `engine` — which is
what D2's topology provides. The numbers are warm-cache floors; cold runners will be
slower, and the conclusion holds regardless because the ratio, not the absolute, is
what decides it.

## 11. Findings from the first real runs — 2026-08-05/06

The pipeline was landed on PR #48 (stacked on #47) and run against hosted runners
before the ruleset. Everything below is VERIFIED from those runs.

### 11.1 Correction: §3.3's local measurements were taken on the wrong toolchain

> ~~`cargo clippy --workspace --all-targets -- -D warnings` | green | `CLIPPY_RC=0`, 13.431s, 0 diagnostics~~

**Corrected.** That was measured with `rustc 1.95.0`, not the pinned 1.85.0.
`RUSTUP_TOOLCHAIN=1.95.0` is exported in the development environment, and that
variable **overrides `rust-toolchain.toml`**. VERIFIED: `rustup toolchain list`
reports `1.95.0 (active)` inside the repository while `rust-toolchain.toml` pins
`1.85.0`, and `env | grep RUSTUP` shows the override. CI honoured the pin correctly
— its "Report toolchain" step printed `rustc 1.85.0 (4d91de4e4 2025-02-17)`.

Every local Rust measurement in §3.3 is therefore about 1.95.0. The §3.3 claims
about `fmt`, the release contracts and the ignored-test count are unaffected; the
clippy claim was wrong for CI's purposes. **Measure with
`RUSTUP_TOOLCHAIN=1.85.0` until the override is removed.**

### 11.2 The tree did not compile on its own pinned toolchain

**VERIFIED.** `cargo build --workspace` under `RUSTUP_TOOLCHAIN=1.85.0` exits 101:
`error[E0658]: let expressions in this position are unstable` at
`crates/gascan/src/daemon.rs:1182`. Let-chains stabilised in Rust 1.88, while
`rust-toolchain.toml` pins 1.85.0 and `Cargo.toml:8` declares
`rust-version = "1.85"`. It was the only let-chain in the tree.

Fixed as a nested `if` (`9bee529`), preserving the short-circuit, because that
honours the declared MSRV. **Raising the pin to ≥1.88 instead is a live
alternative and is an MSRV policy decision for the maintainer**, not one this
phase should make unilaterally.

### 11.3 Clippy had never passed on the pinned toolchain

**VERIFIED.** Once the tree compiled, `clippy::format_collect` fired at **eleven**
call sites across `gascan-core`, `gascan`, `gascand`, `gascan-apple` and
`gascan-e2e`. They surfaced one crate at a time, because clippy stops at the first
failing crate — so the first CI run showed only one.

All eleven were the same idiom, `bytes.iter().map(|b| format!("{b:02x}")).collect()`.
Replaced by one `gascan_core::hex::lower` (`b2003df`), which is DRY and fixes the
lint in a single place. `CLIPPY_185_RC` went 101 → **0**, the first time clippy has
passed on the pinned toolchain. Equivalence is asserted over all 256 byte values,
because these strings become filenames, owner tokens and persisted digests.

### 11.4 The `contracts` job needed full history — a defect in §5.1

**VERIFIED.** 4 of 15 contracts failed on the first run:
`fatal: Failed to resolve 'HEAD~1' as a valid ref`, with
`distributable-package rc=65`, `publish rc=128`, `release-script rc=128`,
`signal rc=1`. `actions/checkout` defaults to `fetch-depth: 1`.
`release-script-contract.sh` resolves `HEAD~1`. Fixed by `fetch-depth: 0` on that
job (`06d4c67`). §5.1's table set it only on `changes`.

### 11.5 The hosted runner has no Apple container runtime — §3.4's PLAN settled

**VERIFIED**, replacing the PLAN in §3.4: the `runtime-probe` job failed with
`./scripts/apple-test-preflight.sh: line 8: container: command not found`,
exit **127**, on `macos-26` (`ProductVersion: 26.5.2`). **D4 stands on evidence.**
The heavy Apple tier cannot run on hosted runners, independent of the
candidate-image problem.

### 11.6 `gate` reddens correctly — §7.1 satisfied without staging a break

**VERIFIED** from run `31074653442`: `changes=success`, `engine=success`,
`rust=failure`, `contracts=failure`, and **`gate=failure`**. The aggregation
propagates failure, so the required check would have blocked. The green and
`skipped` directions still need confirming (§7.3).

`changes` and `engine` both succeeded on real runners, so the classifier works and
the folded-in engine build is intact.

### 11.7 BLOCKING: the test suite is flaky, and Task 8 must wait

**VERIFIED and root-caused.** `cargo test --workspace` fails intermittently with a
**different test each run** — 3 red / 1 green locally. Observed failures:
`accepted_socket_without_http2_cannot_block_initial_probe`
("initial readiness probe exceeded its bound"),
`environment_teardown_terminates_its_exact_live_daemon`,
`daemon::tests::inherited_startup_diagnostic_survives_path_replacement`,
`client::tests::daemon_spawner_uses_protected_cwd_environment_and_detached_stdin`,
and 4 in `gascan-e2e --test autostart`.

Root cause, measured rather than inferred:

- `cargo test --workspace` runs **6–8 test binaries concurrently** (sampled with
  `ps` during a run).
- Each binary independently defaults to `--test-threads` = `num_cpus` = 10 on this
  machine, so ~60–80 concurrent test threads on 10 cores. Cargo's `-j` bounds build
  jobs and binary launches, not threads *inside* each binary. Nothing enforces a
  global budget.
- The waits are hard wall-clock deadlines — `FIXTURE_DAEMON_DIAGNOSTIC_DEADLINE`
  is `Duration::from_secs(5)` (`client.rs:20`) — on a spawned fixture, and every
  failure message is such a deadline.
- Isolated, the same binary is green **5/5** (`-p gascan --lib`, at both
  `--test-threads=1` and the default), and each failing test passes 3/3 alone.
- It is pre-existing: red on a base branch that changes zero Rust.

Two things worth recording about the mechanism. The fixture "daemon" is a
`#!/bin/sh` script (`daemon.rs:3390`), not the 41 MB binary, so an earlier
Gatekeeper/`syspolicyd` hypothesis does not explain it — 5 s is ~500× what the
script needs. And the wait loop never checks whether the child is still alive, so
it cannot distinguish "slow" from "dead"; the failure reports an empty `stderr`
either way. That diagnostic gap should be closed before or alongside any fix.

> ~~**Consequence: §8 step 3 (the ruleset) must not proceed until this is fixed.**~~
> **Superseded 2026-08-06 (night).** The ruleset was applied and `ci / gate` is now a
> required check on ruleset `20492137`, with enforcement proven — run `31134223492`,
> `gate` job `92729989072` = `failure`, `mergeStateStatus: BLOCKED`. The flakiness is
> **not** resolved: it was reproduced on CI across two runs of a byte-identical tree
> (`2c7de30…`), `31129682364` green and `31130737502` red. The maintainer's decision was
> to accept it for now, re-run flaky jobs, and keep watching, with an
> `OrganizationAdmin` bypass on the ruleset so a flake cannot wedge the repository.
> See the handoff's "Session of 2026-08-06 (night)".

**Consequence: §8 step 3 (the ruleset) must not proceed until this is fixed.** A
required `ci / gate` over a suite with this failure rate would block merges for
reasons unrelated to the change under review. Reducing parallelism or adding
retries would hide it rather than fix it, which this project's conventions forbid,
so the fix needs its own scoped task — most likely closing the child-death
diagnostic gap, then deciding between a global test-parallelism budget and
deadlines that bound hangs rather than racing load.

## 12. Explicitly out of scope

- **`workspace-bundles.yml`.** 459 lines that have never executed once, triggered
  only by pushes to `feature/provisioning` (§3.1). A latent liability deserving its
  own decision — verify it or retire it — but folding it in would double this scope.
- **The heavy Apple e2e tier.** D4. Revisit if §7.2 says the hosted runner can.
- **Arca's CI.** §9, as a roadmap step.
- **P2.2.** Extending `build-manifest.json` to attest engine and guest binaries
  cannot complete until P5.1 produces an engine binary
  (`arca-engine-pin-design.md` §2.3, §7). P2 therefore stays open after P2.1.
- **Caching.** D8, deliberately.
- **The flaky test suite.** §11.7 root-causes it; fixing it is its own task and gates the ruleset.
