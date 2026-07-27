# Task 5 Report: Document Migration and Restore a Green Local Baseline

## Outcome

Task 5 is implemented and locally verified.

- The stale SSH warning fixture now uses unsafe mode `0775`.
- A neighboring regression test proves conventional mode `0755` succeeds
  without a warning.
- The README documents the version-2 managed roots, exact pre-0.1.10
  destroy/recreate sequence, approximately 1.5 GiB Rust seed cost,
  independent capacities, and copyable package-manager workflows.
- The macOS release checklist records the layout migration and installed
  release write checks.
- The installed release smoke inspects the exact three volume targets, checks
  writable runtime homes, runs Cargo against exact crates.io dependency
  `cfg-if = "=1.0.4"`, installs local Rust/npm/Go/Python/Ruby commands, proves
  those commands resolve below `/home/workspace/.local`, and creates XDG
  configuration.
- Stale Tasks 1-4 mount fixtures and Clippy failures found by the full Task 5
  baseline were corrected.
- The connected-gate test fixture no longer launches nested Cargo commands
  against the shared `scripts/target` directory.

## TDD Evidence

Initial RED:

```text
rtk cargo test -p gascan optional_include_offer_failure_preserves_successful_up_result -- --nocapture
test result: FAILED. 0 passed; 1 failed
```

The test failed because its `0755` directory is intentionally accepted.

The release/document source contract was extended before the production
script and documentation:

```text
rtk bash tests/release/smoke-contract.sh
release smoke omits writable runtime-home proof: /home/workspace/.local
```

Focused GREEN after implementation:

```text
rtk bash tests/release/smoke-contract.sh
PASS: Gas Can release smoke command contract

rtk cargo test -p gascan optional_include_offer -- --nocapture
cargo test: 2 passed, 89 filtered out
```

The connected-gate dispatcher received focused coverage for:

- exact Cargo `run --quiet --locked --offline --manifest-path ... --bin ... --`
  prefix validation;
- rejection with exit 64 of unknown commands, flags, manifests, and binaries;
- direct execution of only the four compile-time `CARGO_BIN_EXE_validate-*`
  paths;
- preservation of validator stdin, stdout, stderr, arguments, and exit status;
- both the real scripts manifest and the canonical fixture manifest;
- the real receipt validator operating through the dispatcher.

Focused dispatcher results:

```text
rtk cargo test --manifest-path scripts/Cargo.toml --test connected_image_gate fixture_cargo_dispatcher -- --nocapture
cargo test: 2 passed, 29 filtered out

rtk cargo test --manifest-path scripts/Cargo.toml --test connected_image_gate
cargo test: 31 passed (1 suite, 49.37s)
```

## Baseline Failures Found and Fixed

The first Clippy run found five Tasks 1-4 issues:

- a platform-dependent same-type cast in SSH file identity;
- a production `expect` after storage-layout validation;
- two `expect` uses in the storage-layout API test, reported through the
  crate's denied `expect_used` lint.

The fixes retain the native `rustix::fs::RawMode`, return the stable
`StorageLayoutRequiresRecreate` error if the invariant is absent, and use
non-panicking test decoding.

The first workspace run found:

- one stale e2e expectation for the old tools/config leaf mount targets;
- one PTY test blocked by the execution sandbox with `EPERM`.

The e2e expectation and the same stale targets in the Apple live storage
fixture were updated to `/home/workspace/.local`,
`/home/workspace/.cache`, and `/home/workspace/.config`. The isolated PTY test
passed immediately with the host process permissions it requires, proving the
failure environmental rather than behavioral.

## Connected-Gate Contention Diagnosis and Fix

The pre-existing Task 4 scripts run had remained active for approximately
48 minutes:

```text
44232 rtk cargo test --manifest-path scripts/Cargo.toml
44254 .../cargo test --manifest-path scripts/Cargo.toml
54071 .../scripts/target/debug/deps/connected_image_gate-...
```

The exact collision was `scripts/tests/connected_image_gate.rs::fixture()`
setting every parallel fixture's `CARGO_TARGET_DIR` to the single real
`scripts/target`. Every gate and copied smoke script then spawned repeated
`cargo run ... --bin validate-*` commands against that shared target. Fixture
container/volume state was already temp-local; the collision was the Cargo
target lock, amplified by overlapping full-suite invocations.

A bounded reproduction made no output for more than 60 seconds and was
stopped with Ctrl-C. After re-verifying the exact command lines, the approved
stale Task 4 process group 44232 and its descendants were terminated with
TERM. A targeted `ps -p 44232,44254,54071` then returned no entries.

The fixture now places a strict temp-local `cargo` dispatcher first on PATH.
It accepts only the production validator invocation shape and directly
executes the already-built validators. It does not mutate or lock a Cargo
target. The legitimate copied receipt path required preserving its symlink
pathname while canonicalizing only the fixture parent, because macOS exposes
the temporary root as `/var/...` to Rust and `/private/var/...` to `pwd -P`.

The corrected complete scripts suite finished normally:

```text
rtk cargo test --manifest-path scripts/Cargo.toml
cargo test: 419 passed (47 suites, 99.18s)
```

No bounded or stale verification child process remained afterward.

## Complete Verification

```text
rtk cargo fmt --all -- --check
PASS

rtk cargo fmt --all --manifest-path scripts/Cargo.toml -- --check
PASS

rtk cargo clippy --workspace --all-targets -- -D warnings
cargo clippy: No issues found

rtk cargo clippy --manifest-path scripts/Cargo.toml --all-targets -- -D warnings
cargo clippy: No issues found

rtk cargo test --workspace
cargo test: 887 passed, 20 ignored, 63 filtered out (60 suites, 43.17s)

rtk cargo test --manifest-path scripts/Cargo.toml
cargo test: 419 passed (47 suites, 99.18s)

rtk swift test --package-path helpers/apple-attach
11 tests passed, 0 failures

rtk bash -c 'set -e; for contract in tests/release/*-contract.sh; do bash "$contract"; done'
all 11 release contracts PASS

rtk git diff --check
PASS
```

The plan's Swift path `helpers/attach` does not exist in this checkout. The
tracked package is `helpers/apple-attach`, which is the path verified above.
Its first sandboxed attempt could not write SwiftPM/Clang user caches; the
host-permission rerun passed.

The plan's release-contract loop does not set `-e`, so a sandboxed Homebrew
cache failure in the middle was initially masked by later successful
contracts. `release-script-contract.sh` passed individually with the required
host cache permissions, and the final all-contract run added `set -e` so every
contract's status was authoritative.

## Concerns and Deferred Evidence

- The new installed-release smoke is covered by source/cleanup contracts here,
  but it has not been run against a published 0.1.10 package. That is Task 8
  release evidence.
- No connected image was built or approved in this task. That remains Task 6.
- SwiftPM populated the ignored helper build directory and dependency cache;
  neither is staged.
- The configured signing agent failed with `communication with agent failed`
  when creating the local Task 5 commit. As already documented for Task 4,
  the task commit was therefore created with `--no-gpg-sign`; release tags
  remain subject to their separate signed provenance requirement.
