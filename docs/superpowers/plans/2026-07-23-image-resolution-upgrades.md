# Durable Image Resolution and Container Replacement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Detect approved workspace-image changes and let `gascan apply` replace only the owned container while preserving the workspace, network, and managed volumes.

**Architecture:** Reuse the existing version-1 `ImageResolution` record, add structured runtime image inspection and a sealed retained-resource recreation request, then implement a failure-atomic replacement state machine in `SandboxService`. `up` and `status` report `image_changed`; only `apply` performs replacement and commits the new digest after provisioning and health succeed.

**Tech Stack:** Rust 1.95, Tokio, SQLite/rusqlite, Tonic/Protobuf API v1, Apple `container` 1.1 structured JSON, existing fake-runtime failure injection.

**Design:** `docs/superpowers/specs/2026-07-23-default-ssh-workstation-design.md`

## Global Constraints

- Workspace images are accepted only as digest-qualified immutable references.
- Existing bind mount, tools/cache/config volumes, network, ownership labels, sandbox ID, and durable setup/tool/storage resolutions must be preserved.
- Mutable container-root changes are not durable and may be discarded.
- `up` never performs an image replacement.
- `apply` prepares the replacement image before stopping the old container.
- Durable image resolution changes only after replacement provisioning and health pass.
- Rollback must retain the primary error and report a separate rollback failure.
- Cleanup and replacement operate only on exact structured `GasCanOwned` resources.
- Human output stays concise; JSON retains stable codes and structured fields.
- No production path may infer ownership, image identity, or retained resources from names alone.

---

### Task 1: Validate Durable and Runtime Image Identity

**Files:**
- Modify: `crates/gascan-core/src/runtime.rs`
- Modify: `crates/gascan-core/src/fake_runtime.rs`
- Modify: `crates/gascan-apple/src/inspect.rs`
- Modify: `crates/gascan-apple/tests/inspect.rs`
- Modify: `crates/gascand/src/service.rs`
- Modify: `crates/gascand/tests/lifecycle.rs`
- Modify: `crates/gascand/tests/store.rs`

**Interfaces:**
- Consumes: existing `ImageResolution { version, details }`
- Produces: `RuntimeSandbox.image: String`
- Produces: `ImageState { recorded: Option<String>, running: String, approved: String }`
- Produces: `ImageState::change_required() -> bool`

- [ ] **Step 1: Write failing structured-inspection tests**

Add an exact Apple inspect fixture assertion:

```rust
assert_eq!(
    sandbox.image,
    "ghcr.io/liquescent-development/gascan/workspace:fixture@sha256:\
     aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
);
```

Add lifecycle table tests for:

```rust
[
    (None, running_digest, true),
    (Some(json!({"digest": running_digest})), running_digest, false),
    (Some(json!({"digest": old_digest})), running_digest, true),
    (Some(json!({"digest": 7})), running_digest, true),
]
```

The malformed and missing cases must remain readable but must never be treated
as proof of the approved image.

- [ ] **Step 2: Run the focused tests and confirm RED**

Run:

```bash
rtk cargo test -p gascan-apple --test inspect
rtk cargo test -p gascand --test lifecycle image_
```

Expected: compile failure because `RuntimeSandbox` has no `image` and no image
state validator exists.

- [ ] **Step 3: Add the structured runtime image field**

Change the runtime model:

```rust
pub struct RuntimeSandbox {
    pub id: SandboxId,
    pub image: String,
    pub state: ContainerState,
    pub ownership: OwnershipMetadata,
}
```

Populate `image` only from the Apple structured inspect configuration. Reject
empty, tag-only, or non-digest references with `RuntimeError::InvalidOutput`.
Update every fake and test fixture with an explicit immutable image.

- [ ] **Step 4: Add one image-resolution decoder**

In `service.rs`, introduce:

```rust
#[derive(Clone, Debug, Eq, PartialEq)]
struct ImageState {
    recorded: Option<String>,
    running: String,
    approved: String,
}

impl ImageState {
    fn change_required(&self) -> bool {
        self.recorded.as_deref() != Some(self.approved.as_str())
            || self.running != self.approved
    }
}

fn stored_image(record: &SandboxRecord) -> Option<String> {
    let resolution = record.image_resolution.as_ref()?;
    if resolution.version != 1 {
        return None;
    }
    resolution
        .details
        .get("digest")?
        .as_str()
        .filter(|value| immutable_workspace_image(value))
        .map(ToOwned::to_owned)
}
```

Use the same immutable-reference validator for approved, stored, and runtime
images. Do not fall back from a malformed resolution to the running image.

- [ ] **Step 5: Run focused and store tests**

Run:

```bash
rtk cargo test -p gascan-apple --test inspect
rtk cargo test -p gascand --test lifecycle image_
rtk cargo test -p gascand --test store
```

Expected: all selected tests pass.

- [ ] **Step 6: Commit**

```bash
rtk git add crates/gascan-core/src/runtime.rs crates/gascan-core/src/fake_runtime.rs \
  crates/gascan-apple/src/inspect.rs crates/gascan-apple/tests/inspect.rs \
  crates/gascand/src/service.rs crates/gascand/tests/lifecycle.rs \
  crates/gascand/tests/store.rs
rtk git commit -m "feat: validate running workspace image"
```

### Task 2: Add a Sealed Retained-Resource Recreation Contract

**Files:**
- Modify: `crates/gascan-core/src/runtime.rs`
- Modify: `crates/gascan-core/src/fake_runtime.rs`
- Modify: `crates/gascan-core/tests/backend_contract.rs`
- Modify: `crates/gascan-apple/src/backend.rs`
- Modify: `crates/gascan-apple/src/translate.rs`
- Modify: `crates/gascan-apple/tests/backend_fake_runner.rs`
- Modify: `crates/gascan-apple/tests/translate.rs`

**Interfaces:**
- Consumes: `CreateRequest`, exact `RuntimeResource` inventory
- Produces: `RetainedResources::new(&CreateRequest, Vec<RuntimeResource>)`
- Produces: `RecreateRequest::new(CreateRequest, RetainedResources)`
- Produces: `RuntimeBackend::prepare_image(&str)`
- Produces: `RuntimeBackend::create_container(RecreateRequest)`

- [ ] **Step 1: Write failing contract tests**

Add tests proving `RetainedResources::new` accepts exactly:

```rust
[
    all expected GasCanOwned volume resources,
    the expected GasCanOwned network resource when networked,
]
```

and rejects:

- A container resource.
- A missing volume.
- An extra or duplicate volume.
- A foreign, mismatched, or unknown ownership class.
- A network for an offline request.
- A network whose identity differs from `CreateRequest::network()`.

Add a fake-backend contract test whose calls are exactly:

```rust
[
    RuntimeCall::PrepareImage(new_image.to_owned()),
    RuntimeCall::CreateContainer(recreate_request.clone()),
]
```

- [ ] **Step 2: Run backend-contract tests and confirm RED**

Run:

```bash
rtk cargo test -p gascan-core --test backend_contract recreate
```

Expected: compile failure because the retained-resource and backend contracts do
not exist.

- [ ] **Step 3: Add sealed request types**

Add output-readable, input-sealed types:

```rust
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RetainedResources {
    resources: Vec<RuntimeResource>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecreateRequest {
    create: CreateRequest,
    retained: RetainedResources,
}
```

Only `RetainedResources::new(&CreateRequest, Vec<RuntimeResource>)` and
`RecreateRequest::new(CreateRequest, RetainedResources)` may construct them.
Expose read-only accessors. Add compile-fail documentation showing callers
cannot forge either with a struct literal or deserialization.

- [ ] **Step 4: Extend the backend trait and fake runtime**

Add:

```rust
async fn prepare_image(&self, image: &str) -> Result<(), RuntimeError>;
async fn create_container(
    &self,
    request: RecreateRequest,
) -> Result<CreateOutcome, CreateFailure>;
```

`CreateOutcome` for `create_container` must contain exactly the newly created
container. Add `RuntimeCall::PrepareImage` and
`RuntimeCall::CreateContainer`. Add distinct failure-injection boundaries.

- [ ] **Step 5: Implement Apple preparation and retained creation**

`prepare_image` uses `AppleCommandBuilder::pull` and inherits immutable image
validation.

Add:

```rust
pub fn create_with_retained(request: &RecreateRequest) -> Result<CommandSpec, TranslationError>
```

It emits one `container run` command with the existing volume and network names
from the validated `CreateRequest`; it emits no network-create or volume-create
commands. Apple backend inventory must prove all retained resources still equal
the sealed evidence immediately before run.

On an ambiguous command-I/O result, reconcile only the expected container and
return it through `CreateFailure::created()`.

- [ ] **Step 6: Run all affected backend tests**

Run:

```bash
rtk cargo test -p gascan-core --test backend_contract
rtk cargo test -p gascan-apple --test translate
rtk cargo test -p gascan-apple --test backend_fake_runner
```

Expected: all tests pass, including exact command order and partial-container
evidence.

- [ ] **Step 7: Commit**

```bash
rtk git add crates/gascan-core/src/runtime.rs crates/gascan-core/src/fake_runtime.rs \
  crates/gascan-core/tests/backend_contract.rs crates/gascan-apple/src/backend.rs \
  crates/gascan-apple/src/translate.rs crates/gascan-apple/tests/backend_fake_runner.rs \
  crates/gascan-apple/tests/translate.rs
rtk git commit -m "feat: add retained-resource recreation"
```

### Task 3: Implement Failure-Atomic Container Replacement

**Files:**
- Modify: `crates/gascand/src/service.rs`
- Modify: `crates/gascand/tests/lifecycle.rs`
- Modify: `crates/gascand/tests/apply_setup.rs`
- Modify: `crates/gascand/tests/apply_tools.rs`

**Interfaces:**
- Consumes: `ImageState`, `RecreateRequest`, `RuntimeBackend::prepare_image`
- Produces: `SandboxService::replace_image(...)`
- Produces: operation phases `before_image_replace`, `image_replaced`,
  `image_rollback`, and `after_image_replace`

- [ ] **Step 1: Write failing lifecycle and rollback tests**

Build a sandbox whose stored and running image are `old_image`, while
`PolicyCompiler` requests `new_image`.

Assert `up`:

```rust
assert_eq!(runtime.calls().await.len(), before);
assert!(events.iter().any(|event| {
    event["phase"] == "apply_required" && event["reason"] == "image_changed"
}));
```

Assert successful `apply` preserves exact volume/network identities and call
ordering:

```text
prepare new image
inspect/list exact owned resources
stop old container
remove old container only
create new container from retained resources
provision setup
health
persist new image resolution
```

Inject failure after each mutation and assert:

- Partial replacement container is stopped and removed.
- Previous image is recreated using the same retained resources.
- Previous image resolution remains durable.
- Primary and rollback errors are both available when rollback fails.
- No volume or network is removed.

- [ ] **Step 2: Run focused tests and confirm RED**

Run:

```bash
rtk cargo test -p gascand --test lifecycle image_replace
rtk cargo test -p gascand --test apply_setup image_replace
rtk cargo test -p gascand --test apply_tools image_replace
```

Expected: image changes reach normal start/provision behavior instead of the
replacement state machine.

- [ ] **Step 3: Make `up` report apply-required without mutation**

Before beginning runtime start/provision work, compare stored, running, and
approved images. Emit:

```rust
json!({
    "phase": "apply_required",
    "reason": "image_changed",
    "recorded_image": state.recorded,
    "running_image": state.running,
    "approved_image": state.approved,
})
```

Complete the `up` operation without calling start, stop, prepare, remove,
create, exec, or provision.

- [ ] **Step 4: Implement exact replacement helpers**

Add focused helpers with no duplicate cleanup logic:

```rust
async fn retained_resources(
    &self,
    create: &CreateRequest,
) -> Result<(RuntimeResource, RetainedResources), ServiceError>;

async fn replace_image(
    &self,
    spec: &SandboxSpec,
    create: &CreateRequest,
    previous_image: &str,
    operation_id: OperationId,
    sender: &mpsc::Sender<OperationEvent>,
) -> Result<ProvisionedResolution, ServiceError>;
```

The first returns the exact old container separately from sealed retained
resources. The second:

1. Prepares the new image.
2. Stops the old container when running.
3. Removes only the exact old container.
4. Creates the replacement from retained resources.
5. Forces setup and Gascamp verification; reuses the persistent tool volume
   without reinstalling unchanged tools.
6. Runs health.
7. On failure, cleans partial replacement evidence and recreates the previous
   immutable image.

Do not update `record.image_resolution` inside the helper.

- [ ] **Step 5: Commit the resolution only after success**

After provisioning and health:

```rust
record.image_resolution = Some(ImageResolution::new(
    1,
    json!({"digest": create.image()}),
));
```

Persist the record and complete the operation in the same ordering used by
normal apply. A database failure after replacement must trigger a rollback to
the previous image before returning.

- [ ] **Step 6: Run all lifecycle and provisioning tests**

Run:

```bash
rtk cargo test -p gascand --test lifecycle
rtk cargo test -p gascand --test apply_setup
rtk cargo test -p gascand --test apply_tools
rtk cargo test -p gascand --lib
```

Expected: all tests pass.

- [ ] **Step 7: Commit**

```bash
rtk git add crates/gascand/src/service.rs crates/gascand/tests/lifecycle.rs \
  crates/gascand/tests/apply_setup.rs crates/gascand/tests/apply_tools.rs
rtk git commit -m "feat: replace outdated sandbox images"
```

### Task 4: Expose Image Upgrade State Through API and CLI

**Files:**
- Modify: `proto/gascan/v1/gascan.proto`
- Modify: `crates/gascan-proto/src/lib.rs`
- Modify: `crates/gascan-proto/tests/api_compatibility.rs`
- Modify: `crates/gascand/src/api.rs`
- Modify: `crates/gascand/tests/daemon_idle.rs`
- Modify: `crates/gascan/src/presentation.rs`
- Modify: `crates/gascan/src/cli.rs`

**Interfaces:**
- Produces: API minor version 2
- Produces: `ApplyRequirement { reason, current, requested }`
- Produces: stable error codes `image_upgrade_required` and
  `image_replacement_failed`

- [ ] **Step 1: Add failing protocol and presentation tests**

Append, without renumbering existing fields:

```proto
message ApplyRequirement {
  string reason = 1;
  string current = 2;
  string requested = 3;
  reserved 4;
}

message SandboxStatus {
  // existing fields 1..6 unchanged
  reserved 7;
  repeated ApplyRequirement apply_requirements = 8;
}
```

Tests must prove old fields retain numbers, API major remains 1, API minor
becomes 2, and unknown requirements are safe for old clients.

Add human expectation:

```text
Update available
  Workspace image  old@sha256:… → new@sha256:…
  Run gascan apply
```

JSON must retain the full exact current and requested references.

- [ ] **Step 2: Run protocol and CLI tests and confirm RED**

Run:

```bash
rtk cargo test -p gascan-proto
rtk cargo test -p gascan presentation
```

Expected: generated types and presentation fields are absent.

- [ ] **Step 3: Wire status and errors**

Map image state into `apply_requirements`. Use stable reason
`image_changed`. Add both error codes to `error_code::ALL`. Map replacement
precondition failures to `FailedPrecondition` and replacement execution
failures to structured operation errors with primary and rollback details.

- [ ] **Step 4: Render professional human and JSON output**

Human status truncates digest display but never truncates stored JSON values.
Operation progress uses:

```text
⠋ Preparing workspace image
⠋ Replacing sandbox container
⠋ Restoring previous workspace image
```

No raw internal phase identifier is printed in human mode.

- [ ] **Step 5: Run API boundary and CLI suites**

Run:

```bash
rtk cargo test -p gascan-proto
rtk cargo test -p gascand --test daemon_idle
rtk cargo test -p gascan
```

Expected: all tests pass, including real Unix-socket status and apply errors.

- [ ] **Step 6: Commit**

```bash
rtk git add proto/gascan/v1/gascan.proto crates/gascan-proto/src/lib.rs \
  crates/gascan-proto/tests/api_compatibility.rs crates/gascand/src/api.rs \
  crates/gascand/tests/daemon_idle.rs crates/gascan/src/presentation.rs \
  crates/gascan/src/cli.rs
rtk git commit -m "feat: report workspace image upgrades"
```

### Task 5: Document and Verify Image Replacement

**Files:**
- Modify: `README.md`
- Modify: `crates/gascan-e2e/tests/apple_apply.rs`
- Modify: `crates/gascan-e2e/tests/apple_common/mod.rs`

**Interfaces:**
- Consumes: complete container-replacement behavior
- Produces: live predecessor-to-approved-image acceptance

- [ ] **Step 1: Add an ignored live replacement test**

Use two immutable, digest-qualified workspace-image fixtures that satisfy the
same workspace-user and volume contract. Assert:

- `status` reports `image_changed`.
- `up` performs no mutation.
- `apply` changes only the container identity/image.
- Exact volume and network identities remain.
- A sentinel in each managed volume survives.
- Setup and health rerun.
- Injected replacement failure restores the predecessor image.
- Post-test owned inventory is empty.

The test must use bounded waits and exact cleanup evidence.

- [ ] **Step 2: Update README**

Document:

```text
Workspace image updates are reported by gascan status.
Run gascan apply to replace only the container while preserving the workspace
and managed tools, cache, and configuration volumes.
Changes made directly to the container root filesystem are not durable.
```

Include primary/rollback failure recovery guidance.

- [ ] **Step 3: Run complete verification**

Run:

```bash
rtk cargo fmt --all -- --check
rtk cargo clippy --workspace --all-targets --all-features -- -D warnings
rtk cargo test --workspace
rtk cargo test --manifest-path scripts/Cargo.toml
rtk git diff --check
rtk bash ./scripts/apple-test-preflight.sh
rtk cargo test -p gascan-e2e --test apple_apply image_replace -- --ignored --nocapture
```

Expected: static and automated suites pass; eligible Apple host live replacement
and cleanup pass.

- [ ] **Step 4: Commit**

```bash
rtk git add README.md crates/gascan-e2e/tests/apple_apply.rs \
  crates/gascan-e2e/tests/apple_common/mod.rs
rtk git commit -m "docs: describe workspace image upgrades"
```
