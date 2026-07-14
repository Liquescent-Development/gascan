# Plan 2 Task 1 Implementation Report

## Status

Implemented and verified Task 1 only: strict `gascan.toml` parsing, validated resource values, canonical sandbox roots, stable sandbox identity, and the single writable `/workspace` bind-mount invariant.

## Implementation

- Added `Manifest::parse` and `Manifest::load` for `<root>/gascan.toml`.
- Made unknown top-level and `[resources]` keys fatal with Serde `deny_unknown_fields`.
- Enforced manifest version 1, fail-closed offline networking default, workspace-user default, bundled Gascamp default, and ordered tools/ports maps.
- Added validated binary resource sizes (`KiB`, `MiB`, `GiB`, `TiB`) with checked arithmetic and positive CPU/resource values.
- Rejected absolute or parent-traversing setup paths.
- Represented bundled versus `/workspace/gascamp`-subtree Gascamp sources.
- Added typed I/O, non-directory, non-UTF8 canonical path, parse, version, and validation errors.
- Added `SandboxId` slugging plus the first 12 lowercase hexadecimal SHA-256 characters of the canonical root.
- Added `SandboxSpec::from_root`, which canonicalizes with `std::fs::canonicalize`, requires a directory, rejects non-UTF8 canonical paths, and creates exactly one writable bind mount from that canonical root to `/workspace`.
- Preserved the existing Apple implementation; no `gascan-apple` files or imports changed.

## TDD Evidence

### RED

Command:

```text
cargo test -p gascan-core --test manifest --test sandbox_identity
```

Expected failure observed before production implementation (exit 101):

```text
error[E0432]: unresolved import `gascan_core::manifest`
error[E0432]: unresolved import `gascan_core::sandbox`
error[E0433]: cannot find `manifest` in `gascan_core`
```

The failure was specifically caused by the missing Task 1 modules/types.

### GREEN

Required focused command:

```text
cargo test -p gascan-core --test manifest --test sandbox_identity && cargo clippy -p gascan-core --all-targets -- -D warnings
```

Result: exit 0; 5 manifest tests passed, 5 sandbox identity tests passed, 0 failed; all-target clippy completed with warnings denied.

Fresh completion verification:

```text
cargo test -p gascan-core && cargo fmt --all -- --check && git diff --check
```

Result: exit 0; 11 total integration tests passed (including the pre-existing runtime-capability test), 0 failed; doc tests passed; formatting and whitespace checks passed.

One intermediate GREEN run exposed a macOS fixture expectation (`/var` versus canonical `/private/var`). The production result was correct; the test was corrected to compare with `std::fs::canonicalize`, after which the required command passed.

## Files

- Modified `Cargo.toml` (shared-manifest dependency coordination)
- Modified `crates/gascan-core/Cargo.toml`
- Modified `crates/gascan-core/src/lib.rs`
- Added `crates/gascan-core/src/manifest.rs`
- Added `crates/gascan-core/src/sandbox.rs`
- Added `crates/gascan-core/tests/manifest.rs`
- Added `crates/gascan-core/tests/sandbox_identity.rs`

## Shared-Manifest Dependency Coordination

- Workspace production dependency: `camino = 1` with `serde1`, for UTF-8 path types.
- Workspace production dependency: `sha2 = 0.10`, for stable canonical-root identity digests.
- Workspace production dependency: `toml = 0.8`, for strict manifest decoding.
- `gascan-core` test-only dependency: `tempfile = 3`, for canonicalization, symlink, load, and directory fixtures.
- `Cargo.lock` is globally ignored in this checkout and therefore has no tracked change.

## Self-Review

- Scope: only authorized Plan 2 Task 1 source/tests plus minimal root/core manifest dependencies changed.
- Mount safety: `SandboxSpec` constructs the bind-mount vector internally with exactly one entry; its source is the post-canonicalization code root and its target is the fixed `/workspace` path.
- Fail-closed parsing: unknown keys, unsupported versions, invalid units, zero resources, setup traversal, and out-of-scope Gascamp paths return errors.
- Stable identity: digest input is the full canonical UTF-8 root, while the human name affects only the slug prefix.
- Production constraints: no unsafe blocks, unwraps, expects, or panics were introduced; strict clippy passed.
- Compatibility: no changes were made to frozen runtime capability types or Apple backend code.

## Concerns

- `Cargo.lock` is globally ignored, so dependency resolution is reproducible only to the semver constraints in the manifests unless repository policy changes later.
- Race-resistant setup execution belongs to the later setup-policy task; Task 1 validates lexical containment and symlink resolution visible at load time.

## Review Fix Follow-Up

### Findings Addressed

- Made all `BindMount` storage private and exposed immutable `source`, `target`, and `is_writable` accessors.
- Made all `SandboxSpec` storage private and exposed immutable `id`, `canonical_root`, `manifest`, and bind-mount slice accessors. External callers cannot construct, replace, append, or mutate bind mounts.
- Made `Manifest::load` validate an existing setup path or its nearest existing ancestor after resolving symlinks, rejecting resolution outside the already-canonical workspace root with typed `ManifestError::SetupOutsideRoot`.
- Preserved `Manifest::parse` lexical validation because it has no filesystem root.
- Preserved valid relative setup paths whose final components do not exist yet beneath the root.

### Review-Fix TDD Evidence

Initial focused RED command:

```text
cargo test -p gascan-core --test manifest --test sandbox_identity
```

Result: exit 101 with five `E0599` errors because the required read-only `SandboxSpec`/`BindMount` accessors did not yet exist.

Independent behavioral RED command:

```text
cargo test -p gascan-core --test manifest load_rejects_setup_symlink_that_escapes_the_canonical_root
```

Result: exit 101; the regression failed because `Manifest::load` returned `Ok(Manifest { setup: Some("./escape/setup.sh"), ... })` for an in-root symlink resolving to an outside temporary directory.

Review-fix GREEN command:

```text
cargo fmt --all && cargo test -p gascan-core --test manifest --test sandbox_identity && cargo clippy -p gascan-core --all-targets -- -D warnings
```

Result: exit 0; 7 manifest tests and 5 sandbox identity tests passed, 0 failed; strict all-target clippy passed.

### Review-Fix Self-Review

- The mount invariant is enforced structurally: only `SandboxSpec::from_root` creates the private mount vector, and only immutable access is public.
- The setup containment walk never relies on the untrusted lexical path alone: it resolves the first existing candidate encountered while walking toward the canonical root and compares the resolved path to that root.
- Existing broken symlinks return typed I/O errors rather than being treated as future safe paths.
- No unsafe code, production unwraps, expects, or panics were introduced.
- No Apple backend or Task 2 paths changed.

### Updated Concern

- Full race-resistant setup execution remains the later setup-policy task; Task 1 now rejects setup symlink escapes visible at manifest-load time.

### Review-Fix Completion Verification and Commit

```text
cargo test -p gascan-core && cargo fmt --all -- --check && git diff --check
```

Result: exit 0; 13 core integration tests passed (7 manifest, 5 sandbox identity, 1 pre-existing runtime capability), 0 failed; doc tests, formatting, and diff checks passed.

Review-fix commit: `694dc7d fix: enforce sandbox manifest invariants`.
