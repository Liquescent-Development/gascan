# Durable Controller State Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Persist controller inventory outside ephemeral runtime storage, migrate legacy state without guessing or losing data, and hide destroyed tombstones from ordinary sandbox listings.

**Architecture:** Add a focused `gascand::controller_state` boundary for safe paths, SQLite snapshot migration, conflict detection, and archival. Keep `GASCAN_STATE_PATH` as an explicit bypass, wire production startup through the new boundary, and implement `gascan list --all` at the CLI boundary so the daemon protocol remains compatible.

**Tech Stack:** Rust workspace, Tokio, rusqlite 0.32 with bundled SQLite and online backup support, rustix filesystem APIs, Clap, Bash macOS packaging contracts.

## Global Constraints

- The macOS database path is exactly `~/Library/Application Support/dev.gascan/controller/state.sqlite3`, resolved from the effective account rather than mutable `HOME`.
- Only daemon IPC files remain under `/private/tmp/gascan-<uid>` or `$XDG_RUNTIME_DIR/gascan`.
- `GASCAN_STATE_PATH` opens exactly its configured path and bypasses migration.
- Migration never overwrites or merges conflicting active databases.
- Existing records, sandbox IDs, managed resources, tombstones, and operation history remain intact.
- New controller directories use mode `0700`; database, temporary, and backup files use `0600`; symlinks, foreign ownership, and unsafe modes are rejected.
- Ordinary human and JSON lists hide `Absent`; `--all` exposes it and human output calls it `Destroyed`.
- No protobuf or RPC change is required.
- All behavior changes follow red-green-refactor.

## File Map

- Create `crates/gascand/src/controller_state.rs`: durable paths and migration state machine.
- Create `crates/gascand/tests/controller_state.rs`: path, security, WAL, conflict, and crash tests.
- Modify `crates/gascand/src/lib.rs` and `main.rs`: export and use controller state.
- Modify root `Cargo.toml` and `Cargo.lock`: enable rusqlite backup support.
- Modify `crates/gascan/src/cli.rs` and `presentation.rs`: list filtering and labels.
- Modify `crates/gascan-e2e/tests/fake_backend.rs` and `autostart.rs`: lifecycle acceptance.
- Modify macOS release/uninstall scripts and contracts: preserve or explicitly remove durable state.
- Modify `README.md` and the macOS release checklist: document the contract.

---

### Task 1: Durable Paths and Fresh Store

**Files:**
- Create: `crates/gascand/src/controller_state.rs`
- Create: `crates/gascand/tests/controller_state.rs`
- Modify: `crates/gascand/src/lib.rs`

**Interfaces:**
- Consumes: `gascan_core::account::effective_account_home()`, `SocketPaths::directory()`, `Store::open(PathBuf)`.
- Produces:
  - `pub struct ControllerStatePaths`
  - `ControllerStatePaths::for_user(runtime_directory: &Path) -> Result<Self, ControllerStateError>`
  - `ControllerStatePaths::for_home_and_runtime(home: &Path, runtime_directory: &Path, expected_uid: u32) -> Result<Self, ControllerStateError>`
  - `durable_database(&self) -> &Path` and `legacy_database(&self) -> &Path`
  - `open_controller_store(&ControllerStatePaths) -> Result<Store, ControllerStateError>`
  - `ControllerStateError::code(&self) -> &'static str`

- [ ] **Step 1: Write failing path and fresh-store tests**

Add these tests plus cases for relative/parent paths, symlinked managed components, non-regular files, foreign ownership where supported, and unsafe modes:

```rust
#[test]
fn default_paths_split_durable_state_from_runtime_ipc() -> TestResult {
    let fixture = ControllerFixture::new()?;
    assert_eq!(
        fixture.paths.durable_database(),
        fixture.home.join("Library/Application Support/dev.gascan/controller/state.sqlite3")
    );
    assert_eq!(fixture.paths.legacy_database(), fixture.runtime.join("state.sqlite3"));
    Ok(())
}

#[test]
fn fresh_open_creates_only_a_private_durable_store() -> TestResult {
    let fixture = ControllerFixture::new()?;
    let store = open_controller_store(&fixture.paths)?;
    assert!(store.list_sandboxes()?.is_empty());
    assert_private_directory(fixture.controller_directory(), 0o700)?;
    assert_private_file(fixture.paths.durable_database(), 0o600)?;
    assert!(!fixture.paths.legacy_database().exists());
    Ok(())
}
```

- [ ] **Step 2: Verify RED**

Run `cargo test -p gascand --test controller_state -- --nocapture`.

Expected: compilation fails because the controller-state interfaces do not exist.

- [ ] **Step 3: Implement safe path creation and fresh opening**

Define exact constants and the path object:

```rust
const APPLICATION_ID: &str = "dev.gascan";
const CONTROLLER_DIRECTORY: &str = "controller";
const DATABASE_NAME: &str = "state.sqlite3";
const DIRECTORY_MODE: u32 = 0o700;
const FILE_MODE: u32 = 0o600;

pub struct ControllerStatePaths {
    durable_database: PathBuf,
    legacy_database: PathBuf,
    expected_uid: u32,
}
```

Use the effective account home in production. Traverse existing home, `Library`, and `Application Support` with no-follow directory descriptors. Create only `dev.gascan/controller` with `mkdirat`, mode `0700`, and immediate ownership/type/mode validation. Open or create the database with no-follow semantics and mode `0600` before handing its path to `Store`.

- [ ] **Step 4: Verify GREEN**

Run:

```sh
cargo test -p gascand --test controller_state
cargo test -p gascand --lib
```

- [ ] **Step 5: Commit**

```sh
git add crates/gascand/src/controller_state.rs crates/gascand/tests/controller_state.rs crates/gascand/src/lib.rs
git commit -S -m "feat: add durable controller state paths"
```

---

### Task 2: Lossless Migration and Conflict Refusal

**Files:**
- Modify: `Cargo.toml`
- Modify: `Cargo.lock`
- Modify: `crates/gascand/src/controller_state.rs`
- Modify: `crates/gascand/tests/controller_state.rs`

**Interfaces:**
- Consumes: Task 1 interfaces.
- Produces `MigrationFault`, `open_controller_store_with_fault`, and stable error codes `controller_state_conflict`, `controller_state_unsafe`, `controller_state_invalid`, and `controller_state_migration_failed`.

- [ ] **Step 1: Write failing migration tests**

Cover legacy-only, committed uncheckpointed WAL content, durable-only, identical dual-state, conflicting dual-state, backup-name collision, malformed schemas, sidecars, and each crash boundary. The conflict test must capture every active database/sidecar before calling the function and assert byte-for-byte equality afterward:

```rust
#[test]
fn conflicting_active_databases_are_untouched() -> TestResult {
    let fixture = ControllerFixture::new()?;
    fixture.seed_durable_store("durable")?;
    fixture.seed_legacy_store("legacy")?;
    let before = fixture.capture_active_files()?;
    let error = open_controller_store(&fixture.paths).expect_err("conflict must refuse");
    assert_eq!(error.code(), "controller_state_conflict");
    assert!(error.to_string().contains("No data was changed"));
    assert_eq!(fixture.capture_active_files()?, before);
    Ok(())
}
```

- [ ] **Step 2: Verify RED**

Run `cargo test -p gascand --test controller_state migration -- --nocapture` and the exact conflict test. Expected: no migration/fault behavior exists.

- [ ] **Step 3: Enable online backup support**

Change only the existing workspace dependency:

```toml
rusqlite = { version = "0.32", features = ["backup", "bundled"] }
```

Refresh the lockfile offline only if Cargo changes feature metadata; do not update unrelated packages.

- [ ] **Step 4: Implement the migration state machine**

```rust
match (safe_regular_file(durable)?, safe_regular_file(legacy)?) {
    (false, false) => create_fresh_durable_store(paths),
    (true, false) => open_validated_durable_store(paths),
    (false, true) => migrate_legacy_store(paths, fault),
    (true, true) => resolve_dual_store(paths, fault),
}
```

Use `rusqlite::backup::Backup` to a collision-free temporary database inside the durable directory, validate it through `Store`, set private mode, sync file and directory, and atomically rename. Archive the legacy database and sidecars through private copy/fsync/rename/unlink so `/private/tmp` and the home directory may be different filesystems.

Compare dual stores by validated logical content, not file bytes: make consistent snapshots, attach them read-only, and run bidirectional `EXCEPT` comparisons for `schema_version`, `sandboxes`, `operations`, and `operation_events`. Different rows refuse without mutation. Identical rows archive the legacy active names.

Place deterministic faults at `BeforeSnapshotComplete`, `BeforeDurableRename`, `AfterDurableRename`, and `DuringLegacyArchive`. Abandoned-temp cleanup accepts only exact names, regular files, current ownership, and private modes.

- [ ] **Step 5: Verify GREEN**

```sh
cargo test -p gascand --test controller_state -- --nocapture
cargo test -p gascand --test store
cargo test -p gascand --lib
```

- [ ] **Step 6: Commit**

```sh
git add Cargo.toml Cargo.lock crates/gascand/src/controller_state.rs crates/gascand/tests/controller_state.rs
git commit -S -m "feat: migrate controller state safely"
```

---

### Task 3: Production Startup and Daemon Replacement

**Files:**
- Modify: `crates/gascand/src/main.rs`
- Modify: `crates/gascan-e2e/tests/fake_backend.rs`
- Modify: `crates/gascan-e2e/tests/autostart.rs`

**Interfaces:**
- Consumes: Task 2 state opener and the existing daemon lifecycle supervisor.
- Produces: durable default selection with an exact explicit override.

- [ ] **Step 1: Write failing E2E tests**

Add a dedicated debug environment that omits `GASCAN_STATE_PATH`, uses `GASCAN_E2E_ACCOUNT_HOME` only as the existing debug account-home hook, and keeps runtime and Application Support roots separate. Seed a legacy sandbox record, start and replace the daemon, delete/recreate only the runtime directory, and prove the record plus fake managed-volume marker remain. Add a subprocess test proving an explicit state path creates no default database.

- [ ] **Step 2: Verify RED**

```sh
cargo test -p gascan-e2e --test fake_backend durable_controller_state_survives_daemon_replacement -- --exact --nocapture
cargo test -p gascan-e2e --test autostart explicit_state_path_bypasses_default_migration -- --exact --nocapture
```

- [ ] **Step 3: Wire startup**

```rust
let store = match std::env::var_os("GASCAN_STATE_PATH") {
    Some(path) => Store::open(std::path::PathBuf::from(path))?,
    None => {
        let state = ControllerStatePaths::for_user(paths.directory())?;
        open_controller_store(&state)?
    }
};
```

Allow the debug E2E binary to inject its existing account-home fixture; production ignores mutable `HOME`. Preserve the controller error code and actionable text through automatic start, `daemon start`, and restart rather than collapsing it into `backend_unavailable`.

- [ ] **Step 4: Verify GREEN**

```sh
cargo test -p gascan-e2e --test autostart -- --nocapture
cargo test -p gascan-e2e --test fake_backend -- --nocapture
cargo test -p gascand --test daemon_idle -- --nocapture
```

- [ ] **Step 5: Commit**

```sh
git add crates/gascand/src/main.rs crates/gascan-e2e/tests/fake_backend.rs crates/gascan-e2e/tests/autostart.rs
git commit -S -m "fix: preserve controller state across upgrades"
```

---

### Task 4: Honest List UX

**Files:**
- Modify: `crates/gascan/src/cli.rs`
- Modify: `crates/gascan/src/presentation.rs`
- Modify: `crates/gascan-e2e/tests/fake_backend.rs`

**Interfaces:**
- Consumes unchanged List RPC records.
- Produces `List { all: bool, json: bool }` and CLI-side filtering.

- [ ] **Step 1: Write failing unit and E2E tests**

Prove ordinary filtering, `--all` retention, human `Destroyed`, JSON `absent`, empty list after final destroy, implicit selection ignoring tombstones, and recreation with the same sandbox ID.

```rust
#[test]
fn ordinary_list_filters_absent_records() {
    let listed = listed_sandboxes(
        vec![status("running", v1::ActualState::Running), status("old", v1::ActualState::Absent)],
        false,
    );
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].sandbox_id, "running");
}
```

- [ ] **Step 2: Verify RED**

```sh
cargo test -p gascan ordinary_list_filters_absent_records -- --exact
cargo test -p gascan-e2e --test fake_backend destroyed_tombstones_require_list_all -- --exact --nocapture
```

Expected: Clap rejects `--all`, ordinary output includes `Absent`, and human output is mislabeled.

- [ ] **Step 3: Implement filtering and labels**

Add `all: bool` to `Command::List`, then filter the unchanged RPC result:

```rust
fn listed_sandboxes(sandboxes: Vec<v1::SandboxStatus>, all: bool) -> Vec<v1::SandboxStatus> {
    sandboxes
        .into_iter()
        .filter(|sandbox| all || sandbox.actual_state != v1::ActualState::Absent as i32)
        .collect()
}
```

Keep `status_json` unchanged. Change only list-table rendering of included `Absent` records to `Destroyed`.

- [ ] **Step 4: Verify GREEN**

```sh
cargo test -p gascan
cargo test -p gascan-e2e --test fake_backend destroyed_tombstones_require_list_all -- --exact --nocapture
```

- [ ] **Step 5: Commit**

```sh
git add crates/gascan/src/cli.rs crates/gascan/src/presentation.rs crates/gascan-e2e/tests/fake_backend.rs
git commit -S -m "fix: hide destroyed sandboxes from ordinary lists"
```

---

### Task 5: Packaging, Smoke Isolation, and Documentation

**Files:**
- Modify: `packaging/macos/release-common.sh`
- Modify: `packaging/macos/uninstall.sh`
- Modify: `packaging/macos/release-smoke.sh`
- Modify: `scripts/tests/macos_release_smoke.rs`
- Modify: `README.md`
- Modify: `docs/release/macos-checklist.md`

**Interfaces:**
- Consumes: durable path contract and `gascan list --all --json`.
- Produces packaging-only `gascan_user_controller_root()` and isolated release-smoke state.

- [ ] **Step 1: Write failing packaging contracts**

Require release smoke to export `GASCAN_STATE_PATH` inside its owned fixture, assert ordinary human/JSON lists are empty after destroy, and use `list --all --json` only for the retained-record assertion. Require ordinary uninstall to preserve durable state and `--remove-data` to remove it only after verified destruction and daemon shutdown.

- [ ] **Step 2: Verify RED**

```sh
cargo test --manifest-path scripts/Cargo.toml --test macos_release_smoke -- --nocapture
./packaging/macos/release-script-contract.sh
```

- [ ] **Step 3: Implement packaging behavior**

Add:

```bash
gascan_user_controller_root() {
  printf '%s/Library/Application Support/dev.gascan/controller\n' "$HOME"
}
```

This helper is packaging-only; Rust remains authoritative for production path resolution. Add `GASCAN_STATE_PATH` to the release-smoke sanitized allowlist, point it inside the smoke fixture, and clean it in the trap. Normal uninstall preserves durable state. `--remove-data` destroys active records from normal JSON list, verifies remaining `--all` records are `absent`, stops the attested daemon, and removes only safely validated runtime and `dev.gascan/controller` roots.

- [ ] **Step 4: Document the contract**

Document the durable path, automatic legacy migration, conflict refusal, upgrade and ordinary-uninstall preservation, explicit data removal, and `list --all`. Update the release checklist to verify a managed-volume marker across daemon/package replacement and empty ordinary list output after final destroy.

- [ ] **Step 5: Verify GREEN**

```sh
cargo test --manifest-path scripts/Cargo.toml --test macos_release_smoke -- --nocapture
./packaging/macos/release-script-contract.sh
./packaging/macos/installer-contract.sh
git diff --check
```

- [ ] **Step 6: Commit**

```sh
git add packaging/macos/release-common.sh packaging/macos/uninstall.sh packaging/macos/release-smoke.sh scripts/tests/macos_release_smoke.rs README.md docs/release/macos-checklist.md
git commit -S -m "docs: define durable upgrade and cleanup behavior"
```

---

### Task 6: Full Verification and Independent Review

**Files:**
- Modify only through a new failing regression test if verification exposes a defect.

**Interfaces:**
- Consumes all prior tasks.
- Produces a clean branch ready for PR review.

- [ ] **Step 1: Format and check diff hygiene**

```sh
cargo fmt --all -- --check
git diff --check
```

- [ ] **Step 2: Run workspace tests and strict Clippy**

```sh
cargo test --workspace --all-targets --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
```

- [ ] **Step 3: Run scripts and macOS contracts**

```sh
cargo test --manifest-path scripts/Cargo.toml --all-targets --locked
./packaging/macos/release-script-contract.sh
./packaging/macos/installer-contract.sh
./packaging/macos/smoke-cleanup-contract.sh
```

- [ ] **Step 4: Run installed release smoke when sudo is available**

```sh
sudo -v && (
  GASCAN_RELEASE_GASCAN="$PWD/target/debug/gascan" \
  GASCAN_RELEASE_GASCAND="$PWD/target/debug/gascand" \
  GASCAN_RELEASE_APPLE_ATTACH_HELPER="$PWD/target/gascan-apple-attach" \
  ./packaging/macos/release-smoke.sh
)
```

Expected: the smoke reports `PASS`, preserves its managed-volume marker through daemon replacement, and exposes the final tombstone only through `list --all`.

- [ ] **Step 5: Inspect final branch state**

```sh
git status --short
git log --oneline --decorate origin/main..HEAD
git diff --stat origin/main...HEAD
```

- [ ] **Step 6: Request independent review**

The reviewer must explicitly verify that no migration path overwrites, unlinks, or ignores a conflict; WAL data survives; filesystem checks prevent symlink/ownership/mode attacks; list behavior is correct in both output modes; and package upgrade, uninstall, explicit removal, and smoke isolation use the intended durable-state semantics.

Resolve findings through the receiving-code-review workflow and rerun proportional verification before pushing or opening the PR.
