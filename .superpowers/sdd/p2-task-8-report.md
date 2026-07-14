# Plan 2 Task 8 Report: CLI and fake-backend control plane

## Scope

- Added the `gascan` CLI with `up`, `apply`, `shell`, `run`, `down`, `destroy`, `list`, `status`, `logs`, and `doctor`.
- Added connect-first daemon autostart, bounded readiness, API v1 and local-transport negotiation, stable inspection JSON, typed non-command exit classes, and exact guest-command exit propagation.
- Filled the Task 7 Run, Shell, Logs, and Attach stubs through `SandboxService` and the existing nine-method `RuntimeBackend` contract. Exec and logs require an existing Gas Can-owned runtime; exec additionally requires it to be running.
- Added opaque one-use session tokens and Attach framing for byte-safe stdout, stderr, and exact exit. Attach uses the v1 one-session binder and rejects changed tokens.
- Added a TTY-only raw-terminal RAII guard and non-TTY `destroy` refusal without `--yes`.
- Added a real-process fake-backend package and lifecycle acceptance test. No Apple integration or live Apple test was added.

## TDD evidence

- Initial RED: `cargo test -p gascan-e2e --test fake_backend` failed with `gascan binary is not built`.
- First process run under the managed sandbox failed at daemon connection because UDS bind is denied.
- The approved escalated run exposed a security-test fixture issue: macOS presents the temporary root through `/var`, a symlink rejected by Task 7's nofollow traversal. Canonicalizing only the test runtime root fixed the fixture without weakening production.
- GREEN: the escalated real-process scenario passes and exercises all ten commands, daemon autostart, Up/Apply, exact exit 42, byte stdout, Shell/Attach, Logs, Doctor/List JSON, non-TTY destroy refusal, Down/Status, and confirmed Destroy.

## Runtime/session design

The controller remains the only path to runtime exec and logs. It validates exact sandbox identity and ownership before reaching the backend, and rejects exec unless runtime inspection reports Running. Run and Shell produce random opaque session tokens stored only in the daemon. Attach consumes a token once, binds every received frame to the first token, and emits raw stdout/stderr followed by the backend's exact exit.

The fake daemon reconstructs only its fake in-memory runtime from durable sandbox records after process restart. This is limited to the Plan 2 non-live backend and keeps subsequent inspection and command behavior deterministic; it does not add an Apple seam.

The CLI forwards only `TERM`, `LANG`, `LC_ALL`, and `COLORTERM`. It never forwards the host environment map wholesale. JSON inspection fields and state strings are stable lowercase values. Human lifecycle progress comes from typed operation event phases.

## Exit codes

- `64`: command usage/configuration, including ambiguous selection and non-TTY destroy without `--yes`.
- `69`: daemon I/O, transport, and RPC connection failures.
- `70`: runtime/operation and local I/O failures.
- `76`: API or advertised local-transport incompatibility.
- Run/Shell guest command exit values are returned unchanged, including 42.

## Verification

- `cargo test -p gascan-e2e --test fake_backend -- --nocapture` (approved UDS escalation) — 1 passed.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` — passed.
- `cargo test --workspace` (approved UDS escalation) — all platform-neutral tests and doctests passed; 9 Apple live tests ignored.
- `cargo fmt --all -- --check` and `git diff --check` — passed.

## Review status

Task 8 implementation is complete and pending independent review.
