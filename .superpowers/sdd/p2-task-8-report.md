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

## Review correction: live sessions, independent runtime truth, and adversarial E2E

The first implementation completed backend execution before Attach and buffered its result. The correction changes `ExecSession` into a backend-neutral live session with bounded input/output channels. `RuntimeBackend` remains exactly nine methods. Inputs are byte stdin, resize, signal, and close; outputs are byte stdout/stderr and exactly one exit. FakeRuntime interprets explicit fake commands from literal argv without invoking a host shell.

Run and Shell now validate the selected sandbox but allocate only a pending session. Pending tokens expire after 30 seconds, use a bounded expired-token history, and are claimed atomically. Attach validates the first token and frame before claiming it, then concurrently forwards bounded inputs and outputs. Every later frame is validated against the bound token before forwarding. A mismatch closes the live backend session and emits the stable mismatch error. Replay and simultaneous claims cannot execute twice.

The invoking CLI, not the daemon environment, supplies the fixed TERM/COLORTERM/LANG/LC allowlist. The pre-release v1 payload correction adds environment and TTY to `CommandPayload` and makes Shell use that payload; exhaustive descriptor compatibility remains 10/10. RPC application failures now use runtime exit 70 while connection/daemon failures remain 69. Lifecycle commands accept JSON event rendering while retaining human phase progress.

TTY Attach uses a real bounded mpsc producer/consumer. It sends the initial terminal size, subsequent SIGWINCH sizes, stdin chunks, SIGINT/SIGTERM controls, and Close on EOF. A shared idempotent restore handle restores the terminal before a signal frame is forwarded; the RAII Drop path covers success, errors, and unwinding.

Fake runtime truth is no longer inferred from controller SQLite. A distinct `GASCAN_FAKE_STATE_PATH` atomically snapshots exact containers, volumes, states, timestamped binary logs, and reconstructs fresh process-local removal observations on reopen. SIGKILL and idle-restart E2Es prove the runtime state, rather than SandboxRecord, permits subsequent execution.

Logs are timestamped append-only records in fake runtime truth. The nine-method backend contract accepts an optional millisecond boundary. `--since-millis` filters actual records. Follow polls the backend-neutral append-only view, emits only new byte suffixes with backpressure, owns an activity lease, and ends on client cancellation or daemon shutdown.

Review-correction TDD evidence:

- RED: live session tests failed because ExecInput/ExecOutput/send/next did not exist.
- RED: persistence reopen failed because FakeRuntime had no independent state source.
- RED: the descriptor reported two CommandPayload fields instead of four.
- GREEN: backend contract 18/18 and API compatibility 10/10.
- GREEN: real-process fake backend 11/11, including concurrent autostart, two sandboxes, API mismatch, SIGKILL restart, binary/environment isolation, one-use concurrent token claims, since/follow, and real PTY resize/SIGINT/SIGTERM/restoration.
- GREEN: strict workspace Clippy and the full workspace test/doc suite; 9 Apple live tests remained ignored.
