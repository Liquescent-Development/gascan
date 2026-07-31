# Task 7 Implementer Report

## Status

Complete.

Task 7 adds the public `gascan configure` guide and the focused `git`, `gh`,
and `glab` commands. It deliberately does not add the Task 8 post-`up` offer.

## Inherited state

- Worktree: `/Users/kiener/code/gascan/.worktrees/developer-onboarding`
- Base commit: `4e0d9ceda2af868e7df42eb8beeff1a02f3b7b95`
- The inherited uncommitted Task 7 scaffold modified only `cli.rs`: it exposed
  the Clap command shape but dispatched every configure form to
  `configure is not implemented`.
- Existing Task 5/6 host, Git, prompt, and forge adapters were preserved and
  reused. Small extensions to `git.rs` and `prompt.rs` were necessary to load
  current Git state, complete the receipt, provide terminal I/O, and make
  ordinary prompts safely cancellable.

## Implemented behavior

- Added the aggregate and focused public command forms with the existing
  global `--sandbox` selector.
- Uses the existing selector, fetches sandbox status, and requires
  `ActualState::Running` before constructing exactly one `ClientGuestRunner`.
- Added TTY preflight before daemon connection. Interactive aggregate, Git,
  and hidden-token flows require terminal stdin and stderr.
- `--token-stdin` requires piped stdin, is bounded to 1 MiB, is retained in
  zeroizing storage, is read once after the running-sandbox gate, and is never
  printed. HTTPS is required for this noninteractive mode. SSH token-pipe use
  is rejected before daemon connection with instructions to rerun
  interactively for first-use host verification or pass
  `--git-protocol https`.
- Added aggregate order: Git/key, GitHub, GitLab, concise summary.
- Displays current Git state and host defaults as editable prompt context,
  reuses coherent existing Git/key state, and defaults transport to SSH.
- Performs the exact no-secret default-route probe before remote sections.
  Offline and failed-probe outcomes retain and summarize Git, do not complete
  the receipt, and provide focused retry guidance.
- Lists detected host accounts by validated login/hostname, retrieves a host
  token only after explicit confirmation, and falls back to hidden entry
  without displaying native failure output.
- Hidden entry supports manually entered enterprise/self-managed hostnames;
  declining or failing import retains the selected account hostname.
- Explicit GitHub/GitLab section skips may complete the receipt after Git is
  configured. Declining an absent Git setup leaves dependent sections partial
  and never completes the receipt.
- Structured forge failures retain partial registration state, print safe
  focused retries, and never expose token or native output.
- Summaries include only identity, hostname/login, protocol, fingerprint,
  authentication state, and registration state. They omit token and public-key
  material.
- Prompts and their decision context use stderr; final summaries use stdout.
- Ordinary and hidden prompts install a process-lifetime SIGINT policy:
  Ctrl-C during a prompt returns clean cancellation, while SIGINT outside a
  prompt retains its default disposition. Terminal flags and echo state are
  restored on every exit path.

## TDD evidence

Initial RED evidence was established before the main implementation:

- The public process suite failed because configure help exited 64, aggregate
  redirected input reached daemon handling, and token-stdin with a TTY reached
  daemon handling.
- Coordinator tests initially failed to compile because `ConfigureIo`, the
  outcome type, and all coordinator entry points did not exist.
- CLI tests initially failed to compile because configure preflight,
  selector/running helpers, and bounded token input did not exist.

Independent review repairs also began from focused RED evidence:

- Piped token plus default SSH reached daemon handling instead of failing
  before mutation.
- Declining absent Git attempted to write a completion receipt.
- Manual enterprise hidden entry produced a partial result for the hardcoded
  public hostname.
- Route-probe transport failure aborted without retained-work summary.
- A real SIGINT during an ordinary canonical line prompt hung or terminated
  outside the clean cancellation path.
- Prompt stream tests showed prompts on stdout rather than stderr.
- A subprocess proved SIGINT was ignored after a completed prompt because the
  prior signal action had been unregistered without restoring disposition.
- Coordinator tests showed current/default/account decision context on stdout.

Each of those failures now has a passing regression test.

## Verification

Final verification at the uncommitted implementation state:

- `rtk cargo fmt --all -- --check` — pass.
- `rtk git diff --check` — pass.
- `rtk cargo test -p gascan --lib --locked --offline configure::` — 61 passed.
- `rtk cargo test -p gascan --test configure_cli --locked --offline` — 6 passed.
- `rtk cargo clippy -p gascan --all-targets --locked --offline -- -D warnings`
  — pass with zero warnings.
- `rtk cargo test -p gascan --locked --offline` with host process/socket access
  — 283 passed across 7 suites in 3.90 seconds.

The first restricted full-package attempt was stopped after more than three
minutes without output because process/socket-heavy tests stalled under the
restricted sandbox. The same command completed immediately with the
established host-process access and is the full-package evidence above.

The plan names standalone `configure_host`, `configure_git`, and
`configure_forge` integration targets, but this repository keeps those tests
inside the gascan library. `configure::` and the full package run cover all of
those inherited suites.

## Review

Independent review initially requested changes for:

- token-pipe SSH first-use verification;
- completion without explicit dependent work;
- enterprise hidden-token hostname loss;
- missing retained-work output after route-probe failure;
- ordinary Ctrl-C handling;
- prompt/output stream mismatch;
- SIGINT disposition after a prompt; and
- redirected prompt decision context.

All Important findings and the adjacent stream finding were repaired and
reverified. Final scoped re-review approved the correctness and security
repairs without a remaining functional finding. Its strict-clippy observation
identified two inherited warning sites; the test-only mutability was expressed
without a production warning and the plan-mandated reserved error variant was
given a narrow documented allowance.
The reviewer then returned unconditional approval with no remaining Critical,
Important, or Minor findings.

## Remaining concern

The process suite directly covers command forms, global sandbox parsing,
invalid/forbidden arguments, both token-stdin usage errors, the accepted HTTPS
pipe boundary, redirected aggregate refusal, and sentinel absence. No reusable
fake-daemon process harness exists in this crate, so no/multiple/stopped
sandbox process behavior and exact runner construction remain covered by pure
CLI helper tests plus code inspection rather than a spawned binary connected
to a fake daemon. This was recorded as a Minor review coverage gap; the
selector and running-state regressions are nevertheless tested.

## Files

- `crates/gascan/src/cli.rs`
- `crates/gascan/src/configure/git.rs`
- `crates/gascan/src/configure/host.rs`
- `crates/gascan/src/configure/mod.rs`
- `crates/gascan/src/configure/onboarding.rs`
- `crates/gascan/src/configure/onboarding_tests.rs`
- `crates/gascan/src/configure/prompt.rs`
- `crates/gascan/tests/configure_cli.rs`
- `.superpowers/sdd/2026-07-30-developer-onboarding/task-7-implementer-report.md`
