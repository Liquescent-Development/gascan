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
