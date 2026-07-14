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

## Controller Fix Cycle 2

### Findings Addressed

- Made every `Manifest` and `Resources` field private and added immutable accessors for version, name, network, user, Gascamp source, setup, resources, tools, and ports.
- Replaced the publicly constructible `GascampSource::Workspace(Utf8PathBuf)` enum variant with a public opaque `GascampSource` whose representation and constructors are private. Callers have ergonomic `is_bundled` and `workspace_path` read access.
- Removed public rootless `Manifest::parse` and public `Default` construction. `Manifest::load` is now the public construction boundary, including the ergonomic no-file defaults case.
- Recorded the canonical root privately in every loaded/default manifest. `SandboxSpec::from_root` verifies that provenance and reruns root-dependent setup containment immediately before constructing the spec.
- Replaced derived `SandboxId` deserialization with checked deserialization through typed `SandboxIdError` validation. Accepted IDs require a nonempty normalized slug, one separator, and exactly 12 lowercase hexadecimal digest characters.
- Added explicit regressions for unknown `[resources]` fields, zero CPUs, zero resource sizes, overflowing resource sizes, a malformed Gascamp sibling path, cross-root manifest reuse, and malformed serialized sandbox IDs.

### RED Evidence

Focused command after test-only API/regression changes:

```text
cargo test -p gascan-core --test manifest --test sandbox_identity
```

Result: exit 101 with 19 `E0599` errors because immutable `Manifest`, `Resources`, and `GascampSource` accessors did not exist.

Independent behavioral RED command:

```text
cargo test -p gascan-core --test sandbox_identity
```

Result: exit 101; 5 passed and 2 failed. `sandbox_id_deserialization_rejects_unchecked_strings` reported `accepted code`, and `manifest_loaded_for_another_root_is_rejected` received `Ok(SandboxSpec { ... })` for a manifest loaded from root A and consumed at root B.

### GREEN Evidence

Focused command:

```text
cargo fmt --all && cargo test -p gascan-core --test manifest --test sandbox_identity
```

Result: exit 0; 8 manifest tests and 7 sandbox identity tests passed, 0 failed.

Full verification command:

```text
cargo clippy -p gascan-core --all-targets -- -D warnings && cargo test -p gascan-core && cargo fmt --all -- --check && git diff --check
```

Result: exit 0; strict all-target clippy passed; 16 core integration tests passed (8 manifest, 7 sandbox identity, 1 pre-existing runtime capability), 0 failed; doc tests, formatting, and diff checks passed.

### Controller-Fix Self-Review

- Construction boundary: external code cannot construct or mutate `Manifest`, `Resources`, `ResourceSize`, or `GascampSource` policy state. The only public manifest constructor is root-aware `Manifest::load`.
- Provenance: no-file defaults also carry canonical-root provenance; cross-root consumption fails before sandbox identity or mounts are constructed.
- Revalidation: setup containment is checked both during load and during `SandboxSpec::from_root`, catching a changed symlink at spec construction time.
- ID validation: tuple storage remains private; generated IDs round-trip through Serde, while uppercase, missing/short/invalid digests, empty slugs, and noncanonical separators fail.
- Production constraints: no unsafe blocks, unwraps, expects, or panics were introduced.
- Scope: only Task 1 core source/tests changed; no Apple or Task 2 paths changed.

### Remaining Concern

- As before, execution-time filesystem race resistance is owned by the later setup execution policy; Task 1 validates at manifest load and spec construction boundaries.

## Controller Fix Cycle 3

### Finding Addressed

- Removed `Deserialize` from the public validated `Resources` type, sealing the standalone Serde construction path that could create an invalid zero-CPU policy.
- Added a private `RawResources` deserialization schema with `deny_unknown_fields`; `RawManifest::validate` now checks CPU policy and converts the private raw values into `Resources` only inside the manifest validation boundary.
- Preserved `ResourceSize`'s checked positive-unit parsing, resource-size overflow rejection, unknown resource-key rejection, and existing validation error text.

### RED Evidence

Command after adding the focused public-API regression and before changing production behavior:

```text
cargo test -p gascan-core --doc
```

Result: exit 101. The `compile_fail` regression failed with `Test compiled successfully, but it's marked compile_fail.` because `toml::from_str::<Resources>("cpus = 0")` was still accepted by the type system.

### GREEN Evidence

Focused command:

```text
cargo fmt --all && cargo test -p gascan-core --test manifest --test sandbox_identity && cargo test -p gascan-core --doc
```

Result: exit 0; 8 manifest tests, 7 sandbox identity tests, and the focused `Resources` compile-fail doctest passed, 0 failed.

Fresh full verification command:

```text
cargo test -p gascan-core && cargo clippy -p gascan-core --all-targets -- -D warnings && cargo fmt --all -- --check && git diff --check
```

Result: exit 0; 16 core integration tests and 1 compile-fail doctest passed, 0 failed; strict all-target clippy, formatting, and diff checks passed.

### Controller-Fix Self-Review

- Construction boundary: `Resources` no longer implements `Deserialize`; only private `RawResources` accepts manifest input, and conversion occurs after the zero-CPU check.
- Strictness: `RawResources` retains `deny_unknown_fields`, while `ResourceSize` continues rejecting zero sizes, invalid units, and checked-arithmetic overflow with the existing messages.
- API regression: the compile-fail doctest directly proves external code cannot deserialize standalone `Resources`; it failed against the prior implementation and passed after the fix.
- Production constraints: no unsafe code, unwraps, expects, or panics were introduced.
- Scope: only Task 1 manifest source and this report changed; no Task 2 or Apple paths changed.

### Remaining Concern

- `NetworkMode` and `UserMode` remain independently deserializable value enums, but they contain no invalid representable policy state; validated aggregate construction remains sealed behind `Manifest::load`.

## Controller Fix Cycle 4

### Finding Addressed

- Removed the public `Default` implementation from `Resources`, closing the remaining external `Resources::default()` construction path.
- Replaced no-file manifest default construction with private `Resources::empty()`, preserving the empty resource policy exclusively inside the manifest construction boundary.
- Added a second compile-fail public-API regression alongside the standalone-deserialization regression.

### RED Evidence

Command after adding the `Resources::default()` compile-fail regression and before removing the trait:

```text
cargo test -p gascan-core --doc
```

Result: exit 101; the existing standalone-deserialization compile-fail test passed, while the new default-construction regression failed with `Test compiled successfully, but it's marked compile_fail.`

### GREEN Evidence

Focused command:

```text
cargo fmt --all && cargo test -p gascan-core --test manifest --test sandbox_identity && cargo test -p gascan-core --doc
```

Result: exit 0; 8 manifest tests, 7 sandbox identity tests, and both `Resources` compile-fail doctests passed, 0 failed. The no-file/default-policy behavior remained covered by the manifest suite.

Fresh full verification command:

```text
cargo test -p gascan-core && cargo clippy -p gascan-core --all-targets -- -D warnings && cargo fmt --all -- --check && git diff --check
```

Result: exit 0; 16 core integration tests and 2 compile-fail doctests passed, 0 failed; strict all-target clippy, formatting, and diff checks passed.

### Controller-Fix Self-Review

- Construction boundary: public `Resources` implements neither `Deserialize` nor `Default`; its fields remain private and no public constructor exists.
- Defaults: `Manifest::load` still produces the documented empty resource policy when `gascan.toml` is absent, using the private `Resources::empty()` helper.
- Validation: private `RawResources` retains `deny_unknown_fields`; CPU, resource-size, unit, and overflow validation paths are unchanged.
- Production constraints: no unsafe code, unwraps, expects, or panics were introduced.
- Scope: only Task 1 manifest source and this report changed; no Task 2 or Apple paths changed.
