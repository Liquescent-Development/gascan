# SSH Readiness and Diagnostics Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make strict native SSH activation tolerate bounded transition delays, preserve actionable OpenSSH failures, show precise doctor findings, and clean obsolete managed host-key generations safely.

**Architecture:** `gascand::ssh::manager` owns a retry policy and captures a bounded diagnostic tail while preserving exact strict SSH arguments. Managed generation cleanup remains in `ssh::config`, and doctor/presentation report the precise managed-state invariant rather than replacing it with generic prose.

**Tech Stack:** Rust 1.95, Tokio process management, OpenSSH, content-addressed managed files, rustix no-follow filesystem operations, tonic error details, Cargo integration tests.

## Global Constraints

- SSH remains bound to host IPv4 loopback.
- Readiness retains `StrictHostKeyChecking=yes`, `IdentitiesOnly=yes`, `BatchMode=yes`, disabled forwarding, exact alias, identity, and immutable known-hosts generation.
- All retries use the same prepared SSH evidence.
- The readiness deadline is exactly 15 seconds in production.
- Captured diagnostics are bounded, UTF-8 safe, and never include private-key contents or environment dumps.
- Conventional user `~/.ssh/config` mode 0644 remains accepted.
- Gas Can-managed private state retains strict ownership, link, type, and mode checks.
- Generation cleanup occurs only after durable config publication.

---

### Task 1: Retry strict SSH readiness and capture bounded diagnostics

**Files:**
- Modify: `crates/gascand/src/ssh/manager.rs`
- Modify: `crates/gascand/tests/ssh_config.rs`

**Interfaces:**
 - Consumes: `readiness_ssh_args`, configured absolute readiness program.
 - Produces:
   - `SshReadinessPolicy { deadline, retry_delay, maximum_stderr }`
   - `run_readiness(program: &OsStr, args: &[OsString], endpoint: &str, policy: SshReadinessPolicy) -> Result<(), ServiceError>`
  - `SshManager::prepare_activation_for_paths_with_policy(id: &SandboxId, runtime: &impl RuntimeBackend, expected: Option<&SshResolution>, paths: &SshPaths, readiness_program: &Utf8Path, host_key_timeout: Duration, policy: SshReadinessPolicy) -> Result<Option<PreparedSshActivation>, ServiceError>` for deterministic integration tests.

- [ ] **Step 1: Add a failing transient-readiness test**

Use a temporary executable script that increments a counter, writes
`connection refused` to stderr for its first two executions, and exits zero on
the third. Call
`SshManager::prepare_activation_for_paths_with_policy` with a 500 ms deadline
and 10 ms retry delay, using the existing fake runtime and managed SSH
fixtures. Assert:

```rust
let activation = manager
    .prepare_activation_for_paths_with_policy(
        &id,
        &runtime,
        Some(&resolution),
        &paths,
        &program,
        Duration::from_secs(1),
        SshReadinessPolicy {
            deadline: Duration::from_millis(500),
            retry_delay: Duration::from_millis(10),
            maximum_stderr: 128,
        },
    )
    .await?;
assert!(activation.is_some());
assert_eq!(std::fs::read_to_string(counter)?.trim(), "3");
```

Also record every argv invocation and assert it is byte-for-byte identical.

- [ ] **Step 2: Add failing permanent-error and truncation tests**

Use a fake readiness program that always writes:

```text
Host key verification failed.
```

and exits 255. Assert the resulting `ServiceError` contains the endpoint,
deadline, final OpenSSH detail, and:

```text
Run `gascan doctor` for managed SSH configuration details.
```

Add invalid UTF-8 plus output larger than the configured maximum. Assert the
tail is lossy-decoded, truncated on a character boundary, and bounded.

- [ ] **Step 3: Run focused tests and verify RED**

Run:

```bash
rtk cargo test --locked -p gascand --test ssh_config readiness
```

Expected: the first nonzero command exits immediately and stderr is absent.

- [ ] **Step 4: Implement the readiness policy**

Add:

```rust
#[derive(Clone, Copy)]
pub struct SshReadinessPolicy {
    pub deadline: Duration,
    pub retry_delay: Duration,
    pub maximum_stderr: usize,
}

impl Default for SshReadinessPolicy {
    fn default() -> Self {
        Self {
            deadline: Duration::from_secs(15),
            retry_delay: Duration::from_millis(100),
            maximum_stderr: 4096,
        }
    }
}
```

Keep `SshManager` stateless. The existing production activation and reconcile
methods pass `SshReadinessPolicy::default()`. Add the explicit
`prepare_activation_for_paths_with_policy` seam above for tests; the existing
`prepare_activation_for_paths` delegates to it with the default policy.

- [ ] **Step 5: Implement one absolute-deadline retry loop**

Use `Command::output()` with null stdin/stdout and piped stderr. For each
attempt, rebuild the command from the unchanged `program` and `args`.

Pseudocode:

```rust
let deadline = Instant::now() + policy.deadline;
loop {
    let remaining = deadline.saturating_duration_since(Instant::now());
    let output = timeout(remaining, command.output()).await;
    match output {
        Ok(Ok(output)) if output.status.success() => return Ok(()),
        Ok(Ok(output)) => last_detail = bounded_tail(&output.stderr, policy.maximum_stderr),
        Ok(Err(error)) => return Err(could_not_start(error)),
        Err(_) => return Err(timed_out(last_detail)),
    }
    if Instant::now() >= deadline {
        return Err(deadline_error(last_detail));
    }
    sleep(policy.retry_delay.min(deadline.saturating_duration_since(Instant::now()))).await;
}
```

Do not add `-v`; normal OpenSSH failures already write useful stderr.

- [ ] **Step 6: Run focused tests and verify GREEN**

Run:

```bash
rtk cargo test --locked -p gascand --test ssh_config readiness
rtk cargo test --locked -p gascand --test reconcile ssh
```

Expected: all pass.

- [ ] **Step 7: Commit**

```bash
rtk git add crates/gascand/src/ssh/manager.rs crates/gascand/tests/ssh_config.rs
rtk git commit -m "fix: retry strict SSH readiness with diagnostics"
```

---

### Task 2: Preserve actionable SSH errors through the API and CLI

**Files:**
- Modify: `crates/gascand/src/service.rs`
- Modify: `crates/gascand/src/api.rs`
- Modify: `crates/gascand/tests/ssh_config.rs`
- Modify: `crates/gascan/src/cli.rs`
- Modify: `crates/gascan/src/presentation.rs`

**Interfaces:**
- Consumes: current stable `ssh_not_ready` error code and error-detail cause transport.
- Produces:
  - owned, structured `SshNotReady` error detail;
  - human error containing precise cause plus doctor instruction;
  - unchanged stable error code for JSON consumers.

- [ ] **Step 1: Add failing error-transport tests**

Assert a permanent readiness failure crosses tonic with:

```rust
assert_eq!(status.message(), gascan_proto::error_code::SSH_NOT_READY);
assert!(decoded_cause.contains("127.0.0.1:2222"));
assert!(decoded_cause.contains("Host key verification failed"));
assert!(decoded_cause.contains("gascan doctor"));
```

Add a CLI rendering assertion that the stable code is not shown in place of
the human cause.

- [ ] **Step 2: Run tests and verify RED**

Run:

```bash
rtk cargo test --locked -p gascand --test ssh_config permanent
rtk cargo test --locked -p gascan cli::tests::ssh
```

Expected: only `strict SSH readiness command failed` is available.

- [ ] **Step 3: Make `SshNotReady` own precise detail**

Replace the static-string variant with:

```rust
SshNotReady {
    endpoint: Option<String>,
    detail: String,
}
```

Keep `ServiceError::code()` mapped to `SSH_NOT_READY`. Update existing call
sites to use `endpoint: None` with their current precise detail. Readiness
failure uses `Some("127.0.0.1:2222")`.

- [ ] **Step 4: Preserve bounded detail through tonic and rendering**

Use the existing error-detail cause channel; do not add a protobuf field.
Ensure both human and JSON paths retain the stable code, while human output
prints the cause and doctor instruction.

- [ ] **Step 5: Run focused tests**

Run the commands from Step 2.

Expected: all pass.

- [ ] **Step 6: Commit**

```bash
rtk git add crates/gascand/src/service.rs crates/gascand/src/api.rs \
  crates/gascand/tests/ssh_config.rs crates/gascan/src/cli.rs \
  crates/gascan/src/presentation.rs
rtk git commit -m "fix: explain native SSH readiness failures"
```

---

### Task 3: Show exact SSH doctor findings

**Files:**
- Modify: `crates/gascand/src/doctor.rs`
- Modify: `crates/gascand/tests/doctor_state.rs`
- Modify: `crates/gascan-core/src/doctor.rs`
- Modify: `crates/gascan/src/presentation.rs`

**Interfaces:**
- Consumes: detailed `DoctorFact` values already generated for identity and config.
- Produces:
  - `DoctorFact { status, detail, remedy: Option<String> }`
  - `DoctorFact::with_remedy(remedy) -> DoctorFact`
  - exact human and JSON detail plus state-specific remedy.

- [ ] **Step 1: Add failing presentation regressions**

Construct checks containing:

```text
generated SSH config at /Users/test/.config/gascan/ssh/config is missing while durable or generated SSH state exists
```

Assert human output includes that entire detail and does not contain:

```text
Managed SSH configuration is missing, inconsistent, or unsafe
```

Add unsafe-mode and durable-state-mismatch cases.
In `doctor_state.rs`, assert the serialized `Capability` structured detail and
the decoded JSON doctor response preserve the same exact detail and selected
remedy.

- [ ] **Step 2: Add failing remedy tests**

Assert:

- missing/inconsistent generated state recommends `gascan up`;
- unsafe managed state names the path that must be repaired or removed;
- absent not-yet-created state remains a pass;
- `~/.ssh/config` at 0644 remains accepted by the client-side include manager.

- [ ] **Step 3: Run focused tests and verify RED**

Run:

```bash
rtk cargo test --locked -p gascan presentation::tests::doctor
rtk cargo test --locked -p gascand --test doctor_state ssh
rtk cargo test --locked -p gascan --test ssh_config
```

Expected: the formatter replaces exact SSH detail with generic prose.

- [ ] **Step 4: Remove detail suppression and select precise remedies**

Delete the `human_doctor_detail` special cases for `ssh.identity` and
`ssh.config`. Preserve the daemon-provided detail verbatim after existing
bounded validation.

Extend `DoctorFact` with an optional remedy override:

```rust
pub struct DoctorFact {
    pub status: DoctorStatus,
    pub detail: String,
    pub remedy: Option<String>,
}

pub fn with_remedy(mut self, remedy: impl Into<String>) -> Self {
    self.remedy = Some(remedy.into());
    self
}
```

Every existing constructor initializes `remedy: None`.
`DoctorFacts::into_report` uses
`fact.remedy.unwrap_or_else(|| default.to_owned())`. Do not parse arbitrary
human prose to select a remedy. Instead, classify managed SSH state with:

```rust
enum SshDoctorCondition {
    Ready,
    NotCreated,
    Missing,
    Inconsistent,
    Unsafe,
    TransitionPending,
}
```

Give `SshDoctorCondition` methods
`detail(&self, paths: &SshPaths) -> String` and
`remedy(&self, paths: &SshPaths) -> String`, then construct each fact as:

```rust
DoctorFact {
    status: condition.status(),
    detail: condition.detail(paths),
    remedy: Some(condition.remedy(paths)),
}
```

Add `status(&self) -> DoctorStatus` to the enum: `Ready` and `NotCreated`
return `Pass`; `Missing`, `Inconsistent`, `Unsafe`, and `TransitionPending`
return `Fail`. `Missing`, `Inconsistent`, and `TransitionPending` recommend
`gascan up`; `Unsafe` names the exact managed path to repair or remove.

- [ ] **Step 5: Run focused tests and verify GREEN**

Run the commands from Step 3.

Expected: all pass.

- [ ] **Step 6: Commit**

```bash
rtk git add crates/gascand/src/doctor.rs crates/gascand/tests/doctor_state.rs \
  crates/gascan-core/src/doctor.rs crates/gascan/src/presentation.rs \
  crates/gascan/tests/ssh_config.rs
rtk git commit -m "fix: show precise managed SSH doctor findings"
```

---

### Task 4: Garbage-collect obsolete known-hosts generations safely

**Files:**
- Modify: `crates/gascand/src/ssh/config.rs`
- Modify: `crates/gascand/src/ssh.rs`
- Modify: `crates/gascand/src/ssh/manager.rs`
- Modify: `crates/gascand/src/doctor.rs`
- Modify: `crates/gascand/tests/ssh_config.rs`
- Modify: `crates/gascand/tests/doctor_state.rs`

**Interfaces:**
- Consumes: content-addressed `known_hosts.<64 lowercase hex>` naming and publication lock.
- Produces:
  - `prune_known_hosts_generations(paths: &SshPaths, retained: &BTreeSet<String>) -> Result<GenerationCleanup, SshError>`
  - stale-generation count for nonblocking doctor warnings.

- [ ] **Step 1: Add failing safe-cleanup tests**

Create active, obsolete, malformed, symlink, and foreign/hard-linked fixtures.
After durable publication assert:

- active generation remains;
- valid obsolete regular generations are removed;
- malformed names are untouched and reported unsafe;
- symlinks are never followed;
- cleanup does not run when config commit is fault-injected before durability.

- [ ] **Step 2: Add failing cleanup-retry doctor tests**

Inject a cleanup failure after successful commit. Assert publication succeeds,
doctor returns a nonblocking SSH config warning naming the number of stale
generations, and a later reconciliation removes them.

- [ ] **Step 3: Run focused tests and verify RED**

Run:

```bash
rtk cargo test --locked -p gascand --test ssh_config generation
rtk cargo test --locked -p gascand --test doctor_state obsolete
```

Expected: obsolete generations remain and no warning exists.

- [ ] **Step 4: Implement no-follow generation enumeration**

Under the existing publication lock, enumerate directory entries. Accept only
names matching `known_hosts.` plus 64 lowercase hex characters. For deletion:

- inspect with `SYMLINK_NOFOLLOW`;
- require regular file, current UID, mode 0644, and one link;
- retain every explicitly supplied generation;
- unlink by directory file descriptor;
- fsync the directory after any removal.

Return:

```rust
pub struct GenerationCleanup {
    pub removed: usize,
    pub stale: usize,
    pub unsafe_entries: usize,
}
```

- [ ] **Step 5: Invoke cleanup only after durable commit**

Call cleanup after `commit_openssh_files` has completed its rename and directory
sync. Reconciliation retries cleanup. A cleanup error cannot roll back or fail
the already-working SSH publication.

- [ ] **Step 6: Surface stale valid generations as a warning**

Doctor validates active publication first. If it is exact but valid stale
generations remain, return `DoctorFact::warning` with count and managed
directory. Unsafe entries remain a blocking failure rather than a warning.

- [ ] **Step 7: Run focused tests and verify GREEN**

Run the commands from Step 3 plus:

```bash
rtk cargo test --locked -p gascand --test reconcile ssh
```

Expected: all pass.

- [ ] **Step 8: Commit**

```bash
rtk git add crates/gascand/src/ssh crates/gascand/src/doctor.rs \
  crates/gascand/tests/ssh_config.rs crates/gascand/tests/doctor_state.rs
rtk git commit -m "fix: prune obsolete managed SSH host keys"
```

---

### Task 5: Verify connected behavior and document recovery

**Files:**
- Modify: `README.md`
- Modify: `docs/release/macos-checklist.md`
- Modify: `crates/gascan-e2e/tests/apple_apply.rs`

**Interfaces:**
- Consumes: final SSH errors, doctor details, generation cleanup.
- Produces: user guidance and connected release evidence.

- [ ] **Step 1: Extend the Apple SSH scenario**

Prove:

- strict readiness succeeds with exact host-key checking;
- nested `gascan ssh` works when Gas Can is invoked from an SSH-like
  non-GUI environment;
- a transient readiness failure is retried without changing identity or host
  key;
- final status is `ssh.state == "ready"`;
- only the referenced known-hosts generation remains.

Use the existing E2E fake readiness hook for injected transient failure; do not
weaken the live Apple host-key test.

- [ ] **Step 2: Document actionable recovery**

Explain:

- sandbox SSH is loopback-only;
- `gascan ssh` works inside an SSH session to the Mac;
- direct remote access to port 2222 is intentionally unavailable;
- readiness errors include OpenSSH detail and direct users to `gascan doctor`;
- doctor names exact managed-state problems.

- [ ] **Step 3: Run complete SSH and release-focused verification**

Run:

```bash
rtk cargo fmt --all -- --check
rtk cargo test --locked -p gascand --test ssh_config
rtk cargo test --locked -p gascand --test doctor_state
rtk cargo test --locked -p gascand --test reconcile
rtk cargo test --locked -p gascan --test ssh_config
rtk cargo test --locked -p gascan --test ssh_cli
rtk cargo test --locked -p gascan presentation::tests
rtk git diff --check
```

Then run the repository's connected image gate and Apple SSH E2E scenario
against the already-approved workspace image. Expected: all pass with strict
host-key verification intact.

- [ ] **Step 4: Commit**

```bash
rtk git add README.md docs/release/macos-checklist.md crates/gascan-e2e/tests
rtk git commit -m "docs: explain native SSH diagnostics and remote use"
```

---

### Task 6: Combined branch verification

**Files:**
- Verify only; modify source only for defects exposed by these gates.

**Interfaces:**
- Consumes: completed runtime compatibility plan and Tasks 1-5 above.
- Produces: merge-ready evidence for one patch release.

- [ ] **Step 1: Run formatting and static checks**

```bash
rtk cargo fmt --all -- --check
rtk cargo clippy --locked --workspace --all-targets -- -D warnings
rtk git diff --check
```

- [ ] **Step 2: Run the full workspace suite**

```bash
rtk cargo test --locked --workspace -- --skip recovery_observer_starts_before_gated_outdated_shutdown
```

Expected: all applicable tests pass. Run outside the filesystem sandbox when
daemon tests require process or Unix-socket inspection. Record the separately
known hanging test exclusion explicitly.

- [ ] **Step 3: Run release tooling tests**

```bash
rtk cargo test --locked --manifest-path scripts/Cargo.toml
```

Expected: all pass; localhost-listener tests may require running outside the
filesystem sandbox.

- [ ] **Step 4: Run connected Apple gates**

Run the connected workspace image gate with the approved immutable image, then
the Apple apply/SSH E2E scenario. Expected: networked lifecycle, strict SSH,
shell, offline certified behavior, and cleanup pass.

- [ ] **Step 5: Inspect final branch**

```bash
rtk git status --short --branch
rtk git log --show-signature --oneline origin/main..HEAD
rtk git diff --stat origin/main...HEAD
rtk git diff --check origin/main...HEAD
```

Expected: clean branch, valid signed commits, only approved feature/spec/plan
changes, and no generated artifacts.
