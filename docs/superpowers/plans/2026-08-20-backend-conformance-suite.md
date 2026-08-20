# Backend Conformance Suite Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the existing `RuntimeBackend` conformance contract importable from a shared crate, and run it against all three backends — fake, apple, and arca — so P5's first exit clause can be measured rather than assumed.

**Architecture:** A new dev-dependency-only crate, `gascan-conformance`, owns the contract walk and the `CreateRequest` fixtures. Each backend instantiates it from its own test target. `gascan-core/tests/backend_contract.rs` keeps only what genuinely tests the double's controllability. The contract takes a *fixture* rather than building one, because apple and arca pin different images.

**Tech Stack:** Rust 2024 (workspace `resolver = "3"`), `tokio` test harness, `async_trait`, `tempfile`, `camino`.

**Spec:** `docs/superpowers/specs/2026-08-20-backend-conformance-suite-design.md` — read it first; this plan argues from it.

## Global Constraints

- **Never weaken `gascan-core`'s lint gate.** `crates/gascan-core/src/lib.rs:2` is `#![deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)]`. Do not add `#[allow]` for any of these anywhere, and do not edit that line. The whole reason `gascan-conformance` exists is to avoid it.
- **`PolicyCompiler` is the only way to build a `CreateRequest`.** Its fields are `pub(crate)` to `gascan-core` and it derives no `Deserialize`. Do not add a constructor, do not widen visibility.
- **A missing live prerequisite panics; it never skips.** Rule and rationale at `crates/gascan-arca/tests/live/common/mod.rs:137-140`.
- **Never edit an assertion to make a backend pass.** If arca fails, that is the deliverable. See Task 8.
- CI runs `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace` (`.github/workflows/ci.yml:51`, `:54`, `:57`). All three must be clean at every commit.
- Workspace lints are only `unsafe_code = "forbid"` (`Cargo.toml`, `[workspace.lints.rust]`). The new crate uses `[lints] workspace = true` like its siblings.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/gascan-conformance/Cargo.toml` | New crate manifest; `gascan-core` as a normal dependency |
| `crates/gascan-conformance/src/lib.rs` | `backend_contract()`, `CreateRequestFixture`, `capabilities()` |
| `crates/gascan-conformance/tests/fake.rs` | Instantiation 1 — `FakeRuntime` |
| `crates/gascan-apple/tests/live/backend_contract.rs` | Instantiation 2 — replaces a 65-line hand-rolled duplicate |
| `crates/gascan-arca/tests/live/conformance.rs` | Instantiation 3 — new |
| `crates/gascan-arca/tests/live.rs` | Add `mod conformance;` |
| `crates/gascan-core/tests/backend_contract.rs` | Loses the generic fn and promoted tests; keeps fake-only ones |
| `crates/gascan-core/tests/common/mod.rs` | Loses the fixtures that move; keeps whatever other tests still need |
| `tests/ci/expected-ignored-tests.txt` | The `#[ignore]` baseline; changes in Tasks 6 and 7 |
| `Cargo.toml` | Add the new crate to `members` |

---

### Task 1: Create the `gascan-conformance` crate with its fixtures

**Files:**
- Create: `crates/gascan-conformance/Cargo.toml`
- Create: `crates/gascan-conformance/src/lib.rs`
- Modify: `Cargo.toml` (workspace `members`)

**Interfaces:**
- Consumes: `gascan_core::{manifest::Manifest, policy::PolicyCompiler, runtime::*, sandbox::SandboxSpec}`
- Produces: `gascan_conformance::{capabilities, CreateRequestFixture}`. `CreateRequestFixture::pinned(name: &str, network: &str) -> Self`, `CreateRequestFixture::for_image(name: &str, image: &str, manifest: &str) -> Self`, `fixture.request() -> CreateRequest`, and `Deref<Target = CreateRequest>`.

**`for_image` takes a whole manifest, not a network string, and the argument order matches `policy_request_from_manifest(name, image, manifest)` at `crates/gascan-arca/tests/live/common/mod.rs:718`.** That harness documents why: *"The manifest is the only knob, deliberately. Ports and the guest user are manifest facts and nothing else in this tier may set them."* Arca needs `user = 'root'` in its manifest (Task 5), which a network-only parameter cannot express.

**Why two constructors:** `PolicyCompiler::compile` pins the approved workspace image (`policy.rs:85-90`), which no engine under test holds. Arca's live tier seeds a store with `arca-engine image load` and must ask for what it seeded, so it needs `compile_for_image` (`policy.rs:92-98`). This is recorded at `crates/gascan-arca/tests/live/common/mod.rs:691-694`.

- [ ] **Step 1: Add the crate to the workspace**

In `Cargo.toml`, add `"crates/gascan-conformance"` to `members`, keeping the list alphabetical:

```toml
members = ["crates/gascan", "crates/gascan-apple", "crates/gascan-arca", "crates/gascan-conformance", "crates/gascan-core", "crates/gascan-e2e", "crates/gascan-engine-proto", "crates/gascan-inherited-fd", "crates/gascan-oci-fixture", "crates/gascan-proto", "crates/gascand"]
```

- [ ] **Step 2: Write the manifest**

Create `crates/gascan-conformance/Cargo.toml`:

```toml
[package]
name = "gascan-conformance"
version.workspace = true
edition.workspace = true
license.workspace = true
rust-version.workspace = true
publish = false

[dependencies]
camino.workspace = true
gascan-core = { path = "../gascan-core" }
tempfile = "3"

[dev-dependencies]
tokio = { workspace = true, features = ["macros", "rt", "rt-multi-thread", "sync", "time"] }

[lints]
workspace = true
```

If `version.workspace = true` fails because the workspace defines no shared version, copy the literal version from `crates/gascan-core/Cargo.toml` instead. Check with `sed -n '1,10p' crates/gascan-core/Cargo.toml`.

- [ ] **Step 3: Write the fixtures**

Create `crates/gascan-conformance/src/lib.rs`. This is `crates/gascan-core/tests/common/mod.rs` with a second constructor added — `capabilities()` and the compile body are copied verbatim so behaviour cannot drift:

```rust
//! Backend conformance: one contract, run against every `RuntimeBackend`.
//!
//! This crate exists because `gascan-core/src/lib.rs:2` denies
//! `clippy::unwrap_used`, and a conformance suite is built from unwrapping
//! assertions. It is a dev-dependency of its consumers and ships nowhere.

use camino::Utf8Path;
use gascan_core::manifest::Manifest;
use gascan_core::policy::PolicyCompiler;
use gascan_core::runtime::{CreateRequest, NetworkIsolation, RuntimeCapabilities, RuntimeVersion};
use gascan_core::sandbox::SandboxSpec;
use std::ops::Deref;

pub struct CreateRequestFixture {
    _root: tempfile::TempDir,
    request: CreateRequest,
}

impl CreateRequestFixture {
    /// A request against the approved workspace image.
    ///
    /// Correct for the fake and for apple. **Wrong for a live engine**, whose
    /// store holds only what the tier seeded -- use [`Self::for_image`] there.
    pub fn pinned(name: &str, network: &str) -> Self {
        assert!(matches!(network, "offline" | "networked"));
        Self::build(name, &format!("version = 1\nnetwork = '{network}'\n"), None)
    }

    /// A request against `image`, for a backend whose store was seeded with it.
    ///
    /// The manifest is the only knob, matching `policy_request_from_manifest`
    /// in arca's live tier: the guest user and any ports are manifest facts,
    /// and a caller reaching around them would build a request gascan itself
    /// cannot produce.
    pub fn for_image(name: &str, image: &str, manifest: &str) -> Self {
        Self::build(name, manifest, Some(image))
    }

    pub fn request(&self) -> CreateRequest {
        self.request.clone()
    }

    fn build(name: &str, manifest_text: &str, image: Option<&str>) -> Self {
        let temp = tempfile::tempdir().expect("temporary backend-contract root");
        let root = Utf8Path::from_path(temp.path()).expect("UTF-8 temporary path");
        std::fs::write(root.join("gascan.toml"), manifest_text)
            .expect("write backend-contract manifest");
        let manifest = Manifest::load(root).expect("load backend-contract manifest");
        let spec = SandboxSpec::from_root(name, root, manifest).expect("build sealed sandbox spec");
        let request = match image {
            None => PolicyCompiler::compile(spec, &capabilities()),
            Some(image) => PolicyCompiler::compile_for_image(spec, &capabilities(), image),
        }
        .expect("compile backend-contract policy");
        Self {
            _root: temp,
            request,
        }
    }
}

impl Deref for CreateRequestFixture {
    type Target = CreateRequest;

    fn deref(&self) -> &Self::Target {
        &self.request
    }
}

/// Every flag true. The compiler gates on what a runtime CLAIMS, and the
/// contract only needs a well-formed request; what is under test is the
/// backend's behaviour, not the compiler's gating.
pub fn capabilities() -> RuntimeCapabilities {
    RuntimeCapabilities {
        version: RuntimeVersion::new(1, 1, 0),
        bind_mounts: true,
        named_volumes: true,
        tty: true,
        signals: true,
        loopback_publish: true,
        resource_limits: true,
        offline: NetworkIsolation::Proven,
    }
}
```

**Note the one deliberate change from the original:** `RuntimeVersion::new(1, 0, 0)` becomes `(1, 1, 0)`. Verified safe — `capabilities.version` reaches only an error message (`crates/gascan-core/src/policy.rs:422`, inside `PolicyError::OfflineUnsupported`), and neither backend validates a request's version. `1.1.0` matches apple's floor at `crates/gascan-apple/src/probe.rs:36` and the value arca's live tier already uses.

- [ ] **Step 4: Verify it builds and lints**

Run: `cargo build -p gascan-conformance && cargo clippy -p gascan-conformance --all-targets -- -D warnings`
Expected: both exit 0, no warnings.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/gascan-conformance/
git commit -m "feat: a conformance crate, because gascan-core denies the unwraps a suite needs"
```

---

### Task 2: Move the contract in and instantiate it against the fake

**Files:**
- Modify: `crates/gascan-conformance/src/lib.rs`
- Create: `crates/gascan-conformance/tests/fake.rs`

**Interfaces:**
- Consumes: `CreateRequestFixture` from Task 1.
- Produces: `pub async fn backend_contract(backend: &dyn RuntimeBackend, fixture: &CreateRequestFixture)`.

**The signature change from the original:** the existing `backend_contract` at `crates/gascan-core/tests/backend_contract.rs:149` builds its own request via `create_request("contract")`. It must take one instead, because apple and arca pin different images (Task 1's note).

- [ ] **Step 1: Write the failing test**

Create `crates/gascan-conformance/tests/fake.rs`:

```rust
use gascan_conformance::{CreateRequestFixture, backend_contract, capabilities};
use gascan_core::fake_runtime::FakeRuntime;
use gascan_core::runtime::RuntimeBackend;

#[tokio::test]
async fn fake_runtime_satisfies_the_backend_contract() {
    let backend: Box<dyn RuntimeBackend> = Box::new(FakeRuntime::new(capabilities()));
    let fixture = CreateRequestFixture::pinned("contract", "offline");
    backend_contract(backend.as_ref(), &fixture).await;
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p gascan-conformance --test fake`
Expected: FAIL to compile — `cannot find function 'backend_contract' in crate 'gascan_conformance'`.

- [ ] **Step 3: Move the contract into the crate**

Append to `crates/gascan-conformance/src/lib.rs`. The body is copied **verbatim** from `crates/gascan-core/tests/backend_contract.rs:149-179`; only the signature and the first two lines change, so that a diff shows the move rather than a rewrite:

```rust
use gascan_core::runtime::{
    ContainerState, ExecInput, ExecOutput, ExecRequest, RemoveRequest, ResourceKind,
    RuntimeBackend,
};

/// The contract every `RuntimeBackend` owes, whatever it is implemented over.
///
/// `fixture` is a parameter and not built here because `PolicyCompiler::compile`
/// pins the approved workspace image, which a live engine's seeded store does
/// not hold -- see `CreateRequestFixture::for_image`.
pub async fn backend_contract(backend: &dyn RuntimeBackend, fixture: &CreateRequestFixture) {
    let id = fixture.id().clone();
    assert_eq!(backend.inspect(&id).await.unwrap(), None);
    let created = backend.create(fixture.request()).await.unwrap();
    assert!(
        created
            .created()
            .iter()
            .any(|resource| resource.kind() == ResourceKind::Container)
    );
    assert_eq!(
        backend.inspect(&id).await.unwrap().unwrap().state,
        ContainerState::Stopped
    );
    backend.start(&id).await.unwrap();
    let mut session = backend
        .exec(ExecRequest::fixture(id.clone(), ["true"]))
        .await
        .unwrap();
    session.send(ExecInput::Close).await.unwrap();
    assert_eq!(
        session.next().await.unwrap().unwrap(),
        ExecOutput::Exit { code: 0, signal: 0 }
    );
    backend.stop(&id).await.unwrap();
    backend
        .remove(RemoveRequest::from_resources(created.created().to_vec()).unwrap())
        .await
        .unwrap();
    assert_eq!(backend.inspect(&id).await.unwrap(), None);
}
```

If `fixture.id()` does not resolve, it is reached through the `Deref` to `CreateRequest`; confirm with `grep -n "pub fn id" crates/gascan-core/src/runtime.rs`.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p gascan-conformance --test fake`
Expected: PASS, `1 passed`.

- [ ] **Step 5: Prove the contract can fail**

This guards against a contract that asserts nothing. Temporarily edit the assertion after `stop`:

```rust
    assert_eq!(backend.inspect(&id).await.unwrap(), None);
```

to compare against `Some(...)`-shaped nonsense — simplest is to change the final line to `assert!(backend.inspect(&id).await.unwrap().is_some());`

Run: `cargo test -p gascan-conformance --test fake`
Expected: FAIL. Then **revert the edit** and re-run to confirm PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/gascan-conformance/
git commit -m "feat: the backend contract moves to where every backend can reach it"
```

---

### Task 3: Strip the moved code out of `gascan-core`

**Files:**
- Modify: `crates/gascan-core/tests/backend_contract.rs` (delete `:149-179` and the fake trait-object test)
- Modify: `crates/gascan-core/tests/common/mod.rs`

**Interfaces:**
- Consumes: nothing new.
- Produces: nothing. This task only removes duplication.

- [ ] **Step 1: Delete the generic function and its fake instantiation**

Remove `pub async fn backend_contract(...)` (`:149-179`) and `fake_runtime_satisfies_backend_contract_through_trait_object` (which calls it — locate with `grep -n "fake_runtime_satisfies_backend_contract_through_trait_object" crates/gascan-core/tests/backend_contract.rs`). Task 2's `tests/fake.rs` replaces both.

- [ ] **Step 2: Remove now-unused fixtures and imports**

`create_request`, `create_request_with_network`, `capabilities` and `CreateRequestFixture` may still be used by the fake-only tests that remain in this file, and by other test files in `crates/gascan-core/tests/`. **Check before deleting:**

Run: `grep -rn "create_request\|capabilities()\|CreateRequestFixture" crates/gascan-core/tests/`

Delete from `common/mod.rs` only what nothing references. If everything still references them, `common/mod.rs` is unchanged and that is a correct outcome — the duplication with `gascan-conformance` is then deliberate and short-lived, and Task 9 revisits it.

- [ ] **Step 3: Verify the crate still tests clean**

Run: `cargo test -p gascan-core`
Expected: PASS. The count drops by exactly 1 (the removed trait-object test). Record the before and after numbers in the commit message.

- [ ] **Step 4: Verify no warnings**

Run: `cargo clippy -p gascan-core --all-targets -- -D warnings`
Expected: exit 0. Unused-import warnings here are the signal that Step 2 missed something.

- [ ] **Step 5: Commit**

```bash
git add crates/gascan-core/
git commit -m "refactor: gascan-core stops owning a contract every backend needs"
```

---

### Task 4: Instantiate against apple, deleting the hand-rolled duplicate

**Files:**
- Modify: `crates/gascan-apple/tests/live/backend_contract.rs` (replace all 65 lines)
- Modify: `crates/gascan-apple/Cargo.toml` (add the dev-dependency)

**Interfaces:**
- Consumes: `gascan_conformance::{backend_contract, CreateRequestFixture}`.
- Produces: nothing later tasks use.

- [ ] **Step 1: Add the dev-dependency**

In `crates/gascan-apple/Cargo.toml`, under `[dev-dependencies]`:

```toml
gascan-conformance = { path = "../gascan-conformance" }
```

- [ ] **Step 2: Replace the file**

`crates/gascan-apple/tests/live/backend_contract.rs` becomes:

```rust
use gascan_apple::{AppleBackend, ProcessRunner};
use gascan_conformance::{CreateRequestFixture, backend_contract};
use std::time::{SystemTime, UNIX_EPOCH};

#[tokio::test]
#[ignore = "requires Apple silicon macOS 26+ with container service and locked workspace image"]
async fn backend_contract_holds_on_apple() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let name = format!("gascan-live-backend-{}-{nonce}", std::process::id());
    let fixture = CreateRequestFixture::pinned(&name, "offline");
    backend_contract(&AppleBackend::new(ProcessRunner), &fixture).await;
}
```

**Two behaviours the old file had that the shared contract also has**, so nothing is lost: the `inspect`-absent bookends and the create/start/stop/remove walk. **One it had that the contract does not** — a final `list_resources()` check that no resource name starts with the sandbox name. Add it after the `backend_contract` call rather than dropping it:

```rust
    let backend = AppleBackend::new(ProcessRunner);
    backend_contract(&backend, &fixture).await;
    assert!(
        !backend
            .list_resources()
            .await
            .unwrap()
            .iter()
            .any(|resource| resource.name().starts_with(&name))
    );
```

with `use gascan_core::runtime::RuntimeBackend;` added for `list_resources`. Restructure the test to bind `backend` once, as above.

**The old file called `start` twice and `stop` twice** to assert idempotence. The shared contract calls each once. Do **not** silently drop that — it is a real assertion and Task 7 promotes it. Until then, note it in the commit message as temporarily uncovered on apple.

- [ ] **Step 3: Verify it compiles**

Run: `cargo test -p gascan-apple --no-run`
Expected: compiles clean. It cannot be *run* without an Apple runtime; that is Step 5.

- [ ] **Step 4: Update the ignored-test baseline**

The test's name changed from `backend_contract` to `backend_contract_holds_on_apple`, so the baseline must change or CI fails.

Run: `./scripts/ci-check-ignored-tests.sh`
Expected: FAIL, naming both the removed and added entries.

Edit `tests/ci/expected-ignored-tests.txt`: replace the `backend_contract::backend_contract` line with `backend_contract::backend_contract_holds_on_apple`, keeping the file's existing sort order.

Run: `./scripts/ci-check-ignored-tests.sh`
Expected: PASS.

- [ ] **Step 5: Run it for real, if this machine can**

Prerequisites, verified present on the maintainer's machine on 2026-08-20: `container` CLI 1.1.0 at `/usr/local/bin/container` with `container system status` reporting `running`, and two `ghcr.io/liquescent-development/gascan/workspace` images.

Run: `cargo test -p gascan-apple --test live -- --ignored backend_contract_holds_on_apple`
Expected: PASS.

**If it fails, stop and read the failure before changing anything.** A failure here means either the extraction changed behaviour (Task 2's fault) or apple never satisfied the shared contract's extra assertions (a finding). Record which, in the commit.

**If this machine cannot run it, say so explicitly in the commit message** — do not imply it passed. Apple's tier runs in no CI job (no workflow step passes `--ignored` for `gascan-apple`), so this run is the only evidence that will ever exist.

- [ ] **Step 6: Commit**

```bash
git add crates/gascan-apple/ tests/ci/expected-ignored-tests.txt
git commit -m "refactor: apple runs the shared contract instead of its own copy of it"
```

---

### Task 5: Instantiate against arca

**Files:**
- Create: `crates/gascan-arca/tests/live/conformance.rs`
- Modify: `crates/gascan-arca/tests/live.rs` (add `mod conformance;`)
- Modify: `crates/gascan-arca/Cargo.toml` (add the dev-dependency)

**Interfaces:**
- Consumes: `gascan_conformance::{backend_contract, CreateRequestFixture}`, and the live tier's existing engine harness in `crates/gascan-arca/tests/live/common/mod.rs`.
- Produces: nothing later tasks use.

**Read first:** `crates/gascan-arca/tests/live/lifecycle.rs` — it is the closest existing test and shows how the tier starts an engine, seeds an image, and builds a backend. Copy its setup shape rather than inventing one.

- [ ] **Step 1: Add the dev-dependency**

In `crates/gascan-arca/Cargo.toml`, under `[dev-dependencies]`:

```toml
gascan-conformance = { path = "../gascan-conformance" }
```

- [ ] **Step 2: Write the test**

Create `crates/gascan-arca/tests/live/conformance.rs`. **Three things differ from apple and each one would fail the run if missed:**

1. **`network = 'networked'`, not `'offline'`.** `lifecycle.rs:31` uses `networked`, and offline is exactly the capability the pinned engine does not honour (`docs/evidence/2026-08-18-arca-engine-offline.md`). An offline request here would test the refuted property by accident.
2. **`user = 'root'`.** `lifecycle.rs:24-30` records why: the base layout is a stock alpine with no `workspace` account, and the engine translates `UserMode::Workspace` to the literal string `workspace` and hands it to `createContainer` — so `start` would fail on the image rather than on anything under test.
3. **A staying-up image, not the bare base.** Alpine's own `Cmd` is `/bin/sh`, which exits immediately with no tty attached (`lifecycle.rs:35-37`). The contract does `start` → `exec` → `stop`, so the container has to still be there. `layout_running` rewrites the `Cmd` without rebuilding the rootfs.

```rust
use crate::common::{LiveEngine, base_oci_layout, layout_running};
use camino::Utf8Path;
use gascan_arca::ArcaBackend;
use gascan_conformance::{CreateRequestFixture, backend_contract};

/// The tag the derived layout is loaded under.
const TAG: &str = "gascan-conformance:latest";

/// `user = 'root'` because the base layout is a stock alpine with no
/// `workspace` account -- see `lifecycle.rs`'s note on the same constant.
const MANIFEST: &str = "version = 1\nnetwork = 'networked'\nuser = 'root'\n";

#[tokio::test]
#[ignore = "requires a built arca-engine, a kernel, a vminit layout and a base OCI layout"]
async fn backend_contract_holds_on_arca() {
    let temp = tempfile::tempdir().expect("a temporary layout root");
    let destination = Utf8Path::from_path(temp.path()).expect("a utf-8 temporary path");
    let layout = layout_running(
        &base_oci_layout(),
        destination,
        TAG,
        &["sh", "-c", "while :; do sleep 1; done"],
    );
    let engine = LiveEngine::start_with_images(&[layout.as_path()]).await;
    let backend = ArcaBackend::new(engine.transport().await);
    let fixture = CreateRequestFixture::for_image("conformance", TAG, MANIFEST);
    backend_contract(&backend, &fixture).await;
}
```

Signatures this uses, all re-derived on 2026-08-20: `layout_running(base: &Utf8Path, destination: &Utf8Path, tag: &str, command: &[&str]) -> Utf8PathBuf` (`crates/gascan-oci-fixture/src/lib.rs:39`, re-exported through `crate::common`); `LiveEngine::start_with_images(layouts: &[&Utf8Path])` (`live/common/mod.rs:347`); `LiveEngine::transport(&self) -> ChannelTransport` (`:447`); `base_oci_layout() -> Utf8PathBuf` (`:156`); `ArcaBackend::new(engine.transport().await)` (`lifecycle.rs:14-16`).

- [ ] **Step 3: Register the module**

Add `mod conformance;` to `crates/gascan-arca/tests/live.rs`, in the file's existing alphabetical position.

- [ ] **Step 4: Verify it compiles**

Run: `cargo test -p gascan-arca --test live --no-run`
Expected: compiles clean.

- [ ] **Step 5: Update the ignored-test baseline**

Run: `./scripts/ci-check-ignored-tests.sh`
Expected: FAIL, naming the added entry.

Add `conformance::backend_contract_holds_on_arca` to `tests/ci/expected-ignored-tests.txt` in sort order, then re-run.
Expected: PASS.

- [ ] **Step 6: Run it — this is the measurement the whole plan exists for**

Set the four variables the tier requires (`crates/gascan-arca/tests/live/common/mod.rs` names each and panics with a directive when absent), then:

Run: `cargo test -p gascan-arca --test live -- --ignored backend_contract_holds_on_arca`

**There is no expected result.** Whatever happens is the finding. Record the exact output.

- [ ] **Step 7: Commit the result, whichever it is**

If it passes, say so with the command and the engine revision from `engine/arca-pin.json`.

If it fails, **commit the failing test anyway**, with the failure quoted in full in the commit message and the assertion that failed named. Do not weaken the contract, do not add `#[ignore]` beyond the tier's existing one, and do not "fix" arca in this commit — that is separate work, and conflating them destroys the evidence.

```bash
git add crates/gascan-arca/ tests/ci/expected-ignored-tests.txt
git commit -m "test: arca meets the shared backend contract, and here is what it did"
```

---

### Task 6: Promote start/stop idempotence

**Files:**
- Modify: `crates/gascan-conformance/src/lib.rs`
- Modify: `crates/gascan-core/tests/backend_contract.rs` (remove the promoted half)

**Interfaces:**
- Consumes: `backend_contract` from Task 2.
- Produces: no new public names; `backend_contract`'s body grows.

**Why this one first:** apple's deleted file already asserted it (Task 4 recorded it as temporarily uncovered), so promoting it closes a regression this plan itself opened. It is also the clearest case of the §4 rule — idempotent `start`/`stop` is implemented separately by every backend.

- [ ] **Step 1: Read the source test**

Run: `grep -n "duplicate_create_is_rejected_and_start_stop_are_idempotent" -A 40 crates/gascan-core/tests/backend_contract.rs`

Separate what any backend owes (repeated `start` and `stop` succeed; a second `create` of the same id is rejected) from what the fake happens to do (any assertion reaching `calls()`, `outcomes()`, or a `seed_*` method — those cannot move).

- [ ] **Step 2: Add the generic half to the contract**

In `backend_contract`, replace the single `start`/`stop` calls with doubled ones:

```rust
    backend.start(&id).await.unwrap();
    backend.start(&id).await.unwrap();
```

and

```rust
    backend.stop(&id).await.unwrap();
    backend.stop(&id).await.unwrap();
```

- [ ] **Step 3: Verify the fake still passes**

Run: `cargo test -p gascan-conformance --test fake`
Expected: PASS.

- [ ] **Step 4: Prove the new assertion can fail**

In `crates/gascan-core/src/fake_runtime.rs`, make `start` fail when the sandbox is already running — find it with `grep -n "async fn start" crates/gascan-core/src/fake_runtime.rs` and return `RuntimeError::InvalidState { .. }` on the second call.

Run: `cargo test -p gascan-conformance --test fake`
Expected: **FAIL** on the doubled `start`.

**Then revert the fake edit** and re-run.
Expected: PASS.

A promoted assertion that cannot be made to fail is testing nothing. This repository has measured exactly that outcome — `docs/status/START-HERE.md:604` records a guard whose deletion *and* inversion both left the suite green.

- [ ] **Step 5: Remove the now-duplicated half from `gascan-core`**

Delete only the start/stop-idempotence assertions from `duplicate_create_is_rejected_and_start_stop_are_idempotent`. **Keep the duplicate-create rejection there** unless Step 1 showed it needs no fake-only machinery — if it needs none, promote it too, repeating Steps 2-4 for it.

Run: `cargo test -p gascan-core`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/gascan-conformance/ crates/gascan-core/
git commit -m "test: start and stop are idempotent on every backend, not just the fake"
```

---

### Task 7: Triage the remaining promotion candidates

**Files:**
- Modify: `crates/gascan-conformance/src/lib.rs`
- Modify: `crates/gascan-core/tests/backend_contract.rs`
- Modify: `docs/superpowers/specs/2026-08-20-backend-conformance-suite-design.md` (record the outcome)

**Interfaces:**
- Consumes: `backend_contract` from Task 2.
- Produces: no new public names.

**This task is a judgment, and the plan supplies the criterion rather than the answer.** The spec's §4 rule: *an assertion earns promotion only if the behaviour it names is implemented separately by each backend.* Shared code tested once is done — that is why the ownership assertions are **not** on this list; `classify_resource_ownership` is one pure function at `crates/gascan-core/src/runtime.rs:85-103`, already tested exhaustively by `crates/gascan-core/tests/resource_ownership.rs`.

**Candidates, in the order to take them:**

| Test | The question to answer |
|---|---|
| `exec_and_logs_preserve_binary_bytes_and_exact_exit_code` | Does it need `set_exec_result`/`queue_exec_results`? Those are fake-only. Can the same property be asserted by exec'ing a real command that emits known bytes? |
| `exec_session_is_live_bidirectional_and_emits_one_exit` | Same question; bidirectional streaming against a real backend needs a real interactive command. |
| `create_collision_reports_resources_created_before_the_collision` | Needs a pre-existing colliding resource. On a real backend, can that be produced by creating twice? |
| `offline_fake_create_has_no_managed_network` | Real for apple. **Arca cannot honour offline** — if promoted, arca's instantiation must keep `networked` and this assertion must be conditional, which is a design change. Consider leaving it fake-and-apple-only. |
| `networked_fake_create_reports_network_then_volumes_then_container` | The *ordering* is asserted through the fake's call recorder. Only the *effect* is portable. Likely stays. |
| `persistent_logs_are_isolated_by_exact_sandbox_id` | Does isolation-by-id need `FakeRuntime::persistent`? If so it is fake-only. |

- [ ] **Step 1: Take each candidate in turn**

For each row: read the test, apply the criterion, and reach one of two outcomes.

- [ ] **Step 2: If it promotes — repeat Task 6's cycle exactly**

Add the generic assertion to `backend_contract`; run the fake instantiation and see it pass; **break the behaviour in `fake_runtime.rs` and see it fail**; revert; remove the duplicated half from `gascan-core`; commit one candidate per commit.

- [ ] **Step 3: If it does not promote — record why, in the spec**

Append a row to §3's fake-only list in `docs/superpowers/specs/2026-08-20-backend-conformance-suite-design.md` naming the test and the fake-only machinery it depends on. A candidate silently left behind is indistinguishable from one that was forgotten.

- [ ] **Step 4: Re-run everything**

Run: `cargo test -p gascan-conformance && cargo test -p gascan-core && ./scripts/ci-check-ignored-tests.sh`
Expected: all PASS.

- [ ] **Step 5: Reconcile the count**

The spec estimated **6-8** promotions including Task 6's. Count what actually promoted. **If the real number is outside that range, update the spec's §3 rather than leaving the estimate standing** — a spec whose numbers went stale inside its own implementation is this project's most-repeated failure.

- [ ] **Step 6: Commit the spec update**

```bash
git add docs/superpowers/specs/2026-08-20-backend-conformance-suite-design.md
git commit -m "docs: what promoted, what did not, and the machinery that decided each"
```

---

### Task 8: Record the measurement and close out

**Files:**
- Create: `docs/evidence/2026-08-20-backend-conformance.md`
- Modify: `docs/status/START-HERE.md`

**Interfaces:**
- Consumes: Task 5's arca result and Task 7's promotion outcome.
- Produces: the durable record.

**The spec's acceptance criterion 8 is the one under pressure here:** arca's result is a *finding*, not a pass criterion. If arca failed, the deliverable is the measurement and its write-up — not a green suite.

- [ ] **Step 1: Write the evidence document**

`docs/evidence/2026-08-20-backend-conformance.md`, following the shape of `docs/evidence/2026-08-18-arca-engine-offline.md`. It must state: the exact command run for each backend, the engine revision from `engine/arca-pin.json`, the machine, the date, what passed, what failed, and — for anything not run — that it was not run and why. Never write a counterfactual as an event.

- [ ] **Step 2: Update the handoff**

In `docs/status/START-HERE.md`, update the queue entry for P5.3 to record it as done or partially done, with the evidence document referenced. If arca failed an assertion, that becomes a new open item naming the assertion and pointing at the evidence.

- [ ] **Step 3: Full verification, against CI's own step list**

Run each, and record the exit code of each:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
./scripts/ci-check-ignored-tests.sh
```

Expected: all exit 0. Note that `cargo test --workspace` on this machine has a measured ~28% failure rate from pre-existing flakes (`START-HERE.md`, THE NINTH MECHANISM) — a failure in `gascan --lib` that is not in the crates this plan touched is very likely one of those, and should be checked against that list before being treated as a regression.

- [ ] **Step 4: Confirm the crate ships nowhere**

Run: `cargo tree -p gascan --edges normal | grep gascan-conformance || echo "not in gascan's normal dependency graph"`
Expected: the `echo` branch. Repeat for `gascand`. A conformance crate reachable from a shipped binary is a packaging defect.

- [ ] **Step 5: Commit and open the PR**

```bash
git add docs/
git commit -m "docs: what the conformance suite measured on each backend"
```

Open a PR against `main`. **Never merge to `main` directly.** The PR body states, for each of the three backends, whether the contract was run, on what, and with what result.

---

## Self-Review

**Spec coverage:** §1 (contract exists, needs relocating) → Tasks 2-3. §2 (separate crate, why not `gascan-core`) → Task 1. §3 (promote/fake-only split) → Tasks 6-7. §4 (ownership overbuild rejected) → Task 7's preamble, which restates the criterion and excludes ownership by name. §5 (out of scope) → nothing implements it, correctly; the product-e2e work is absent from every task. §6 (testing, ignored-set guard, the fail-open hazard not applying) → Tasks 4-5 update the baseline, Task 8 Step 3 runs the guard. §7 (acceptance, all 8 criteria) → criterion 1 Task 1, 2 Global Constraints, 3 Task 4, 4 Tasks 2/4/5, 5 Tasks 4/5/8, 6 Tasks 6/7, 7 Task 8 Step 3, 8 Task 5 Step 7 and Task 8.

**No placeholders remain.** An earlier draft left three `/* … */` holes in Task 5 for the arca harness's constructor names. Reading `lifecycle.rs` and `gascan-oci-fixture` to close them surfaced **three** constraints the holes were hiding, each of which would have failed the first run: the manifest needs `user = 'root'` (stock alpine has no `workspace` account), the image needs a rewritten `Cmd` (alpine's own exits immediately, so there would be no container left to `exec` into), and the network must be `networked` (offline is the refuted capability). That is the argument for filling holes rather than annotating them — the hole was not missing syntax, it was three missing facts.

That discovery also changed Task 1's API: `for_image` takes a whole manifest rather than a network string, matching `policy_request_from_manifest(name, image, manifest)`, because `user` is a manifest fact and a network-only parameter cannot carry it.

**Type consistency:** `CreateRequestFixture` is introduced in Task 1 with `pinned`/`for_image`/`request`, and used under exactly those names in Tasks 2, 4 and 5. `backend_contract(&dyn RuntimeBackend, &CreateRequestFixture)` is defined in Task 2 and called with that arity in Tasks 4, 5, 6 and 7. `capabilities()` is defined in Task 1 and used in Task 2.
