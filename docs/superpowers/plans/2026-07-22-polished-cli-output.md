# Polished CLI Output Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace Gascan's raw lifecycle and diagnostic text with polished interactive progress, deterministic redirected output, concise diagnostics, and actionable human errors while preserving JSON and exit-code contracts.

**Architecture:** Add a focused `presentation` module that converts structured protocol values into semantic human messages. Lifecycle commands feed events into a small stateful presenter backed by `indicatif` for interactive terminals and returned static lines for redirected stderr; inspection commands use pure string renderers. CLI handlers retain API calls, JSON serialization, and exit-code ownership.

**Tech Stack:** Rust 1.85, Tokio/Tonic, `indicatif` 0.18.6, `console` 0.16.4, existing fake-backend and PTY test infrastructure.

## Global Constraints

- Interactive styling and animation are automatic and only appear on a terminal.
- Redirected human output is non-animated, uncolored, ASCII-safe, deterministic, and contains no cursor controls.
- `NO_COLOR` disables color without disabling interactive animation or Unicode symbols.
- Existing JSON/JSON Lines structures and exit codes do not change.
- Lifecycle progress remains on stderr; `doctor`, `status`, and `list` remain on stdout; errors remain on stderr.
- Opaque event payloads are never parsed for human output.
- `run`, `shell`, `logs`, daemon behavior, and protobuf schemas are out of scope.
- Existing edits under `.superpowers/sdd/` belong to the user and must not be staged or modified.

## File Structure

- Create `crates/gascan/src/presentation.rs`: terminal capabilities, semantic operation progress, doctor/status/list rendering, and human error layout.
- Modify `crates/gascan/src/main.rs`: register the presentation module and route terminal errors through the human error renderer.
- Modify `crates/gascan/src/cli.rs`: pass operation kinds and known sandbox IDs, call presentation functions, retain JSON serialization, and expose structured error information.
- Modify `crates/gascan/src/client.rs`: expose stable RPC codes and decoded causes without changing RPC classification.
- Modify `Cargo.toml`, `crates/gascan/Cargo.toml`, `crates/gascan-e2e/Cargo.toml`, and `Cargo.lock`: add progress dependencies to both production CLI package graphs.
- Modify `crates/gascan-e2e/tests/doctor.rs`: assert concise grouped doctor output and unchanged JSON.
- Modify `crates/gascan-e2e/tests/fake_backend.rs`: assert lifecycle, status/list, error, JSON, and PTY output contracts.
- Modify `README.md`: document automatic interactive/static output and `NO_COLOR`.

---

### Task 1: Semantic lifecycle presentation core

**Files:**
- Create: `crates/gascan/src/presentation.rs`
- Modify: `crates/gascan/src/main.rs:4-7`
- Modify: `Cargo.toml:8-31`
- Modify: `crates/gascan/Cargo.toml:9-28`
- Modify: `crates/gascan-e2e/Cargo.toml:14-34`
- Modify: `Cargo.lock`

**Interfaces:**
- Produces: `OperationKind::{Up, Apply, Down, Destroy}`.
- Produces: `OutputCapabilities { interactive: bool, color: bool, unicode: bool }` plus `for_stdout()`, `for_stderr()`, and a test-only explicit constructor.
- Produces: `OperationProgress::new(kind, sandbox_id, capabilities) -> (OperationProgress, Option<String>)`.
- Produces: `OperationProgress::update(&mut self, event: &v1::OperationEvent) -> Option<String>`.
- Produces: `OperationProgress::finish_success(self) -> Option<String>` and `clear(&mut self)`.

- [ ] **Step 1: Write failing phase-mapping and static-mode tests**

Add tests in `presentation.rs` that create `OutputCapabilities { interactive: false, color: false, unicode: false }`, then assert:

```rust
#[test]
fn static_up_uses_semantic_messages_and_suppresses_plumbing() {
    let capabilities = OutputCapabilities::plain();
    let (mut progress, initial) = OperationProgress::new(OperationKind::Up, None, capabilities);
    assert_eq!(initial.as_deref(), Some("Preparing sandbox"));
    assert_eq!(progress.update(&event("operation")), None);
    assert_eq!(progress.update(&event("validated")).as_deref(), Some("Validating configuration"));
    assert_eq!(progress.update(&event("created")).as_deref(), Some("Creating sandbox"));
    assert_eq!(progress.update(&event("started")).as_deref(), Some("Starting sandbox"));
    assert_eq!(progress.update(&event("before_provision")), None);
    assert_eq!(progress.update(&event("after_provision")), None);
    assert_eq!(progress.update(&event("before_health")).as_deref(), Some("Checking sandbox health"));
    assert_eq!(progress.update(&event("after_health")), None);
    assert_eq!(progress.finish_success().as_deref(), Some("Sandbox is running"));
}

#[test]
fn provision_steps_are_typed_human_copy_and_deduplicated() {
    let (mut progress, _) = OperationProgress::new(OperationKind::Apply, None, OutputCapabilities::plain());
    let install = provision_event(v1::ProvisionStep::InstallTools);
    assert_eq!(progress.update(&install).as_deref(), Some("Installing project tools"));
    assert_eq!(progress.update(&install), None);
    assert_eq!(progress.update(&provision_event(v1::ProvisionStep::RunSetup)).as_deref(), Some("Running project setup"));
    assert_eq!(progress.update(&provision_event(v1::ProvisionStep::VerifyGascamp)).as_deref(), Some("Verifying Gascamp"));
    assert_eq!(progress.update(&provision_event(v1::ProvisionStep::HealthCheck)).as_deref(), Some("Checking sandbox health"));
}

#[test]
fn opaque_payload_and_unknown_phases_never_reach_human_output() {
    let (mut progress, _) = OperationProgress::new(OperationKind::Up, None, OutputCapabilities::plain());
    let mut unknown = event("private_internal_phase");
    unknown.payload = b"secret-material".to_vec();
    assert_eq!(progress.update(&unknown), None);
}

#[test]
fn known_selector_is_used_only_in_completion_copy() {
    let (progress, _) = OperationProgress::new(OperationKind::Down, Some("code-123".to_owned()), OutputCapabilities::plain());
    assert_eq!(progress.finish_success().as_deref(), Some("Sandbox code-123 is stopped"));
}
```

The test helpers construct `v1::OperationEvent` values only from `phase` and `provision_step`; they never decode `payload`.

- [ ] **Step 2: Run the focused test and verify RED**

Run: `rtk cargo test -p gascan presentation -- --nocapture`

Expected: compilation fails because `presentation.rs`, `OperationKind`, `OutputCapabilities`, and `OperationProgress` do not exist.

- [ ] **Step 3: Add progress dependencies and implement the static semantic core**

Add these workspace dependencies:

```toml
console = "0.16.4"
indicatif = { version = "0.18.6", features = ["in_memory"] }
```

Add both as workspace dependencies in `crates/gascan/Cargo.toml` and `crates/gascan-e2e/Cargo.toml`, because the e2e binary compiles `crates/gascan/src/main.rs` directly. Register `mod presentation;` in `main.rs`.

Implement the interfaces above. The exact semantic mapping is:

```rust
match (event.phase.as_str(), v1::ProvisionStep::try_from(event.provision_step).ok()) {
    ("validated", _) => Some("Validating configuration"),
    ("created", _) => Some("Creating sandbox"),
    ("started", _) => Some("Starting sandbox"),
    ("apply_required", _) => Some("Preparing configuration changes"),
    ("before_health", _) => Some("Checking sandbox health"),
    ("provision_step", Some(v1::ProvisionStep::WriteSafeMiseConfig)) => Some("Writing safe mise configuration"),
    ("provision_step", Some(v1::ProvisionStep::InstallTools)) => Some("Installing project tools"),
    ("provision_step", Some(v1::ProvisionStep::RunSetup)) => Some("Running project setup"),
    ("provision_step", Some(v1::ProvisionStep::VerifyGascamp)) => Some("Verifying Gascamp"),
    ("provision_step", Some(v1::ProvisionStep::HealthCheck)) => Some("Checking sandbox health"),
    _ => None,
}
```

`OperationProgress` stores the last semantic message and suppresses repeats. In plain mode it returns each new line to the caller. Initial and completion copy are:

| Kind | Initial | Completion without ID | Completion with ID |
| --- | --- | --- | --- |
| Up | Preparing sandbox | Sandbox is running | Sandbox `{id}` is running |
| Apply | Applying configuration | Sandbox configuration is up to date | Sandbox `{id}` configuration is up to date |
| Down | Stopping sandbox | Sandbox is stopped | Sandbox `{id}` is stopped |
| Destroy | Destroying sandbox | Sandbox is destroyed | Sandbox `{id}` is destroyed |

- [ ] **Step 4: Run the focused tests and verify GREEN**

Run: `rtk cargo test -p gascan presentation -- --nocapture`

Expected: all new semantic/static tests pass.

- [ ] **Step 5: Commit Task 1**

```bash
rtk git add Cargo.toml Cargo.lock crates/gascan/Cargo.toml crates/gascan-e2e/Cargo.toml crates/gascan/src/main.rs crates/gascan/src/presentation.rs
rtk git commit -m "feat: add semantic CLI presentation core"
```

---

### Task 2: Interactive spinner lifecycle and command integration

**Files:**
- Modify: `crates/gascan/src/presentation.rs`
- Modify: `crates/gascan/src/cli.rs:168-225,320-362,664-686`
- Test: `crates/gascan/src/presentation.rs`
- Test: `crates/gascan-e2e/tests/fake_backend.rs`

**Interfaces:**
- Consumes: Task 1's `OperationKind`, `OutputCapabilities`, and `OperationProgress`.
- Produces: `OperationProgress::with_draw_target(kind, sandbox_id, capabilities, ProgressDrawTarget)` for deterministic interactive tests.
- Changes: `operation(stream, json, kind, sandbox_id) -> Result<i32, CliError>`.

- [ ] **Step 1: Write failing interactive and non-TTY lifecycle tests**

In `presentation.rs`, use `indicatif::InMemoryTerm` and `ProgressDrawTarget::term_like_with_hz` to verify an interactive presenter replaces its active message, finishes with `✓`, and clears on drop without completion. Explicitly create a `color: false, unicode: true` capability to prove animation survives the no-color setting.

Also add `interactive_lifecycle_progress_updates_in_place_and_finishes_cleanly` to `fake_backend.rs` using `rustix_openpty`. Connect the child CLI's stderr to the PTY, run fake-backend `up`, read to EOF, and assert that output contains a Braille spinner frame, the semantic progress messages, an in-place redraw control, and a final newline-terminated `✓ Sandbox is running` line. Run a second invocation with `NO_COLOR=1`; assert spinner frames and `✓` remain while ANSI SGR color sequences are absent.

In `fake_backend.rs`, extend `complete_cli_lifecycle_uses_daemon_api` with exact non-TTY assertions:

```rust
let up = env.invoke(&["up", env.root()?])?;
let stderr = String::from_utf8(up.stderr)?;
assert!(stderr.contains("Preparing sandbox\n"));
assert!(stderr.contains("Validating configuration\n"));
assert!(stderr.contains("Sandbox is running\n"));
for raw in ["operation", "before_provision", "after_provision", "provision_step"] {
    assert!(!stderr.contains(raw), "raw phase leaked: {raw}");
}
assert!(!stderr.contains('\u{1b}'));
```

Add equivalent outcome assertions for `down` and `destroy`, including their known sandbox IDs.

- [ ] **Step 2: Run focused tests and verify RED**

Run:

```bash
rtk cargo test -p gascan presentation -- --nocapture
rtk cargo test -p gascan-e2e complete_cli_lifecycle_uses_daemon_api -- --nocapture
rtk cargo test -p gascan-e2e interactive_lifecycle_progress_updates_in_place_and_finishes_cleanly -- --nocapture
```

Expected: the interactive constructor is missing, non-TTY lifecycle output still contains raw phases, and the PTY contract lacks polished progress.

- [ ] **Step 3: Implement interactive progress and wire lifecycle commands**

For production interactive stderr, construct the draw target with:

```rust
ProgressDrawTarget::term_like_with_hz(Box::new(console::Term::stderr()), 12)
```

Using `term_like` is deliberate: `NO_COLOR` must disable style only, not hide the progress indicator. Use `ProgressStyle::with_template("{spinner:.cyan} {msg}")` only when `capabilities.color`; otherwise use `"{spinner} {msg}"`. Use the tick sequence `⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏`. Start steady ticking at 80 ms. Finish with a green `✓` when color is enabled and an uncolored `✓` otherwise. `Drop` calls `finish_and_clear()` unless success already finished.

Replace `event_phase_label` and direct `eprintln!` calls in `cli.rs`. Each lifecycle branch passes its `OperationKind`; `down` and `destroy` clone `selector.sandbox_id` before moving the selector into the request. The JSON branch does not construct `OperationProgress`. Static lines returned from the presenter are written to stderr in event order.

If a JSON operation event contains an error, return `Ok(EXIT_RUNTIME)` after printing that event so `main` does not append human text. Human mode clears progress and returns the structured operation error.

- [ ] **Step 4: Run focused tests and verify GREEN**

Run:

```bash
rtk cargo test -p gascan presentation -- --nocapture
rtk cargo test -p gascan-e2e complete_cli_lifecycle_uses_daemon_api -- --nocapture
rtk cargo test -p gascan-e2e interactive_lifecycle_progress_updates_in_place_and_finishes_cleanly -- --nocapture
```

Expected: unit, static integration, and PTY integration tests pass; raw phases and ANSI are absent from captured non-TTY stderr.

- [ ] **Step 5: Commit Task 2**

```bash
rtk git add crates/gascan/src/presentation.rs crates/gascan/src/cli.rs crates/gascan-e2e/tests/fake_backend.rs
rtk git commit -m "feat: render polished lifecycle progress"
```

---

### Task 3: Concise doctor, status, and list rendering

**Files:**
- Modify: `crates/gascan/src/presentation.rs`
- Modify: `crates/gascan/src/cli.rs:226-280,608-646`
- Modify: `crates/gascan-e2e/tests/doctor.rs`
- Modify: `crates/gascan-e2e/tests/fake_backend.rs`

**Interfaces:**
- Produces: `DoctorCheck { id: String, status: String, detail: String, remedy: String }`.
- Produces: `render_doctor(checks: &[DoctorCheck], capabilities: OutputCapabilities) -> String`.
- Produces: `render_status(status: &v1::SandboxStatus, capabilities: OutputCapabilities) -> String`.
- Produces: `render_list(sandboxes: &[v1::SandboxStatus], capabilities: OutputCapabilities) -> String`.
- Keeps: existing `actual_name(i32) -> &'static str` for JSON lowercase state values.

- [ ] **Step 1: Write failing pure-renderer and e2e assertions**

Add pure renderer tests for an all-pass report, a mixed report, an unknown group, one/many grammar, status, a two-row aligned list, and an empty list. The core exact assertions are:

```rust
assert_eq!(render_doctor(&passing_checks(), OutputCapabilities::plain()),
    "Gascan is ready\n  Host       2/2 checks passed\n  Runtime    1/1 check passed\n");
assert_eq!(render_status(&running_status("code-123"), OutputCapabilities::plain()),
    "Sandbox: code-123\nState:   Running\n");
assert_eq!(render_list(&[], OutputCapabilities::plain()), "No sandboxes found.\n");
```

The mixed doctor assertion must include `Gascan needs attention`, `Offline`, its detail, and `Fix:`, while excluding detail from every passing check. In `doctor.rs`, assert human output contains category totals and does not contain `report sha256`, `fixture sha256`, or `runtime.offline`; retain all existing JSON field assertions.

- [ ] **Step 2: Run focused tests and verify RED**

Run:

```bash
rtk cargo test -p gascan presentation -- --nocapture
rtk cargo test -p gascan-e2e doctor -- --nocapture
```

Expected: renderer functions are missing and the current doctor integration still exposes internal evidence.

- [ ] **Step 3: Implement pure inspection renderers and wire them into CLI branches**

Parse each capability detail once into `DoctorCheck`, then use that same typed vector for JSON serialization and human rendering. Group by the substring before the first dot while preserving first-seen group order. Humanize group/check identifiers by replacing `_` with spaces and title-casing the first word. Render `✓`/`✗` headings only when `capabilities.unicode`; plain mode emits the same heading without a symbol. Apply color only through `console::Style` when `capabilities.color`.

For lists, compute the sandbox column width as `max("SANDBOX".len(), sandbox_id.len())`, then render `SANDBOX  STATE` and each row with two separating spaces. Title-case state only in human output. Delete the old human branches in `cli.rs`; keep their JSON expressions structurally unchanged.

- [ ] **Step 4: Run focused tests and verify GREEN**

Run:

```bash
rtk cargo test -p gascan presentation -- --nocapture
rtk cargo test -p gascan-e2e doctor -- --nocapture
```

Expected: all renderer and doctor integration tests pass; JSON checks still contain full detail/remedy data.

- [ ] **Step 5: Commit Task 3**

```bash
rtk git add crates/gascan/src/presentation.rs crates/gascan/src/cli.rs crates/gascan-e2e/tests/doctor.rs crates/gascan-e2e/tests/fake_backend.rs
rtk git commit -m "feat: polish inspection command output"
```

---

### Task 4: Consistent actionable human errors

**Files:**
- Modify: `crates/gascan/src/client.rs:7-35,209-252`
- Modify: `crates/gascan/src/cli.rs:74-116,291-318,338-362`
- Modify: `crates/gascan/src/presentation.rs`
- Modify: `crates/gascan/src/main.rs:9-14`
- Modify: `crates/gascan-e2e/tests/fake_backend.rs`

**Interfaces:**
- Produces: `ClientError::stable_code(&self) -> Option<&str>` and `cause(&self) -> Option<String>`.
- Produces: `CliError::stable_code(&self) -> Option<&str>`, `message(&self) -> String`, and `suggestion(&self) -> Option<&'static str>`.
- Produces: `render_error(message: &str, suggestion: Option<&str>, capabilities: OutputCapabilities) -> String`.

- [ ] **Step 1: Write failing error presentation tests**

Add unit tests for these exact plain-mode outputs:

```text
Error: no sandbox is available
Try: gascan up <project-root>
```

```text
Error: multiple sandboxes are available
Try: run `gascan list`, then pass `--sandbox <sandbox-id>`
```

```text
Error: sandbox not found
Try: run `gascan list` and use the sandbox ID shown there
```

For `resource_conflict`, assert the daemon message including its concrete resource name remains present and the stable code does not become the only explanation. Add an e2e assertion that `status` with no sandbox starts stderr with `Error:` and contains the `Try:` command while retaining exit code 64.

- [ ] **Step 2: Run focused tests and verify RED**

Run:

```bash
rtk cargo test -p gascan error -- --nocapture
rtk cargo test -p gascan-e2e no_sandbox -- --nocapture
```

Expected: errors are still emitted through raw `Display` without consistent headings or separate recovery lines.

- [ ] **Step 3: Implement structured error access and centralized rendering**

Keep `ClientError` variants and exit classification intact. `stable_code()` returns the tonic status message for RPC errors and the API message for API mismatches. `cause()` decodes RPC details using `gascan_proto::error_detail::decode_message`.

Use stable codes for suggestions, not substring matching. For local selector failures, construct `CliError::Usage` with a small internal `UsageKind::{NoSandbox, MultipleSandboxes, Other}` so their suggestions are structural. Operation errors retain both `code` and `message`. Map `sandbox_not_found` and `resource_conflict` as specified; unknown errors retain their current concrete message without inventing a remedy.

Change `main` to capture the exit code first and emit `cli::render_error(&error)` once. Normalize an existing lowercase `error:` prefix so it is never duplicated after the new `Error:` label. Interactive mode may color only the `Error:` and `Try:` labels.

- [ ] **Step 4: Run focused tests and verify GREEN**

Run:

```bash
rtk cargo test -p gascan error -- --nocapture
rtk cargo test -p gascan-e2e no_sandbox -- --nocapture
```

Expected: error unit/e2e tests pass with unchanged exit codes and preserved daemon causes.

- [ ] **Step 5: Commit Task 4**

```bash
rtk git add crates/gascan/src/client.rs crates/gascan/src/cli.rs crates/gascan/src/presentation.rs crates/gascan/src/main.rs crates/gascan-e2e/tests/fake_backend.rs
rtk git commit -m "feat: make CLI errors actionable"
```

---

### Task 5: Documentation and full verification

**Files:**
- Modify: `crates/gascan-e2e/tests/fake_backend.rs`
- Modify: `README.md:115-144`

**Interfaces:**
- Consumes all prior presentation interfaces.
- Produces no new production interface.

- [ ] **Step 1: Document the verified terminal behavior**

Add this README paragraph below the lifecycle example:

```markdown
Gascan shows live, in-place progress when stderr is an interactive terminal.
When output is redirected, the same meaningful milestones are printed as
stable plain text without animation or color. Set `NO_COLOR=1` to disable color
while keeping interactive progress. Use `--json` on supported commands for
machine-readable output.
```

- [ ] **Step 2: Run formatting and focused verification**

Run:

```bash
rtk cargo fmt --all -- --check
rtk cargo test -p gascan
rtk cargo test -p gascan-e2e doctor -- --nocapture
rtk cargo test -p gascan-e2e complete_cli_lifecycle_uses_daemon_api -- --nocapture
rtk cargo test -p gascan-e2e interactive_lifecycle_progress_updates_in_place_and_finishes_cleanly -- --nocapture
rtk cargo clippy -p gascan -p gascan-e2e --all-targets -- -D warnings
rtk git diff --check
```

Expected: every command exits 0 with no warnings or whitespace errors.

- [ ] **Step 3: Run the complete workspace regression suite**

Run: `rtk cargo test --workspace`

Expected: all workspace unit, integration, and documentation tests pass.

- [ ] **Step 4: Commit Task 5**

```bash
rtk git add README.md crates/gascan-e2e/tests/fake_backend.rs crates/gascan/src/presentation.rs
rtk git commit -m "test: verify polished CLI terminal UX"
```
