# Daemon Management and Automatic Recovery Implementation Plan

> **For Codex:** REQUIRED SUB-SKILL: Use
> `superpowers:test-driven-development` for every behavior change and
> `superpowers:verification-before-completion` before committing the final
> result.

**Goal:** Add safe public daemon lifecycle commands, automatically replace
outdated daemons, detach daemon health from the launch directory, and make
Doctor's workspace result specific to each CLI caller.

**Architecture:** Extend the local protobuf API with daemon release identity,
status, graceful shutdown, and request-scoped Doctor workspace fields. Build a
single CLI-side daemon supervisor that serializes lifecycle transitions,
classifies local daemon state without autostarting, uses RPC shutdown for
current daemons, uses double-attested signaling for legacy daemons, and starts
the adjacent installed daemon from a protected stable runtime directory.

**Tech Stack:** Rust, Tokio, Tonic/protobuf, Clap, rustix, serde/serde_json,
existing Gas Can presentation helpers, and the fake-backend E2E binaries.

---

## Ground Rules

- Work only in
  `/Users/kiener/code/gascan/.worktrees/daemon-management` on
  `feat/daemon-management`.
- Run every shell command through `rtk`.
- Preserve the API-major compatibility contract; all protobuf changes are
  additive.
- Do not signal a PID based only on a file or PID value.
- Do not let automatic recovery escalate to force.
- Add each behavior with a failing test first, observe the intended failure,
  implement the minimum code, then rerun the focused test.
- Keep the worktree clean between task commits.

## Task 1: Extend the Local Protocol Contract

**Files:**

- Modify: `proto/gascan/v1/gascan.proto`
- Modify: `crates/gascan-proto/src/lib.rs`
- Modify: `crates/gascan-proto/tests/api_compatibility.rs`

### Step 1: Write failing descriptor assertions

Update `api_compatibility.rs` first to require:

- `HandshakeResponse.release_version` at field 11;
- `HandshakeResponse.daemon_started_at` at field 12 as
  `google.protobuf.Timestamp`;
- `DoctorRequest.workspace` at field 2 and
  `DoctorRequest.workspace_error` at field 3 while field 1 remains reserved;
- additive `DaemonStatusRequest`, `DaemonStatusResponse`,
  `ShutdownDaemonRequest`, and `ShutdownDaemonResponse` messages;
- unary `DaemonStatus` and `ShutdownDaemon` service methods.

The daemon status response must carry release version, PID, executable,
platform start identity, instance token, start timestamp, and a health value.
Exactly one Doctor workspace field is populated: the absolute UTF-8 path or a
local `current_dir`/encoding error. The shutdown request must carry the
observed instance token; the response must acknowledge acceptance.

Increment `API_MINOR` from 3 to 4.

Run:

```sh
rtk cargo test -p gascan-proto --test api_compatibility
```

Expected: FAIL because the descriptor still exposes the old fields and RPC
set.

### Step 2: Add the protobuf fields and RPCs

Add only new field numbers and new messages. Preserve every existing field,
reservation, enum number, and RPC signature. Use a closed daemon health enum
with an explicit unknown value.

Run:

```sh
rtk cargo test -p gascan-proto --test api_compatibility
```

Expected: PASS.

### Step 3: Run all protocol tests

```sh
rtk cargo test -p gascan-proto
```

Expected: PASS.

### Step 4: Commit

```sh
rtk git add proto/gascan/v1/gascan.proto \
  crates/gascan-proto/src/lib.rs \
  crates/gascan-proto/tests/api_compatibility.rs
rtk git commit -m "feat: extend daemon control protocol"
```

## Task 2: Add Daemon Metadata and Graceful RPC Shutdown

**Files:**

- Modify: `crates/gascand/src/api.rs`
- Modify: `crates/gascand/src/lib.rs`
- Modify: `crates/gascand/tests/daemon_idle.rs`

### Step 1: Test release identity in handshake and status

Add focused tests that create a test API and assert:

- handshake reports `env!("CARGO_PKG_VERSION")`;
- daemon status returns the same release, PID, executable, start identity,
  instance token, and valid start timestamp;
- status reports healthy while the daemon accepts work.

Run:

```sh
rtk cargo test -p gascand daemon_metadata -- --nocapture
```

Expected: FAIL because the generated service requires unimplemented methods
and metadata fields are empty.

### Step 2: Test shutdown token authentication

Add tests proving:

- an empty or wrong instance token returns `permission_denied`;
- the correct token acknowledges shutdown;
- shutdown closes admission to new durable operations.

Run:

```sh
rtk cargo test -p gascand shutdown_rpc -- --nocapture
```

Expected: FAIL because no shutdown implementation exists.

### Step 3: Implement metadata and shutdown notification

Extend `DaemonIdentity` with release version and a protobuf-compatible start
timestamp captured once at process identity creation. Add a distinct
termination-request notification to `ActivityTracker`; do not reuse the
attachment-cancellation notification.

Implement `daemon_status` and `shutdown_daemon` on `SandboxApi`. The shutdown
method must compare the exact request token, mark admission closed, trigger
graceful server termination, and allow the RPC response to flush.

Make `Daemon::serve` select between SIGTERM, idle timeout, and RPC-requested
termination. Preserve the existing order:

1. stop admission;
2. drain durable operations without the two-second stream timeout;
3. cancel attachment streams;
4. bound connection closure.

### Step 4: Add process-level graceful-drain coverage

Adapt the existing SIGTERM durable-operation test to call `ShutdownDaemon`
with the attested token. Assert the daemon remains alive while the fake
operation is active and exits after it completes. Keep the SIGTERM regression
test as legacy behavior coverage.

Run:

```sh
rtk cargo test -p gascand --test daemon_idle
```

Expected: PASS.

### Step 5: Commit

```sh
rtk git add crates/gascand/src/api.rs crates/gascand/src/lib.rs \
  crates/gascand/tests/daemon_idle.rs
rtk git commit -m "feat: expose graceful daemon shutdown"
```

## Task 3: Make Doctor Workspace State Request-Scoped

**Files:**

- Modify: `crates/gascand/src/doctor.rs`
- Modify: `crates/gascand/src/main.rs`
- Modify: `crates/gascand/src/api.rs`
- Modify: `crates/gascand/tests/doctor_state.rs`
- Modify: `crates/gascan/src/cli.rs`
- Modify: `crates/gascan-e2e/tests/doctor.rs`

### Step 1: Write failing daemon Doctor tests

Add tests proving two Doctor requests against the same daemon can produce
different `workspace.access` results:

- an existing absolute directory passes;
- a missing absolute path fails and names the access problem;
- an omitted workspace field returns an explicit unknown/failure that does
  not inspect the daemon CWD.

Also prove a relative or malformed request path is rejected as an invalid
request rather than interpreted relative to `gascand`.

Run:

```sh
rtk cargo test -p gascand doctor_workspace -- --nocapture
```

Expected: FAIL because Doctor ignores its request.

### Step 2: Extract and apply the workspace fact per request

Move the workspace fact helper from `gascand/src/main.rs` into the existing
`gascand/src/doctor.rs` module. Keep the cached production report limited to
host/runtime facts and give its workspace check an explicit pending/unknown
value.

In the Doctor RPC:

- parse the absolute UTF-8 request path;
- clone the cached report;
- replace exactly the `workspace.access` check;
- derive findings from the request-specific report.

Do not mutate shared cached state.

### Step 3: Send the caller's directory from the CLI

Capture `std::env::current_dir()` before connecting to the daemon. Put its
absolute UTF-8 representation in `DoctorRequest.workspace`. If the OS cannot
resolve the caller directory or the path is not UTF-8, populate
`workspace_error` instead of falling back to the daemon directory.

Add unit coverage for request construction.

### Step 4: Add deleted-launch-directory E2E coverage

In the fake-backend Doctor E2E test:

1. start the daemon from a temporary directory;
2. remove that directory after the daemon is ready;
3. invoke Doctor from a valid directory and assert workspace passes;
4. assert daemon management remains possible.

Run:

```sh
rtk cargo test -p gascan-e2e --test doctor
```

Expected: PASS.

### Step 5: Commit

```sh
rtk git add crates/gascand/src/doctor.rs crates/gascand/src/main.rs \
  crates/gascand/src/api.rs crates/gascand/tests/doctor_state.rs \
  crates/gascan/src/cli.rs crates/gascan-e2e/tests/doctor.rs
rtk git commit -m "fix: scope doctor workspace checks to callers"
```

## Task 4: Build Safe Runtime State and Process Attestation

**Files:**

- Create: `crates/gascan/src/daemon.rs`
- Modify: `crates/gascan/src/lib.rs`
- Modify: `crates/gascan/src/client.rs`
- Modify: `crates/gascan/Cargo.toml`
- Modify: `crates/gascand/src/api.rs`
- Modify: `crates/gascand/src/socket.rs`
- Modify: `crates/gascand/src/lib.rs`

### Step 1: Test protected runtime paths and lifecycle locking

In `daemon.rs`, first add tests for a `DaemonPaths`/`LifecycleLock` abstraction:

- runtime directory must be absolute, owned by the effective user, mode
  `0700`, and free of symlink traversal;
- lifecycle lock is a regular owned mode-`0600` file;
- two contenders serialize, then the second rechecks state after acquiring;
- an unsafe directory, symlink, wrong owner, or permissive lock fails closed.

Use `rustix::fs::flock` and the repository's existing descriptor-relative
filesystem style.

Run:

```sh
rtk cargo test -p gascan daemon::tests::runtime -- --nocapture
```

Expected: FAIL until the state layer exists.

### Step 2: Test instance-record validation

Add table-driven tests for:

- valid current record;
- wrong owner or mode;
- symlink/non-regular file;
- malformed JSON;
- changed identity between reads;
- absent record;
- PID reuse/start-identity mismatch;
- executable mismatch.

The record is evidence only when combined with live OS identity or a protected
endpoint attestation. A PID alone is never sufficient.

Run:

```sh
rtk cargo test -p gascan daemon::tests::attestation -- --nocapture
```

Expected: FAIL until validation is implemented.

### Step 3: Share normal runtime record locations

Expose the instance-record and lifecycle-lock paths alongside the existing
socket path, without weakening `SocketPaths` validation. Make normal daemon
startup always set:

- `GASCAN_DAEMON_INSTANCE_PATH`;
- a fresh `GASCAN_DAEMON_OWNER_TOKEN`.

Update the daemon record to include release version and start timestamp.
Write it atomically as an owned mode-`0600` regular file. Remove it on clean
shutdown only after identity comparison.

Keep the existing E2E environment overrides functional.

### Step 4: Implement live process verification

Add a small platform process inspector that verifies:

- PID is still live;
- start identity exactly matches;
- executable identity/path matches the attestation;
- the process identity is rechecked immediately before signaling.

Use safe Rust/rustix and bounded `ps` execution where macOS lacks a suitable
safe direct API. Do not parse a shell command.

Run:

```sh
rtk cargo test -p gascan daemon::tests -- --nocapture
rtk cargo test -p gascand socket -- --nocapture
```

Expected: PASS.

### Step 5: Commit

```sh
rtk git add crates/gascan/src/daemon.rs crates/gascan/src/lib.rs \
  crates/gascan/src/client.rs crates/gascan/Cargo.toml \
  crates/gascand/src/api.rs crates/gascand/src/socket.rs \
  crates/gascand/src/lib.rs
rtk git commit -m "feat: add attested daemon runtime state"
```

## Task 5: Implement the Daemon Supervisor

**Files:**

- Modify: `crates/gascan/src/daemon.rs`
- Modify: `crates/gascan/src/client.rs`
- Modify: `crates/gascan/src/lib.rs`

### Step 1: Test state classification without autostart

Introduce a testable endpoint/process abstraction and write failing tests for:

- stopped;
- running and current;
- running and outdated;
- running legacy daemon with no release field;
- running unhealthy daemon with contradictory identity;
- unreachable endpoint with a valid new record;
- unsafe endpoint/record state.

`inspect` must never spawn a process or delete state.

Run:

```sh
rtk cargo test -p gascan daemon::tests::classification -- --nocapture
```

Expected: FAIL.

### Step 2: Test idempotent start

Write tests proving:

- stopped starts exactly one daemon;
- current is a successful no-op;
- outdated is not accepted as current;
- two concurrent starts converge on one daemon after locking/reinspection;
- the spawned daemon uses the protected runtime directory as its CWD;
- readiness requires a current version and matching identity.

Run:

```sh
rtk cargo test -p gascan daemon::tests::start -- --nocapture
```

Expected: FAIL.

### Step 3: Test graceful and legacy stop

Write tests proving:

- stopped is a successful no-op;
- current uses the authenticated shutdown RPC;
- graceful timeout returns a typed error suggesting `--force`;
- legacy uses two identical endpoint attestations plus immediate process
  verification before `SIGTERM`;
- changed token, PID, executable, or start identity prevents signaling;
- automatic mode never sends a force signal;
- explicit force revalidates identity immediately and confirms exit.

Run:

```sh
rtk cargo test -p gascan daemon::tests::stop -- --nocapture
```

Expected: FAIL.

### Step 4: Implement supervisor transitions

Implement:

- `inspect`;
- `start`;
- `stop`;
- `restart`;
- `connect_current_or_recover`.

All mutating transitions acquire the lifecycle lock, re-inspect state, and
use bounded waits. Keep a connected client plus its validated daemon identity
together so later shutdown cannot accidentally use stale identity.

Refactor `Client::connect_or_start()` to delegate to
`connect_current_or_recover`. Treat an absent release version as legacy and
an exact version mismatch as outdated, not as a generic API-major error.

### Step 5: Implement stable daemon spawning

Set `Command::current_dir()` to the validated protected runtime directory.
Null stdin/stdout, preserve the current optional diagnostic stderr path, and
keep the existing 15-second readiness bound. Re-read state after spawn
instead of trusting the returned child PID.

### Step 6: Run focused client tests

```sh
rtk cargo test -p gascan client -- --nocapture
rtk cargo test -p gascan daemon -- --nocapture
```

Expected: PASS.

### Step 7: Commit

```sh
rtk git add crates/gascan/src/daemon.rs crates/gascan/src/client.rs \
  crates/gascan/src/lib.rs
rtk git commit -m "feat: supervise daemon lifecycle safely"
```

## Task 6: Add Public CLI Commands and Polished Output

**Files:**

- Modify: `crates/gascan/src/cli.rs`
- Modify: `crates/gascan/src/presentation.rs`
- Modify: `crates/gascan/src/daemon.rs`

### Step 1: Write failing parser and output tests

Add parser tests for:

```text
gascan daemon status [--json]
gascan daemon start [--json]
gascan daemon stop [--force] [--json]
gascan daemon restart [--force] [--json]
```

Prove `--force` is rejected for status/start and the hidden
`daemon-attest` command remains hidden.

Add presentation tests for:

- stopped;
- healthy current;
- outdated;
- unreachable/unhealthy;
- human fields: health, PID, uptime, installed version, running version,
  executable;
- stable JSON nullability and lifecycle transition fields;
- force warning;
- no human progress bytes in JSON mode.

Run:

```sh
rtk cargo test -p gascan cli::tests::daemon -- --nocapture
rtk cargo test -p gascan presentation::tests::daemon -- --nocapture
```

Expected: FAIL.

### Step 2: Route management commands before ordinary autostart

Parse and execute `daemon` and `daemon-attest` before calling the ordinary
connection path. Management commands must not load a project manifest or
require a valid workspace.

Map supervisor typed errors into stable CLI error codes and actionable
suggestions. `stop --force` and `restart --force` must emit a clear human
warning before force is actually used.

### Step 3: Add automatic-recovery progress

Let the supervisor report a typed recovery transition to the CLI presentation
layer. Human mode renders one updating progress item:

```text
Restarting outdated Gascan daemon…
```

JSON mode suppresses that presentation completely and prints only the
ordinary command's documented JSON response.

### Step 4: Run CLI tests

```sh
rtk cargo test -p gascan
```

Expected: PASS.

### Step 5: Commit

```sh
rtk git add crates/gascan/src/cli.rs crates/gascan/src/presentation.rs \
  crates/gascan/src/daemon.rs
rtk git commit -m "feat: add public daemon management commands"
```

## Task 7: Prove Upgrade Recovery and Footgun Resistance End to End

**Files:**

- Modify: `crates/gascan-e2e/tests/autostart.rs`
- Modify: `crates/gascan-e2e/tests/doctor.rs`
- Modify: `crates/gascan-e2e/Cargo.toml` only if another fixture binary is
  required
- Modify: `crates/gascand/src/main.rs` for a debug-only E2E release override
  only if dependency injection cannot cover the scenario

### Step 1: Add management-command E2E tests

Using the existing fake-backend binaries and isolated runtime root, prove:

- `daemon status --json` reports stopped without autostart;
- start is idempotent;
- stop is idempotent;
- restart replaces the PID and returns healthy;
- status works from a directory unrelated to any project;
- status/stop work after the daemon's original launch directory is deleted.

Run:

```sh
rtk cargo test -p gascan-e2e --test autostart daemon_ -- --nocapture
```

Expected: FAIL before the CLI wiring is complete, then PASS.

### Step 2: Add Brew-style outdated-daemon recovery

Create a test fixture daemon that reports a deliberately different release
while retaining the current compatible API and full legacy attestation. Prove
that an ordinary `doctor --json` invocation:

1. detects the old release;
2. gracefully terminates it through the supported path, or uses the
   double-attested legacy `SIGTERM` path when the version/RPC is absent;
3. starts the current E2E daemon;
4. returns only valid Doctor JSON;
5. leaves exactly one live daemon.

Also test a held durable operation: automatic recovery must time out
actionably and must not force-kill it.

### Step 3: Add adversarial safety E2E tests

Prove:

- a forged instance file cannot cause an unrelated test-owned sleep process
  to receive a signal;
- changing the instance token between attestations aborts;
- replacing the endpoint between inspection and shutdown aborts;
- an unsafe symlink/socket path fails closed.

Every test must own and clean up its fixture processes.

### Step 4: Run E2E tests

```sh
rtk cargo test -p gascan-e2e --test autostart -- --nocapture
rtk cargo test -p gascan-e2e --test doctor -- --nocapture
```

Expected: PASS.

### Step 5: Commit

```sh
rtk git add crates/gascan-e2e/tests/autostart.rs \
  crates/gascan-e2e/tests/doctor.rs crates/gascan-e2e/Cargo.toml \
  crates/gascand/src/main.rs
rtk git commit -m "test: prove daemon upgrade recovery"
```

Only stage files that were actually required.

## Task 8: Document Daemon Lifecycle UX

**Files:**

- Modify: `README.md`
- Modify: `tests/release/documentation-contract.sh`

### Step 1: Write failing documentation contract assertions

Require the README to name:

- on-demand per-user daemon behavior;
- automatic replacement after upgrades;
- all four `gascan daemon` commands;
- graceful default and `--force` interruption risk;
- status and JSON examples;
- Doctor's caller-workspace semantics.

Run:

```sh
rtk bash tests/release/documentation-contract.sh
```

Expected: FAIL.

### Step 2: Update README

Add a concise daemon-management subsection near the commands/troubleshooting
material and add the command group to the command reference. Keep Quickstart
focused; mention only that ordinary commands normally start and upgrade the
daemon automatically.

### Step 3: Verify documentation

```sh
rtk bash tests/release/documentation-contract.sh
```

Expected: PASS.

### Step 4: Commit

```sh
rtk git add README.md tests/release/documentation-contract.sh
rtk git commit -m "docs: explain daemon management"
```

## Task 9: Full Verification and Review

**Files:**

- Modify only files required by verified failures.

### Step 1: Format and lint

```sh
rtk cargo fmt --all -- --check
rtk cargo clippy --workspace --all-targets -- -D warnings
```

Expected: PASS.

If formatting fails, run `rtk cargo fmt --all`, inspect the diff, and rerun
the check.

### Step 2: Run the complete workspace

```sh
rtk cargo test --workspace
```

Expected: PASS with only the repository's intentional ignored tests.

### Step 3: Run release contracts affected by the change

```sh
rtk bash tests/release/documentation-contract.sh
rtk bash tests/release/installer-contract.sh
rtk bash tests/release/clean-host-contract.sh
```

Expected: PASS. The installer contract is important because hidden daemon
attestation remains part of release cleanup.

### Step 4: Review the diff and history

```sh
rtk git diff --check
rtk git status --short
rtk git log --oneline origin/main..HEAD
rtk git diff --stat origin/main...HEAD
```

Expected: no unstaged changes, no whitespace errors, and only scoped feature
commits.

### Step 5: Request code review

Use `superpowers:requesting-code-review`. Address findings through
`superpowers:receiving-code-review`, rerun focused tests after each repair,
then rerun the full verification above.

### Step 6: Final verification commit if needed

If review requires changes:

```sh
rtk git add <reviewed-files>
rtk git commit -m "fix: harden daemon management"
```

Do not create a PR, merge, bump a version, tag, or publish a release unless
the user separately requests those actions after reviewing the completed
implementation.
