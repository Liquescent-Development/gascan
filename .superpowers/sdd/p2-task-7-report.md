# Plan 2 Task 7 Report: Private daemon transport

## Scope

- Added a Unix-domain-only Tonic daemon, private socket-path management, same-effective-UID peer authentication, lifecycle API adapter, activity accounting, idle shutdown, and SIGTERM cleanup.
- Added the narrowly authorized pre-release v1 correction that binds Up/Apply to an absolute project root; it is committed separately as `c8b78c6`.
- Run, Shell, Logs, and Attach return the stable `backend_unavailable` code because Task 5 exposes no session/log lifecycle seam. Task 8 was not started.

## TDD evidence

- Initial RED: `cargo test -p gascand --test socket_security --test daemon_idle` failed because `SocketPaths`, `PeerUid`, `ActivityTracker`, `DaemonConfig`, and `Daemon` did not exist.
- Schema RED: `cargo test -p gascan-proto --test api_compatibility v1_descriptor_exactly_covers_every_exported_message_enum_and_rpc` failed on former `UpRequest.manifest` versus required `project_root`.
- GREEN: the focused daemon suite passed 9/9 and API compatibility passed 10/10.

## Security and lifecycle design

The daemon creates one user-owned runtime directory at exact mode 0700 and one socket at exact mode 0600. It rejects symlink directory/socket endpoints, unsafe existing directory modes, arbitrary files, foreign ownership, and live sockets. A stale socket is eligible only after a connection attempt fails and type/UID/inode checks match. Removal first atomically quarantines the path, verifies the moved inode, and removes only that instance; replacement files survive cleanup.

The accept stream uses Tokio Unix peer credentials and admits only the daemon's effective UID before handing a connection to Tonic. There is no TCP listener or TCP configuration surface.

`SandboxApi` validates absolute UTF-8 project roots, loads the root-bound manifest on a blocking worker, derives the stable manifest/root sandbox identity, and adapts Status, List, Up, Apply, Down, and Destroy to `SandboxService`. Durable events retain operation IDs, sequence, status, JSON media type/payload, and terminal error structure. Unsupported Task 5 surfaces fail explicitly with the stable backend-unavailable code.

Unary RPC calls hold activity leases. Lifecycle calls additionally hold operation leases until Task 5's durable mutation returns; response streams own a lease for their full lifetime. Idle countdown begins only at zero leases and zero operations and restarts on every generation change. SIGTERM triggers Tonic graceful shutdown, stops accepts, waits for active RPC work, and drops the inode-owned socket guard.

## Adversarial coverage

- Exact directory/socket modes and socket file type.
- Symlink directory and socket refusal.
- Unsafe directory-mode refusal.
- Live socket and arbitrary-file refusal.
- Safe stale socket replacement.
- Exact peer-UID validator behavior.
- Cleanup preserving a replacement inode.
- Unary/stream lease and durable-operation idle retention.
- Idle exit socket cleanup.
- Real child-process SIGTERM exit and socket cleanup.
- Absolute, canonicalizable, manifest-bound project roots.

The managed command sandbox denies Unix socket bind with OS `EPERM`. Re-running the identical commands with approved escalation passed; production behavior was not weakened or retried.

## Verification

- `cargo test -p gascan-proto --test api_compatibility` — 10 passed.
- `cargo test -p gascand` (approved escalation for UDS bind) — 65 tests passed plus 2 doctests.
- `cargo test --workspace` (approved escalation for UDS bind) — all non-live tests passed; 9 Apple live tests ignored.
- `cargo doc -p gascand --no-deps` — passed.
- `cargo clippy -p gascand --all-targets -- -D warnings` — passed.
- `cargo fmt --all -- --check` and `git diff --check` — passed.

Task 7 implementation is complete and pending independent review.

## Review correction

The first review found that pathname checks could race directory substitution,
the executable served a handshake-only API, and wire status/events omitted
durable metadata. The correction replaces those boundaries rather than
relaxing them.

Socket directory traversal now starts from an open root descriptor and opens
every component with `openat(O_DIRECTORY | O_NOFOLLOW)`. The final directory
is created with `mkdirat`, forced to 0700 with `fchmod`, and retained as a
capability for all stat, rename, quarantine, and unlink operations. Binding
uses a unique staging node and accepts it only after descriptor-relative stat
proves it landed in that retained directory; it is then atomically renamed
without replacement. Cleanup continues to address the retained directory even
if its pathname is swapped. Quarantine collisions select a fresh generated
name and never overwrite an existing entry.

The executable now constructs a durable Store, FakeRuntime (the configured
non-live backend available in this plan phase), provisioner, SandboxService,
and SandboxApi. The handshake-only LocalApi and its lifecycle stubs were
removed. A real peer-authenticated UDS Tonic test performs Handshake and Up
through the generated client and observes the completed durable event stream.
Another child-process test sends SIGTERM while provisioning is active, proves
the daemon remains alive while the durable operation drains, then proves the
connection and owned socket close.

Store schema v2 durably records event timestamps/error codes and sandbox
updated timestamps. Migration from exact v1 is transactional. Status responses
carry the durable last operation ID and updated timestamp; events carry their
stored timestamp and exact stored error code. Pending-operation conflicts map
to the public `operation_conflict` contract.

Review-wave TDD evidence:

- RED: intermediate symlink traversal unexpectedly succeeded and created the
  directory through the link.
- RED: swapping the runtime directory after bind left the daemon socket behind
  in the displaced directory.
- GREEN: expanded socket security passed 8/8 under the required UDS escalation.
- GREEN: real daemon lifecycle/SIGTERM suite passed 5/5.
- GREEN: metadata API unit tests passed 3/3 and Store passed 23/23.
