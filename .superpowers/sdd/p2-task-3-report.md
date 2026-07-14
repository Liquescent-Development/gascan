# Plan 2 Task 3: Policy Compiler Report

## Scope

Implemented only core-control-plane Task 3. No SQLite, daemon, Apple adapter, or Task 4 work was started.

## TDD evidence

RED command:

```text
cargo test -p gascan-core --test policy
```

RED result: exit 101 with `E0432`; `gascan_core::policy` was intentionally unresolved before production implementation.

GREEN command:

```text
cargo test -p gascan-core --test policy
```

GREEN result: exit 0; 10 passed, 0 failed.

Final verification commands:

```text
cargo test -p gascan-core --all-targets
cargo test -p gascan-core --doc
cargo clippy -p gascan-core --all-targets -- -D warnings
cargo fmt --all -- --check
git diff --check
```

Final result before commit: every command exited 0. The core suite passed 34 integration tests plus 2 compile-fail doctests; strict Clippy, formatting, and diff checks were clean.

## Policy constants

- CPU: default 4, maximum 16.
- Memory: default 8 GiB, maximum 64 GiB.
- Disk: default 64 GiB, maximum 512 GiB.
- Processes: fixed ceiling 1,024.

These defaults leave meaningful host capacity for a broad polyglot workspace without granting unbounded availability. The maxima permit larger builds while remaining deliberate host-protection ceilings. Process count is fixed because the manifest does not expose a process field; 1,024 accommodates compilers and test runners without silently making the ceiling user-controlled.

The compiler uses one pinned digest-form workspace image reference because its required signature has no image-resolution input. A later image-resolution integration may replace this named policy constant with a separately resolved immutable digest while retaining the compiler's digest-only invariant.

## Files

- `crates/gascan-core/src/policy.rs`: fail-closed capability validation, policy compilation, resource bounds, fixed environment allowlist, canonical mounts, owned volume layout, loopback ports, immutable network/user/image/init/ownership, and stable policy errors.
- `crates/gascan-core/tests/policy.rs`: focused capability, mount, offline, port, environment, resources, ownership, user, image, init, and approved JSON-shape coverage using real root-aware Task 1 construction.
- `crates/gascan-core/src/runtime.rs`: minimal backend-neutral `CreateRequest::init` requirement and fixture update.
- `crates/gascan-core/src/lib.rs`: exports the policy module.

## Self-review

- All mandatory capabilities are checked before request construction. Offline accepts only `Proven`; unsupported and unverified both fail closed.
- The sealed `SandboxSpec` is revalidated as exactly one writable canonical-root mount at `/workspace`.
- Ports reject zero, duplicate guest/host values, offline publication, and unavailable loopback enforcement. Every emitted binding is IPv4 loopback with equal host and guest ports.
- Every request receives CPU, memory, disk, and process ceilings. Explicit CPU, memory, and disk values cannot exceed named maxima.
- Named volumes have deterministic `gascan-` names, workspace-only guest targets, writable flags, and explicit ownership matching the container.
- Host environment filtering admits only `TERM`, `COLORTERM`, `LANG`, and nonempty `LC_*`; secrets, host paths, and sockets are excluded.
- `CreateRequest` remains incapable of expressing devices, privileged mode, arbitrary capabilities, raw backend options, or arbitrary host mounts. The approved serialized shape test checks this boundary.
- Production changes contain no unsafe code, unwrap, expect, or panic.

## Concerns

No Task 3 blocker remains. The pinned image constant is intentionally isolated and immutable, but its release digest must eventually be supplied by the image build/resolution track before a real runtime lifecycle is released.

## Critical review follow-up: sealed create boundary

Review found that `CreateRequest` still had public fields and `Deserialize`, allowing external crates to bypass `PolicyCompiler` with a struct literal or serialized input. This was a critical policy-boundary defect.

Follow-up RED command:

```text
cargo test -p gascan-core --doc
```

RED result: exit 101. Both new `compile_fail` examples compiled successfully, demonstrating that an external caller could replace the image through struct update syntax and deserialize an unchecked request.

Follow-up GREEN commands:

```text
cargo test -p gascan-core --test policy --test backend_contract
cargo test -p gascan-core --doc
```

GREEN result: exit 0. The 10 policy tests and 8 backend-contract tests passed; all 4 doctests passed, including the 2 new public API compile-fail regressions.

Follow-up implementation and review evidence:

- All `CreateRequest` fields are now crate-private and `Deserialize` was removed while `Serialize`, `Clone`, `Eq`, and `Debug` remain.
- Immutable getters cover ID, image, mounts, volumes, ports, environment, resources, network, user, init, and ownership. No mutable accessor or builder was added.
- `CreateRequest::fixture` remains the only public constructor and returns one fixed known-valid shape without inputs that can alter policy fields.
- The positive policy tests exercise every getter and serialize the approved JSON request shape.
- Public nested request-shape types remain constructible for adapter inspection, but cannot be inserted into or used to mutate the sealed `CreateRequest`; this invariant is documented on the request type.
- A source scan found only the `CreateRequest` declaration and its single inherent implementation; there is no `From`, `TryFrom`, alternate public constructor, or public function returning an unchecked request.

Final follow-up verification:

```text
cargo fmt --all
cargo test -p gascan-core --test policy --test backend_contract
cargo test -p gascan-core --doc
cargo test -p gascan-core --all-targets
cargo clippy -p gascan-core --all-targets -- -D warnings
cargo fmt --all -- --check
git diff --check
```

Every command exited 0. The full core suite passed 34 integration tests, all 4 doctests passed, and strict Clippy, formatting, and diff checks were clean. No Task 4 or Apple work was performed.

## Critical review follow-up: fixture constructor removal

Review confirmed that the unconditional public `CreateRequest::fixture` remained a production policy bypass. Its hard-coded request combined an arbitrary `/tmp/code` bind mount, offline networking with a published port, and empty resource limits without calling `PolicyCompiler`.

TDD RED command:

```text
cargo test -p gascan-core --doc
```

RED result: exit 101. The new standalone `CreateRequest::fixture` compile-fail example compiled successfully while the other 4 doctests passed, specifically proving the public fixture constructor was still exposed before its removal.

Focused GREEN commands:

```text
cargo test -p gascan-core --test backend_contract
cargo test -p gascan-core --doc
cargo test -p gascan-core --test policy --test backend_contract
cargo test -p gascan-core --doc
```

GREEN result: every command exited 0. Backend contract passed 8 tests, policy passed 10 tests, and the expanded doctest suite passed all 8 cases.

Final verification commands:

```text
cargo test -p gascan-core --all-targets
cargo clippy -p gascan-core --all-targets -- -D warnings
cargo fmt --all -- --check
git diff --check
```

Final result before commit: every command exited 0. The full core suite passed all 34 integration tests; strict Clippy, formatting, and diff checks were clean.

Files changed:

- `crates/gascan-core/src/runtime.rs`: removed `CreateRequest::fixture`, documented `PolicyCompiler` as the sole construction path, and split external compile-fail coverage across fixture, `new`, `builder`, struct literal, struct update, and deserialization bypasses.
- `crates/gascan-core/tests/common/mod.rs`: added the shared integration-test request helper. It creates a real temporary root, writes and loads a valid offline manifest, builds a sealed `SandboxSpec`, and compiles the request with all mandatory capabilities.
- `crates/gascan-core/tests/backend_contract.rs`: replaced every unchecked create fixture with the shared validated helper. `FakeRuntime::failing_once` remains limited to its capabilities fixture and does not construct policy state.
- `.superpowers/sdd/p2-task-3-report.md`: recorded this review follow-up and its exact verification evidence.

Self-review:

- Production library code contains no public or private fixture/new/builder constructor for `CreateRequest`; construction remains inside `PolicyCompiler` through crate-private fields.
- `CreateRequest` remains `Serialize` with immutable getters and remains non-`Deserialize`. No `From`, `TryFrom`, mutable accessor, deserializer, or alternate request factory was introduced.
- Each external API bypass has an independent compile-fail example, so an unrelated earlier compiler error cannot mask accidental exposure of fixture, generic constructor, builder, struct literal/update, or deserialization.
- Backend tests compile the library normally and keep all test-only request assembly in the integration-test helper rather than relying on library `cfg(test)` code.
- The helper's default offline manifest emits no ports and the compiler supplies bounded CPU, memory, disk, and process limits plus the canonical temporary-root bind mount.
- No Task 4, Plan 3, or Apple files were changed. Downstream Plan 3 examples that mention `CreateRequest::fixture` will need conversion to validated integration helpers when that plan is implemented.

Concerns: no Task 3 blocker remains. The temporary directory is required while loading and compiling policy; the returned request intentionally owns its canonical source path value, matching the backend contract's value-oriented API even after helper-local temporary cleanup.

## Important review follow-up: fixture root lifetime

Final review found that the first integration helper returned only `CreateRequest`. Its local `TempDir` was therefore dropped on return, deleting the canonical bind source before a backend could consume the request.

TDD RED command:

```text
cargo test -p gascan-core --test backend_contract validated_fixture_keeps_its_canonical_bind_source_alive -- --exact
```

RED result: exit 101. The focused test failed its `fixture.bind_mounts()[0].source.exists()` assertion, directly reproducing that the canonical source had already been removed.

Focused GREEN commands:

```text
cargo fmt --all
cargo test -p gascan-core --test backend_contract validated_fixture_keeps_its_canonical_bind_source_alive -- --exact
cargo test -p gascan-core --test backend_contract
```

GREEN result: every command exited 0. The lifetime regression passed, followed by all 9 backend-contract tests.

Final verification commands:

```text
cargo test -p gascan-core --test policy --test backend_contract
cargo test -p gascan-core --doc
cargo test -p gascan-core --all-targets
cargo clippy -p gascan-core --all-targets -- -D warnings
cargo fmt --all -- --check
git diff --check
```

Final result before commit: every command exited 0. Policy passed 10 tests, backend contract passed 9 tests, all 8 doctests passed, and the full core suite passed all 35 integration tests. Strict Clippy, formatting, and diff checks were clean.

Files changed:

- `crates/gascan-core/tests/common/mod.rs`: `create_request` now returns an integration-only `CreateRequestFixture` that owns both the `TempDir` guard and compiled request. It exposes immutable dereference access and a cloned request for backend consumption.
- `crates/gascan-core/tests/backend_contract.rs`: every backend setup retains the owning fixture across `.await` calls, including the create-failure branch; added direct canonical bind-source lifetime coverage.
- `.superpowers/sdd/p2-task-3-report.md`: appended this follow-up's exact RED/GREEN and review evidence.

Self-review:

- The `TempDir` guard and `CreateRequest` now have the same owner, so the canonical source cannot disappear while a test retains its fixture.
- No test passes `create_request(...)` as a temporary expression into an async backend call. Each fixture is bound to a local variable before cloning its request, and the owner remains in scope across `.await`.
- Request clones retain sealed policy values but do not own filesystem lifetime; the test-only fixture makes that distinction explicit without adding a constructor or filesystem policy to production library or `FakeRuntime` code.
- The focused regression observes the exact external invariant: the compiler-emitted canonical bind source exists while the fixture is alive.
- No Task 4, Plan 3, or Apple files were changed.

Concerns: no Task 3 blocker remains. Backend adapter integration tests should follow this owning-fixture pattern whenever a runtime may inspect or mount request source paths asynchronously.
