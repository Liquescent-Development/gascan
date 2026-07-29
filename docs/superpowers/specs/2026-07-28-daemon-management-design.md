# Daemon Management and Automatic Recovery Design

## Summary

Gas Can currently starts `gascand` on demand, but it has no public daemon
lifecycle commands and its handshake does not report the daemon's Gas Can
release version. A package upgrade can therefore leave an older daemon
serving a newer CLI indefinitely when their API major versions remain
compatible.

The daemon also inherits the CLI process's current working directory. If that
directory is later removed, daemon-wide Doctor state records the deleted
directory as the current workspace. A user can consequently start Gas Can
from the "wrong" directory, upgrade the installed package, and be left with a
daemon that appears unhealthy but has no supported management interface.

Gas Can will add public daemon status, start, stop, and restart commands;
release-version negotiation; safe automatic replacement of outdated daemons;
and request-scoped workspace diagnosis. Current daemons will shut down through
a graceful RPC. Legacy daemons will be stopped only after the CLI revalidates
the existing random instance token over the protected endpoint and revalidates
process identity immediately before signaling.

## Goals

- Let users inspect and control the per-user Gas Can daemon directly.
- Detect a running daemon whose release differs from the installed CLI.
- Automatically replace an outdated daemon before executing an ordinary
  command.
- Preserve active durable operations during normal graceful shutdown.
- Provide an explicit, clearly labeled force escape hatch.
- Never signal a process that has not been proven to be the attested Gas Can
  daemon.
- Make daemon startup independent of the caller's working directory.
- Evaluate Doctor's workspace check for the calling CLI, not the daemon's
  launch directory.
- Keep human output concise and JSON output strictly structured.
- Recover safely from legacy daemons and stale local socket metadata.

## Non-goals

- No launch agent, system daemon, or always-running service.
- No daemon management through Homebrew or package installer scripts.
- No unattended background upgrade outside a user-invoked Gas Can command.
- No cross-user daemon discovery or management.
- No API-major compatibility bridge between unsupported Gas Can releases.
- No automatic forced shutdown when active work fails to drain.
- No change to sandbox lifecycle, sandbox ownership, or sandbox persistence.

## Public CLI

The CLI gains a visible `daemon` command group:

```text
gascan daemon status [--json]
gascan daemon start [--json]
gascan daemon stop [--force] [--json]
gascan daemon restart [--force] [--json]
```

`start` and `stop` are idempotent. Starting an already-current daemon and
stopping an already-stopped daemon both succeed and report the resulting
state. `restart` starts the installed daemon even when no daemon was running.

The existing hidden `daemon-attest` command remains an internal compatibility
mechanism for release packaging and legacy recovery. It is not the public
management interface.

### Human Output

Human output follows the existing polished presentation conventions. Typical
results are:

```text
✓ Gascan daemon is running
  Health             Healthy
  PID                40382
  Uptime             12m 8s
  Installed version  0.1.12
  Running version    0.1.12
  Executable         /usr/local/bin/gascand
```

```text
○ Gascan daemon is stopped
```

An ordinary command that finds an outdated daemon displays a single progress
message:

```text
Restarting outdated Gascan daemon…
```

Interactive terminals may render that message with the existing updating
progress treatment. Non-interactive human output prints one stable line.

### JSON Output

JSON mode emits one command result and never emits human progress text.
Daemon status includes a stable state, health, installed version, and nullable
runtime identity:

```json
{
  "state": "running",
  "health": "healthy",
  "installed_version": "0.1.12",
  "running_version": "0.1.12",
  "pid": 40382,
  "started_at_millis": 1785263800000,
  "uptime_millis": 728000,
  "executable": "/usr/local/bin/gascand",
  "legacy": false
}
```

Lifecycle results additionally identify whether a transition occurred and
whether force was used. Nullable runtime fields are `null` when stopped.
Automatic recovery that precedes another JSON command is silent; stdout
contains only that command's documented JSON result. Errors continue to use
the CLI's structured error contract.

## Protocol and Identity

The handshake response gains the exact daemon Gas Can release version and
daemon start time in Unix milliseconds. Existing process identity fields
remain authoritative:

- daemon instance token;
- PID;
- executable path;
- platform start identity.

The CLI compares the running version with its own release version exactly.
Matching API versions do not make different product releases interchangeable.
An absent release version identifies a legacy daemon and requires
replacement. An invalid version or contradictory identity makes the daemon
unhealthy rather than current.

The service gains:

- a daemon-status RPC returning health, version, start time, and identity;
- a graceful-shutdown RPC that causes the server to stop accepting new work,
  wait for durable operations, cancel attach streams, and exit.

The shutdown request carries the daemon instance token observed during the
current connection. The daemon rejects a token mismatch. Peer-UID validation
on the Unix socket remains mandatory.

New protocol fields and RPCs are additive. A new CLI can recognize an old
daemon because the version field is absent and the shutdown RPC is
unimplemented. An old CLI can continue negotiating with a new daemon under
the existing API-major rules.

## Daemon State Model

Inspection does not implicitly start a daemon. The CLI classifies local state
as:

| State | Meaning |
| --- | --- |
| `stopped` | No live daemon owns the endpoint |
| `running/current` | Attested daemon matches the installed release |
| `running/outdated` | Attested daemon reports another release or is legacy |
| `running/unhealthy` | A daemon responds but fails health or identity validation |
| `unreachable` | Local ownership metadata indicates a possible daemon but the endpoint cannot be negotiated |
| `unsafe` | The endpoint or metadata could belong to an unverified process |

Only `stopped` and an attested outdated daemon are eligible for automatic
startup/replacement. Unsafe state fails closed with actionable diagnostics.

Stale socket or instance-record files may be removed only when process and
endpoint inspection proves that no live valid daemon owns them. Gas Can does
not infer process ownership from a PID alone.

## Starting the Daemon

The CLI locates `gascand` adjacent to its own installed executable as it does
today, but launches it with a stable current directory independent of the
caller's workspace. The per-user Gas Can runtime directory is preferred when
it exists safely; otherwise a fixed system directory is used. Failure to
establish a safe stable directory prevents startup.

The child is detached from transient CLI standard streams as today. Readiness
requires a successful handshake, a current release version, and matching
executable identity. A process that binds the endpoint but fails those checks
is not accepted as the newly started daemon.

## Graceful Shutdown

For a current daemon, `stop` calls the graceful-shutdown RPC. The daemon:

1. authenticates the peer and instance token;
2. stops accepting new work;
3. waits for active durable operations to complete;
4. cancels interactive attachment streams;
5. closes connections and removes its owned endpoint state;
6. exits.

The CLI waits for the exact attested process to exit and confirms that the
endpoint is no longer owned. A bounded timeout returns an actionable error and
suggests `--force`; it does not automatically escalate.

This flow uses the daemon's existing shutdown tracker and durable-operation
drain semantics rather than inventing a second shutdown mechanism.

## Legacy and Forced Shutdown

A legacy daemon cannot service the new shutdown RPC and released legacy
daemons do not necessarily have an instance record. The CLI may signal one
only through the attested fallback:

1. negotiate the protected endpoint and capture token, PID, executable, and start
   identity;
2. verify that the executable is a Gas Can daemon at the expected trusted
   installed location;
3. immediately negotiate the endpoint again and require an identical token,
   PID, executable, and start identity;
4. immediately revalidate the live process start identity and executable;
5. when an instance record exists, require it to match both attestations;
6. send `SIGTERM`;
7. wait for the exact process identity to exit.

This adapts the release tooling's existing double-attestation model into
shared tested Rust code. New daemons always publish a protected instance
record, which also permits safe diagnosis of an attested daemon whose endpoint
later becomes unreachable. A changed token, reused PID, changed executable,
unsafe record, or inconsistent endpoint fails closed.

`--force` is available only for explicit `stop` and `restart`. It performs the
same immediate identity revalidation, warns in human mode, sends the
platform's forceful termination signal if graceful termination does not
finish, and confirms exit. It may interrupt active sandbox operations and
attachments. Automatic recovery never force-kills a daemon.

## Automatic Recovery

Every ordinary daemon-backed command uses a single connection supervisor:

1. inspect the endpoint without starting;
2. accept a healthy current daemon;
3. start a daemon when stopped;
4. gracefully replace an attested outdated or legacy daemon;
5. reconnect and require a healthy current handshake;
6. execute the requested command.

If an outdated daemon cannot drain within the timeout, the ordinary command
fails with the running version, installed version, and explicit recovery
instructions. It does not execute against the outdated daemon and does not
force shutdown.

Concurrent CLIs coordinate through the existing protected runtime state plus
a bounded per-user lifecycle lock. After acquiring the lock, each CLI
re-inspects state before acting. This prevents simultaneous commands from
starting two daemons or signaling a replacement another command already
started.

`gascan daemon status` is inspection-only and never performs recovery.
`gascan daemon start`, `stop`, and `restart` use the same supervisor primitives
but apply their explicit idempotent semantics.

## Doctor Workspace Semantics

Doctor's workspace check becomes request-scoped. The CLI resolves its current
directory without requiring canonicalization to succeed and includes either
the absolute workspace path or the local resolution error in `DoctorRequest`.
The daemon computes the workspace fact for that request. A non-UTF-8 path is
reported as a caller-path error instead of being lossily rewritten.

The daemon's release-static and host-wide Doctor facts may remain cached.
The workspace fact is replaced with the per-request result before returning
the report. A deleted or inaccessible caller directory therefore produces a
useful workspace failure for that invocation without marking the daemon
itself unhealthy or affecting calls from valid directories.

A legacy caller that omits the new request field receives a clearly defined
fallback workspace result that does not consult the daemon's current
directory. The daemon process no longer treats its launch directory as a
workspace.

## Error Handling

Errors distinguish:

- daemon stopped;
- daemon outdated;
- graceful shutdown timed out;
- process identity changed during shutdown;
- endpoint state is unsafe;
- daemon executable is missing or inconsistent;
- current daemon failed readiness;
- caller workspace is inaccessible.

Each error includes one safe next action. Diagnostics never recommend deleting
runtime files or killing a PID manually when Gas Can cannot establish
ownership.

Management commands remain usable when the caller's current directory has
been deleted. They do not load a workspace manifest unless the requested
operation actually requires one.

## Testing

Implementation follows test-driven development and adds:

- protocol tests for version/start-time fields and shutdown authentication;
- client tests for current, outdated, legacy, unreachable, and unsafe states;
- lifecycle-lock concurrency tests;
- idempotent start and stop tests;
- graceful operation-drain and timeout tests;
- explicit forced-shutdown tests;
- PID reuse, token change, executable mismatch, and unrelated-process safety
  tests;
- stale socket and instance-record recovery tests;
- human and JSON output snapshots/contracts;
- a Brew-style replacement integration test in which the executable on disk
  changes while the old daemon remains alive;
- a deleted-launch-directory regression test;
- Doctor tests proving workspace facts are caller-specific;
- full workspace tests and relevant macOS release-smoke coverage.

Tests use short-lived fixture daemons and never signal a process without a
unique test-owned identity. Release tests verify that public management does
not weaken the protected runtime-directory and attestation contracts.

## Documentation

The README documents:

- the on-demand per-user daemon model;
- automatic replacement after upgrades;
- all four public daemon commands;
- graceful versus forced shutdown;
- the status fields and JSON mode;
- Doctor's caller-workspace behavior.

Command help includes the interruption risk for `--force` and makes clear that
ordinary commands normally manage daemon startup and upgrades automatically.
