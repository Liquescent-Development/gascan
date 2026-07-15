# Offline Workspace Image Bundles — Task 3 Report

## Outcome

Implemented the connected Linux ARM64 producer and fail-closed validator for the exact mise runtime bundle. No bundle was produced or published locally, no release was changed, and `versions.lock` remains in its approved pending state.

## TDD evidence

- Added `scripts/tests/mise_runtime_bundle.rs` before the producer existed.
- Confirmed the required RED state with `cargo test --manifest-path scripts/Cargo.toml --test mise_runtime_bundle`; the test target failed because `produce-mise-runtime-bundle.sh` and the workflow contract were absent.
- Implemented the verifier/producer and iterated to GREEN. The final focused suite has 16 passing tests.

The tests reject wrong platform, mise version/digest, config digest, missing/extra/wrong-version tools, missing real runtime entrypoints, writable or non-root canonical tree evidence, unsafe archive entries, and unsorted manifests. They also enforce the pinned connected ARM64 and privilege-separated workflow shape.

## Implementation

- `scripts/produce-mise-runtime-bundle.sh`
  - refuses production outside connected Linux ARM64 or without root;
  - verifies the exact approved lock/config/base inputs;
  - installs with mise 2026.5.0 and literal `MISE_DATA_DIR=/opt/gascan/mise`;
  - creates mise's own lockfile and derives per-tool upstream URL, SHA-256, and backend provenance from that lockfile (failing if any locked runtime lacks it);
  - captures exact `mise current --json` and the generated `mise.lock`;
  - removes caches/downloads, normalizes root ownership, explicit file/directory modes, and timestamps;
  - emits a canonical manifest and deterministic tar+zstd archive;
  - independently derives provenance from the emitted `mise.lock`, requires its canonical rows to match `upstream-artifacts.tsv` byte-for-byte, and validates archive safety, exact tree metadata/content, real runtime entrypoints, sidecar hash/size, and the exact seven-key tool map.
- `.github/workflows/workspace-bundles.yml`
  - runs two independent builds inside the exact SHA-pinned Ubuntu ARM64 base;
  - compares archive bytes and every evidence file before upload;
  - revalidates in a separate unprivileged job;
  - uploads only short-lived CI artifacts. It does not publish release assets or change lock publication state.

## Verification

Fresh verification completed successfully:

```text
cargo test --manifest-path scripts/Cargo.toml --all-targets
cargo clippy --manifest-path scripts/Cargo.toml --all-targets -- -D warnings
cargo fmt --manifest-path scripts/Cargo.toml -- --check
find scripts -type f -name '*.sh' -exec bash -n {} +
git diff --check
```

All commands exited zero. The final full Rust suite includes 16/16 Task 3 tests passing.

## Remaining operational concern

The actual producer is intentionally connected-CI-only and was not run from this macOS workspace. The first real ARM64 workflow run is therefore still required to prove that mise 2026.5.0 emits complete URL/checksum/backend records for all seven selected backends and that two full runtime installations are byte-identical. The workflow fails closed and does not publish if either condition is false.
