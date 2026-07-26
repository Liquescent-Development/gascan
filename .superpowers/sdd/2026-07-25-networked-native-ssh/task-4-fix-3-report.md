# Task 4 fix round 3 report

Status: DONE

Base: `24ab64b`

## Critical finding resolved

The removed PTY e2e test launched the production CLI with a temporary `HOME`
and assumed that selected its temporary `.ssh` directory. Production
intentionally resolves the effective account home from the account database,
so the test could inspect or mutate the real `~/.ssh/config` and Gas Can offer
receipt.

The production-binary test is gone. Its behavioral coverage now lives in a
hermetic CLI component test:

- it constructs `SshConfig` with an explicit temporary home;
- it creates an unsafe temporary `.ssh` mode to make the real offer inspection
  fail;
- it proves the successful `up` result remains exit code 0;
- it proves the concise manual-recovery warning is emitted;
- it proves no offer receipt is written beneath the injected temporary home.

The small result-preservation helper is the same production boundary used
after a successful `up`. Production account-home resolution is unchanged.

## Test-isolation guard and audit

Added a subprocess regression with overridden `HOME` and
`XDG_CONFIG_HOME`. It performs no mutation and proves:

- production `SshConfig::for_user` still selects the effective account's
  `.ssh/config` and managed offer directory;
- environment overrides cannot select those production mutation paths;
- only the explicit injected-home constructor selects the temporary paths.

The guard's mutation check was observed: temporarily restoring environment
selection made it fail with the overridden temporary `.ssh/config` instead of
the effective account `.ssh/config`; restoring account-database authority made
it pass.

The complete test-tree audit found:

- no `HOME` or `XDG_CONFIG_HOME` override remains in `gascan-e2e`;
- every test that calls `install`, `remove`, `record_offer_receipt`,
  `answer_first_use_offer`, or the failing offer component uses an explicit
  `SshConfig::for_environment` temporary home;
- remaining environment overrides are read-only OpenSSH/path authority checks,
  daemon path checks, or disabled-SSH behavior;
- no test invokes production `ssh-config install` or `ssh-config remove`.

No test reads or writes the real account SSH config or offer receipt.

## TDD evidence

- The hermetic replacement test first failed to compile because
  `preserve_up_result_with_optional_offer` did not exist.
- The minimal helper was then added and the focused regression passed.
- The environment-authority guard passed against accepted production behavior,
  failed under the deliberate environment-selection mutation, and passed
  again after restoring the accepted implementation.

## Verification

Focused and package suites:

- hermetic optional-offer component: 1 passed.
- `gascan --test ssh_config`: 16 passed.
- `gascan-e2e --test fake_backend`: 21 passed.
- `gascan`: 86 passed, 59 filtered.
- `gascand`: 255 passed.
- `gascan-e2e`: 237 passed, 8 ignored.

Complete workspace:

- `rtk cargo test --workspace`: 828 passed, 19 ignored, 59 filtered across
  60 suites.

Static verification:

- `rtk cargo check --workspace --all-targets --all-features`: passed.
- `rtk cargo clippy --workspace --all-targets --all-features -- -D warnings`:
  no issues.
- `rtk cargo fmt --all -- --check`: passed.
- `rtk git diff --check`: passed.

## Self-review

- Confirmed the unsafe PTY test and its environment/CI overrides are fully
  removed.
- Confirmed the replacement uses the real SSH configuration component with
  explicit temporary authority rather than a mock.
- Confirmed optional-offer failure cannot replace a successful operation
  result.
- Confirmed production effective-account home behavior has no diff.
- Confirmed all mutating include and receipt tests are hermetic.

Open concerns: none.
