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
- RED: preflight ordering test initially reached workspace validation before the injected runtime failure.
- GREEN: all focused tests and the full workspace suite pass.

## Verification

- `cargo test -p gascan-core --test doctor`
- `cargo test -p gascand --test backend_selection`
- `cargo test -p gascan-e2e --test doctor`
- `cargo test -p gascan-e2e --test fake_backend pre_begin_rpc_failures_keep_stable_statuses -- --exact`
- `cargo test -p gascand --test daemon_idle`
- `cargo test --workspace`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo fmt --all -- --check`
- `git diff --check`

All completed successfully. Live `gascan doctor` was not claimed or executed: although the host is Darwin/arm64 and has a `container` executable, the repository's structured capability baseline does not establish every mandatory production capability on this host.

## Concern for review

The stable host/service/kernel/storage/workspace check IDs are present, but the current backend capability interface supplies only CLI version and runtime capability facts. Those ancillary checks are reported as prerequisites satisfied upon reaching the structured runtime probe; richer independently injectable probes would require expanding the Apple probe interface beyond the Task 5 file list.
