# Plan 2 Task 8 Report: CLI and fake-backend control plane

## Final implementation

Gas Can now ships the ten-command `gascan` CLI, connect-first bounded daemon autostart, API v1/local-security negotiation, stable JSON inspection and lifecycle-event rendering, distinct usage/daemon/runtime/API exits, exact command exits, confirmation safety, and TTY-only RAII terminal handling.

Run and Shell allocate an expiring pending token only after the daemon validates an existing, running, Gas Can-owned sandbox. Attach atomically reserves the first valid token, starts a backend-neutral live `ExecSession`, and concurrently forwards bounded stdin/resize/signal/close inputs and stdout/stderr/exit outputs. Every subsequent frame is validated before forwarding. Empty, mismatch, unknown, expired, and replay failures retain their distinct stable codes. Pending and expired registries are capped at 1024; claim/allocation prune them and an independent weak cleanup task expires abandoned sessions.

`RuntimeBackend` remains exactly nine methods. Exec returns a live channel session. Logs accept an optional millisecond boundary. The deterministic fake command interpreter uses literal argv and never invokes a host shell. It supports exact exits, byte stdin/stdout/stderr, environment inspection, and observable resize. It emits exactly one exit.

Fake runtime truth is independent of controller SQLite. `GASCAN_FAKE_STATE_PATH` atomically persists containers, volumes, states, and timestamped binary log records scoped to exact SandboxId. Reopen reconstructs fresh process-local removal observations; it never deserializes opaque removal proof. Logs filter by exact sandbox and timestamp, and follow emits later appended bytes until cancellation or shutdown.

The pre-release v1 command payload carries repeated `EnvironmentVariable` entries and TTY state; repeated entries preserve duplicate detection. The daemon rejects empty/control/NUL/duplicate/disallowed environment entries, then applies `gascan_core::policy::filtered_host_environment`. The CLI sends TERM, COLORTERM, LANG, and every nonempty `LC_*`; direct Tonic callers cannot inject secrets.

TTY Attach sends the initial size, SIGWINCH changes, stdin, SIGINT/SIGTERM, and Close. A shared idempotent restore handle duplicates the exact terminal descriptor before any mutation and restores termios before forwarding a terminating signal; its setup guard also restores after every later setup failure. Drop restores on success, exact nonzero command exit, daemon/Attach loss, and panic unwind. Real PTY subprocess tests cover those paths, while test-only injected setup failures and `catch_unwind` prove restoration without adding a production panic path.

Lifecycle RPCs publish the checked durable operation ID and live Pending stream immediately after `begin_operation`; keyed mutation and its daemon activity lease continue in a spawned task independent of client cancellation, with post-begin errors persisted and streamed as the same operation's Failed terminal. Command payload field 2 is reserved: all stdin travels only through bounded 16 KiB Attach frames with channel backpressure after token acquisition. An unbounded-producer E2E proves flushed output arrives before EOF while cancellation closes the producer promptly. Follow logs use Pending snapshot/appends with strictly increasing sequence and emit one Completed terminal only on graceful server shutdown (or one Failed terminal on backend error); client cancellation fabricates no terminal and releases the activity lease, while non-follow remains Completed sequence 1.

Lifecycle startup publication carries either the live operation or a typed pre-begin rejection; direct RPC tests preserve NotFound, InvalidArgument, AlreadyExists/operation-conflict, and Unavailable instead of collapsing to Internal. Autostart bounds the complete initial UDS connect, HTTP/2 setup, and Handshake probe before daemon election as well as later readiness probes. A withholding-UDS regression accepts the initial connection without speaking HTTP/2 and proves startup still advances within the bound.

The daemon arms its SIGTERM receiver before binding or publishing the owned socket, then carries that already-registered termination future into the bound server. A 20-iteration subprocess regression sends TERM immediately when the socket pathname appears and proves exit zero plus owned-socket cleanup every time.

## TDD and verification

- Initial RED: the real-process scenario failed because the CLI binary was absent.
- Review RED: live session types/channels and independent fake persistence did not exist; CommandPayload had the wrong descriptor.
- Rereview RED: logs were not sandbox-scoped, direct API environment injection crossed the boundary, resize was unobservable, and abandoned registries were unbounded.
- Backend contract: 19 tests, including live control flow, exact log isolation, and persistence reopen.
- API compatibility: 10 exhaustive v1 tests.
- Real-process fake E2E: 17 tests covering live/disconnect-independent lifecycle and post-begin failure, bounded unbounded-producer stdin, binary streams/logs, exact follow terminal/cancellation behavior, environment defense, two sandboxes, idle and SIGKILL restart, API mismatch, atomic token misuse, and real PTY success/nonzero/connection loss/resize/SIGINT/SIGTERM/restoration. Autostart has a separate deterministic race test, and a real-PTY unit test covers panic unwind.
- Logs-since tests establish a future millisecond boundary by observing the clock, retain inclusive `timestamp >= since` semantics, and exclude the completed pre-boundary record without scheduler-timing assumptions.
- Concurrent autostart serializes schema discovery/creation under SQLite's immediate transaction, then uses a deterministic client barrier and bounded connect/HTTP2/Handshake readiness probes within the fixed overall deadline; only transient startup transport failures retry, while compatibility and security failures remain terminal. The E2E harness records the PID only after socket ownership and terminates that exact live daemon before deleting its private runtime, preventing orphan accumulation.
- Managed execution denies UDS bind; identical approved-escalation commands are required. Production security was not weakened.
- Strict workspace Clippy, full workspace tests/doctests, formatting, and diff checks are the final gate. Apple live tests remain ignored and no Apple integration was added.

Task 8 is pending independent rereview.
