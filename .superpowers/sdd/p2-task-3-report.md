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
