# Whole-branch fix round 1 report

Status: READY

Base commit: `e8b6331`

## Implemented fixes

### Durable SSH publication recovery

- Added durable `before_ssh_publication`,
  `before_ssh_publication_marker`, and `after_ssh_publication` phases.
- Pending SSH-enabled creates no longer complete from provisioning or health
  hooks alone. Recovery verifies the exact canonical SSH configuration and
  durable resolution under the publication guard.
- Recovery repairs missing resolution or configuration state and persists the
  final publication marker before completing the operation.
- Failed repair removes the alias before stopping and deleting owned runtime
  resources, clears SSH policy and resolution state, and fails the create
  absent.
- Uncertain alias removal, cleanup, or publication preserves pending state and
  runtime resources for a safe retry.
- Added a debug-only crash hook and child-process `SIGABRT` tests for all three
  required publication windows.

### Existing-running failure cleanup

- Retained `up` now removes the published alias before stopping the runtime
  after host-key, readiness, or configuration activation failures.
- The original activation error remains primary when deactivation also fails.
- Alias removal is attempted before cleanup inspection. Inspection returning
  an error or no runtime cannot skip deactivation or replace the primary SSH
  failure.
- If alias removal cannot be proven, the runtime remains running so an alias
  cannot point at a stopped sandbox.
- Operation failure-reporting and terminal-event errors are wrapped with the
  original activation failure kept primary.

### Unambiguous configuration commit

- Configuration replacement snapshots the exact prior bytes and identity,
  stages and syncs the replacement, then verifies publication.
- Post-rename directory-sync and metadata failures atomically restore and
  verify the prior configuration, or remove and verify a newly created
  configuration.
- Added typed `unpublished` and `published-but-uncertain` outcomes.
- Fresh creates roll back only proven-unpublished updates. A
  published-but-uncertain outcome preserves the runtime, durable SSH
  resolution, and alias consistency.

## TDD evidence

The new tests failed against the prior implementation before production
changes:

- Existing-running host-key and readiness failures both left the prior alias.
- Configuration failure with unsafe deactivation stopped the runtime instead
  of preserving it.
- Post-rename directory-sync and metadata faults left the replacement rather
  than restoring the prior configuration.
- A fresh post-rename failure left a newly created configuration present.
- Restoration failure returned `unpublished` instead of a typed uncertain
  outcome.
- All three seeded recovery windows completed without the required durable
  publication marker or complete SSH state.
- Failed recovery repair retained owned runtime resources.
- Fresh-create publication uncertainty was misclassified as an ordinary
  configuration update failure.
- The child-process crash matrix completed normally instead of aborting at the
  required durable boundaries.
- A cleanup-time runtime inspection error replaced the original readiness
  failure before alias removal.
- Cleanup inspection returning no runtime skipped alias removal and left the
  stale published alias.

The same focused tests pass with the completed implementation.

## Verification

- Focused configuration tests: 6 passed.
- Full lifecycle integration suite: 78 passed.
- Full reconciliation integration suite: 24 passed.
- Concurrent SSH publication regression: passed.
- Complete daemon package: 296 passed, 15 suites.
- Complete workspace: 875 passed, 20 ignored, 61 filtered, 60 suites.
- Complete scripts workspace: 410 passed, 47 suites.
- `cargo check --workspace --all-targets --all-features`: passed.
- Strict Clippy for the workspace, all targets, and all features: passed with
  no warnings.
- `cargo fmt --all -- --check`: passed.
- `git diff --check`: passed.

One redundant workspace run exposed a transient doctor-test wait. Both doctor
suites passed serially, and a fresh complete workspace run passed. A prior
scripts rerun left only the command-output wrapper waiting after its child
tests completed; the task-owned wrapper and child processes were terminated,
and the fresh 410-test run above is the recorded result.

## Live Apple acceptance and cleanup

- Apple preflight passed on macOS 26.5.1 arm64.
- Apple Container CLI and API server reported version 1.1.0 and revision
  `5973b9c`.
- Native SSH acceptance passed: 1 passed, 0 failed, 59 filtered, in 39.52
  seconds.
- The test used the unchanged approved immutable candidate image and its
  distinct compatible predecessor.
- The repository's official cleanup accepted and removed
  `native-ssh-fix1-3C9KyTdq8Csc.json`.
- The scoped Apple cleanup root was empty after cleanup.

## Review

Two independent focused re-reviews completed.

- Critical findings: none.
- Important findings: none.
- Minor findings: none.
- Verdict: Ready.

The first re-review identified a cleanup inspection ordering gap after the
three original blockers were closed. Two additional RED-to-GREEN regressions
now prove both inspection-error and missing-runtime behavior. The final
review accepted the corrected deactivation ordering and primary-error
precedence.

No approved image, bridge, offline SSH support, push, pull request, version,
tag, or release was changed in this fix round.
