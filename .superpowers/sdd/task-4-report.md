# Plan 4 Task 4 Report

Status: implemented with structural/unit verification; live image build and runtime smoke unavailable in this worktree.

## RED

Tests were added before production code:

- `crates/gascan-core/tests/gascamp_source.rs`
- `tests/image/gascamp-smoke.sh`

Initial online command:

```text
$ cargo test -p gascan-core --test gascamp_source
error: failed to download from https://index.crates.io/config.json
Caused by: Could not resolve host: index.crates.io
```

The test was rerun offline to distinguish environment failure from the intended RED:

```text
$ cargo test --offline -p gascan-core --test gascamp_source
error[E0432]: unresolved import `gascan_core::gascamp`
  --> crates/gascan-core/tests/gascamp_source.rs:2:18
   |
2  | use gascan_core::gascamp::{...};
   |                  ^^^^^^^ could not find `gascamp` in `gascan_core`
```

Image contract RED:

```text
$ ./tests/image/gascamp-smoke.sh
exit 1 (the required executable selector was absent)
```

## GREEN

Focused source test and image contract:

```text
$ cargo test --offline -p gascan-core --test gascamp_source
running 3 tests
test bundled_source_reports_the_locked_revision_as_trusted ... ok
test local_gascamp_must_resolve_beneath_workspace ... ok
test workspace_override_reports_its_canonical_container_path_as_untrusted ... ok
test result: ok. 3 passed; 0 failed

$ ./tests/image/gascamp-smoke.sh
SKIP live Gascamp image smoke: missing image reference .../.artifacts/workspace-image-ref
```

Relevant regression verification:

```text
$ cargo clippy --offline -p gascan-core --test gascamp_source -- -D warnings
Finished `dev` profile ...

$ cargo test --offline -p gascan-core
gascan-core integration and doc tests: all passed

$ cargo test --offline --manifest-path scripts/Cargo.toml --test image_lock --test polyglot_image_contract
image_lock: 3 passed; polyglot_image_contract: 3 passed

$ bash -n images/workspace/bin/select-gascamp tests/image/gascamp-smoke.sh
$ cargo fmt --all -- --check
$ git diff --check
all exited 0
```

## Implementation

- Added the public `gascamp` source model and resolver with the exact locked revision, lexical normalization, exact `/workspace/gascamp` subtree enforcement, and trusted/untrusted reporting.
- Added a multi-stage Gascamp build from revision `f6b248c5926240856dbea83d1d2c5c90ea1c1456`; the Dockerfile verifies the fetched commit, runs locked tests, builds and strips `camp`, creates the `campd` symlink, records `REVISION`, and makes the output read-only.
- Added `select-gascamp`, which defaults to the bundled executable and emits JSON metadata. Workspace overrides are lexically and physically canonicalized, their executable is required and separately canonicalized, directory and executable symlink escapes are rejected, and workspace metadata is untrusted.
- Added structural and optional live image smoke coverage for `camp --version`, `campd --version` argv0 dispatch, bundled selection, local selection, executable/directory symlink escape, and disallowed paths.

## Files changed

- `crates/gascan-core/src/lib.rs`
- `crates/gascan-core/src/gascamp.rs`
- `crates/gascan-core/tests/gascamp_source.rs`
- `images/workspace/Dockerfile`
- `images/workspace/bin/select-gascamp`
- `tests/image/gascamp-smoke.sh`
- `.superpowers/sdd/task-4-report.md`

## Self-review

- Confirmed no Task 5 or Task 6 provisioning planner/CLI work was introduced.
- Confirmed production Rust contains no unsafe, unwrap, expect, or panic.
- Confirmed traversal and prefix-collision paths cannot escape the allowed subtree.
- Independent review found no critical issues. Its two important findings were fixed: the globally ignored selector is force-staged, and `bin/camp` itself is canonicalized and confined.
- The Docker build uses the already locked base/snapshot/mise inputs and verifies the exact Gascamp commit before checkout.

## Concerns

- A genuine image build/runtime smoke was not possible: this isolated worktree has no `.artifacts` directory or `.artifacts/workspace-image-ref`, and the container runtime probe is sandbox-restricted. The smoke reports this explicitly and does not claim live evidence.
- Consequently, the remote Gascamp repository availability, its locked revision build against Rust 1.97.0, and actual `camp`/`campd` runtime behavior remain to be proven by the image build gate.

## Commit

Commit SHA: `bc6deaa` (the final amended commit SHA is included in the handoff response).

## Follow-up: fail-closed image gate

The review finding was reproduced with a subprocess contract test before changing the smoke script:

```text
$ cargo test --offline --manifest-path scripts/Cargo.toml --test image_user_contract gascamp_smoke_fails_closed_without_a_built_image_reference -- --exact
running 1 test
test gascamp_smoke_fails_closed_without_a_built_image_reference ... FAILED
assertion failed: !output.status.success()
test result: FAILED. 0 passed; 1 failed
```

The missing-reference branch now emits a clear error on stderr and exits nonzero. Fresh verification:

```text
$ cargo test --offline --manifest-path scripts/Cargo.toml --test image_user_contract
running 4 tests
test gascamp_smoke_fails_closed_without_a_built_image_reference ... ok
test result: ok. 4 passed; 0 failed

$ cargo test --offline -p gascan-core --test gascamp_source
running 3 tests
test result: ok. 3 passed; 0 failed

$ GASCAN_IMAGE_REF_FILE=.artifacts/definitely-missing ./tests/image/gascamp-smoke.sh
missing Gascamp image reference: .artifacts/definitely-missing
exit 1 (expected)

$ bash -n tests/image/gascamp-smoke.sh images/workspace/bin/select-gascamp
$ git diff --check
both exited 0
```

Live image acceptance remains unclaimed; the required image reference is still unavailable.
