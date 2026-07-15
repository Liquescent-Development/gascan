# Offline Workspace Image Bundles — Task 3 Report

## Outcome

Implemented the connected Linux ARM64 producer and fail-closed validator for the exact mise runtime bundle. No bundle was produced or published locally, no release was changed, and `versions.lock` remains in its approved pending state.

## TDD evidence

- Added `scripts/tests/mise_runtime_bundle.rs` before the producer existed.
- Confirmed the required RED state with `cargo test --manifest-path scripts/Cargo.toml --test mise_runtime_bundle`; the test target failed because `produce-mise-runtime-bundle.sh` and the workflow contract were absent.
- Implemented the verifier/producer and iterated to GREEN. Follow-up security review drove additional RED tests; the final focused suite has 19 passing tests.

The tests reject wrong platform, mise version/digest, config digest, missing/extra/wrong-version tools, missing or tiny fake runtime entrypoints, writable or non-root canonical tree evidence, unsafe archive entries, unsorted manifests, missing base attestation, tampered retained downloads, and provenance not bound to mise's lock. They also enforce the pinned connected ARM64 and privilege-separated workflow shape.

## Implementation

- `scripts/produce-mise-runtime-bundle.sh`
  - refuses production outside connected Linux ARM64, without root, or without the producer-independent attestation mounted as its own read-only filesystem;
  - verifies the exact approved lock/config/base inputs;
  - installs with mise 2026.5.0 and literal `MISE_DATA_DIR=/opt/gascan/mise`;
  - enables mise's retained-download mode, captures actual downloaded bytes, and requires each backend-specific lock URL/checksum to appear in the backend trace and uniquely match retained bytes;
  - emits canonical URL/SHA-256/size/backend/tool/path evidence, sanitized trace evidence, and the captured artifacts for independent re-hashing;
  - captures exact `mise current --json` and the generated `mise.lock`;
  - removes caches/downloads, normalizes root ownership, explicit file/directory modes, and timestamps;
  - emits a canonical manifest and deterministic tar+zstd archive;
  - independently derives provenance from the emitted `mise.lock`, requires its canonical rows to match trace and captured-byte evidence, and validates archive safety, exact tree metadata/content, AArch64 ELF or narrowly reviewed shebang entrypoint formats, sidecar hash/size, and the exact seven-key tool map;
  - on Linux ARM64, safely extracts to a temporary directory and executes every runtime's version command against exact expected output semantics.
- `.github/workflows/workspace-bundles.yml`
  - pulls and inspects the exact SHA-pinned Ubuntu ARM64 base, then creates a workflow-owned receipt containing workflow commit, image digest, image ID, platform, and invocation type;
  - mounts that receipt read-only and separately from producer output, runs two independent builds, and copies the receipt into evidence only after each producer exits;
  - compares archive bytes and every evidence file before upload;
  - revalidates in a separate unprivileged job;
  - rechecks receipt/workflow binding in the unprivileged validation job and uploads only short-lived CI artifacts. It does not publish release assets or change lock publication state;
  - performs no `apt-get install` in either Task 3 job; producer and runtime build dependencies are installed with `dpkg` solely from the independently validated Task 2 archive, whose digest is bound into the attestation.

The reviewed system-tool root list now includes `curl`, `python3`, and `zstd`, which are required by the connected producer. Its trusted SHA-256 and the Task 2 producer/test fixture were updated together, so the signed snapshot closure supplies exact package versions rather than mutable runner APT state.

## Verification

Fresh verification completed successfully:

```text
cargo test --manifest-path scripts/Cargo.toml --all-targets
cargo clippy --manifest-path scripts/Cargo.toml --all-targets -- -D warnings
cargo fmt --manifest-path scripts/Cargo.toml -- --check
find scripts -type f -name '*.sh' -exec bash -n {} +
git diff --check
```

All commands exited zero. The final full Rust suite includes 19/19 Task 3 tests passing.

## Remaining operational concern

The actual producer is intentionally connected-CI-only and was not run from this macOS workspace. A real ARM64 run must prove that the independently validated Task 2 closure configures cleanly with offline `dpkg`, that mise 2026.5.0 retains lock-matching artifacts for all seven backends, and that two complete installations are byte-identical. Every condition fails closed. No Task 3 publication occurs on failure, and `versions.lock` remains pending.
