# Task 5 report

## Implementation

- Production `gascand` selects `AppleBackend<ProcessRunner>`.
- The persistent fake runtime is reachable only through `GASCAN_TEST_FAKE_BACKEND` in debug builds; all binary-spawning tests opt in explicitly.
- Added serializable `DoctorReport`, `DoctorCheck`, and `DoctorStatus` with stable IDs, details, and remedies.
- The existing Doctor RPC transports the report without changing the protobuf/API surface. The CLI renders stable human and JSON forms and returns the runtime exit code when findings exist.
- `up` and `apply` perform runtime readiness before workspace parsing/canonicalization or `PolicyCompiler`, including the hard-offline capability gate.
- Retained the Task 4 follow-logs channel capacity of 2 deliberately: it buffers the initial pending event plus a terminal event during shutdown/cancellation and is covered by the Task 4 drain-ordering tests.

## TDD evidence

- RED: backend selection integration test failed because the selector/types were absent.
- RED: core doctor tests failed because the doctor module/report was absent.
- RED: doctor e2e tests failed because the CLI exposed only counts/readiness text.
- RED: evidence tests failed until unavailable kernel/image facts became `Unknown`, malformed schemas became failures, and host/storage/workspace mismatches failed closed.
- GREEN: all focused tests and the full workspace suite pass.

## Verification

- `cargo test -p gascan-core --test doctor`
- `cargo test -p gascand --test backend_selection`
- `cargo test --release -p gascand --test backend_selection`
- `cargo test -p gascan-e2e --test doctor`
- `cargo test -p gascan-e2e --test fake_backend pre_begin_rpc_failures_keep_stable_statuses -- --exact`
- `cargo test -p gascand --test daemon_idle`
- `cargo test --workspace`
- `cargo build -p gascan -p gascand` followed by isolated `target/debug/gascan doctor --json`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo fmt --all -- --check`
- `git diff --check`

All completed successfully. An isolated live production `gascan doctor --json` also completed with valid JSON and all 17 checks passing on macOS 26.5.1/arm64 with the exact Apple 1.1.0 client/API revision. Both storage checks observed 376,786,288,640 free bytes on the shared structured-status `appRoot` filesystem.

## Evidence-bearing review follow-up

- Replaced inferred prerequisite passes with `DoctorFacts`; facts carry `Pass`, `Fail`, or `Unknown` plus exact evidence.
- Host architecture comes from the compiled process target. macOS comes from structured parsing of `/System/Library/CoreServices/SystemVersion.plist` with the `plist` crate and requires ProductVersion major 26+.
- CLI version uses exact `container system version --format json` release/commit/version schema. Only exact 1.1.0 enables the signed-off capability matrix.
- Service readiness uses the captured Apple 1.1 `container system status --format json` fixture and requires a running release `container-apiserver` with matching 1.1.0 schema.
- State and image free space use shared `statvfs` evidence on the exact structured-status `appRoot`, which the approved Apple 1.1 schema defines as the application/state/image filesystem; both require the documented 10 GiB threshold.
- MVP kernel readiness is activated by the frozen Gate 2 kernel/live lifecycle proof plus the current exact running service identity on the supported host. It is not inferred from an undocumented status field.
- Workspace accessibility canonicalizes and reads metadata for the daemon's inherited current/request context before policy compilation; request-specific canonical validation remains immediately after runtime preflight and before `PolicyCompiler`.
- Doctor RPC returns the stored report even when collection found a missing CLI, malformed schema, unsupported version, stopped service, or unavailable fact; those conditions no longer become a Doctor RPC transport error.
- Fake backend enum/flag are cfg-elided from release builds. `cargo test --release -p gascand --test backend_selection` proves a fake request selects Apple and the debug-only flag symbol is not referenced.

The binding evidence revision is Gate 2 report commit `6bedef8`, report SHA-256 `df51167b450c3fd0eb80699db76b4decbd7c44ab7f73788eee3240eb19057ad1`, status fixture SHA-256 `00e66b6721f5b9ce185b98bef47f0699425d06bff6396b4e29e90f55e9079cf9`, and Apple client/API revision `5973b9cc626a3e7a499bb316a958237ebe14e2ed`. Production DoctorFact details record all four identifiers. Client/service/schema/host mismatch prevents activation.
