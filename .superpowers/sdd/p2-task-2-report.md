# Plan 2 Task 2 Report: Runtime Backend Contract

## Status

Implemented the backend-neutral async runtime contract and deterministic in-memory fake. Work stops before Task 3.

## TDD evidence

### RED

Command:

`cargo test -p gascan-core --test backend_contract`

Exit: `101`

The compiler reported the intended missing surface: no `fake_runtime` module; no `CreateRequest`, `ExecRequest`, `RuntimeBackend`, `RuntimeCall`, or `ContainerState`; and no Tokio test support. This was the expected failure caused by the absent Task 2 API, not a test typo.

### GREEN

Focused command:

`cargo test -p gascan-core --test backend_contract`

Exit: `0`; `8 passed; 0 failed`.

The suite covers the reusable happy-path contract through `&dyn RuntimeBackend`, duplicate-create rejection, idempotent start/stop, owned-resource filtering, raw binary stdin/stdout/stderr/log representation, exact exit codes, literal request ordering, stable error codes, and fail-once injection at every named backend boundary.

Full verification command:

`cargo fmt --all && cargo test -p gascan-core && cargo clippy -p gascan-core --all-targets -- -D warnings`

Exit: `0`. Results: all gascan-core unit/integration/doc tests passed (8 backend contract, 8 manifest, 1 capability, 7 sandbox identity, 2 compile-fail doctests), and strict all-target Clippy completed without warnings.

## API decisions

- `RuntimeBackend` uses `async-trait`, `&self`, and `Send + Sync`, and is proven usable as a trait object.
- The nine boundaries are `capabilities`, `inspect`, `create`, `start`, `stop`, `remove`, `exec`, `logs`, and `list_owned`.
- `CreateRequest` is backend-neutral and policy-shaped: immutable image reference text, typed bind mounts, volumes, loopback-capable IP/port mappings, environment, resource limits, network/user policy, and explicit ownership metadata. It contains no Apple command, program, option, or argv shape.
- `ExecRequest` records argv literally and keeps stdin as bytes. `ExecSession` retains stdout/stderr as bytes and the exact signed exit code. Logs are bytes.
- `RuntimeError::code()` provides stable category strings while retaining structured diagnostic fields.
- `OwnedResource` and `RuntimeSandbox` carry the sandbox identity and ownership metadata explicitly.
- `FakeRuntime` uses a Tokio mutex, deterministic sorted owned listing, literal call records, exact configured output, and one-shot boundary failures.
- `SandboxId::test`, `CreateRequest::fixture`, and `ExecRequest::fixture` are documented public fixture helpers. The sandbox helper delegates to `SandboxId::from_root`, so it does not introduce an unchecked identity constructor or weaken deserialization validation.

## Files

- Modified `crates/gascan-core/Cargo.toml`
- Modified `crates/gascan-core/src/lib.rs`
- Modified `crates/gascan-core/src/runtime.rs`
- Modified `crates/gascan-core/src/sandbox.rs`
- Added `crates/gascan-core/src/fake_runtime.rs`
- Added `crates/gascan-core/tests/backend_contract.rs`
- Added `.superpowers/sdd/p2-task-2-report.md`

Dependencies were added only to the gascan-core manifest from existing workspace dependencies: `async-trait` and Tokio. The root workspace manifest was not changed.

## Self-review

- Confirmed there are no Apple CLI identifiers or backend argv construction in core production code.
- Confirmed every state access in the fake is serialized behind Tokio's mutex and clones are returned instead of exposing internal state.
- Confirmed failures occur after literal call recording but before mutation, are consumed exactly once, and cover all nine boundaries.
- Confirmed duplicate creation does not overwrite state, start/stop converge idempotently, exec requires a running sandbox, and owned discovery excludes seeded foreign state.
- Confirmed production code introduces no unsafe, unwrap, expect, or panic.
- Confirmed Task 1 identity validation remains sealed; fixture identity construction follows the existing validated production derivation path.
- Confirmed formatting and diff whitespace checks are clean.

## Concerns

`ExecSession` is a completed byte-oriented result rather than a live bidirectional attachment. That matches Task 2's exact-exit contract and leaves the later attachment bridge to its dedicated plan task. No current blocker or known correctness defect remains.
