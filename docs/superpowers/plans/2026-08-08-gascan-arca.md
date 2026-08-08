# `gascan-arca` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement `RuntimeBackend` over Arca's published engine contract in a new `gascan-arca` crate, behind a transport seam so every mapping is tested without a live engine.

**Architecture:** `ArcaBackend<T: EngineTransport>` mirrors the existing `AppleBackend<R: CommandRunner>`. `EngineTransport` is stated in **wire types**, so the mapping — the part with the bugs — sits above the fake. Pure translation lives in `translate.rs` with inline unit tests; backend behaviour is tested through the public `ArcaBackend` against a fake transport in `tests/`. Inbound responses are built by calling `gascan-core`'s existing validating constructors, so the boundary check against a lying engine is code that already exists and is already tested.

**Tech Stack:** Rust 2024, `tonic` 0.12, `prost` 0.13, `tokio`, `async-trait`, `tower` 0.4, `hyper-util` 0.1. Generated client from `gascan-engine-proto` (`v1` module), which reaches Arca's proto across the signed pin at build time.

Design: `docs/superpowers/specs/2026-08-08-gascan-arca-backend-design.md` (approved, committed `372961a`).

## Global Constraints

- **Branch `feat/gascan-arca`. Never commit to `main`.** Both repositories forbid squash- and rebase-merge; merge commits only.
- **A green `cargo test --workspace --no-fail-fast` on this machine counts as a pass.** CI reports but does not gate. Do not re-enable a required status check, and do not spend time on CI stability.
- **`RUSTUP_TOOLCHAIN=1.95.0` is exported in this environment and overrides `rust-toolchain.toml`.** Prefix every cargo invocation with `env -u RUSTUP_TOOLCHAIN`.
- **Use `--no-fail-fast`.** `cargo test --workspace` stops after the first failing binary and hides everything after it.
- **`cargo test <name>` without a full module path silently runs ZERO tests and exits 0.** Always confirm the `running N tests` line.
- **Capture exit codes directly, never through a pipe:** `if cmd; then rc=0; else rc=$?; fi`. `${PIPESTATUS[0]}` after an `if` block reads empty.
- **Never count with a truncating pipe.** Use `grep -c`. A `grep | head -10` that returns ten lines looks exactly like a complete answer; this plan's own spec carries a correction for that.
- **`cargo clippy --fix` is NOT safe in this repository.** It has emitted invalid Rust here. Fix lints by hand.
- **The clippy gate for Tasks 3-7 is `-D warnings -A dead_code`; the plain gate returns at Task 8.** **Added 2026-08-08 after Task 3 hit it.** `translate.rs`'s mapping functions have no caller until `ArcaBackend` wires them in at Task 6, and `exec_start`'s caller does not exist until Task 8, so rustc's `dead_code` fires and `-D warnings` promotes it to an error — 17 errors on the lib target, 13 on the lib-test target, VERIFIED. **Allow it on the command line; never with an attribute in the source.** An `#[allow(dead_code)]` outlives the condition that justified it, so a genuinely dead function added later becomes invisible; a flag on one command cannot rot that way. VERIFIED 2026-08-08: `cargo clippy -p gascan-arca --all-targets -- -D warnings -A dead_code` → **rc=0**, so `dead_code` is the only lint firing and every other lint stays hard-gated. Task 8 restores the plain `-D warnings` because `exec` is the last function to acquire a caller, and Task 10 runs the workspace-wide gate with nothing allowed. The gate is deferred to the point where it can pass honestly, not weakened.
- **`crates/gascan-arca` does NOT deny `clippy::panic`/`unwrap_used`/`expect_used`** — those are per-crate lints on `gascan` and `gascand` only, and this crate matches `gascan-apple` instead. `unsafe_code` is forbidden workspace-wide.
- **Do not vendor the proto, and do not add a second parser of `engine/arca-pin.json`.** `scripts/sync-arca-proto.sh` owns what the pinned contract means.
- **Do not generate or write a Rust server**, not even as a test double.
- **Do not modify the proto.** It is published and pinned by signed tag.
- **Signing:** commits are signed through the 1Password SSH agent. If signing fails with "communication with agent failed", the agent is locked — ask the user. Never fall back to `--no-gpg-sign`.
- Every commit message: no co-author, no mention of the agent.

**Naming, fixed across all tasks.** Later tasks depend on these exact names:

| Symbol | Where |
|---|---|
| `MANAGED_BY`, `MANAGED_BY_LABEL`, `SANDBOX_ID_LABEL` | `gascan_core::runtime` (Task 1) |
| `SandboxLabel<'a>` = `Absent` \| `Unparseable` \| `Parsed(&'a SandboxId)` | `gascan_core::runtime` (Task 1) |
| `classify_resource_ownership(kind, name, managed_by, sandbox) -> ResourceOwnership` | `gascan_core::runtime` (Task 1) |
| `immutable_image_identity(image) -> Option<(&str, &str)>` | `gascan_core::runtime`, made `pub` (Task 1) |
| `EngineTransport`, `TransportError`, `ExecStream`, `LogsStream` | `gascan_arca` (Task 2) |
| `ArcaBackend<T>`, `ArcaBackend::new(transport)` | `gascan_arca` (Task 6) |
| `ChannelTransport`, `ChannelTransport::connect(socket)` | `gascan_arca` (Task 9) |

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/gascan-core/src/runtime.rs` (modify) | publish `MANAGED_BY` and the two label keys, `SandboxLabel`, `classify_resource_ownership`, and `immutable_image_identity` |
| `crates/gascan-core/src/policy.rs` (modify) | drop its private `MANAGED_BY`, use the published one |
| `crates/gascan-apple/src/backend.rs` (modify) | drop private constants and `classify`, call the shared classifier |
| `crates/gascan-apple/src/inspect.rs` (modify) | drop private constants and `classify_inventory_ownership`, call the shared classifier |
| `crates/gascan-apple/src/translate.rs` (modify) | drop its private `MANAGED_BY` |
| `crates/gascan-arca/Cargo.toml` (create) | crate manifest |
| `crates/gascan-arca/src/lib.rs` (create) | module wiring and the public surface |
| `crates/gascan-arca/src/transport.rs` (create) | `EngineTransport`, `TransportError`, `ExecStream`, `LogsStream` |
| `crates/gascan-arca/src/translate.rs` (create) | pure wire↔core mapping plus inline unit tests. No I/O |
| `crates/gascan-arca/src/error.rs` (create) | the `EngineError` code table and its rejection path |
| `crates/gascan-arca/src/backend.rs` (create) | `ArcaBackend<T>` and its `RuntimeBackend` impl |
| `crates/gascan-arca/src/channel.rs` (create) | `ChannelTransport`, the `tonic` arm |
| `crates/gascan-arca/tests/fake_transport/mod.rs` (create) | the fake `EngineTransport` shared by the behaviour tests |
| `crates/gascan-arca/tests/backend_unary.rs` (create) | the nine unary methods through the fake |
| `crates/gascan-arca/tests/backend_streams.rs` (create) | logs and exec through the fake |
| `Cargo.toml` (modify) | add `crates/gascan-arca` to `members` |

---

### Task 1: Publish the shared ownership rule from `gascan-core`

**Files:**
- Modify: `crates/gascan-core/src/runtime.rs` (add constants, `SandboxLabel`, `classify_resource_ownership`; make `immutable_image_identity` `pub`)
- Modify: `crates/gascan-core/src/policy.rs:22,217`
- Modify: `crates/gascan-apple/src/backend.rs:20-22,74-111,650-663`
- Modify: `crates/gascan-apple/src/inspect.rs:17-19,257-275`
- Modify: `crates/gascan-apple/src/translate.rs:15`
- Test: `crates/gascan-core/tests/resource_ownership.rs` (create)

**Interfaces:**
- Consumes: nothing.
- Produces: `gascan_core::runtime::{MANAGED_BY, MANAGED_BY_LABEL, SANDBOX_ID_LABEL, SandboxLabel, classify_resource_ownership, immutable_image_identity}`.

**Why the two existing classifiers are not duplicates.** `backend.rs:650` ignores the resource name; `inspect.rs:257` requires `id.as_str() == name`. That is correct, because a container is named by its sandbox id and a volume is not. The shared function keeps the difference, keyed on `ResourceKind`. They also differ on an *unparseable* label — `backend.rs:74-81` fails the whole listing with `invalid_output`, `inspect.rs:268` returns `Mismatched` — so the shared function owns **the rule** and each caller keeps **its own error policy**.

- [ ] **Step 1: Write the failing test**

Create `crates/gascan-core/tests/resource_ownership.rs`:

```rust
use gascan_core::runtime::{
    ResourceKind, ResourceOwnership, SandboxLabel, classify_resource_ownership,
};
use gascan_core::sandbox::SandboxId;

fn owned_container_id() -> SandboxId {
    SandboxId::test("owned")
}

#[test]
fn a_container_must_be_named_by_its_sandbox_id() {
    let id = owned_container_id();
    assert_eq!(
        classify_resource_ownership(
            ResourceKind::Container,
            id.as_str(),
            Some("gascan"),
            SandboxLabel::Parsed(&id),
        ),
        ResourceOwnership::GasCanOwned,
    );
    assert_eq!(
        classify_resource_ownership(
            ResourceKind::Container,
            "some-other-name",
            Some("gascan"),
            SandboxLabel::Parsed(&id),
        ),
        ResourceOwnership::Mismatched,
        "a container whose name and sandbox-id label disagree is not ours to delete",
    );
}

#[test]
fn a_volume_or_network_need_not_be_named_by_its_sandbox_id() {
    let id = owned_container_id();
    for kind in [ResourceKind::Volume, ResourceKind::Network] {
        assert_eq!(
            classify_resource_ownership(kind, "workspace-data", Some("gascan"), SandboxLabel::Parsed(&id)),
            ResourceOwnership::GasCanOwned,
            "kind {kind:?}",
        );
    }
}

#[test]
fn an_unlabelled_resource_is_foreign_and_a_foreign_manager_is_foreign() {
    for kind in [ResourceKind::Container, ResourceKind::Volume, ResourceKind::Network] {
        assert_eq!(
            classify_resource_ownership(kind, "anything", None, SandboxLabel::Absent),
            ResourceOwnership::Foreign,
            "kind {kind:?}",
        );
        assert_eq!(
            classify_resource_ownership(kind, "anything", Some("other-tool"), SandboxLabel::Absent),
            ResourceOwnership::Foreign,
            "kind {kind:?}",
        );
    }
}

#[test]
fn a_half_labelled_or_unparseable_resource_is_mismatched() {
    let id = owned_container_id();
    for kind in [ResourceKind::Container, ResourceKind::Volume, ResourceKind::Network] {
        assert_eq!(
            classify_resource_ownership(kind, id.as_str(), Some("gascan"), SandboxLabel::Unparseable),
            ResourceOwnership::Mismatched,
            "kind {kind:?}",
        );
        assert_eq!(
            classify_resource_ownership(kind, id.as_str(), Some("gascan"), SandboxLabel::Absent),
            ResourceOwnership::Mismatched,
            "kind {kind:?}",
        );
        assert_eq!(
            classify_resource_ownership(kind, id.as_str(), None, SandboxLabel::Parsed(&id)),
            ResourceOwnership::Mismatched,
            "kind {kind:?}",
        );
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `env -u RUSTUP_TOOLCHAIN cargo test -p gascan-core --test resource_ownership`
Expected: FAIL to compile — `SandboxLabel` and `classify_resource_ownership` are not found in `gascan_core::runtime`.

- [ ] **Step 3: Add the constants, the label enum, and the classifier**

In `crates/gascan-core/src/runtime.rs`, after the `OwnershipMetadata` struct:

```rust
/// The `managed_by` value Gas Can attaches to everything it creates.
pub const MANAGED_BY: &str = "gascan";
/// Label key carrying [`MANAGED_BY`].
pub const MANAGED_BY_LABEL: &str = "dev.gascan.managed-by";
/// Label key carrying the owning sandbox's id.
pub const SANDBOX_ID_LABEL: &str = "dev.gascan.sandbox-id";

/// The three states a sandbox-id label can be in, as the classifier sees it.
///
/// `Unparseable` is distinct from `Absent` on purpose: a present-but-invalid
/// label is evidence of a collision or of another tool's label, not of an
/// unlabelled resource. Parsing happens in the caller, which keeps the decision
/// about whether a parse failure is fatal where it belongs — `AppleInspector`
/// treats it as `Mismatched` and continues, while the volume and network listing
/// fails the whole call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SandboxLabel<'a> {
    Absent,
    Unparseable,
    Parsed(&'a SandboxId),
}

/// Decides whether a labelled runtime resource is ours, foreign, or mismatched.
///
/// This is the consumer's judgment and it stays in the consumer: an engine that
/// decided "this one is yours" would be answering a policy question inside the
/// component the policy boundary exists to constrain.
///
/// The rule is per kind. A container is named by its sandbox id, so a container
/// whose name disagrees with its label is `Mismatched`. A volume or network is
/// not, so its name carries no ownership information.
pub fn classify_resource_ownership(
    kind: ResourceKind,
    name: &str,
    managed_by: Option<&str>,
    sandbox: SandboxLabel<'_>,
) -> ResourceOwnership {
    match (managed_by, sandbox) {
        (Some(MANAGED_BY), SandboxLabel::Parsed(id)) => {
            if kind == ResourceKind::Container && id.as_str() != name {
                ResourceOwnership::Mismatched
            } else {
                ResourceOwnership::GasCanOwned
            }
        }
        (None, SandboxLabel::Absent) => ResourceOwnership::Foreign,
        (Some(manager), _) if manager != MANAGED_BY => ResourceOwnership::Foreign,
        _ => ResourceOwnership::Mismatched,
    }
}
```

Then change `fn immutable_image_identity` (`runtime.rs:647`) to `pub fn immutable_image_identity`, and extend its doc comment:

```rust
/// Splits an immutable reference into its tag-stripped repository and its digest.
///
/// This is the pair a structured wire digest needs, and it is the same pair
/// [`same_immutable_image`] compares by — so a caller that canonicalises through
/// this function is canonicalising consistently with every comparison in the
/// workspace.
pub fn immutable_image_identity(image: &str) -> Option<(&str, &str)> {
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `env -u RUSTUP_TOOLCHAIN cargo test -p gascan-core --test resource_ownership`
Expected: PASS, `running 4 tests`. Confirm the count — a filtered run that matches nothing exits 0.

- [ ] **Step 5: Migrate `policy.rs` onto the published constant**

In `crates/gascan-core/src/policy.rs`, delete line 22 (`const MANAGED_BY: &str = "gascan";`) and add `MANAGED_BY` to the existing `use crate::runtime::{...}` import list at the top of the file. The use at `:217` is unchanged.

- [ ] **Step 6: Migrate `gascan-apple/src/backend.rs`**

Delete lines 20-22 (`MANAGED_BY`, `MANAGED_BY_LABEL`, `SANDBOX_ID_LABEL`) and the whole `fn classify` at `:650-663`. Add `MANAGED_BY`, `MANAGED_BY_LABEL`, `SANDBOX_ID_LABEL`, `SandboxLabel` and `classify_resource_ownership` to the `gascan_core::runtime` import list at `:5-10`.

At the two call sites (`:81` for volumes, `:108` for networks), replace `classify(sandbox_id.as_ref(), &record.configuration.labels)` with:

```rust
            let ownership = classify_resource_ownership(
                ResourceKind::Volume,
                &record.id,
                record.configuration.labels.get(MANAGED_BY_LABEL).map(String::as_str),
                sandbox_id.as_ref().map_or(SandboxLabel::Absent, SandboxLabel::Parsed),
            );
```

and the network site identically but with `ResourceKind::Network`.

**This preserves behaviour exactly.** The parse at `:74-81` and `:101-108` still runs first and still turns a parse failure into `invalid_output`, so this call site never passes `SandboxLabel::Unparseable`. `MANAGED_BY` remains referenced at `:236,237,279,280` for label *writing*, now from `gascan-core`.

- [ ] **Step 7: Migrate `gascan-apple/src/inspect.rs`**

Delete lines 17-19 (`MANAGED_BY_LABEL`, `SANDBOX_ID_LABEL`, `MANAGED_BY_GASCAN`) and the whole `fn classify_inventory_ownership` at `:257-275`. Import the shared items from `gascan_core::runtime`. Replace the call site with:

```rust
    let parsed = labels
        .get(SANDBOX_ID_LABEL)
        .and_then(|value| SandboxId::try_from(value.clone()).ok());
    let label = match (labels.get(SANDBOX_ID_LABEL), &parsed) {
        (None, _) => SandboxLabel::Absent,
        (Some(_), Some(id)) => SandboxLabel::Parsed(id),
        (Some(_), None) => SandboxLabel::Unparseable,
    };
    let ownership = classify_resource_ownership(
        ResourceKind::Container,
        name,
        labels.get(MANAGED_BY_LABEL).map(String::as_str),
        label,
    );
    let sandbox_id = match ownership {
        ResourceOwnership::GasCanOwned => parsed,
        _ => None,
    };
    (sandbox_id, ownership)
```

**Read the old function before deleting it and check this against it.** The old one returned `Some(id)` alongside `Mismatched` in exactly one arm — `Ok(id)` where `id.as_str() != name` (`:267`). The replacement above returns `None` there. If `gascan-apple`'s tests fail on that, restore the old behaviour by matching that arm explicitly rather than by weakening the assertion.

- [ ] **Step 8: Migrate `gascan-apple/src/translate.rs`**

Delete line 15 (`const MANAGED_BY: &str = "gascan";`) and import `MANAGED_BY` from `gascan_core::runtime`.

- [ ] **Step 9: Verify no private copy survives**

Run: `grep -rn 'MANAGED_BY[A-Z_]* *: *&str' crates/*/src/*.rs`
Expected: exactly the three `pub const` declarations in `crates/gascan-core/src/runtime.rs`, and nothing else.

- [ ] **Step 10: Run the full `gascan-apple` and `gascan-core` suites — this is the parity evidence**

Run: `env -u RUSTUP_TOOLCHAIN cargo test -p gascan-apple -p gascan-core --no-fail-fast`
Expected: PASS, zero failures. `mixed_list_classifies_owned_foreign_and_mismatched_resources` (`gascan-apple/tests/inspect.rs:134`) and `foreign_container_names_do_not_have_to_be_valid_sandbox_ids` (`:164`) passing unchanged is what proves the extraction did not change a rule. If either fails, the extraction is wrong — fix the extraction, never the test.

- [ ] **Step 11: Clippy and fmt**

Run: `env -u RUSTUP_TOOLCHAIN cargo clippy -p gascan-core -p gascan-apple --all-targets -- -D warnings` then `env -u RUSTUP_TOOLCHAIN cargo fmt --all --check`
Expected: both rc=0. Fix any lint by hand; `clippy --fix` is unsafe here.

- [ ] **Step 12: Commit**

```bash
git add crates/gascan-core crates/gascan-apple
git commit -m "refactor: share the resource-ownership rule from gascan-core

gascan-apple carried two ownership classifiers that are not duplicates:
backend.rs ignored the resource name and inspect.rs required name ==
sandbox_id, which is right because a container is named by its sandbox id
and a volume is not. A third consumer is about to need the same rule, and
three implementations of a deletion-authority decision is one too many.

The shared function is keyed on ResourceKind and keeps that difference. It
takes a three-state SandboxLabel so that parsing, and the decision about
whether a parse failure is fatal, stay with each caller: the container
listing treats it as Mismatched and continues, the volume listing fails.

Also publishes MANAGED_BY and the two label keys, which existed as four and
two private copies, and immutable_image_identity, which already computes the
tag-stripped repository and digest pair a structured wire digest needs.

gascan-apple's existing tests pass unchanged, which is the parity evidence."
```

---

### Task 2: The crate and the transport seam

**Files:**
- Create: `crates/gascan-arca/Cargo.toml`, `crates/gascan-arca/src/lib.rs`, `crates/gascan-arca/src/transport.rs`
- Modify: `Cargo.toml` (workspace `members`)
- Test: `crates/gascan-arca/tests/transport_contract.rs` (create)

**Interfaces:**
- Consumes: `gascan_engine_proto::v1` (generated), `gascan_core::runtime::RuntimeError`.
- Produces: `EngineTransport` (11 methods), `TransportError`, `TransportError::rpc`, `TransportError::into_runtime_error`, `ExecStream::{new, split}`, `LogsStream::{new, recv}`.

**Registration is one line.** VERIFIED: `scripts/ci-classify-paths.sh:40` matches `crates/*` as a glob and no script or workflow enumerates crate names, so only `members` changes.

- [ ] **Step 1: Write the failing test**

Create `crates/gascan-arca/tests/transport_contract.rs`:

```rust
use gascan_arca::{EngineTransport, ExecStream, LogsStream, TransportError};
use gascan_engine_proto::v1;

/// A transport that fails every call, which is enough to prove the trait is
/// object-safe in the shape the backend needs and that the error mapping holds.
struct Unreachable;

#[async_trait::async_trait]
impl EngineTransport for Unreachable {
    async fn capabilities(
        &self,
        _request: v1::CapabilitiesRequest,
    ) -> Result<v1::CapabilitiesResponse, TransportError> {
        Err(TransportError::rpc("capabilities", "connection refused"))
    }
    async fn inspect(
        &self,
        _request: v1::InspectRequest,
    ) -> Result<v1::InspectResponse, TransportError> {
        Err(TransportError::rpc("inspect", "connection refused"))
    }
    async fn create(
        &self,
        _request: v1::CreateRequest,
    ) -> Result<v1::CreateResponse, TransportError> {
        Err(TransportError::rpc("create", "connection refused"))
    }
    async fn prepare_image(
        &self,
        _request: v1::PrepareImageRequest,
    ) -> Result<v1::PrepareImageResponse, TransportError> {
        Err(TransportError::rpc("prepare_image", "connection refused"))
    }
    async fn create_container(
        &self,
        _request: v1::CreateContainerRequest,
    ) -> Result<v1::CreateResponse, TransportError> {
        Err(TransportError::rpc("create_container", "connection refused"))
    }
    async fn start(&self, _request: v1::StartRequest) -> Result<v1::AckResponse, TransportError> {
        Err(TransportError::rpc("start", "connection refused"))
    }
    async fn stop(&self, _request: v1::StopRequest) -> Result<v1::AckResponse, TransportError> {
        Err(TransportError::rpc("stop", "connection refused"))
    }
    async fn remove(&self, _request: v1::RemoveRequest) -> Result<v1::AckResponse, TransportError> {
        Err(TransportError::rpc("remove", "connection refused"))
    }
    async fn exec(&self, _start: v1::ExecStart) -> Result<ExecStream, TransportError> {
        Err(TransportError::rpc("exec", "connection refused"))
    }
    async fn logs(&self, _request: v1::LogsRequest) -> Result<LogsStream, TransportError> {
        Err(TransportError::rpc("logs", "connection refused"))
    }
    async fn list_resources(
        &self,
        _request: v1::ListResourcesRequest,
    ) -> Result<v1::ListResourcesResponse, TransportError> {
        Err(TransportError::rpc("list_resources", "connection refused"))
    }
}

#[tokio::test]
async fn a_transport_fault_becomes_command_io_naming_the_rpc() {
    let error = Unreachable
        .capabilities(v1::CapabilitiesRequest {})
        .await
        .expect_err("this transport always fails")
        .into_runtime_error();

    assert_eq!(
        error.code(),
        "command_io",
        "a transport fault is I/O, not engine semantics",
    );
    let rendered = error.to_string();
    assert!(rendered.contains("capabilities"), "must name the rpc: {rendered}");
    assert!(rendered.contains("connection refused"), "must carry the cause: {rendered}");
}

#[test]
fn the_trait_is_usable_behind_a_reference() {
    fn accepts<T: EngineTransport>(_transport: &T) {}
    accepts(&Unreachable);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `env -u RUSTUP_TOOLCHAIN cargo test -p gascan-arca --test transport_contract`
Expected: FAIL — `error: package ID specification 'gascan-arca' did not match any packages`.

- [ ] **Step 3: Create the manifest and register the crate**

Create `crates/gascan-arca/Cargo.toml`:

```toml
[package]
name = "gascan-arca"
version = "0.1.0"
edition.workspace = true
license.workspace = true
rust-version.workspace = true

[dependencies]
async-trait.workspace = true
gascan-core = { path = "../gascan-core" }
gascan-engine-proto = { path = "../gascan-engine-proto" }
hyper-util.workspace = true
prost.workspace = true
thiserror.workspace = true
tokio.workspace = true
tokio-stream.workspace = true
tonic.workspace = true
tower.workspace = true

[dev-dependencies]
camino.workspace = true
tempfile = "3"
tokio = { workspace = true, features = ["macros", "rt", "sync", "time"] }

[lints]
workspace = true
```

`tokio-stream` is used by Task 9 and `camino`/`tempfile` by Task 6's `CreateRequest`
fixture; both are declared now so no later task edits the manifest. `camino` is a
**dev**-dependency only — the library itself needs no path type, because
`RuntimeSandbox` carries no mounts.

In the root `Cargo.toml`, add `"crates/gascan-arca"` to `members`, keeping the list alphabetical:

```toml
members = ["crates/gascan", "crates/gascan-apple", "crates/gascan-arca", "crates/gascan-core", "crates/gascan-e2e", "crates/gascan-engine-proto", "crates/gascan-inherited-fd", "crates/gascan-proto", "crates/gascand"]
```

- [ ] **Step 4: Write the transport module**

Create `crates/gascan-arca/src/transport.rs`:

```rust
use async_trait::async_trait;
use gascan_core::runtime::RuntimeError;
use gascan_engine_proto::v1;
use thiserror::Error;
use tokio::sync::mpsc;

/// A transport fault: an unreachable engine, or a stream that broke.
///
/// The contract reserves gRPC status codes for exactly this and carries every
/// engine meaning in the response body, so engine semantics never arrive here.
#[derive(Debug, Error)]
#[error("{operation}: engine transport failure: {message}")]
pub struct TransportError {
    operation: String,
    message: String,
}

impl TransportError {
    pub fn rpc(operation: &str, message: impl Into<String>) -> Self {
        Self {
            operation: operation.to_owned(),
            message: message.into(),
        }
    }

    /// A transport fault is I/O against the engine, so it reports as
    /// `command_io` — the code the daemon's exec path already expects when a
    /// stream breaks.
    pub fn into_runtime_error(self) -> RuntimeError {
        RuntimeError::CommandIo {
            operation: self.operation,
            message: self.message,
        }
    }
}

/// A live bidirectional exec stream, already opened.
pub struct ExecStream {
    input: mpsc::Sender<v1::ExecClientFrame>,
    output: mpsc::Receiver<Result<v1::ExecServerFrame, TransportError>>,
}

impl ExecStream {
    pub const fn new(
        input: mpsc::Sender<v1::ExecClientFrame>,
        output: mpsc::Receiver<Result<v1::ExecServerFrame, TransportError>>,
    ) -> Self {
        Self { input, output }
    }

    /// Hands both halves to the pump task that owns the session.
    pub fn split(
        self,
    ) -> (
        mpsc::Sender<v1::ExecClientFrame>,
        mpsc::Receiver<Result<v1::ExecServerFrame, TransportError>>,
    ) {
        (self.input, self.output)
    }
}

/// A server-streaming log response.
pub struct LogsStream {
    chunks: mpsc::Receiver<Result<v1::LogsChunk, TransportError>>,
}

impl LogsStream {
    pub const fn new(chunks: mpsc::Receiver<Result<v1::LogsChunk, TransportError>>) -> Self {
        Self { chunks }
    }

    pub async fn recv(&mut self) -> Option<Result<v1::LogsChunk, TransportError>> {
        self.chunks.recv().await
    }
}

/// The engine, in wire types.
///
/// The seam is deliberately stated in the generated types rather than in Gas
/// Can's: a seam in core types would put the mapping below the fake, and the
/// mapping is the part with the bugs.
#[async_trait]
pub trait EngineTransport: Send + Sync {
    async fn capabilities(
        &self,
        request: v1::CapabilitiesRequest,
    ) -> Result<v1::CapabilitiesResponse, TransportError>;

    async fn inspect(
        &self,
        request: v1::InspectRequest,
    ) -> Result<v1::InspectResponse, TransportError>;

    async fn create(&self, request: v1::CreateRequest)
    -> Result<v1::CreateResponse, TransportError>;

    async fn prepare_image(
        &self,
        request: v1::PrepareImageRequest,
    ) -> Result<v1::PrepareImageResponse, TransportError>;

    async fn create_container(
        &self,
        request: v1::CreateContainerRequest,
    ) -> Result<v1::CreateResponse, TransportError>;

    async fn start(&self, request: v1::StartRequest) -> Result<v1::AckResponse, TransportError>;

    async fn stop(&self, request: v1::StopRequest) -> Result<v1::AckResponse, TransportError>;

    async fn remove(&self, request: v1::RemoveRequest) -> Result<v1::AckResponse, TransportError>;

    /// Opens an exec session.
    ///
    /// Takes the `ExecStart` payload, not a first frame: the contract requires
    /// exactly one `ExecStart` and requires it first, so building that frame
    /// here means no implementation of this trait can get it wrong.
    async fn exec(&self, start: v1::ExecStart) -> Result<ExecStream, TransportError>;

    async fn logs(&self, request: v1::LogsRequest) -> Result<LogsStream, TransportError>;

    async fn list_resources(
        &self,
        request: v1::ListResourcesRequest,
    ) -> Result<v1::ListResourcesResponse, TransportError>;
}
```

Create `crates/gascan-arca/src/lib.rs`:

```rust
//! Gas Can's client for Arca's sandbox-engine contract.
//!
//! `ArcaBackend` implements `gascan_core::runtime::RuntimeBackend` over the
//! generated client in `gascan-engine-proto`, behind [`EngineTransport`] so that
//! every mapping is testable without a live engine.
//!
//! The type mapping this crate implements is recorded in
//! `docs/superpowers/specs/2026-08-07-arca-engine-proto-design.md` §9, and the
//! decisions specific to this crate in
//! `docs/superpowers/specs/2026-08-08-gascan-arca-backend-design.md`.

mod transport;

pub use transport::{EngineTransport, ExecStream, LogsStream, TransportError};
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `env -u RUSTUP_TOOLCHAIN cargo test -p gascan-arca --test transport_contract`
Expected: PASS, `running 2 tests`.

Note the first build of this crate runs `gascan-engine-proto`'s build script, which reaches Arca across the signed pin. On a cold cache that touches the network; on a warm cache it is ~3s.

- [ ] **Step 6: Add `async-trait` to dev-dependencies if the test needs it**

The test's `#[async_trait::async_trait]` needs the crate in scope for the test target. `async-trait` is already a normal dependency, which covers integration tests. If the build says otherwise, add `async-trait.workspace = true` to `[dev-dependencies]` rather than changing the test.

- [ ] **Step 7: Clippy, fmt, and commit**

Run: `env -u RUSTUP_TOOLCHAIN cargo clippy -p gascan-arca --all-targets -- -D warnings` and `env -u RUSTUP_TOOLCHAIN cargo fmt --all --check`

Expected: both rc=0. This task needs no `-A dead_code` — everything it introduces has a caller, either the trait's own test or the trait itself. **VERIFIED: Task 2 passed the plain gate.** The `dead_code` problem starts at Task 3, where `translate.rs` arrives ahead of its consumer.

```bash
git add Cargo.toml crates/gascan-arca
git commit -m "feat: add gascan-arca with its engine transport seam

The seam is stated in wire types rather than in Gas Can's, so the mapping
sits above the fake: a seam in core types would put the part with the bugs
underneath the thing that substitutes for the engine.

exec takes an ExecStart payload rather than a first frame. The contract
requires exactly one ExecStart and requires it first, so constructing that
frame inside the transport makes the rule structural instead of a thing
every implementation has to remember.

A transport fault maps to command_io. The contract reserves gRPC status
codes for transport faults and carries engine meaning in the response body,
so nothing semantic ever arrives as a TransportError."
```

---

### Task 3: Outbound mapping — core requests to wire

**Files:**
- Create: `crates/gascan-arca/src/translate.rs`
- Modify: `crates/gascan-arca/src/lib.rs` (add `mod translate;`)

**Interfaces:**
- Consumes: `gascan_core::runtime::{CreateRequest, RecreateRequest, RemoveRequest, ExecRequest, RuntimeBindMount, RuntimeVolume, RuntimePort, RuntimeNetwork, RuntimeUser, RuntimeResourceLimits, OwnershipMetadata, ResourceIdentity, ResourceKind, RuntimeResource, RuntimeError, MANAGED_BY, immutable_image_identity}`.
- Produces, all `pub(crate)`: `image_digest`, `owner_labels`, `project_mount`, `volumes`, `port_mappings`, `resource_limits`, `network`, `user`, `resource_kind`, `resource_identity`, `wire_resource`, `create_request`, `create_container_request`, `remove_request`, `exec_start`, `invalid_output`, `boundary`.

Unit tests live **inside** this module (`#[cfg(test)] mod tests`) so the mapping needs no public surface. Behaviour is tested through `ArcaBackend` in Tasks 6-8.

- [ ] **Step 1: Write the failing tests**

Create `crates/gascan-arca/src/translate.rs` containing only the test module for now, so the first run fails on missing functions:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use gascan_core::runtime::{RuntimeBindMount, RuntimePort};
    use std::net::{IpAddr, Ipv4Addr};

    const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn an_image_reference_splits_into_repository_and_digest_and_drops_the_tag() {
        let digest = image_digest(&format!("registry.example/workspace:1.2@sha256:{DIGEST}"))
            .expect("a named sha256 reference maps");
        assert_eq!(digest.repository, "registry.example/workspace");
        assert_eq!(digest.sha256_hex, DIGEST);
    }

    #[test]
    fn a_reference_without_a_digest_is_refused_rather_than_coerced() {
        let error = image_digest("registry.example/workspace:latest")
            .expect_err("a tag-only reference is not expressible");
        assert_eq!(error.code(), "invalid_state");
    }

    #[test]
    fn exactly_one_writable_project_mount_is_expressible() {
        let mount = RuntimeBindMount {
            source: "/host/project".into(),
            target: "/workspace".into(),
            writable: true,
        };
        let wire = project_mount(std::slice::from_ref(&mount)).expect("one writable mount maps");
        assert_eq!(wire.host_path, "/host/project");
        assert_eq!(wire.guest_path, "/workspace");

        assert_eq!(
            project_mount(&[]).expect_err("zero mounts is not expressible").code(),
            "invalid_state",
        );
        assert_eq!(
            project_mount(&[mount.clone(), mount.clone()])
                .expect_err("two mounts is not expressible")
                .code(),
            "invalid_state",
        );

        let read_only = RuntimeBindMount { writable: false, ..mount };
        assert_eq!(
            project_mount(std::slice::from_ref(&read_only))
                .expect_err("a read-only project mount is not expressible")
                .code(),
            "invalid_state",
        );
    }

    fn port(host: u16, guest: u16) -> RuntimePort {
        RuntimePort {
            host_address: IpAddr::V4(Ipv4Addr::LOCALHOST),
            host_port: host,
            guest_port: guest,
        }
    }

    #[test]
    fn ports_widen_to_uint32_and_keep_their_order() {
        let wire = port_mappings(&[port(22222, 22), port(33333, 80)]).expect("ports map");
        assert_eq!(
            wire.iter().map(|p| (p.host_port, p.guest_port)).collect::<Vec<_>>(),
            [(22222, 22), (33333, 80)],
        );
    }

    #[test]
    fn a_zero_port_a_duplicate_or_a_non_loopback_address_is_refused() {
        assert_eq!(port_mappings(&[port(0, 22)]).expect_err("zero host port").code(), "invalid_state");
        assert_eq!(port_mappings(&[port(22222, 0)]).expect_err("zero guest port").code(), "invalid_state");
        assert_eq!(
            port_mappings(&[port(22222, 22), port(22222, 80)])
                .expect_err("a duplicated host port")
                .code(),
            "invalid_state",
        );

        let routable = RuntimePort {
            host_address: IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)),
            host_port: 22222,
            guest_port: 22,
        };
        assert_eq!(
            port_mappings(std::slice::from_ref(&routable))
                .expect_err("loopback is implied, so a routable address cannot be honoured")
                .code(),
            "invalid_state",
        );
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Add `mod translate;` to `crates/gascan-arca/src/lib.rs`, then run:
`env -u RUSTUP_TOOLCHAIN cargo test -p gascan-arca --lib`
Expected: FAIL to compile — `image_digest`, `project_mount`, `port_mappings` not found.

- [ ] **Step 3: Write the outbound mapping**

Prepend to `crates/gascan-arca/src/translate.rs`, above the test module:

```rust
use gascan_core::runtime::{
    CreateRequest, ExecRequest, MANAGED_BY, OwnershipMetadata, RecreateRequest, RemoveRequest,
    ResourceIdentity, ResourceKind, RuntimeBindMount, RuntimeError, RuntimeNetwork, RuntimePort,
    RuntimeResource, RuntimeResourceLimits, RuntimeUser, RuntimeVolume, immutable_image_identity,
};
use gascan_engine_proto::v1;
use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr};

/// A request or response shape the contract cannot express, or must not coerce.
pub(crate) fn boundary(resource: &str, message: impl Into<String>) -> RuntimeError {
    RuntimeError::InvalidState {
        resource: resource.to_owned(),
        message: message.into(),
    }
}

/// The engine sent something this client cannot read.
pub(crate) fn invalid_output(operation: &str, message: impl Into<String>) -> RuntimeError {
    RuntimeError::InvalidOutput {
        operation: operation.to_owned(),
        message: message.into(),
    }
}

pub(crate) fn image_digest(image: &str) -> Result<v1::ImageDigest, RuntimeError> {
    let (repository, sha256_hex) = immutable_image_identity(image).ok_or_else(|| {
        boundary(
            "engine image",
            format!("image {image:?} is not a named sha256 digest reference"),
        )
    })?;
    Ok(v1::ImageDigest {
        repository: repository.to_owned(),
        sha256_hex: sha256_hex.to_owned(),
    })
}

pub(crate) fn owner_labels(ownership: &OwnershipMetadata) -> v1::OwnerLabels {
    v1::OwnerLabels {
        managed_by: ownership.managed_by.clone(),
        sandbox_id: ownership.sandbox_id.to_string(),
    }
}

pub(crate) fn project_mount(
    mounts: &[RuntimeBindMount],
) -> Result<v1::ProjectMount, RuntimeError> {
    let [mount] = mounts else {
        return Err(boundary(
            "engine project mount",
            format!(
                "exactly one project mount is expressible, found {}",
                mounts.len()
            ),
        ));
    };
    if !mount.writable {
        return Err(boundary(
            "engine project mount",
            "a read-only project mount is not expressible",
        ));
    }
    Ok(v1::ProjectMount {
        host_path: mount.source.to_string(),
        guest_path: mount.target.to_string(),
    })
}

pub(crate) fn volumes(volumes: &[RuntimeVolume]) -> Result<Vec<v1::Volume>, RuntimeError> {
    volumes
        .iter()
        .map(|volume| {
            if !volume.writable {
                return Err(boundary(
                    "engine volume",
                    format!("volume {:?} is read-only, which is not expressible", volume.name),
                ));
            }
            Ok(v1::Volume {
                name: volume.name.clone(),
                guest_path: volume.target.to_string(),
                capacity_bytes: volume.capacity_bytes,
            })
        })
        .collect()
}

/// Loopback is implied by the contract, so a routable address is refused rather
/// than dropped: publishing on loopback when the caller named another address
/// would be a silent change of meaning.
pub(crate) fn port_mappings(ports: &[RuntimePort]) -> Result<Vec<v1::PortMapping>, RuntimeError> {
    let mut seen = BTreeSet::new();
    ports
        .iter()
        .map(|port| {
            if port.host_address != IpAddr::V4(Ipv4Addr::LOCALHOST) {
                return Err(boundary(
                    "engine port mapping",
                    format!(
                        "loopback is implied, so host address {} cannot be requested",
                        port.host_address
                    ),
                ));
            }
            if port.host_port == 0 || port.guest_port == 0 {
                return Err(boundary(
                    "engine port mapping",
                    format!("port 0 is not a mapping: {}:{}", port.host_port, port.guest_port),
                ));
            }
            if !seen.insert(port.host_port) {
                return Err(boundary(
                    "engine port mapping",
                    format!("host port {} is mapped twice", port.host_port),
                ));
            }
            Ok(v1::PortMapping {
                host_port: u32::from(port.host_port),
                guest_port: u32::from(port.guest_port),
            })
        })
        .collect()
}

pub(crate) const fn resource_limits(limits: &RuntimeResourceLimits) -> v1::ResourceLimits {
    v1::ResourceLimits {
        cpus: match limits.cpus {
            Some(cpus) => Some(cpus as u32),
            None => None,
        },
        memory_bytes: limits.memory_bytes,
        disk_bytes: limits.disk_bytes,
        process_count: limits.process_count,
    }
}

pub(crate) fn network(network: &RuntimeNetwork) -> v1::Network {
    v1::Network {
        mode: Some(match network {
            RuntimeNetwork::Offline => v1::network::Mode::Offline(v1::Offline {}),
            RuntimeNetwork::Networked { name } => v1::network::Mode::NetworkedName(name.clone()),
        }),
    }
}

pub(crate) const fn user(user: RuntimeUser) -> v1::User {
    match user {
        RuntimeUser::Workspace => v1::User::Workspace,
        RuntimeUser::Root => v1::User::Root,
    }
}

pub(crate) const fn resource_kind(kind: ResourceKind) -> v1::ResourceKind {
    match kind {
        ResourceKind::Container => v1::ResourceKind::Container,
        ResourceKind::Volume => v1::ResourceKind::Volume,
        ResourceKind::Network => v1::ResourceKind::Network,
    }
}

pub(crate) fn resource_identity(identity: &ResourceIdentity) -> v1::ResourceIdentity {
    v1::ResourceIdentity {
        kind: resource_kind(identity.kind()) as i32,
        name: identity.name().to_owned(),
    }
}

/// A resource on the way out, for `CreateContainerRequest.retained`.
pub(crate) fn wire_resource(resource: &RuntimeResource) -> Result<v1::Resource, RuntimeError> {
    let sandbox_id = resource.sandbox_id().ok_or_else(|| {
        boundary(
            "engine retained resource",
            format!("resource {:?} carries no sandbox id", resource.name()),
        )
    })?;
    Ok(v1::Resource {
        identity: Some(resource_identity(resource.identity())),
        owner: Some(v1::OwnerLabels {
            managed_by: MANAGED_BY.to_owned(),
            sandbox_id: sandbox_id.to_string(),
        }),
    })
}

pub(crate) fn create_request(request: &CreateRequest) -> Result<v1::CreateRequest, RuntimeError> {
    Ok(v1::CreateRequest {
        sandbox_id: request.id().to_string(),
        image: Some(image_digest(request.image())?),
        project: Some(project_mount(request.bind_mounts())?),
        volumes: volumes(request.volumes())?,
        ports: port_mappings(request.ports())?,
        environment: request
            .environment()
            .iter()
            .map(|(name, value)| v1::EnvironmentVariable {
                name: name.clone(),
                value: value.clone(),
            })
            .collect(),
        resources: Some(resource_limits(request.resources())),
        network: Some(network(request.network())),
        user: user(request.user()) as i32,
        init: request.init(),
        owner: Some(owner_labels(request.ownership())),
    })
}

pub(crate) fn create_container_request(
    request: &RecreateRequest,
) -> Result<v1::CreateContainerRequest, RuntimeError> {
    Ok(v1::CreateContainerRequest {
        create: Some(create_request(request.create())?),
        retained: request
            .retained()
            .resources()
            .iter()
            .map(wire_resource)
            .collect::<Result<Vec<_>, _>>()?,
    })
}

/// One call carries one sandbox's labels.
///
/// Core's `RemoveRequest` holds resources that each carry their own sandbox id
/// and does not require them to agree; the wire carries a single `OwnerLabels`
/// for the whole call. Sending the first resource's labels for a mixed request
/// would ask the engine to delete under labels that do not describe every named
/// resource, so a mixed request is refused instead.
pub(crate) fn remove_request(
    request: &RemoveRequest,
) -> Result<v1::RemoveRequest, RuntimeError> {
    let mut owner = None;
    for resource in request.resources() {
        let id = resource.sandbox_id().ok_or_else(|| {
            RuntimeError::OwnershipMismatch {
                resource: resource.name().to_owned(),
            }
        })?;
        match owner {
            None => owner = Some(id),
            Some(existing) if existing == id => {}
            Some(existing) => {
                return Err(boundary(
                    "engine remove request",
                    format!(
                        "one remove call carries one sandbox's labels; found {existing} and {id}"
                    ),
                ));
            }
        }
    }
    let owner = owner.ok_or_else(|| {
        boundary("engine remove request", "at least one resource is required")
    })?;
    Ok(v1::RemoveRequest {
        resources: request
            .resources()
            .iter()
            .map(|resource| resource_identity(resource.identity()))
            .collect(),
        owner: Some(v1::OwnerLabels {
            managed_by: MANAGED_BY.to_owned(),
            sandbox_id: owner.to_string(),
        }),
    })
}

/// `argv` widens losslessly: the wire takes bytes because `execve` does.
pub(crate) fn exec_start(request: &ExecRequest) -> v1::ExecStart {
    v1::ExecStart {
        sandbox_id: request.id.to_string(),
        argv: request
            .argv
            .iter()
            .map(|argument| argument.clone().into_bytes())
            .collect(),
        environment: request
            .environment
            .iter()
            .map(|(name, value)| v1::EnvironmentVariable {
                name: name.clone(),
                value: value.clone(),
            })
            .collect(),
        tty: request.tty,
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `env -u RUSTUP_TOOLCHAIN cargo test -p gascan-arca --lib`
Expected: PASS, `running 5 tests`.

If `resource_limits` will not compile as `const fn` because of the `as u32` cast in a match, drop `const` from it rather than restructuring the mapping.

- [ ] **Step 5: Clippy, fmt, and commit**

Run: `env -u RUSTUP_TOOLCHAIN cargo clippy -p gascan-arca --all-targets -- -D warnings -A dead_code` and `env -u RUSTUP_TOOLCHAIN cargo fmt --all --check`

`-A dead_code` per the Global Constraints: these functions have no caller until Task 6. Do not add an allow attribute to the source.

```bash
git add crates/gascan-arca
git commit -m "feat: map Gas Can requests onto the engine contract

Every place the two shapes disagree refuses rather than coerces: a project
mount that is not exactly one writable mount, a read-only volume, a zero or
duplicated port, an image without a digest, and a remove request spanning
two sandboxes.

Two of those refusals are worth naming. Loopback is implied by the contract,
so a routable host address is refused instead of dropped -- publishing on
loopback when the caller named another address would silently change what
was asked for. And core's RemoveRequest does not require its resources to
share a sandbox while the wire carries one label set per call, so a mixed
request is refused rather than sent under labels that describe only part of
it.

The image split reuses gascan-core's immutable_image_identity, which is the
same function same_immutable_image compares by, so the canonicalisation
agrees with every existing comparison by construction."
```

---

### Task 4: Inbound mapping — wire responses to core

**Files:**
- Modify: `crates/gascan-arca/src/translate.rs`

**Interfaces:**
- Consumes: Task 3's `invalid_output`; `gascan_core::runtime::{RuntimeCapabilities, RuntimeVersion, NetworkIsolation, ContainerState, RuntimeSandbox, RuntimeResource, ResourceOwnership, SandboxLabel, classify_resource_ownership, immutable_image_reference}`, `gascan_core::sandbox::SandboxId`.
- Produces, all `pub(crate)`: `runtime_capabilities`, `runtime_image`, `runtime_ports`, `runtime_sandbox`, `runtime_resource`, `runtime_resources`, `missing_outcome`.

- [ ] **Step 1: Write the failing tests**

Append to the `mod tests` block in `crates/gascan-arca/src/translate.rs`:

```rust
    use gascan_core::runtime::{ContainerState, NetworkIsolation, ResourceOwnership};

    fn wire_owner(sandbox_id: &str) -> v1::OwnerLabels {
        v1::OwnerLabels {
            managed_by: "gascan".to_owned(),
            sandbox_id: sandbox_id.to_owned(),
        }
    }

    #[test]
    fn capabilities_rename_project_mount_and_widen_the_version() {
        let capabilities = runtime_capabilities(&v1::Capabilities {
            engine_version: Some(v1::Version { major: 1, minor: 2, patch: 3 }),
            contract_minor: 0,
            project_mount: true,
            named_volumes: true,
            tty: true,
            signals: true,
            loopback_publish: true,
            resource_limits: true,
            offline: v1::Isolation::Proven as i32,
        })
        .expect("a fully specified capability set maps");

        assert_eq!(capabilities.version, gascan_core::runtime::RuntimeVersion::new(1, 2, 3));
        assert!(capabilities.bind_mounts, "project_mount is Gas Can's bind_mounts");
        assert_eq!(capabilities.offline, NetworkIsolation::Proven);
    }

    #[test]
    fn an_unspecified_isolation_or_absent_version_is_refused() {
        let unspecified = v1::Capabilities {
            engine_version: Some(v1::Version { major: 1, minor: 0, patch: 0 }),
            contract_minor: 0,
            project_mount: true,
            named_volumes: true,
            tty: true,
            signals: true,
            loopback_publish: true,
            resource_limits: true,
            offline: v1::Isolation::Unspecified as i32,
        };
        assert_eq!(
            runtime_capabilities(&unspecified).expect_err("unspecified is not a value").code(),
            "invalid_output",
        );

        let versionless = v1::Capabilities { engine_version: None, ..unspecified };
        assert_eq!(
            runtime_capabilities(&versionless).expect_err("no version").code(),
            "invalid_output",
        );
    }

    #[test]
    fn an_image_digest_reassembles_into_a_canonical_reference() {
        let image = runtime_image(Some(&v1::ImageDigest {
            repository: "registry.example/workspace".to_owned(),
            sha256_hex: DIGEST.to_owned(),
        }))
        .expect("a digest reassembles");
        assert_eq!(image, format!("registry.example/workspace@sha256:{DIGEST}"));
    }

    #[test]
    fn a_malformed_digest_is_refused_rather_than_concatenated() {
        assert_eq!(
            runtime_image(Some(&v1::ImageDigest {
                repository: "registry.example/workspace".to_owned(),
                sha256_hex: "not-a-digest".to_owned(),
            }))
            .expect_err("a short digest is not a reference")
            .code(),
            "invalid_output",
        );
        assert_eq!(
            runtime_image(None).expect_err("no image at all").code(),
            "invalid_output",
        );
    }

    #[test]
    fn inbound_ports_regain_the_loopback_address_they_never_sent() {
        let ports = runtime_ports(&[v1::PortMapping { host_port: 22222, guest_port: 22 }])
            .expect("a port maps");
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].host_address, IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_eq!((ports[0].host_port, ports[0].guest_port), (22222, 22));
    }

    #[test]
    fn an_out_of_range_zero_or_duplicated_inbound_port_is_refused() {
        for ports in [
            vec![v1::PortMapping { host_port: 65_536, guest_port: 22 }],
            vec![v1::PortMapping { host_port: 22222, guest_port: 70_000 }],
            vec![v1::PortMapping { host_port: 0, guest_port: 22 }],
            vec![v1::PortMapping { host_port: 22222, guest_port: 0 }],
            vec![
                v1::PortMapping { host_port: 22222, guest_port: 22 },
                v1::PortMapping { host_port: 22222, guest_port: 80 },
            ],
        ] {
            assert_eq!(
                runtime_ports(&ports).expect_err("must fail closed").code(),
                "invalid_output",
                "ports: {ports:?}",
            );
        }
    }

    #[test]
    fn a_sandbox_maps_and_its_labels_must_agree_with_its_id() {
        let id = gascan_core::sandbox::SandboxId::test("observed");
        let sandbox = v1::Sandbox {
            sandbox_id: id.as_str().to_owned(),
            image: Some(v1::ImageDigest {
                repository: "registry.example/workspace".to_owned(),
                sha256_hex: DIGEST.to_owned(),
            }),
            state: v1::SandboxState::Running as i32,
            owner: Some(wire_owner(id.as_str())),
            ports: Vec::new(),
        };
        let observed = runtime_sandbox(&sandbox).expect("a labelled running sandbox maps");
        assert_eq!(observed.state, ContainerState::Running);
        assert_eq!(observed.ownership.managed_by, "gascan");

        let disagreeing = v1::Sandbox {
            owner: Some(wire_owner(gascan_core::sandbox::SandboxId::test("other").as_str())),
            ..sandbox.clone()
        };
        assert_eq!(
            runtime_sandbox(&disagreeing).expect_err("labels must describe this sandbox").code(),
            "ownership_mismatch",
        );

        let unlabelled = v1::Sandbox { owner: None, ..sandbox.clone() };
        assert_eq!(
            runtime_sandbox(&unlabelled).expect_err("a sandbox must be labelled").code(),
            "invalid_output",
        );

        let stateless = v1::Sandbox { state: v1::SandboxState::Unspecified as i32, ..sandbox };
        assert_eq!(
            runtime_sandbox(&stateless).expect_err("unspecified is not a state").code(),
            "unknown_actual_state",
        );
    }

    #[test]
    fn a_resource_is_classified_by_the_shared_rule() {
        let id = gascan_core::sandbox::SandboxId::test("owned");
        let container = v1::Resource {
            identity: Some(v1::ResourceIdentity {
                kind: v1::ResourceKind::Container as i32,
                name: id.as_str().to_owned(),
            }),
            owner: Some(wire_owner(id.as_str())),
        };
        assert_eq!(
            runtime_resource(&container).expect("maps").ownership(),
            ResourceOwnership::GasCanOwned,
        );

        let unlabelled = v1::Resource { owner: None, ..container.clone() };
        assert_eq!(
            runtime_resource(&unlabelled).expect("maps").ownership(),
            ResourceOwnership::Foreign,
            "ListResources returns unlabelled resources on purpose; they are not an error",
        );

        let unparseable = v1::Resource {
            owner: Some(v1::OwnerLabels {
                managed_by: "gascan".to_owned(),
                sandbox_id: "not a valid id".to_owned(),
            }),
            ..container.clone()
        };
        assert_eq!(
            runtime_resource(&unparseable).expect("maps").ownership(),
            ResourceOwnership::Mismatched,
            "one malformed label must not blind the consumer to the rest of the inventory",
        );

        let kindless = v1::Resource {
            identity: Some(v1::ResourceIdentity {
                kind: v1::ResourceKind::Unspecified as i32,
                name: id.as_str().to_owned(),
            }),
            ..container.clone()
        };
        assert_eq!(
            runtime_resource(&kindless).expect_err("unspecified is not a kind").code(),
            "invalid_output",
        );

        let identityless = v1::Resource {
            identity: None,
            ..container
        };
        assert_eq!(
            runtime_resource(&identityless)
                .expect_err("a resource with no identity is not addressable")
                .code(),
            "invalid_output",
        );
    }

    #[test]
    fn a_mismatched_resource_still_reports_the_sandbox_id_it_claims() {
        // Parity with gascan-apple's inspect.rs, which reports the claimed id for a
        // mismatched resource because the reconciler finds it by that claim. If the
        // two backends disagree here, one reports an ownership mismatch the other
        // drops -- the divergence Task 1's shared classifier exists to prevent.
        let claimed = gascan_core::sandbox::SandboxId::test("claimed");
        let collision = v1::Resource {
            identity: Some(v1::ResourceIdentity {
                kind: v1::ResourceKind::Container as i32,
                name: "a-name-that-is-not-the-label".to_owned(),
            }),
            owner: Some(wire_owner(claimed.as_str())),
        };
        let resource = runtime_resource(&collision).expect("a collision still maps");
        assert_eq!(resource.ownership(), ResourceOwnership::Mismatched);
        assert_eq!(
            resource.sandbox_id().map(gascan_core::sandbox::SandboxId::as_str),
            Some(claimed.as_str()),
        );
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `env -u RUSTUP_TOOLCHAIN cargo test -p gascan-arca --lib`
Expected: FAIL to compile — `runtime_capabilities`, `runtime_image`, `runtime_ports`, `runtime_sandbox`, `runtime_resource` not found.

- [ ] **Step 3: Write the inbound mapping**

Extend the import list at the top of `translate.rs` with `ContainerState, NetworkIsolation, OwnershipMetadata, ResourceOwnership, RuntimeCapabilities, RuntimeSandbox, RuntimeVersion, SandboxLabel, classify_resource_ownership, immutable_image_reference` and add `use gascan_core::sandbox::SandboxId;`. Then append the mapping functions:

```rust
/// A response whose `oneof` is unset. proto3 makes that representable, and it
/// means the engine sent a message this client cannot interpret.
pub(crate) fn missing_outcome(operation: &str) -> RuntimeError {
    invalid_output(operation, "response carried no outcome")
}

pub(crate) fn runtime_capabilities(
    capabilities: &v1::Capabilities,
) -> Result<RuntimeCapabilities, RuntimeError> {
    let version = capabilities
        .engine_version
        .as_ref()
        .ok_or_else(|| invalid_output("capabilities", "response carried no engine version"))?;
    let offline = match v1::Isolation::try_from(capabilities.offline) {
        Ok(v1::Isolation::Proven) => NetworkIsolation::Proven,
        Ok(v1::Isolation::Unsupported) => NetworkIsolation::Unsupported,
        Ok(v1::Isolation::Unverified) => NetworkIsolation::Unverified,
        Ok(v1::Isolation::Unspecified) | Err(_) => {
            return Err(invalid_output(
                "capabilities",
                format!("offline isolation {} is not a value", capabilities.offline),
            ));
        }
    };
    // contract_minor is deliberately read and dropped: this client populates no
    // additive fields yet, so knowing which it may find tells it nothing.
    Ok(RuntimeCapabilities {
        version: RuntimeVersion::new(
            u64::from(version.major),
            u64::from(version.minor),
            u64::from(version.patch),
        ),
        bind_mounts: capabilities.project_mount,
        named_volumes: capabilities.named_volumes,
        tty: capabilities.tty,
        signals: capabilities.signals,
        loopback_publish: capabilities.loopback_publish,
        resource_limits: capabilities.resource_limits,
        offline,
    })
}

/// Reassembles the canonical reference, then asserts it is one.
///
/// The result is deterministic, which is what lets the daemon compare one
/// observation against another by exact string.
pub(crate) fn runtime_image(image: Option<&v1::ImageDigest>) -> Result<String, RuntimeError> {
    let image =
        image.ok_or_else(|| invalid_output("inspect", "response carried no image digest"))?;
    let reference = format!("{}@sha256:{}", image.repository, image.sha256_hex);
    if !immutable_image_reference(&reference) {
        return Err(invalid_output(
            "inspect",
            format!("engine image {reference:?} is not a named sha256 digest reference"),
        ));
    }
    Ok(reference)
}

/// Loopback is not on the wire because it is the only case, so it is restored
/// here. Every construction site in the policy compiler uses the same address,
/// so this round-trips exactly.
pub(crate) fn runtime_ports(
    ports: &[v1::PortMapping],
) -> Result<Vec<RuntimePort>, RuntimeError> {
    let mut seen = BTreeSet::new();
    ports
        .iter()
        .map(|port| {
            let host_port = u16::try_from(port.host_port).map_err(|_| {
                invalid_output("inspect", format!("host port {} is out of range", port.host_port))
            })?;
            let guest_port = u16::try_from(port.guest_port).map_err(|_| {
                invalid_output("inspect", format!("guest port {} is out of range", port.guest_port))
            })?;
            if host_port == 0 || guest_port == 0 {
                return Err(invalid_output(
                    "inspect",
                    format!("port 0 is not a mapping: {host_port}:{guest_port}"),
                ));
            }
            if !seen.insert(host_port) {
                return Err(invalid_output(
                    "inspect",
                    format!("host port {host_port} is published twice"),
                ));
            }
            Ok(RuntimePort {
                host_address: IpAddr::V4(Ipv4Addr::LOCALHOST),
                host_port,
                guest_port,
            })
        })
        .collect()
}

pub(crate) fn runtime_sandbox(sandbox: &v1::Sandbox) -> Result<RuntimeSandbox, RuntimeError> {
    let id = SandboxId::try_from(sandbox.sandbox_id.clone()).map_err(|error| {
        invalid_output(
            "inspect",
            format!("sandbox id {:?} is invalid: {error}", sandbox.sandbox_id),
        )
    })?;
    let image = runtime_image(sandbox.image.as_ref())?;
    let state = match v1::SandboxState::try_from(sandbox.state) {
        Ok(v1::SandboxState::Creating) => ContainerState::Creating,
        Ok(v1::SandboxState::Running) => ContainerState::Running,
        Ok(v1::SandboxState::Stopped) => ContainerState::Stopped,
        Ok(v1::SandboxState::Unspecified) | Err(_) => {
            return Err(RuntimeError::UnknownActualState {
                resource: id.to_string(),
                state: sandbox.state.to_string(),
            });
        }
    };
    // A sandbox must be labelled, as the Apple backend also requires: an
    // unlabelled container is not one this client may claim to own.
    let owner = sandbox
        .owner
        .as_ref()
        .ok_or_else(|| invalid_output("inspect", format!("sandbox {id} carries no owner labels")))?;
    let sandbox_id = SandboxId::try_from(owner.sandbox_id.clone()).map_err(|error| {
        invalid_output(
            "inspect",
            format!("sandbox {id} has an invalid sandbox-id label: {error}"),
        )
    })?;
    if sandbox_id != id {
        return Err(RuntimeError::OwnershipMismatch {
            resource: id.to_string(),
        });
    }
    let ownership = OwnershipMetadata {
        managed_by: owner.managed_by.clone(),
        sandbox_id,
    };
    Ok(RuntimeSandbox::observed(
        id,
        image,
        state,
        ownership,
        runtime_ports(&sandbox.ports)?,
    ))
}

pub(crate) fn runtime_resource(
    resource: &v1::Resource,
) -> Result<RuntimeResource, RuntimeError> {
    let identity = resource
        .identity
        .as_ref()
        .ok_or_else(|| invalid_output("list_resources", "resource carried no identity"))?;
    let kind = match v1::ResourceKind::try_from(identity.kind) {
        Ok(v1::ResourceKind::Container) => ResourceKind::Container,
        Ok(v1::ResourceKind::Volume) => ResourceKind::Volume,
        Ok(v1::ResourceKind::Network) => ResourceKind::Network,
        Ok(v1::ResourceKind::Unspecified) | Err(_) => {
            return Err(invalid_output(
                "list_resources",
                format!("resource kind {} is not a value", identity.kind),
            ));
        }
    };
    let core_identity = ResourceIdentity::new(kind, identity.name.clone())?;
    let owner = resource.owner.as_ref();
    // An unparseable label is Mismatched, not a failed call: ListResources
    // returns every resource the engine holds so that drift detection can see
    // them, and one malformed foreign label must not hide the rest.
    let parsed = owner.and_then(|owner| SandboxId::try_from(owner.sandbox_id.clone()).ok());
    let label = match (owner, &parsed) {
        (None, _) => SandboxLabel::Absent,
        (Some(_), Some(id)) => SandboxLabel::Parsed(id),
        (Some(_), None) => SandboxLabel::Unparseable,
    };
    let ownership = classify_resource_ownership(
        kind,
        &identity.name,
        owner.map(|owner| owner.managed_by.as_str()),
        label,
    );
    // Some(id) whenever OUR label parsed, including when the resource is
    // Mismatched. **CORRECTED 2026-08-08 — this file previously said
    // `match ownership { GasCanOwned => parsed, _ => None }`, which is the exact
    // rule Task 1's fix round reverted as a regression.** `gascan-apple`'s
    // `inspect.rs` reports the claimed id for a mismatched resource because the
    // reconciler at `gascand/src/service.rs:3001-3012` finds it by that claim.
    // The two backends MUST agree here — a divergence is what Task 1 exists to
    // prevent, and it would mean one backend reports an ownership mismatch that
    // the other silently drops.
    let sandbox_id = if owner.map(|owner| owner.managed_by.as_str()) == Some(MANAGED_BY) {
        parsed
    } else {
        None
    };
    Ok(RuntimeResource::discovered(core_identity, sandbox_id, ownership))
}

pub(crate) fn runtime_resources(
    resources: &[v1::Resource],
) -> Result<Vec<RuntimeResource>, RuntimeError> {
    resources.iter().map(runtime_resource).collect()
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `env -u RUSTUP_TOOLCHAIN cargo test -p gascan-arca --lib`
Expected: PASS, `running 16 tests` — Task 3's 7 plus this task's 9. **Updated 2026-08-08:** was `13`, from Task 3's original 5 plus 8 here, before Task 3's review added two refusal tests and this task gained the mismatched-resource parity test.

- [ ] **Step 5: Clippy, fmt, and commit**

```bash
git add crates/gascan-arca
git commit -m "feat: map engine responses onto Gas Can's runtime types

Ports regain the loopback address the wire never carries, which round-trips
exactly because every construction site in the policy compiler uses that one
address. An image digest reassembles into a canonical reference and is then
asserted to be one, so the result is deterministic -- which is what lets the
daemon compare one observation against another by exact string.

An unspecified enum, an absent version, an unlabelled sandbox and a sandbox
whose labels name a different id all fail closed. A resource with an
unparseable label does not: ListResources deliberately returns every
resource the engine holds so that drift detection can see them, so one
malformed foreign label is Mismatched rather than a failed listing.

contract_minor is read and dropped, recorded here so it reads as considered
rather than missed."
```

---

### Task 5: The `EngineError` code table

**Files:**
- Create: `crates/gascan-arca/src/error.rs`
- Modify: `crates/gascan-arca/src/lib.rs` (add `mod error;`)

**Interfaces:**
- Consumes: `gascan_core::runtime::RuntimeError`, `gascan_engine_proto::v1::EngineError`.
- Produces: `pub(crate) fn engine_error(operation: &str, error: &v1::EngineError) -> RuntimeError`.

**Why a table and not a single variant.** The code string reaches the user: `gascan/src/cli.rs:250-252` rewrites the message a user sees when the stable code is `resource_conflict`, and `gascand/src/service.rs` calls `.code()` at 26 sites, 9 of them stamping it into a telemetry `reason`. And `RuntimeError::code()` returns `&'static str`, so a dynamic code cannot flow through — every accepted code must land on a known variant.

- [ ] **Step 1: Write the failing tests**

Create `crates/gascan-arca/src/error.rs` with only its test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn wire(code: &str) -> v1::EngineError {
        v1::EngineError {
            code: code.to_owned(),
            resource: "code-a1b2c3d4e5f6".to_owned(),
            message: "the engine said so".to_owned(),
        }
    }

    #[test]
    fn every_accepted_code_round_trips_to_itself() {
        for code in [
            "command_io",
            "command_failed",
            "invalid_output",
            "helper_error",
            "unsupported_capability",
            "ownership_mismatch",
            "foreign_resource_refused",
            "invalid_resource_identity",
            "resource_conflict",
            "not_found",
            "invalid_state",
            "unknown_actual_state",
        ] {
            assert_eq!(
                engine_error("create", &wire(code)).code(),
                code,
                "an accepted code must map to the variant that reports it",
            );
        }
    }

    #[test]
    fn an_unknown_code_is_rejected_and_names_itself() {
        let error = engine_error("create", &wire("quantum_flux"));
        assert_eq!(error.code(), "invalid_output");
        let rendered = error.to_string();
        assert!(rendered.contains("quantum_flux"), "must name the code: {rendered}");
    }

    #[test]
    fn a_code_no_engine_may_raise_is_rejected() {
        for code in ["injected_failure", "unsupported_version"] {
            let error = engine_error("create", &wire(code));
            assert_eq!(
                error.code(),
                "invalid_output",
                "{code} is not an engine's to raise",
            );
            assert!(error.to_string().contains(code), "must name {code}");
        }
    }

    #[test]
    fn an_empty_resource_passes_through_rather_than_failing_the_call() {
        let error = engine_error(
            "start",
            &v1::EngineError {
                code: "command_io".to_owned(),
                resource: String::new(),
                message: "socket closed".to_owned(),
            },
        );
        assert_eq!(error.code(), "command_io");
        assert!(error.to_string().contains("socket closed"));
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Add `mod error;` to `lib.rs`, then run: `env -u RUSTUP_TOOLCHAIN cargo test -p gascan-arca --lib error::`
Expected: FAIL to compile — `engine_error` not found.

- [ ] **Step 3: Write the table**

Prepend to `crates/gascan-arca/src/error.rs`:

```rust
use gascan_core::runtime::RuntimeError;
use gascan_engine_proto::v1;

/// Maps an engine failure onto the variant that reports its code.
///
/// A table, not a judgment: the contract's own instruction is that a consumer
/// maps this with a table "so a new engine failure mode cannot quietly become an
/// existing one". Two codes are not an engine's to raise -- `injected_failure`
/// belongs to the fake runtime, and `unsupported_version` is the consumer's own
/// refusal to drive an engine and carries a version the wire never sends -- so
/// they are rejected alongside anything unrecognised.
///
/// Fields the wire cannot carry come from the RPC name, `None`, or the message.
/// An empty `resource` passes through: the contract says it is empty when the
/// failure is not about one, and failing the call over an empty diagnostic field
/// would replace a readable engine error with a confusing protocol one.
pub(crate) fn engine_error(operation: &str, error: &v1::EngineError) -> RuntimeError {
    let resource = error.resource.clone();
    let message = error.message.clone();
    match error.code.as_str() {
        "command_io" => RuntimeError::CommandIo {
            operation: operation.to_owned(),
            message,
        },
        "command_failed" => RuntimeError::CommandFailed {
            operation: operation.to_owned(),
            exit_code: None,
            stderr: message,
        },
        "invalid_output" => RuntimeError::InvalidOutput {
            operation: operation.to_owned(),
            message,
        },
        // The inner code is the wire code. Gas Can's own helper errors carry a
        // nested code, but `RuntimeError::code()` flattens it to "helper_error"
        // on the way out, so there is no field for the engine to have sent it in
        // and nothing to recover.
        "helper_error" => RuntimeError::HelperError {
            operation: operation.to_owned(),
            code: error.code.clone(),
            message,
        },
        "unsupported_capability" => RuntimeError::UnsupportedCapability {
            capability: message,
        },
        "ownership_mismatch" => RuntimeError::OwnershipMismatch { resource },
        "foreign_resource_refused" => RuntimeError::ForeignResourceRefused { resource },
        "invalid_resource_identity" => RuntimeError::InvalidResourceIdentity { name: resource },
        "resource_conflict" => RuntimeError::Conflict { resource, message },
        "not_found" => RuntimeError::NotFound { resource },
        "invalid_state" => RuntimeError::InvalidState { resource, message },
        "unknown_actual_state" => RuntimeError::UnknownActualState {
            resource,
            state: message,
        },
        unacceptable => RuntimeError::InvalidOutput {
            operation: operation.to_owned(),
            message: format!(
                "engine returned unacceptable error code {unacceptable:?}: {message}"
            ),
        },
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `env -u RUSTUP_TOOLCHAIN cargo test -p gascan-arca --lib error::`
Expected: PASS, `running 4 tests`.

- [ ] **Step 5: Clippy, fmt, and commit**

```bash
git add crates/gascan-arca
git commit -m "feat: map engine error codes onto RuntimeError by table

A table rather than a judgment, because the contract asks for one and
because the code string is load-bearing: the CLI rewrites the message a user
sees for resource_conflict, and RuntimeError::code() returns a &'static str,
so a dynamic code physically cannot flow through.

Two of the fourteen codes are not an engine's to raise. injected_failure
belongs to the fake runtime and unsupported_version is the consumer's own
refusal to drive an engine, carrying a version the wire never sends. Both
are rejected alongside anything unrecognised, as InvalidOutput naming the
offending code so it cannot alias a code that is accepted.

An empty resource field passes through. It is empty by design when the
failure is not about a resource, and failing the call over an empty
diagnostic field would turn a readable engine error into a confusing
protocol one."
```

---

### Task 6: `ArcaBackend` and the nine unary methods

**Files:**
- Create: `crates/gascan-arca/src/backend.rs`, `crates/gascan-arca/tests/fake_transport/mod.rs`, `crates/gascan-arca/tests/backend_unary.rs`
- Modify: `crates/gascan-arca/src/lib.rs`

**Interfaces:**
- Consumes: Tasks 2-5.
- Produces: `pub struct ArcaBackend<T>`, `ArcaBackend::new(transport: T) -> Self`, and `impl<T: EngineTransport> RuntimeBackend for ArcaBackend<T>` — nine methods here, `logs` in Task 7, `exec` in Task 8.

**On a create that fails partway.** `CreateFailed` carries the resources already made so the caller can remove exactly them. If any of those resources fails to map, this backend returns `CreateFailure::from_source(mapping_error)` rather than a filtered list: a malformed `Resource` is a contract violation, its identity is by definition unknown, so it could not be removed either way, and the actionable fact for an operator is that the engine sent something malformed. The engine's own message is lost in that case, which is acceptable because a protocol-violating engine's error text is not evidence.

- [ ] **Step 1: Write the fake transport**

Create `crates/gascan-arca/tests/fake_transport/mod.rs`:

```rust
use gascan_arca::{EngineTransport, ExecStream, LogsStream, TransportError};
use gascan_engine_proto::v1;
use std::sync::Mutex;

/// A scripted engine. Each field holds the response the matching RPC returns;
/// `calls` records what was asked, so a test can assert on the request the
/// mapping produced as well as on the answer it made of the response.
#[derive(Default)]
pub struct FakeEngine {
    pub capabilities: Mutex<Option<v1::CapabilitiesResponse>>,
    pub inspect: Mutex<Option<v1::InspectResponse>>,
    pub create: Mutex<Option<v1::CreateResponse>>,
    pub prepare_image: Mutex<Option<v1::PrepareImageResponse>>,
    pub ack: Mutex<Option<v1::AckResponse>>,
    pub list_resources: Mutex<Option<v1::ListResourcesResponse>>,
    pub calls: Mutex<Vec<Call>>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Call {
    Capabilities,
    Inspect(v1::InspectRequest),
    Create(v1::CreateRequest),
    PrepareImage(v1::PrepareImageRequest),
    CreateContainer(v1::CreateContainerRequest),
    Start(v1::StartRequest),
    Stop(v1::StopRequest),
    Remove(v1::RemoveRequest),
    ListResources,
}

impl FakeEngine {
    pub fn record(&self, call: Call) {
        self.calls.lock().expect("test lock").push(call);
    }

    pub fn calls(&self) -> Vec<Call> {
        self.calls.lock().expect("test lock").clone()
    }

    fn take<T>(slot: &Mutex<Option<T>>, operation: &str) -> Result<T, TransportError> {
        slot.lock()
            .expect("test lock")
            .take()
            .ok_or_else(|| TransportError::rpc(operation, "the test scripted no response"))
    }

    pub fn ok_ack() -> v1::AckResponse {
        v1::AckResponse {
            outcome: Some(v1::ack_response::Outcome::Ok(v1::Ack {})),
        }
    }

    pub fn engine_error(code: &str) -> v1::EngineError {
        v1::EngineError {
            code: code.to_owned(),
            resource: "code-a1b2c3d4e5f6".to_owned(),
            message: "the engine refused".to_owned(),
        }
    }
}

#[async_trait::async_trait]
impl EngineTransport for FakeEngine {
    async fn capabilities(
        &self,
        _request: v1::CapabilitiesRequest,
    ) -> Result<v1::CapabilitiesResponse, TransportError> {
        self.record(Call::Capabilities);
        Self::take(&self.capabilities, "capabilities")
    }

    async fn inspect(
        &self,
        request: v1::InspectRequest,
    ) -> Result<v1::InspectResponse, TransportError> {
        self.record(Call::Inspect(request));
        Self::take(&self.inspect, "inspect")
    }

    async fn create(
        &self,
        request: v1::CreateRequest,
    ) -> Result<v1::CreateResponse, TransportError> {
        self.record(Call::Create(request));
        Self::take(&self.create, "create")
    }

    async fn prepare_image(
        &self,
        request: v1::PrepareImageRequest,
    ) -> Result<v1::PrepareImageResponse, TransportError> {
        self.record(Call::PrepareImage(request));
        Self::take(&self.prepare_image, "prepare_image")
    }

    async fn create_container(
        &self,
        request: v1::CreateContainerRequest,
    ) -> Result<v1::CreateResponse, TransportError> {
        self.record(Call::CreateContainer(request));
        Self::take(&self.create, "create_container")
    }

    async fn start(&self, request: v1::StartRequest) -> Result<v1::AckResponse, TransportError> {
        self.record(Call::Start(request));
        Self::take(&self.ack, "start")
    }

    async fn stop(&self, request: v1::StopRequest) -> Result<v1::AckResponse, TransportError> {
        self.record(Call::Stop(request));
        Self::take(&self.ack, "stop")
    }

    async fn remove(&self, request: v1::RemoveRequest) -> Result<v1::AckResponse, TransportError> {
        self.record(Call::Remove(request));
        Self::take(&self.ack, "remove")
    }

    async fn exec(&self, _start: v1::ExecStart) -> Result<ExecStream, TransportError> {
        Err(TransportError::rpc("exec", "this fake scripts no exec"))
    }

    async fn logs(&self, _request: v1::LogsRequest) -> Result<LogsStream, TransportError> {
        Err(TransportError::rpc("logs", "this fake scripts no logs"))
    }

    async fn list_resources(
        &self,
        _request: v1::ListResourcesRequest,
    ) -> Result<v1::ListResourcesResponse, TransportError> {
        self.record(Call::ListResources);
        Self::take(&self.list_resources, "list_resources")
    }
}

/// A policy-validated `CreateRequest`, which is the only kind that exists.
///
/// `CreateRequest`'s fields are `pub(crate)` to `gascan-core` and it derives no
/// `Deserialize`, so `PolicyCompiler` is the only construction path — there is
/// deliberately no fixture constructor. This mirrors
/// `gascan-apple/tests/backend_fake_runner.rs:411-431`, which solves the same
/// problem the same way. The `TempDir` must outlive the request: the compiled
/// request names its canonical root.
pub fn policy_request(name: &str) -> (tempfile::TempDir, gascan_core::runtime::CreateRequest) {
    use camino::Utf8Path;
    use gascan_core::manifest::Manifest;
    use gascan_core::policy::PolicyCompiler;
    use gascan_core::runtime::{NetworkIsolation, RuntimeCapabilities, RuntimeVersion};
    use gascan_core::sandbox::SandboxSpec;

    let root = tempfile::tempdir().expect("a temporary project root");
    let path = Utf8Path::from_path(root.path()).expect("a utf-8 temporary path");
    std::fs::write(path.join("gascan.toml"), "version = 1\nnetwork = 'networked'\n")
        .expect("a manifest");
    let spec = SandboxSpec::from_root(name, path, Manifest::load(path).expect("a manifest"))
        .expect("a spec");
    let capabilities = RuntimeCapabilities {
        version: RuntimeVersion::new(1, 1, 0),
        bind_mounts: true,
        named_volumes: true,
        tty: true,
        signals: true,
        loopback_publish: true,
        resource_limits: true,
        offline: NetworkIsolation::Proven,
    };
    let request = PolicyCompiler::compile(spec, &capabilities).expect("a validated request");
    (root, request)
}
```

**Check `SandboxSpec::from_root`'s and `Manifest::load`'s signatures against `gascan-apple/tests/backend_fake_runner.rs:415-430` before running.** They are copied from there; if either has a different arity, follow the working call site rather than this block.

- [ ] **Step 2: Write the failing tests**

Create `crates/gascan-arca/tests/backend_unary.rs`:

```rust
mod fake_transport;

use fake_transport::{Call, FakeEngine};
use gascan_arca::ArcaBackend;
use gascan_core::runtime::{ContainerState, ResourceOwnership, RuntimeBackend};
use gascan_core::sandbox::SandboxId;
use gascan_engine_proto::v1;

const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn digest() -> v1::ImageDigest {
    v1::ImageDigest {
        repository: "registry.example/workspace".to_owned(),
        sha256_hex: DIGEST.to_owned(),
    }
}

fn owner(id: &SandboxId) -> v1::OwnerLabels {
    v1::OwnerLabels {
        managed_by: "gascan".to_owned(),
        sandbox_id: id.as_str().to_owned(),
    }
}

#[tokio::test]
async fn capabilities_reads_the_engine_and_renames_project_mount() {
    let engine = FakeEngine::default();
    *engine.capabilities.lock().expect("test lock") = Some(v1::CapabilitiesResponse {
        outcome: Some(v1::capabilities_response::Outcome::Capabilities(
            v1::Capabilities {
                engine_version: Some(v1::Version { major: 1, minor: 0, patch: 0 }),
                contract_minor: 0,
                project_mount: true,
                named_volumes: true,
                tty: true,
                signals: true,
                loopback_publish: true,
                resource_limits: true,
                offline: v1::Isolation::Proven as i32,
            },
        )),
    });

    let capabilities = ArcaBackend::new(engine)
        .capabilities()
        .await
        .expect("a fully specified capability set maps");
    assert!(capabilities.bind_mounts);
    assert!(capabilities.loopback_publish);
}

#[tokio::test]
async fn an_engine_error_arrives_as_its_own_code() {
    let engine = FakeEngine::default();
    *engine.capabilities.lock().expect("test lock") = Some(v1::CapabilitiesResponse {
        outcome: Some(v1::capabilities_response::Outcome::Error(
            FakeEngine::engine_error("not_found"),
        )),
    });

    let error = ArcaBackend::new(engine)
        .capabilities()
        .await
        .expect_err("the engine refused");
    assert_eq!(error.code(), "not_found");
}

#[tokio::test]
async fn inspect_distinguishes_absent_from_a_failure_to_tell() {
    let id = SandboxId::test("observed");

    let present = FakeEngine::default();
    *present.inspect.lock().expect("test lock") = Some(v1::InspectResponse {
        outcome: Some(v1::inspect_response::Outcome::Sandbox(v1::Sandbox {
            sandbox_id: id.as_str().to_owned(),
            image: Some(digest()),
            state: v1::SandboxState::Running as i32,
            owner: Some(owner(&id)),
            ports: vec![v1::PortMapping { host_port: 22222, guest_port: 22 }],
        })),
    });
    let backend = ArcaBackend::new(present);
    let observed = backend.inspect(&id).await.expect("present").expect("some");
    assert_eq!(observed.state, ContainerState::Running);
    assert_eq!(observed.ports().len(), 1);

    let absent = FakeEngine::default();
    *absent.inspect.lock().expect("test lock") = Some(v1::InspectResponse {
        outcome: Some(v1::inspect_response::Outcome::Absent(v1::Absent {})),
    });
    assert!(
        ArcaBackend::new(absent)
            .inspect(&id)
            .await
            .expect("absent is an answer, not a failure")
            .is_none(),
    );

    let unset = FakeEngine::default();
    *unset.inspect.lock().expect("test lock") = Some(v1::InspectResponse { outcome: None });
    assert_eq!(
        ArcaBackend::new(unset)
            .inspect(&id)
            .await
            .expect_err("an unset oneof is not an answer")
            .code(),
        "invalid_output",
    );
}

#[tokio::test]
async fn start_stop_and_prepare_image_report_an_ack() {
    let id = SandboxId::test("lifecycle");

    let starting = FakeEngine::default();
    *starting.ack.lock().expect("test lock") = Some(FakeEngine::ok_ack());
    let backend = ArcaBackend::new(starting);
    backend.start(&id).await.expect("an ack is success");
    assert_eq!(
        backend.into_transport().calls(),
        [Call::Start(v1::StartRequest { sandbox_id: id.as_str().to_owned() })],
    );

    let preparing = FakeEngine::default();
    *preparing.prepare_image.lock().expect("test lock") = Some(v1::PrepareImageResponse {
        outcome: Some(v1::prepare_image_response::Outcome::Ok(v1::Ack {})),
    });
    let backend = ArcaBackend::new(preparing);
    backend
        .prepare_image(&format!("registry.example/workspace@sha256:{DIGEST}"))
        .await
        .expect("a digest the engine holds");
    assert_eq!(
        backend.into_transport().calls(),
        [Call::PrepareImage(v1::PrepareImageRequest { image: Some(digest()) })],
    );
}

#[tokio::test]
async fn prepare_image_refuses_a_reference_without_a_digest_before_calling() {
    let backend = ArcaBackend::new(FakeEngine::default());
    let error = backend
        .prepare_image("registry.example/workspace:latest")
        .await
        .expect_err("a tag-only reference is not expressible");
    assert_eq!(error.code(), "invalid_state");
    assert!(
        backend.into_transport().calls().is_empty(),
        "a request that cannot be expressed must not reach the engine",
    );
}

#[tokio::test]
async fn list_resources_classifies_what_the_engine_returned() {
    let id = SandboxId::test("owned");
    let engine = FakeEngine::default();
    *engine.list_resources.lock().expect("test lock") = Some(v1::ListResourcesResponse {
        outcome: Some(v1::list_resources_response::Outcome::Resources(
            v1::ResourceList {
                resources: vec![
                    v1::Resource {
                        identity: Some(v1::ResourceIdentity {
                            kind: v1::ResourceKind::Container as i32,
                            name: id.as_str().to_owned(),
                        }),
                        owner: Some(owner(&id)),
                    },
                    v1::Resource {
                        identity: Some(v1::ResourceIdentity {
                            kind: v1::ResourceKind::Volume as i32,
                            name: "someone-elses-volume".to_owned(),
                        }),
                        owner: None,
                    },
                ],
            },
        )),
    });

    let resources = ArcaBackend::new(engine)
        .list_resources()
        .await
        .expect("a mixed inventory maps");
    assert_eq!(
        resources
            .iter()
            .map(|resource| (resource.name(), resource.ownership()))
            .collect::<Vec<_>>(),
        [
            (id.as_str(), ResourceOwnership::GasCanOwned),
            ("someone-elses-volume", ResourceOwnership::Foreign),
        ],
    );
}

/// Builds the `Created` a well-behaved engine would answer a compiled request
/// with: the container, every requested volume, and the managed network.
fn created_for(request: &gascan_core::runtime::CreateRequest) -> v1::Created {
    let id = request.id();
    let mut created = vec![v1::Resource {
        identity: Some(v1::ResourceIdentity {
            kind: v1::ResourceKind::Container as i32,
            name: id.as_str().to_owned(),
        }),
        owner: Some(owner(id)),
    }];
    for volume in request.volumes() {
        created.push(v1::Resource {
            identity: Some(v1::ResourceIdentity {
                kind: v1::ResourceKind::Volume as i32,
                name: volume.name.clone(),
            }),
            owner: Some(owner(id)),
        });
    }
    if let Some(name) = request.network().managed_name() {
        created.push(v1::Resource {
            identity: Some(v1::ResourceIdentity {
                kind: v1::ResourceKind::Network as i32,
                name: name.to_owned(),
            }),
            owner: Some(owner(id)),
        });
    }
    v1::Created { created }
}

#[tokio::test]
async fn create_sends_the_compiled_request_and_reports_what_was_made() {
    let (_root, request) = fake_transport::policy_request("creating");
    let engine = FakeEngine::default();
    *engine.create.lock().expect("test lock") = Some(v1::CreateResponse {
        outcome: Some(v1::create_response::Outcome::Created(created_for(&request))),
    });

    let expected_resources = created_for(&request).created.len();
    let backend = ArcaBackend::new(engine);
    let outcome = backend
        .create(request.clone())
        .await
        .expect("a well-formed Created maps");
    assert_eq!(outcome.created().len(), expected_resources);

    let calls = backend.into_transport().calls();
    let Some(Call::Create(sent)) = calls.first() else {
        panic!("create must reach the engine exactly once: {calls:?}");
    };
    assert_eq!(sent.sandbox_id, request.id().as_str());
    assert!(sent.project.is_some(), "the one project mount is always sent");
    assert!(sent.owner.is_some(), "labels are how the engine recognises us later");
}

#[tokio::test]
async fn a_created_naming_a_resource_outside_the_request_is_refused() {
    let (_root, request) = fake_transport::policy_request("creating");
    let mut created = created_for(&request);
    created.created.push(v1::Resource {
        identity: Some(v1::ResourceIdentity {
            kind: v1::ResourceKind::Volume as i32,
            name: "a-volume-nobody-asked-for".to_owned(),
        }),
        owner: Some(owner(request.id())),
    });

    let engine = FakeEngine::default();
    *engine.create.lock().expect("test lock") = Some(v1::CreateResponse {
        outcome: Some(v1::create_response::Outcome::Created(created)),
    });

    let failure = ArcaBackend::new(engine)
        .create(request)
        .await
        .expect_err("a resource outside the requested topology is not ours to accept");
    assert_eq!(
        failure.code(),
        "ownership_mismatch",
        "gascan-core's own constructor is the boundary check",
    );
}

#[tokio::test]
async fn a_partial_create_keeps_the_evidence_and_the_engines_reason() {
    let (_root, request) = fake_transport::policy_request("creating");
    let engine = FakeEngine::default();
    *engine.create.lock().expect("test lock") = Some(v1::CreateResponse {
        outcome: Some(v1::create_response::Outcome::Failed(v1::CreateFailed {
            created: vec![v1::Resource {
                identity: Some(v1::ResourceIdentity {
                    kind: v1::ResourceKind::Container as i32,
                    name: request.id().as_str().to_owned(),
                }),
                owner: Some(owner(request.id())),
            }],
            error: Some(FakeEngine::engine_error("resource_conflict")),
        })),
    });

    let failure = ArcaBackend::new(engine)
        .create(request)
        .await
        .expect_err("a partial create is a failure");
    assert_eq!(failure.code(), "resource_conflict");
    assert_eq!(
        failure.created().len(),
        1,
        "losing partial-create evidence leaks resources nothing later knows to look for",
    );
}
```

The `into_transport` accessor those tests use is part of Step 3. `owner` takes a
`&SandboxId`, which is what `request.id()` returns.

- [ ] **Step 3: Write the backend**

Create `crates/gascan-arca/src/backend.rs`:

```rust
use async_trait::async_trait;
use gascan_core::runtime::{
    CreateFailure, CreateOutcome, CreateRequest, ExecRequest, ExecSession, RecreateRequest,
    RemoveRequest, RuntimeBackend, RuntimeCapabilities, RuntimeError, RuntimeResource,
    RuntimeSandbox,
};
use gascan_core::sandbox::SandboxId;
use gascan_engine_proto::v1;

use crate::{EngineTransport, TransportError, error, translate};

/// `RuntimeBackend` over Arca's engine contract.
///
/// Generic over its transport for the same reason `AppleBackend` is generic over
/// its command runner: the mapping is the part worth testing, and it is testable
/// without an engine only if something can stand in for one.
pub struct ArcaBackend<T> {
    transport: T,
}

impl<T> ArcaBackend<T> {
    pub const fn new(transport: T) -> Self {
        Self { transport }
    }

    /// Recovers the transport, so a test can assert on what was sent.
    pub fn into_transport(self) -> T {
        self.transport
    }
}

/// Unwraps an `Ack` response: success with nothing to say, or the engine's error.
fn ack(operation: &str, response: v1::AckResponse) -> Result<(), RuntimeError> {
    match response.outcome {
        Some(v1::ack_response::Outcome::Ok(_)) => Ok(()),
        Some(v1::ack_response::Outcome::Error(error)) => {
            Err(error::engine_error(operation, &error))
        }
        None => Err(translate::missing_outcome(operation)),
    }
}

impl<T: EngineTransport> ArcaBackend<T> {
    /// Both create paths answer with the same response type, so they share this.
    ///
    /// A resource that fails to map is a hard failure rather than a filtered
    /// list: a malformed `Resource` has no identity this client can act on, so
    /// it could not be removed even if it were reported, and the fact an
    /// operator needs is that the engine sent something malformed.
    fn create_outcome(
        request: &CreateRequest,
        operation: &str,
        response: v1::CreateResponse,
    ) -> Result<CreateOutcome, CreateFailure> {
        match response.outcome {
            Some(v1::create_response::Outcome::Created(created)) => {
                let resources = translate::runtime_resources(&created.created)
                    .map_err(CreateFailure::from_source)?;
                CreateOutcome::new(request, resources).map_err(CreateFailure::from_source)
            }
            Some(v1::create_response::Outcome::Failed(failed)) => {
                let source = failed.error.as_ref().map_or_else(
                    || translate::missing_outcome(operation),
                    |error| error::engine_error(operation, error),
                );
                let resources = translate::runtime_resources(&failed.created)
                    .map_err(CreateFailure::from_source)?;
                Err(CreateFailure::from_created_evidence(
                    request, resources, source,
                ))
            }
            None => Err(CreateFailure::from_source(translate::missing_outcome(
                operation,
            ))),
        }
    }
}

#[async_trait]
impl<T: EngineTransport> RuntimeBackend for ArcaBackend<T> {
    async fn capabilities(&self) -> Result<RuntimeCapabilities, RuntimeError> {
        let response = self
            .transport
            .capabilities(v1::CapabilitiesRequest {})
            .await
            .map_err(TransportError::into_runtime_error)?;
        match response.outcome {
            Some(v1::capabilities_response::Outcome::Capabilities(capabilities)) => {
                translate::runtime_capabilities(&capabilities)
            }
            Some(v1::capabilities_response::Outcome::Error(error)) => {
                Err(error::engine_error("capabilities", &error))
            }
            None => Err(translate::missing_outcome("capabilities")),
        }
    }

    async fn inspect(&self, id: &SandboxId) -> Result<Option<RuntimeSandbox>, RuntimeError> {
        let response = self
            .transport
            .inspect(v1::InspectRequest {
                sandbox_id: id.to_string(),
            })
            .await
            .map_err(TransportError::into_runtime_error)?;
        match response.outcome {
            Some(v1::inspect_response::Outcome::Sandbox(sandbox)) => {
                translate::runtime_sandbox(&sandbox).map(Some)
            }
            Some(v1::inspect_response::Outcome::Absent(_)) => Ok(None),
            Some(v1::inspect_response::Outcome::Error(error)) => {
                Err(error::engine_error("inspect", &error))
            }
            None => Err(translate::missing_outcome("inspect")),
        }
    }

    async fn create(&self, request: CreateRequest) -> Result<CreateOutcome, CreateFailure> {
        let wire = translate::create_request(&request).map_err(CreateFailure::from_source)?;
        let response = self
            .transport
            .create(wire)
            .await
            .map_err(|error| CreateFailure::from_source(error.into_runtime_error()))?;
        Self::create_outcome(&request, "create", response)
    }

    async fn prepare_image(&self, image: &str) -> Result<(), RuntimeError> {
        let response = self
            .transport
            .prepare_image(v1::PrepareImageRequest {
                image: Some(translate::image_digest(image)?),
            })
            .await
            .map_err(TransportError::into_runtime_error)?;
        match response.outcome {
            Some(v1::prepare_image_response::Outcome::Ok(_)) => Ok(()),
            Some(v1::prepare_image_response::Outcome::Error(error)) => {
                Err(error::engine_error("prepare_image", &error))
            }
            None => Err(translate::missing_outcome("prepare_image")),
        }
    }

    async fn create_container(
        &self,
        request: RecreateRequest,
    ) -> Result<CreateOutcome, CreateFailure> {
        let wire =
            translate::create_container_request(&request).map_err(CreateFailure::from_source)?;
        let response = self
            .transport
            .create_container(wire)
            .await
            .map_err(|error| CreateFailure::from_source(error.into_runtime_error()))?;
        Self::create_outcome(request.create(), "create_container", response)
    }

    async fn start(&self, id: &SandboxId) -> Result<(), RuntimeError> {
        let response = self
            .transport
            .start(v1::StartRequest {
                sandbox_id: id.to_string(),
            })
            .await
            .map_err(TransportError::into_runtime_error)?;
        ack("start", response)
    }

    async fn stop(&self, id: &SandboxId) -> Result<(), RuntimeError> {
        let response = self
            .transport
            .stop(v1::StopRequest {
                sandbox_id: id.to_string(),
            })
            .await
            .map_err(TransportError::into_runtime_error)?;
        ack("stop", response)
    }

    async fn remove(&self, request: RemoveRequest) -> Result<(), RuntimeError> {
        let response = self
            .transport
            .remove(translate::remove_request(&request)?)
            .await
            .map_err(TransportError::into_runtime_error)?;
        ack("remove", response)
    }

    async fn exec(&self, _request: ExecRequest) -> Result<ExecSession, RuntimeError> {
        Err(RuntimeError::UnsupportedCapability {
            capability: "exec lands in the next task".to_owned(),
        })
    }

    async fn logs(
        &self,
        _id: &SandboxId,
        _since_millis: Option<i64>,
    ) -> Result<Vec<u8>, RuntimeError> {
        Err(RuntimeError::UnsupportedCapability {
            capability: "logs lands in the next task".to_owned(),
        })
    }

    async fn list_resources(&self) -> Result<Vec<RuntimeResource>, RuntimeError> {
        let response = self
            .transport
            .list_resources(v1::ListResourcesRequest {})
            .await
            .map_err(TransportError::into_runtime_error)?;
        match response.outcome {
            Some(v1::list_resources_response::Outcome::Resources(list)) => {
                translate::runtime_resources(&list.resources)
            }
            Some(v1::list_resources_response::Outcome::Error(error)) => {
                Err(error::engine_error("list_resources", &error))
            }
            None => Err(translate::missing_outcome("list_resources")),
        }
    }
}
```

Update `crates/gascan-arca/src/lib.rs`:

```rust
mod backend;
mod error;
mod transport;
mod translate;

pub use backend::ArcaBackend;
pub use transport::{EngineTransport, ExecStream, LogsStream, TransportError};
```

**The two `UnsupportedCapability` stubs are temporary and Tasks 7 and 8 replace them.** They exist only so this task compiles and its tests run; do not leave them in place, and do not add a test that asserts on them.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `env -u RUSTUP_TOOLCHAIN cargo test -p gascan-arca --test backend_unary`
Expected: PASS, `running 9 tests`.

- [ ] **Step 5: Clippy, fmt, and commit**

```bash
git add crates/gascan-arca
git commit -m "feat: implement the unary half of RuntimeBackend over Arca

Inbound responses are built by calling gascan-core's own constructors, so
the boundary check against a buggy or lying engine is existing tested code
rather than new validation: CreateOutcome::new already rejects a resource
outside the request's topology, not GasCanOwned, or carrying the wrong
sandbox id.

inspect keeps the contract's three arms as three answers. A sandbox that is
not there is Ok(None), not an error, because a reconciler must act
differently on 'it is gone' than on 'I could not tell'.

A request that cannot be expressed never reaches the engine: prepare_image
with a tag-only reference fails before the call, which the test asserts by
checking the fake recorded nothing.

exec and logs are stubbed and land next; the stubs carry no tests."
```

---

### Task 7: `logs`

**Files:**
- Modify: `crates/gascan-arca/src/backend.rs`
- Create: `crates/gascan-arca/tests/backend_streams.rs`
- Modify: `crates/gascan-arca/tests/fake_transport/mod.rs`

**Interfaces:**
- Consumes: `LogsStream`, `error::engine_error`, `translate::missing_outcome`.
- Produces: the real `RuntimeBackend::logs`; `FakeEngine::logs_chunks` scripting field.

- [ ] **Step 1: Extend the fake to script a log stream**

In `crates/gascan-arca/tests/fake_transport/mod.rs`, add a field and replace the `logs` stub:

```rust
    /// Chunks the next `logs` call streams, in order.
    pub logs_chunks: Mutex<Vec<Result<v1::LogsChunk, TransportError>>>,
```

```rust
    async fn logs(&self, request: v1::LogsRequest) -> Result<LogsStream, TransportError> {
        self.record(Call::Logs(request));
        let chunks = std::mem::take(&mut *self.logs_chunks.lock().expect("test lock"));
        let (sender, receiver) = tokio::sync::mpsc::channel(chunks.len().max(1));
        for chunk in chunks {
            sender.send(chunk).await.expect("the receiver is alive");
        }
        Ok(LogsStream::new(receiver))
    }
```

Add `Logs(v1::LogsRequest)` to `enum Call`. Add `tokio = { workspace = true, features = ["macros", "rt", "sync", "time"] }` to `[dev-dependencies]` if `sync` is not already reachable.

- [ ] **Step 2: Write the failing tests**

Create `crates/gascan-arca/tests/backend_streams.rs`:

```rust
mod fake_transport;

use fake_transport::{Call, FakeEngine};
use gascan_arca::ArcaBackend;
use gascan_core::runtime::RuntimeBackend;
use gascan_core::sandbox::SandboxId;
use gascan_engine_proto::v1;

fn data(bytes: &[u8]) -> Result<v1::LogsChunk, gascan_arca::TransportError> {
    Ok(v1::LogsChunk {
        outcome: Some(v1::logs_chunk::Outcome::Data(bytes.to_vec())),
    })
}

#[tokio::test]
async fn logs_concatenate_every_chunk_in_order() {
    let engine = FakeEngine::default();
    *engine.logs_chunks.lock().expect("test lock") =
        vec![data(b"first "), data(b"second "), data(b"third")];

    let id = SandboxId::test("logging");
    let backend = ArcaBackend::new(engine);
    let logs = backend.logs(&id, Some(1_234)).await.expect("three chunks");

    assert_eq!(logs, b"first second third");
    assert_eq!(
        backend.into_transport().calls(),
        [Call::Logs(v1::LogsRequest {
            sandbox_id: id.as_str().to_owned(),
            since_unix_millis: Some(1_234),
        })],
        "since_millis passes through, and absent means from the beginning",
    );
}

#[tokio::test]
async fn a_mid_stream_error_discards_the_partial_buffer() {
    let engine = FakeEngine::default();
    *engine.logs_chunks.lock().expect("test lock") = vec![
        data(b"this much arrived"),
        Ok(v1::LogsChunk {
            outcome: Some(v1::logs_chunk::Outcome::Error(FakeEngine::engine_error(
                "command_failed",
            ))),
        }),
        data(b"and this never should"),
    ];

    let error = ArcaBackend::new(engine)
        .logs(&SandboxId::test("logging"), None)
        .await
        .expect_err("a broken log is a failure, not a short read");
    assert_eq!(error.code(), "command_failed");
}

#[tokio::test]
async fn an_empty_log_is_empty_rather_than_an_error() {
    let engine = FakeEngine::default();
    assert!(
        ArcaBackend::new(engine)
            .logs(&SandboxId::test("logging"), None)
            .await
            .expect("no chunks is a valid empty log")
            .is_empty(),
    );
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `env -u RUSTUP_TOOLCHAIN cargo test -p gascan-arca --test backend_streams`
Expected: FAIL — the stub returns `UnsupportedCapability`, so `logs` errors with code `unsupported_capability` instead of returning bytes.

- [ ] **Step 4: Replace the stub**

In `crates/gascan-arca/src/backend.rs`, replace the `logs` stub with:

```rust
    /// Concatenates the chunk stream into the one buffer the trait returns.
    ///
    /// A mid-stream error discards what arrived. The signature has no way to say
    /// "here is some of it, and also it broke", and returning a short log beside
    /// a swallowed error would make a truncated log indistinguishable from a
    /// complete one.
    async fn logs(
        &self,
        id: &SandboxId,
        since_millis: Option<i64>,
    ) -> Result<Vec<u8>, RuntimeError> {
        let mut stream = self
            .transport
            .logs(v1::LogsRequest {
                sandbox_id: id.to_string(),
                since_unix_millis: since_millis,
            })
            .await
            .map_err(TransportError::into_runtime_error)?;
        let mut buffer = Vec::new();
        while let Some(chunk) = stream.recv().await {
            match chunk.map_err(TransportError::into_runtime_error)?.outcome {
                Some(v1::logs_chunk::Outcome::Data(data)) => buffer.extend_from_slice(&data),
                Some(v1::logs_chunk::Outcome::Error(error)) => {
                    return Err(error::engine_error("logs", &error));
                }
                None => return Err(translate::missing_outcome("logs")),
            }
        }
        Ok(buffer)
    }
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `env -u RUSTUP_TOOLCHAIN cargo test -p gascan-arca --test backend_streams`
Expected: PASS, `running 3 tests`.

- [ ] **Step 6: Clippy, fmt, and commit**

```bash
git add crates/gascan-arca
git commit -m "feat: read a streamed log into the buffer the trait returns

The contract streams logs so that a log larger than the message limit fails
as a log rather than as a size error. The trait returns one buffer, so the
client concatenates in order.

A mid-stream error discards the partial buffer. The signature cannot say
'here is some of it, and also it broke', and a short log returned beside a
swallowed error is indistinguishable from a complete one."
```

---

### Task 8: `exec`

**Files:**
- Modify: `crates/gascan-arca/src/backend.rs`, `crates/gascan-arca/tests/fake_transport/mod.rs`, `crates/gascan-arca/tests/backend_streams.rs`

**Interfaces:**
- Consumes: `ExecStream::split`, `translate::exec_start`, `error::engine_error`.
- Produces: the real `RuntimeBackend::exec`, returning `ExecSession::live_cancellable`.

**Shape, mirroring `AppleBackend::exec` (`gascan-apple/src/backend.rs:517-602`).** One spawned pump; a three-way `select!` over cancellation, consumer input, and server frames; terminal on `Exit`, on an error frame, or on a failed delivery. Two parities: the request's `stdin` is sent as a first frame only when non-empty, and no `Close` is auto-sent — the consumer sends `ExecInput::Close` when it means to.

- [ ] **Step 1: Extend the fake to script an exec session**

Add to `FakeEngine`:

```rust
    /// Frames the next `exec` call streams back, in order.
    pub exec_frames: Mutex<Vec<Result<v1::ExecServerFrame, TransportError>>>,
    /// Frames the client sent, captured by the fake's pump.
    pub exec_sent: std::sync::Arc<Mutex<Vec<v1::ExecClientFrame>>>,
```

and replace the `exec` stub:

```rust
    async fn exec(&self, start: v1::ExecStart) -> Result<ExecStream, TransportError> {
        self.exec_sent
            .lock()
            .expect("test lock")
            .push(v1::ExecClientFrame {
                frame: Some(v1::exec_client_frame::Frame::Start(start)),
            });

        let frames = std::mem::take(&mut *self.exec_frames.lock().expect("test lock"));
        let (server, from_server) = tokio::sync::mpsc::channel(frames.len().max(1));
        for frame in frames {
            server.send(frame).await.expect("the receiver is alive");
        }

        let (to_server, mut client_frames) = tokio::sync::mpsc::channel(16);
        let sent = std::sync::Arc::clone(&self.exec_sent);
        tokio::spawn(async move {
            while let Some(frame) = client_frames.recv().await {
                sent.lock().expect("test lock").push(frame);
            }
        });

        Ok(ExecStream::new(to_server, from_server))
    }
```

- [ ] **Step 2: Write the failing tests**

Append to `crates/gascan-arca/tests/backend_streams.rs`:

```rust
use gascan_core::runtime::{ExecInput, ExecOutput, ExecRequest};

fn server_frame(frame: v1::exec_server_frame::Frame) -> Result<v1::ExecServerFrame, gascan_arca::TransportError> {
    Ok(v1::ExecServerFrame { frame: Some(frame) })
}

#[tokio::test]
async fn exec_opens_with_a_start_frame_and_reports_stdout_then_exit() {
    let engine = FakeEngine::default();
    *engine.exec_frames.lock().expect("test lock") = vec![
        server_frame(v1::exec_server_frame::Frame::Stdout(b"hello".to_vec())),
        server_frame(v1::exec_server_frame::Frame::Exit(v1::Exit { code: 0, signal: 0 })),
    ];
    let sent = std::sync::Arc::clone(&engine.exec_sent);

    let id = SandboxId::test("execing");
    let mut session = ArcaBackend::new(engine)
        .exec(ExecRequest::fixture(id.clone(), ["/bin/true"]))
        .await
        .expect("the session opens");

    assert_eq!(
        session.next().await.expect("a frame").expect("stdout"),
        ExecOutput::Stdout(b"hello".to_vec()),
    );
    assert_eq!(
        session.next().await.expect("a frame").expect("exit"),
        ExecOutput::Exit { code: 0, signal: 0 },
    );

    let frames = sent.lock().expect("test lock").clone();
    assert!(
        matches!(
            frames.first().and_then(|frame| frame.frame.as_ref()),
            Some(v1::exec_client_frame::Frame::Start(start)) if start.sandbox_id == id.as_str()
        ),
        "the first frame must be the one ExecStart: {frames:?}",
    );
    assert_eq!(frames.len(), 1, "an empty stdin buffer sends no stdin frame: {frames:?}");
}

#[tokio::test]
async fn a_non_empty_stdin_buffer_is_sent_once_and_no_close_is_forged() {
    let engine = FakeEngine::default();
    *engine.exec_frames.lock().expect("test lock") = vec![server_frame(
        v1::exec_server_frame::Frame::Exit(v1::Exit { code: 0, signal: 0 }),
    )];
    let sent = std::sync::Arc::clone(&engine.exec_sent);

    let mut request = ExecRequest::fixture(SandboxId::test("execing"), ["/bin/cat"]);
    request.stdin = b"piped".to_vec();

    let mut session = ArcaBackend::new(engine).exec(request).await.expect("opens");
    session.next().await.expect("a frame").expect("exit");
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let frames = sent.lock().expect("test lock").clone();
    let stdin: Vec<_> = frames
        .iter()
        .filter_map(|frame| match frame.frame.as_ref() {
            Some(v1::exec_client_frame::Frame::Stdin(bytes)) => Some(bytes.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(stdin, [b"piped".to_vec()], "the initial buffer is sent exactly once");
    assert!(
        !frames.iter().any(|frame| matches!(
            frame.frame.as_ref(),
            Some(v1::exec_client_frame::Frame::Close(_))
        )),
        "Close is the consumer's to send: {frames:?}",
    );
}

#[tokio::test]
async fn live_input_reaches_the_engine_as_its_own_frame() {
    let engine = FakeEngine::default();
    *engine.exec_frames.lock().expect("test lock") = vec![server_frame(
        v1::exec_server_frame::Frame::Exit(v1::Exit { code: 0, signal: 0 }),
    )];
    let sent = std::sync::Arc::clone(&engine.exec_sent);

    let session = ArcaBackend::new(engine)
        .exec(ExecRequest::fixture(SandboxId::test("execing"), ["/bin/sh"]))
        .await
        .expect("opens");

    session.send(ExecInput::Stdin(b"typed".to_vec())).await.expect("stdin");
    session.send(ExecInput::Resize { columns: 120, rows: 40 }).await.expect("resize");
    session.send(ExecInput::Signal(2)).await.expect("signal");
    session.send(ExecInput::Close).await.expect("close");
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let frames = sent.lock().expect("test lock").clone();
    let shapes: Vec<&str> = frames
        .iter()
        .map(|frame| match frame.frame.as_ref() {
            Some(v1::exec_client_frame::Frame::Start(_)) => "start",
            Some(v1::exec_client_frame::Frame::Stdin(_)) => "stdin",
            Some(v1::exec_client_frame::Frame::Resize(_)) => "resize",
            Some(v1::exec_client_frame::Frame::Signal(_)) => "signal",
            Some(v1::exec_client_frame::Frame::Close(_)) => "close",
            None => "unset",
        })
        .collect();
    assert_eq!(shapes, ["start", "stdin", "resize", "signal", "close"]);
}

#[tokio::test]
async fn a_server_error_frame_is_terminal_and_carries_its_code() {
    let engine = FakeEngine::default();
    *engine.exec_frames.lock().expect("test lock") = vec![
        server_frame(v1::exec_server_frame::Frame::Stderr(b"before".to_vec())),
        server_frame(v1::exec_server_frame::Frame::Error(FakeEngine::engine_error(
            "invalid_state",
        ))),
        server_frame(v1::exec_server_frame::Frame::Stdout(b"never".to_vec())),
    ];

    let mut session = ArcaBackend::new(engine)
        .exec(ExecRequest::fixture(SandboxId::test("execing"), ["/bin/false"]))
        .await
        .expect("opens");

    assert_eq!(
        session.next().await.expect("a frame").expect("stderr"),
        ExecOutput::Stderr(b"before".to_vec()),
    );
    let error = session
        .next()
        .await
        .expect("a frame")
        .expect_err("the error frame");
    assert_eq!(error.code(), "invalid_state");
    assert!(
        session.next().await.is_none(),
        "an error frame ends the session; nothing after it is delivered",
    );
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `env -u RUSTUP_TOOLCHAIN cargo test -p gascan-arca --test backend_streams`
Expected: FAIL — the four new tests fail at `.exec(...)`, which still returns `UnsupportedCapability`. The three `logs` tests still pass.

- [ ] **Step 4: Replace the stub with the pump**

Add to the imports in `backend.rs`: `ExecCancellation, ExecInput, ExecOutput`. Replace the `exec` stub with:

```rust
    /// Opens a session and pumps it until it ends.
    ///
    /// The initial `stdin` buffer is sent only when non-empty, and no `Close` is
    /// forged: the consumer sends `ExecInput::Close` when it means to. Both
    /// match the Apple backend, so a caller cannot tell the two apart by their
    /// framing.
    async fn exec(&self, request: ExecRequest) -> Result<ExecSession, RuntimeError> {
        let initial_stdin = request.stdin.clone();
        let stream = self
            .transport
            .exec(translate::exec_start(&request))
            .await
            .map_err(TransportError::into_runtime_error)?;
        let (to_engine, mut from_engine) = stream.split();

        let (input, mut inputs) = tokio::sync::mpsc::channel(16);
        let (outputs, output) = tokio::sync::mpsc::channel(32);
        let (cancellation, mut cancelled) = ExecCancellation::channel();

        tokio::spawn(async move {
            if !initial_stdin.is_empty() {
                let frame = v1::ExecClientFrame {
                    frame: Some(v1::exec_client_frame::Frame::Stdin(initial_stdin)),
                };
                tokio::select! {
                    result = to_engine.send(frame) => {
                        if result.is_err() {
                            let _ = outputs
                                .send(Err(RuntimeError::CommandIo {
                                    operation: "exec_input".to_owned(),
                                    message: "the engine closed the stream".to_owned(),
                                }))
                                .await;
                            return;
                        }
                    }
                    result = cancelled.changed() => {
                        if result.is_ok() && *cancelled.borrow() { return; }
                    }
                }
            }

            loop {
                tokio::select! {
                    result = cancelled.changed() => {
                        if result.is_ok() && *cancelled.borrow() { break; }
                    }
                    next = inputs.recv() => {
                        let Some(next) = next else { break };
                        let frame = v1::ExecClientFrame {
                            frame: Some(match next {
                                ExecInput::Stdin(bytes) => {
                                    v1::exec_client_frame::Frame::Stdin(bytes)
                                }
                                ExecInput::Resize { columns, rows } => {
                                    v1::exec_client_frame::Frame::Resize(v1::Resize {
                                        columns,
                                        rows,
                                    })
                                }
                                ExecInput::Signal(signal) => {
                                    v1::exec_client_frame::Frame::Signal(signal)
                                }
                                ExecInput::Close => {
                                    v1::exec_client_frame::Frame::Close(v1::Close {})
                                }
                            }),
                        };
                        let delivered = tokio::select! {
                            result = to_engine.send(frame) => result.is_ok(),
                            result = cancelled.changed() => {
                                if result.is_ok() && *cancelled.borrow() { break; }
                                continue;
                            }
                        };
                        if !delivered {
                            let _ = outputs
                                .send(Err(RuntimeError::CommandIo {
                                    operation: "exec_input".to_owned(),
                                    message: "the engine closed the stream".to_owned(),
                                }))
                                .await;
                            break;
                        }
                    }
                    next = from_engine.recv() => {
                        let (mapped, terminal) = match next {
                            None => break,
                            Some(Err(error)) => (Err(error.into_runtime_error()), true),
                            Some(Ok(frame)) => match frame.frame {
                                Some(v1::exec_server_frame::Frame::Stdout(bytes)) => {
                                    (Ok(ExecOutput::Stdout(bytes)), false)
                                }
                                Some(v1::exec_server_frame::Frame::Stderr(bytes)) => {
                                    (Ok(ExecOutput::Stderr(bytes)), false)
                                }
                                Some(v1::exec_server_frame::Frame::Exit(exit)) => (
                                    Ok(ExecOutput::Exit {
                                        code: exit.code,
                                        signal: exit.signal,
                                    }),
                                    true,
                                ),
                                Some(v1::exec_server_frame::Frame::Error(error)) => {
                                    (Err(error::engine_error("exec", &error)), true)
                                }
                                None => (Err(translate::missing_outcome("exec")), true),
                            },
                        };
                        let delivered = tokio::select! {
                            result = outputs.send(mapped) => result.is_ok(),
                            result = cancelled.changed() => {
                                !(result.is_ok() && *cancelled.borrow())
                            }
                        };
                        if !delivered || terminal {
                            break;
                        }
                    }
                }
            }
        });

        Ok(ExecSession::live_cancellable(input, output, cancellation))
    }
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `env -u RUSTUP_TOOLCHAIN cargo test -p gascan-arca --test backend_streams`
Expected: PASS, `running 7 tests`.

If `a_non_empty_stdin_buffer_is_sent_once_and_no_close_is_forged` or `live_input_reaches_the_engine_as_its_own_frame` is flaky, the 50ms sleep is the cause — it waits for the fake's capture task. Replace the sleep with a poll that waits for the expected frame count, bounded and then asserted. **Do not lengthen the sleep**; a wall-clock wait is the failure mode `autostart.rs`'s symlink test already has in this repository.

- [ ] **Step 6: Restore the full clippy gate, fmt, and commit**

**This task restores the plain gate.** `exec_start` was the last mapping function without a caller, and this task gives it one, so `-A dead_code` is no longer needed and must not be used:

```bash
if env -u RUSTUP_TOOLCHAIN cargo clippy -p gascan-arca --all-targets -- -D warnings; then rc=0; else rc=$?; fi
echo "clippy rc=$rc"
```

Expected: **rc=0 with nothing allowed.** If `dead_code` still fires, a mapping function this plan wrote is genuinely unreachable — find out which and why rather than re-adding the flag, because at this point the lint is telling you something true about the code instead of about the plan's ordering.

Then `env -u RUSTUP_TOOLCHAIN cargo fmt --all --check`, and commit:

```bash
git add crates/gascan-arca
git commit -m "feat: pump a live exec session over the engine's bidi stream

Structurally the Apple backend's exec: one spawned pump, a three-way select
over cancellation, consumer input and server frames, terminal on exit, on an
error frame, or on a failed delivery. Returning live_cancellable means
dropping the session cancels the guest work.

Two parities are deliberate, and both are asserted. The initial stdin buffer
is sent only when non-empty, and no Close is forged -- the consumer sends
that when it means to. A caller must not be able to tell the two backends
apart by their framing.

The wire is richer than the Apple path here: Exit carries a signal, where
the Apple backend hardcodes zero, and Resize needs no range check because
both sides are u32."
```

---

### Task 9: The `tonic` arm

**Files:**
- Create: `crates/gascan-arca/src/channel.rs`
- Modify: `crates/gascan-arca/src/lib.rs`

**Interfaces:**
- Consumes: `EngineTransport`, `TransportError`, `ExecStream`, `LogsStream`, `gascan_engine_proto::v1::sandbox_engine_client::SandboxEngineClient`.
- Produces: `pub struct ChannelTransport`, `ChannelTransport::connect(socket: PathBuf) -> Result<Self, TransportError>`.

**VERIFIED** the generated names against the output rather than the service name: `pub mod sandbox_engine_client` and `pub struct SandboxEngineClient<T>` exist, and there is no `sandbox_engine_server`.

**This arm has no live counterpart until P5.1 exists.** It is kept thin deliberately — one call plus an error conversion per unary method — so what goes untested is almost entirely `tonic`'s own code. Do not build a Rust server to test it.

- [ ] **Step 1: Write the transport**

Create `crates/gascan-arca/src/channel.rs`:

```rust
use async_trait::async_trait;
use gascan_engine_proto::v1;
use gascan_engine_proto::v1::sandbox_engine_client::SandboxEngineClient;
use hyper_util::rt::TokioIo;
use std::path::PathBuf;
use tokio::sync::mpsc;
use tonic::transport::{Channel, Endpoint};
use tower::service_fn;

use crate::{EngineTransport, ExecStream, LogsStream, TransportError};

/// `EngineTransport` over a real gRPC channel.
///
/// Thin on purpose: each unary method is one call and one error conversion, so
/// the part that no test can reach until an engine exists is almost entirely
/// `tonic`'s.
#[derive(Clone)]
pub struct ChannelTransport {
    client: SandboxEngineClient<Channel>,
}

impl ChannelTransport {
    /// Dials the engine over a Unix socket.
    ///
    /// The authority is a placeholder that the connector ignores, which is the
    /// same shape the daemon client already uses for its own socket.
    pub async fn connect(socket: PathBuf) -> Result<Self, TransportError> {
        let channel = Endpoint::from_static("http://[::]:50051")
            .connect_with_connector(service_fn(move |_| {
                let socket = socket.clone();
                async move {
                    let stream = tokio::net::UnixStream::connect(socket).await?;
                    Ok::<_, std::io::Error>(TokioIo::new(stream))
                }
            }))
            .await
            .map_err(|error| TransportError::rpc("connect", error.to_string()))?;
        Ok(Self {
            client: SandboxEngineClient::new(channel),
        })
    }

    fn client(&self) -> SandboxEngineClient<Channel> {
        self.client.clone()
    }
}

fn status(operation: &str, status: tonic::Status) -> TransportError {
    TransportError::rpc(operation, format!("{}: {}", status.code(), status.message()))
}

#[async_trait]
impl EngineTransport for ChannelTransport {
    async fn capabilities(
        &self,
        request: v1::CapabilitiesRequest,
    ) -> Result<v1::CapabilitiesResponse, TransportError> {
        self.client()
            .capabilities(request)
            .await
            .map(tonic::Response::into_inner)
            .map_err(|error| status("capabilities", error))
    }

    async fn inspect(
        &self,
        request: v1::InspectRequest,
    ) -> Result<v1::InspectResponse, TransportError> {
        self.client()
            .inspect(request)
            .await
            .map(tonic::Response::into_inner)
            .map_err(|error| status("inspect", error))
    }

    async fn create(
        &self,
        request: v1::CreateRequest,
    ) -> Result<v1::CreateResponse, TransportError> {
        self.client()
            .create(request)
            .await
            .map(tonic::Response::into_inner)
            .map_err(|error| status("create", error))
    }

    async fn prepare_image(
        &self,
        request: v1::PrepareImageRequest,
    ) -> Result<v1::PrepareImageResponse, TransportError> {
        self.client()
            .prepare_image(request)
            .await
            .map(tonic::Response::into_inner)
            .map_err(|error| status("prepare_image", error))
    }

    async fn create_container(
        &self,
        request: v1::CreateContainerRequest,
    ) -> Result<v1::CreateResponse, TransportError> {
        self.client()
            .create_container(request)
            .await
            .map(tonic::Response::into_inner)
            .map_err(|error| status("create_container", error))
    }

    async fn start(&self, request: v1::StartRequest) -> Result<v1::AckResponse, TransportError> {
        self.client()
            .start(request)
            .await
            .map(tonic::Response::into_inner)
            .map_err(|error| status("start", error))
    }

    async fn stop(&self, request: v1::StopRequest) -> Result<v1::AckResponse, TransportError> {
        self.client()
            .stop(request)
            .await
            .map(tonic::Response::into_inner)
            .map_err(|error| status("stop", error))
    }

    async fn remove(&self, request: v1::RemoveRequest) -> Result<v1::AckResponse, TransportError> {
        self.client()
            .remove(request)
            .await
            .map(tonic::Response::into_inner)
            .map_err(|error| status("remove", error))
    }

    async fn exec(&self, start: v1::ExecStart) -> Result<ExecStream, TransportError> {
        let (to_engine, outbound) = mpsc::channel::<v1::ExecClientFrame>(16);
        to_engine
            .send(v1::ExecClientFrame {
                frame: Some(v1::exec_client_frame::Frame::Start(start)),
            })
            .await
            .map_err(|_| TransportError::rpc("exec", "the outbound stream closed immediately"))?;

        let mut streaming = self
            .client()
            .exec(tokio_stream::wrappers::ReceiverStream::new(outbound))
            .await
            .map_err(|error| status("exec", error))?
            .into_inner();

        let (from_engine, inbound) = mpsc::channel(32);
        tokio::spawn(async move {
            loop {
                match streaming.message().await {
                    Ok(Some(frame)) => {
                        if from_engine.send(Ok(frame)).await.is_err() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        let _ = from_engine.send(Err(status("exec", error))).await;
                        break;
                    }
                }
            }
        });

        Ok(ExecStream::new(to_engine, inbound))
    }

    async fn logs(&self, request: v1::LogsRequest) -> Result<LogsStream, TransportError> {
        let mut streaming = self
            .client()
            .logs(request)
            .await
            .map_err(|error| status("logs", error))?
            .into_inner();

        let (sender, receiver) = mpsc::channel(32);
        tokio::spawn(async move {
            loop {
                match streaming.message().await {
                    Ok(Some(chunk)) => {
                        if sender.send(Ok(chunk)).await.is_err() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        let _ = sender.send(Err(status("logs", error))).await;
                        break;
                    }
                }
            }
        });

        Ok(LogsStream::new(receiver))
    }

    async fn list_resources(
        &self,
        request: v1::ListResourcesRequest,
    ) -> Result<v1::ListResourcesResponse, TransportError> {
        self.client()
            .list_resources(request)
            .await
            .map(tonic::Response::into_inner)
            .map_err(|error| status("list_resources", error))
    }
}
```

Add `tokio-stream.workspace = true` to `[dependencies]` in `crates/gascan-arca/Cargo.toml` — `ReceiverStream` comes from it, and it is already a workspace dependency. Update `lib.rs`:

```rust
mod backend;
mod channel;
mod error;
mod transport;
mod translate;

pub use backend::ArcaBackend;
pub use channel::ChannelTransport;
pub use transport::{EngineTransport, ExecStream, LogsStream, TransportError};
```

- [ ] **Step 2: Verify it compiles and satisfies the trait**

Run: `env -u RUSTUP_TOOLCHAIN cargo build -p gascan-arca`
Expected: rc=0.

There is no unit test here on purpose. `ChannelTransport` needs a server to say anything, and the only thing that could answer it today is a Rust test double, which is forbidden — it would make a wrong client look correct. The compiler checking it against `EngineTransport` is the assurance this task carries, and it is stated as exactly that rather than dressed up as coverage.

- [ ] **Step 3: Confirm the whole trait is implemented twice over**

Run: `env -u RUSTUP_TOOLCHAIN cargo test -p gascan-arca --no-fail-fast`
Expected: PASS, all tests from Tasks 2-8, zero failures.

- [ ] **Step 4: Clippy, fmt, and commit**

```bash
git add crates/gascan-arca Cargo.toml
git commit -m "feat: dial the engine over a Unix socket with tonic

Follows the daemon client's existing UDS dial rather than inventing an
endpoint story: a placeholder authority the connector ignores, and a
connect_with_connector over a UnixStream.

Thin by design. Each unary method is one call and one status conversion,
and the two streaming methods bridge tonic's streams onto the channel pairs
the seam already defines. There is no test here, and that is deliberate: the
only thing that could answer this transport today is a Rust server, which
would be a test double that made a wrong client look correct. The compiler
checking it against EngineTransport is the assurance, and it is not dressed
up as more.

The first real integration risk -- whether Arca's generated server agrees
with this client frame for frame on Exec -- is P5.1's to find."
```

---

### Task 10: Prove the tests fail when the code is wrong, then verify the workspace

**Files:** no production changes. Temporary edits, reverted within the task.

**Interfaces:** none.

**A test that does not fail when the thing it tests is broken is not a test.** Two mutations must be shown to flip. Show them by hand, one at a time, reverting each before the next.

- [ ] **Step 1: Flip the `LOCALHOST` synthesis**

In `crates/gascan-arca/src/translate.rs`, inside `runtime_ports`, change
`host_address: IpAddr::V4(Ipv4Addr::LOCALHOST)` to
`host_address: IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1))`.

Run: `env -u RUSTUP_TOOLCHAIN cargo test -p gascan-arca --lib translate::tests::inbound_ports_regain_the_loopback_address_they_never_sent`
Expected: **FAIL**, with an assertion naming the wrong address. Record the message.

If it passes, the test is not testing what it claims — fix the test before continuing.

- [ ] **Step 2: Revert the mutation and confirm green**

Run: `git diff --stat` to confirm only that one line changed, then `git checkout -- crates/gascan-arca/src/translate.rs`.
Run: `env -u RUSTUP_TOOLCHAIN cargo test -p gascan-arca --lib`
Expected: PASS.

- [ ] **Step 3: Flip the unknown-code rejection**

In `crates/gascan-arca/src/error.rs`, change the final catch-all arm to accept instead of reject:

```rust
        unacceptable => RuntimeError::NotFound {
            resource: format!("{unacceptable}: {message}"),
        },
```

Run: `env -u RUSTUP_TOOLCHAIN cargo test -p gascan-arca --lib error::tests::an_unknown_code_is_rejected_and_names_itself`
Expected: **FAIL** — the code is `not_found` where `invalid_output` was asserted. Confirm `a_code_no_engine_may_raise_is_rejected` fails too.

- [ ] **Step 4: Revert and confirm green**

Run: `git checkout -- crates/gascan-arca/src/error.rs` then
`env -u RUSTUP_TOOLCHAIN cargo test -p gascan-arca --lib`
Expected: PASS. Confirm `git status --short` is clean apart from untracked scratch.

- [ ] **Step 5: Run the whole workspace, which is the bar**

Run, capturing the code directly rather than through a pipe:

```bash
if env -u RUSTUP_TOOLCHAIN cargo test --workspace --no-fail-fast; then rc=0; else rc=$?; fi
echo "rc=$rc"
```

Expected: **rc=0.** The last verified figure before this work was **1382 passed, 0 failed, 22 ignored** at `5ad7ea9`, and it must be **re-measured** rather than trusted — it predates this branch.

Record the new figures and **account for the increase against the tests this plan adds**: 5 (Task 1 — 4 unit plus the mismatched-container pinning test its review added) + 2 (Task 2) + 16 (Tasks 3-4) + 4 (Task 5) + 9 (Task 6) + 3 (Task 7) + 4 (Task 8) = **43**.

**This figure has moved twice and will move again if a review adds a test — recount from the ledger rather than trusting this line.** It was 39 when the plan was written: Task 1's review added one pinning test, Task 3's review added two refusal tests, and Task 4 gained a parity test. The ledger records every such addition at the task that made it, so it is the authority and this number is a convenience.

A total that is merely larger is not accounted for; a total that equals the re-measured baseline plus the ledger's sum is. If it does not match, find out why before claiming a pass — an unaccounted difference has twice been a real defect in this project.

- [ ] **Step 6: Clippy over all targets, and fmt**

```bash
if env -u RUSTUP_TOOLCHAIN cargo clippy --workspace --all-targets -- -D warnings; then rc=0; else rc=$?; fi
echo "clippy rc=$rc"
if env -u RUSTUP_TOOLCHAIN cargo fmt --all --check; then rc=0; else rc=$?; fi
echo "fmt rc=$rc"
```

Expected: both rc=0. `--all-targets` matters: a test that `cargo test` accepts can still be rejected by clippy, which has happened in this repository. Fix by hand — `clippy --fix` is unsafe here.

- [ ] **Step 7: Run the release-contract scripts**

```bash
if ./scripts/ci-run-release-contracts.sh; then rc=0; else rc=$?; fi
echo "contracts rc=$rc"
```

Expected: rc=0. The last verified figure was **15/15** at `5ad7ea9`. A new crate should not move it; if it does, that is a consumer this plan did not find, and it must be understood rather than worked around.

- [ ] **Step 8: Commit the verification record**

Nothing to commit if the tree is clean. If a lint fix was needed:

```bash
git add -A
git commit -m "test: prove the port and error-code tests fail when the code is wrong

The LOCALHOST synthesis and the unknown-code rejection were each broken by
hand and the matching test was confirmed to fail, then reverted. A test that
does not fail when its subject is broken is not a test, and this repository
has shipped three instruments that were confidently wrong."
```

---

## Notes for whoever executes this

**Do not touch D7.** The narrowed retry in `crates/gascan/src/daemon.rs` is approved in principle and deliberately unwritten until a CI run says which of the two `0200` states fired. Run `31262577806` was the first D7-capable run after the instrument landed. Writing the retry because the reasoning seems sound is the specific temptation the kickoff exists to interrupt.

**Do not wire `ArcaBackend` into `gascand`'s backend selection.** It is not in P5.2, and it would make an untested transport reachable at runtime.

**If a mapping decision seems wrong**, check it against `docs/superpowers/specs/2026-08-08-gascan-arca-backend-design.md` before changing it — several of these choices are load-bearing in ways that are not local. Two in particular: `CreateOutcome::new` is called rather than replicated, because it *is* the boundary check; and an unparseable label is `Mismatched` rather than fatal, because `ListResources` deliberately returns unlabelled resources so drift detection can see them.

**When you finish, open a PR. Do not merge to `main` directly, and do not squash.**
