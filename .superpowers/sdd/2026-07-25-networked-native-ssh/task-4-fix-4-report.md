# Task 4 fix round 4 report

Status: DONE

Base: `5b6ac4c`

## Critical finding resolved

The progress-rendering e2e helper attached the production Gas Can process's
stderr to a PTY but left stdin inherited. When the test runner itself had
terminal stdin, a successful non-JSON `gascan up` saw both stdin and stderr as
TTYs and could enter the production first-use SSH include offer against the
effective account home.

The progress-only helper now always configures `Stdio::null()` for stdin while
retaining its stderr PTY. Both successful and failing production `up` progress
tests use this exact helper. No production code or account-home behavior
changed.

## Regression and TDD evidence

Added a deterministic helper contract regression:

1. The parent test launches a nested copy of the test with terminal stdin.
2. The nested test runs a shell probe through the same stderr-PTY helper used
   by production `gascan up`.
3. The probe requires fd 0 to be non-TTY and fd 2 to be TTY.

The test first failed to compile because the generic command-level PTY helper
did not exist. After extracting the existing behavior without changing stdin,
it failed behaviorally because the probe inherited TTY stdin. Adding
`Stdio::null()` made the same regression pass.

This contract prevents successful progress-only `gascan up` from satisfying
the CLI's interactive-offer gate.

## Production-binary spawn audit

- `invoke_with_stderr_pty` was the only progress-only production helper that
  combined a stderr PTY with inherited stdin. It is fixed.
- `run_pty_to_eof` gives shell tests TTY stdin but pipes stderr, so the
  first-use offer gate cannot activate.
- Other fake-backend production spawns use piped/null stdin or non-TTY stderr.
- Apple standard invocation helpers pipe stderr.
- Apple full-PTY helpers are deliberately isolated shell TTY, resize, and
  signal tests after sandbox creation. They never invoke `up` and never touch
  SSH include or receipt state.
- Autostart uses null stderr; doctor and version helpers capture output.
- No e2e test overrides `HOME` or `XDG_CONFIG_HOME`.

No e2e path can combine production `up`, TTY stdin, and TTY stderr.

## Verification

Focused and package suites:

- progress PTY contract regression: 1 passed.
- successful production `up` stderr-PTY progress: 1 passed.
- failed production operation stderr-PTY progress: 1 passed.
- `gascan-e2e --test fake_backend`: 22 passed.
- `gascan`: 86 passed, 59 filtered.
- `gascand`: 255 passed.
- `gascan-e2e`: 238 passed, 8 ignored.

Complete workspace:

- `rtk cargo test --workspace`: 829 passed, 19 ignored, 59 filtered across
  60 suites.

Static verification:

- `rtk cargo check --workspace --all-targets --all-features`: passed.
- `rtk cargo clippy --workspace --all-targets --all-features -- -D warnings`:
  no issues.
- `rtk cargo fmt --all -- --check`: passed.
- `rtk git diff --check`: passed.

## Self-review

- Confirmed the only functional change is in the e2e harness.
- Confirmed the progress helper sets stdin explicitly on every call.
- Confirmed stderr remains a PTY and progress rendering remains covered.
- Confirmed the regression is independent of the outer runner's normal stdin
  because it supplies a PTY deliberately.
- Confirmed no test can enter the production SSH offer through this helper.

Open concerns: none.
