# Configurable Managed Storage Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace Gas Can's 100 MiB managed-volume ceiling with independently configurable storage capacities and preserve useful diagnostics from verbose provisioning commands.

**Architecture:** Parse a top-level storage policy into validated manifest values, carry exact byte capacities through backend-neutral runtime volume specifications, and persist the effective capacities as durable sandbox metadata. Treat capacity as immutable after volume creation, and rework guest command collection so bounded stdout is cancellable while stderr is continuously drained into a bounded diagnostic tail.

**Tech Stack:** Rust 1.95+, Tokio, Serde/TOML, rusqlite/SQLite migrations, tonic/protobuf error details, Apple Container CLI, mise

## Global Constraints

- Default `tools` capacity is exactly `10GiB`.
- Default `cache` capacity is exactly `10GiB`.
- Default `config` capacity is exactly `1GiB`.
- Each capacity must be positive and no greater than exactly `512GiB`.
- Accepted units remain exactly `KiB`, `MiB`, `GiB`, and `TiB`.
- Existing volumes are never resized, replaced, copied, or deleted automatically.
- A storage mismatch returns stable code `storage_change_requires_recreate`.
- Structured stdout remains bounded; verbose stderr is drained through process exit and retained only as a bounded tail.
- Existing user changes in `.superpowers/sdd/progress.md`, `.superpowers/sdd/task-2-report.md`, and `.superpowers/sdd/task-4-report.md` must not be staged or modified.

---

## File Structure

- `crates/gascan-core/src/manifest.rs` owns parsing, defaults, validation, and public accessors for `[storage]`.
- `crates/gascan-core/src/runtime.rs` carries immutable per-volume capacity in `RuntimeVolume`.
- `crates/gascan-core/src/policy.rs` maps user-facing storage fields onto the three managed runtime volumes.
- `crates/gascan-apple/src/backend.rs` translates runtime volume capacities to Apple Container `-s` arguments.
- `crates/gascand/migrations/003_storage_resolution.sql` adds durable storage-resolution columns without changing earlier migrations.
- `crates/gascand/src/store.rs` owns schema v3 migration, serialization, and validation for `StorageResolution`.
- `crates/gascand/src/service.rs` enforces immutable storage, drains guest output, and retains bounded diagnostics.
- `crates/gascan-proto/src/lib.rs` defines the stable storage-recreation error code.
- `crates/gascand/src/api.rs` maps the typed service failure to gRPC status plus a human cause.
- `README.md` documents storage defaults, overrides, and recreation behavior.

### Task 1: Parse and Validate Independent Storage Capacities

**Files:**
- Modify: `crates/gascan-core/src/manifest.rs`
- Modify: `crates/gascan-core/tests/manifest.rs`

**Interfaces:**
- Produces: `Storage`, `Manifest::storage() -> &Storage`, `Storage::{tools,cache,config}() -> ResourceSize`
- Produces: `DEFAULT_TOOLS_STORAGE_BYTES`, `DEFAULT_CACHE_STORAGE_BYTES`, `DEFAULT_CONFIG_STORAGE_BYTES`, and `MAX_MANAGED_VOLUME_BYTES`
- Consumes: existing `ResourceSize` parser and manifest `serde(deny_unknown_fields)` boundary

- [ ] **Step 1: Write failing default and override tests**

Add tests that load a minimal manifest and a partial storage table:

```rust
use gascan_core::manifest::{
    DEFAULT_CACHE_STORAGE_BYTES, DEFAULT_CONFIG_STORAGE_BYTES,
    DEFAULT_TOOLS_STORAGE_BYTES,
};

#[test]
fn storage_defaults_and_partial_overrides_are_independent() {
    let defaults = load("version = 1\n").unwrap();
    assert_eq!(
        defaults.storage().tools().bytes(),
        DEFAULT_TOOLS_STORAGE_BYTES
    );
    assert_eq!(
        defaults.storage().cache().bytes(),
        DEFAULT_CACHE_STORAGE_BYTES
    );
    assert_eq!(
        defaults.storage().config().bytes(),
        DEFAULT_CONFIG_STORAGE_BYTES
    );

    let partial = load("version = 1\n[storage]\ntools = '30GiB'\n").unwrap();
    assert_eq!(partial.storage().tools().bytes(), 30 * 1024_u64.pow(3));
    assert_eq!(
        partial.storage().cache().bytes(),
        DEFAULT_CACHE_STORAGE_BYTES
    );
    assert_eq!(
        partial.storage().config().bytes(),
        DEFAULT_CONFIG_STORAGE_BYTES
    );
}
```

- [ ] **Step 2: Run the focused test and confirm the missing API**

Run:

```bash
rtk cargo test -p gascan-core --test manifest storage_defaults_and_partial_overrides_are_independent
```

Expected: compilation fails because `Manifest::storage` and the storage constants do not exist.

- [ ] **Step 3: Implement the validated storage model**

Add these constants and types in `manifest.rs`:

```rust
pub const DEFAULT_TOOLS_STORAGE_BYTES: u64 = 10 * 1024_u64.pow(3);
pub const DEFAULT_CACHE_STORAGE_BYTES: u64 = 10 * 1024_u64.pow(3);
pub const DEFAULT_CONFIG_STORAGE_BYTES: u64 = 1024_u64.pow(3);
pub const MAX_MANAGED_VOLUME_BYTES: u64 = 512 * 1024_u64.pow(3);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct Storage {
    tools: ResourceSize,
    cache: ResourceSize,
    config: ResourceSize,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawStorage {
    tools: Option<ResourceSize>,
    cache: Option<ResourceSize>,
    config: Option<ResourceSize>,
}
```

Add `storage: Storage` to `Manifest`, `storage: RawStorage` with
`#[serde(default)]` to `RawManifest`, and:

```rust
impl Storage {
    const fn defaults() -> Self {
        Self {
            tools: ResourceSize(DEFAULT_TOOLS_STORAGE_BYTES),
            cache: ResourceSize(DEFAULT_CACHE_STORAGE_BYTES),
            config: ResourceSize(DEFAULT_CONFIG_STORAGE_BYTES),
        }
    }

    pub const fn tools(&self) -> ResourceSize { self.tools }
    pub const fn cache(&self) -> ResourceSize { self.cache }
    pub const fn config(&self) -> ResourceSize { self.config }
}

impl Manifest {
    pub const fn storage(&self) -> &Storage {
        &self.storage
    }
}
```

Resolve missing fields to defaults in `RawManifest::validate`, and reject any
effective value above `MAX_MANAGED_VOLUME_BYTES` with a field-specific
`ManifestError::Invalid` message.

- [ ] **Step 4: Add invalid-boundary tests**

Add a table-driven test proving rejection of:

```rust
[
    "version = 1\n[storage]\ntools = '0GiB'\n",
    "version = 1\n[storage]\ncache = '10GB'\n",
    "version = 1\n[storage]\nconfig = '513GiB'\n",
    "version = 1\n[storage]\nunknown = '1GiB'\n",
]
```

Also prove `512GiB` is accepted for each field.

- [ ] **Step 5: Run manifest tests**

Run:

```bash
rtk cargo test -p gascan-core --test manifest
```

Expected: all manifest tests pass.

- [ ] **Step 6: Commit the manifest contract**

```bash
rtk git add crates/gascan-core/src/manifest.rs crates/gascan-core/tests/manifest.rs
rtk git commit -m "feat: add configurable managed storage"
```

### Task 2: Carry Capacities Through Policy and Apple Volume Creation

**Files:**
- Modify: `crates/gascan-core/src/runtime.rs`
- Modify: `crates/gascan-core/src/policy.rs`
- Modify: `crates/gascan-core/tests/policy.rs`
- Modify: `crates/gascan-core/src/fake_runtime.rs`
- Modify: `crates/gascan-apple/src/backend.rs`
- Modify: `crates/gascan-apple/tests/backend_fake_runner.rs`

**Interfaces:**
- Consumes: `Manifest::storage()` from Task 1
- Produces: `RuntimeVolume { name, target, writable, capacity_bytes, ownership }`
- Produces: Apple command `container volume create ... -s <capacity_bytes> <name>`

- [ ] **Step 1: Write a failing policy mapping test**

Extend the policy fixture manifest with:

```toml
[storage]
tools = "11GiB"
cache = "12GiB"
config = "2GiB"
```

Assert the three volumes by mount target:

```rust
let capacities = request
    .volumes()
    .iter()
    .map(|volume| (volume.target.as_str(), volume.capacity_bytes))
    .collect::<BTreeMap<_, _>>();
assert_eq!(
    capacities["/home/workspace/.local/share/mise"],
    11 * 1024_u64.pow(3)
);
assert_eq!(
    capacities["/home/workspace/.cache"],
    12 * 1024_u64.pow(3)
);
assert_eq!(
    capacities["/home/workspace/.config/gascan"],
    2 * 1024_u64.pow(3)
);
```

- [ ] **Step 2: Run the policy test and confirm it fails**

Run:

```bash
rtk cargo test -p gascan-core --test policy
```

Expected: compilation fails because `RuntimeVolume::capacity_bytes` does not
exist.

- [ ] **Step 3: Add capacity to the runtime contract**

Change `RuntimeVolume` to:

```rust
pub struct RuntimeVolume {
    pub name: String,
    pub target: Utf8PathBuf,
    pub writable: bool,
    pub capacity_bytes: u64,
    pub ownership: OwnershipMetadata,
}
```

Change `managed_volumes` in `policy.rs` to accept `&Storage` and assign the
matching `tools`, `cache`, and `config` byte values. Update all
`RuntimeVolume` fixtures, including the fake runtime, with explicit capacities.

- [ ] **Step 4: Run core policy and backend-contract tests**

Run:

```bash
rtk cargo test -p gascan-core --test policy --test backend_contract
```

Expected: both test binaries pass.

- [ ] **Step 5: Write a failing Apple command test**

Update the stateful fake runner to accept a variable size:

```rust
[
    "volume", "create", "--label", manager, "--label", sandbox,
    "-s", size, name,
] => {
    state.volume_sizes.insert((*name).into(), size.parse::<u64>().unwrap());
    // retain the existing ownership and failure behavior
}
```

Add `volume_sizes: BTreeMap<String, u64>` to `State`, create a request with
independent capacities, and assert the recorded values are exactly
`11GiB`, `12GiB`, and `2GiB`.

- [ ] **Step 6: Run the Apple backend test and confirm failure**

Run:

```bash
rtk cargo test -p gascan-apple --test backend_fake_runner
```

Expected: the independent-size assertion fails because the backend still sends
the hardcoded value.

- [ ] **Step 7: Remove the backend hardcode**

Delete `MANAGED_VOLUME_SIZE_BYTES`. Build each create command with:

```rust
let capacity = volume.capacity_bytes.to_string();
let spec = CommandSpec::new(
    "container",
    [
        "volume",
        "create",
        "--label",
        &manager,
        "--label",
        &sandbox,
        "-s",
        &capacity,
        &volume.name,
    ],
);
```

- [ ] **Step 8: Run core and Apple tests**

Run:

```bash
rtk cargo test -p gascan-core
rtk cargo test -p gascan-apple
```

Expected: both crates pass all tests.

- [ ] **Step 9: Commit capacity propagation**

```bash
rtk git add crates/gascan-core/src/runtime.rs crates/gascan-core/src/policy.rs crates/gascan-core/tests/policy.rs crates/gascan-core/src/fake_runtime.rs crates/gascan-apple/src/backend.rs crates/gascan-apple/tests/backend_fake_runner.rs
rtk git commit -m "feat: size managed volumes from policy"
```

### Task 3: Persist Effective Storage in Schema Version 3

**Files:**
- Create: `crates/gascand/migrations/003_storage_resolution.sql`
- Modify: `crates/gascand/src/store.rs`
- Modify: `crates/gascand/src/lib.rs`
- Modify: `crates/gascand/tests/store.rs`
- Modify: all `SandboxRecord` fixtures reported by the compiler under `crates/gascand/tests/` and `crates/gascan-e2e/tests/`

**Interfaces:**
- Produces: `StorageResolution { version: u32, details: Value }`
- Produces: `SandboxRecord::storage_resolution: Option<StorageResolution>`
- Consumes: exact effective byte capacities produced by Tasks 1 and 2

- [ ] **Step 1: Write failing store round-trip and migration tests**

Extend the store fixture:

```rust
storage_resolution: Some(StorageResolution::new(
    1,
    json!({
        "tools_bytes": 10 * 1024_u64.pow(3),
        "cache_bytes": 10 * 1024_u64.pow(3),
        "config_bytes": 1024_u64.pow(3),
    }),
)),
```

Assert it round-trips. Add a v2 database migration test that opens a schema
containing migrations 001 and 002, verifies migration to version 3, and
asserts the legacy row has `storage_resolution == None`.

- [ ] **Step 2: Run the focused store tests and confirm failure**

Run:

```bash
rtk cargo test -p gascand --test store
```

Expected: compilation fails because `StorageResolution` and the record field do
not exist.

- [ ] **Step 3: Add migration 003**

Create:

```sql
ALTER TABLE sandboxes ADD COLUMN storage_resolution_version INTEGER;
ALTER TABLE sandboxes ADD COLUMN storage_resolution_details TEXT;
UPDATE schema_version SET version = 3 WHERE singleton = 1;
```

Set `SCHEMA_VERSION` to `3`, include `STORAGE_RESOLUTION_MIGRATION`, apply it
after migration 002 for new databases and when opening version 2, and add
`validate_v3_schema`.

- [ ] **Step 4: Add the durable resolution type and SQL mapping**

Declare:

```rust
resolution_record!(StorageResolution);
```

Add `storage_resolution` to `SandboxRecord`, `RawSandbox`, `SANDBOX_SELECT`,
row decoding, validation, insert/update SQL, and every record fixture. Export
the type from `crates/gascand/src/lib.rs`.

- [ ] **Step 5: Run store and service tests**

Run:

```bash
rtk cargo test -p gascand --test store
rtk cargo test -p gascand --tests
```

Expected: all gascand tests pass with explicit `storage_resolution` fixture
values.

- [ ] **Step 6: Commit durable storage metadata**

```bash
rtk git add crates/gascand/migrations/003_storage_resolution.sql crates/gascand/src/store.rs crates/gascand/src/lib.rs crates/gascand/tests crates/gascan-e2e/tests
rtk git commit -m "feat: persist sandbox storage capacities"
```

### Task 4: Reject Immutable Storage Changes Without Runtime Mutation

**Files:**
- Modify: `crates/gascand/src/service.rs`
- Modify: `crates/gascand/src/api.rs`
- Modify: `crates/gascand/tests/lifecycle.rs`
- Modify: `crates/gascand/tests/apply_tools.rs`
- Modify: `crates/gascan-proto/src/lib.rs`
- Modify: `crates/gascan-proto/tests/api_compatibility.rs`
- Modify: `crates/gascan/src/client.rs`
- Modify: `crates/gascan/src/cli.rs`

**Interfaces:**
- Consumes: `StorageResolution` from Task 3 and effective capacities from the compiled `CreateRequest`
- Produces: `ServiceError::StorageChangeRequiresRecreate { changes: Vec<StorageCapacityChange> }`
- Produces: stable public code `storage_change_requires_recreate`

- [ ] **Step 1: Write failing lifecycle tests**

Create a sandbox with defaults, then reload its manifest with:

```toml
[storage]
tools = "20GiB"
```

Assert:

```rust
let before = runtime.calls().await.len();
let error = service.apply(UpRequest::new(changed_spec)).await.unwrap_err();
assert_eq!(error.code(), "storage_change_requires_recreate");
assert!(error.to_string().contains("tools"));
assert!(error.to_string().contains("10GiB"));
assert!(error.to_string().contains("20GiB"));
assert_eq!(runtime.calls().await.len(), before);
```

Add a legacy-record test with a present runtime sandbox and
`storage_resolution: None`; it must return the same code and make no runtime
mutation.

- [ ] **Step 2: Run lifecycle tests and confirm failure**

Run:

```bash
rtk cargo test -p gascand --test lifecycle
```

Expected: the changed manifest reaches normal apply behavior instead of the
typed recreation error.

- [ ] **Step 3: Implement normalized capacity comparison**

Add:

```rust
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StorageCapacityChange {
    pub volume: &'static str,
    pub recorded_bytes: Option<u64>,
    pub requested_bytes: u64,
}
```

Encode a successful new sandbox as storage resolution version 1:

```rust
json!({
    "tools_bytes": requested.tools,
    "cache_bytes": requested.cache,
    "config_bytes": requested.config,
})
```

Before any start, provision, or other runtime mutation on an existing runtime
sandbox, compare that normalized object to the current compiled volumes. A
missing or malformed stored resolution is a mismatch; do not infer capacity
from volume names or host filesystem paths.

Add a deterministic formatter used only for diagnostics:

```rust
fn format_binary_size(bytes: u64) -> String {
    for (suffix, divisor) in [
        ("TiB", 1024_u64.pow(4)),
        ("GiB", 1024_u64.pow(3)),
        ("MiB", 1024_u64.pow(2)),
        ("KiB", 1024_u64),
    ] {
        if bytes % divisor == 0 {
            return format!("{}{suffix}", bytes / divisor);
        }
    }
    format!("{bytes} bytes")
}
```

- [ ] **Step 4: Add the stable API code and human cause**

In `gascan-proto/src/lib.rs` add:

```rust
pub const STORAGE_CHANGE_REQUIRES_RECREATE: &str =
    "storage_change_requires_recreate";
```

Include it in `error_code::ALL`. Map the service error to
`tonic::Code::FailedPrecondition` with error details containing the human
message. Update the client/CLI tests to prove human output includes:

```text
storage settings changed for tools (10GiB → 20GiB); run `gascan destroy --yes` and `gascan up` to recreate the sandbox
```

Add a `failure_details` branch containing a `changes` array with `volume`,
`recorded_bytes`, and `requested_bytes`. JSON mode must retain the stable code
separately from the message.

- [ ] **Step 5: Run lifecycle, API, protocol, and CLI tests**

Run:

```bash
rtk cargo test -p gascand --test lifecycle
rtk cargo test -p gascand --lib
rtk cargo test -p gascan-proto
rtk cargo test -p gascan
```

Expected: all tests pass and mismatch tests record zero runtime mutations.

- [ ] **Step 6: Commit immutable-storage handling**

```bash
rtk git add crates/gascand/src/service.rs crates/gascand/src/api.rs crates/gascand/tests/lifecycle.rs crates/gascand/tests/apply_tools.rs crates/gascan-proto/src/lib.rs crates/gascan-proto/tests/api_compatibility.rs crates/gascan/src/client.rs crates/gascan/src/cli.rs
rtk git commit -m "feat: require recreation for storage changes"
```

### Task 5: Drain Verbose Stderr and Surface Terminal Provisioning Errors

**Files:**
- Modify: `crates/gascand/src/service.rs`
- Modify: `crates/gascand/tests/apply_tools.rs`
- Modify: `crates/gascan-core/src/fake_runtime.rs`
- Modify: `crates/gascan-core/tests/backend_contract.rs`

**Interfaces:**
- Produces: private `GuestExecOutcome { stdout: Vec<u8>, stderr_tail: Vec<u8>, code: i32, signal: i32 }`
- Produces: private `BoundedTail` retaining exactly the last `MAX_PROVISION_STDERR_TAIL_BYTES`
- Consumes: `ExecSession::cancel()` for oversized structured stdout

- [ ] **Step 1: Write a failing verbose-stderr test**

Queue a successful install result with empty stdout and stderr larger than
1 MiB, followed by the normal mise inventory result:

```rust
runtime
    .queue_exec_results([
        (Vec::new(), vec![b'x'; 2 * 1024 * 1024], 0),
        (
            br#"{"node":[{"version":"24.18.0","installed":true,"active":true}]}"#
                .to_vec(),
            Vec::new(),
            0,
        ),
        (br#"{"source":"bundled"}"#.to_vec(), Vec::new(), 0),
    ])
    .await;
```

Assert apply succeeds. Add a failed command whose stderr begins with 2 MiB of
noise and ends with `ENOSPC: no space left on device`; assert the error and
durable failure details retain the terminal phrase.

- [ ] **Step 2: Run the focused tests and confirm the current limit failure**

Run:

```bash
rtk cargo test -p gascand --test apply_tools verbose_stderr
rtk cargo test -p gascand --test apply_tools terminal_stderr
```

Expected: failure with `guest provisioning output exceeded its limit`, and the
terminal `ENOSPC` text is absent.

- [ ] **Step 3: Implement a fixed-size tail buffer**

Use constants with distinct purposes:

```rust
const MAX_PROVISION_STDOUT_BYTES: usize = 1024 * 1024;
const MAX_PROVISION_STDERR_TAIL_BYTES: usize = 64 * 1024;
```

Add a `BoundedTail` that:

```rust
fn extend(&mut self, bytes: &[u8]) {
    if bytes.len() >= self.limit {
        self.bytes.clear();
        self.bytes
            .extend_from_slice(&bytes[bytes.len() - self.limit..]);
        return;
    }
    let overflow = self.bytes.len().saturating_add(bytes.len()).saturating_sub(self.limit);
    if overflow > 0 {
        self.bytes.drain(..overflow);
    }
    self.bytes.extend_from_slice(bytes);
}
```

The implementation must avoid panicking on zero, chunk boundaries, or
non-UTF-8 input. Convert the final tail with `String::from_utf8_lossy` and the
existing diagnostic sanitization boundary.

- [ ] **Step 4: Return a complete guest outcome**

Change `exec_guest_raw` to return:

```rust
struct GuestExecOutcome {
    stdout: Vec<u8>,
    stderr_tail: Vec<u8>,
    code: i32,
    signal: i32,
}
```

For every stderr frame, extend the tail and continue reading. On
`ExecOutput::Exit`, return the outcome. Add `stderr_tail: String` to
`ServiceError::ProvisionCommandFailed` and its durable `failure_details`.

- [ ] **Step 5: Cancel oversized structured stdout explicitly**

When stdout would exceed `MAX_PROVISION_STDOUT_BYTES`:

```rust
session.cancel();
while session.next().await.is_some() {}
return Err(ServiceError::Provision(
    "guest provisioning stdout exceeded its limit".to_owned(),
));
```

Extend the fake runtime with cancellation evidence and assert the oversized
stdout test observes cancellation. Keep stderr-only tests uncancelled and
drained through their exit frame.

- [ ] **Step 6: Run provisioning and runtime contract tests**

Run:

```bash
rtk cargo test -p gascand --test apply_tools
rtk cargo test -p gascan-core --test backend_contract
```

Expected: verbose stderr succeeds, terminal `ENOSPC` is surfaced, oversized
stdout cancels, and all existing provisioning tests pass.

- [ ] **Step 7: Commit bounded provisioning diagnostics**

```bash
rtk git add crates/gascand/src/service.rs crates/gascand/tests/apply_tools.rs crates/gascan-core/src/fake_runtime.rs crates/gascan-core/tests/backend_contract.rs
rtk git commit -m "fix: preserve provisioning failure diagnostics"
```

### Task 6: Document, Verify, and Exercise Live Apple Storage

**Files:**
- Modify: `README.md`
- Modify: `crates/gascan-apple/tests/live/storage.rs`
- Modify: `crates/gascan-e2e/tests/apple_common/mod.rs`
- Modify: `crates/gascan-e2e/tests/apple_apply.rs`

**Interfaces:**
- Consumes: all public behavior from Tasks 1–5
- Produces: user documentation and live evidence for independently sized Apple volumes

- [ ] **Step 1: Update the README schema and storage reference**

Add the approved example:

```toml
[storage]
tools = "10GiB"
cache = "10GiB"
config = "1GiB"
```

Document defaults, mount mappings, supported units, the `512GiB` maximum, and
the explicit destroy/up recreation workflow. Remove any implication that
`[resources].disk` controls managed-volume capacity; it remains rejected
because Apple cannot enforce a container root-filesystem ceiling.

- [ ] **Step 2: Add live volume-capacity assertions**

Create a live request using `11GiB`, `12GiB`, and `2GiB`. Inspect or mount each
created Apple volume through the existing live harness and assert its guest
filesystem capacity is greater than or equal to the requested value and less
than or equal to the requested value plus `64MiB`, accounting for Apple
Container filesystem rounding. Also assert ownership labels, mount targets,
and cleanup remain exact.

- [ ] **Step 3: Add a live representative tool-install test**

Use a networked sandbox with a storage override and install one representative
large npm-backed tool plus Neovim. Assert:

```text
mise ls --current --installed --json
```

contains exactly the requested active tools, and prove `gascan apply` completes
without an output-limit failure. Do not require external credentials or launch
an interactive coding-agent session.

- [ ] **Step 4: Run formatting and static verification**

Run:

```bash
rtk cargo fmt --all -- --check
rtk cargo clippy --workspace --all-targets --all-features -- -D warnings
rtk git diff --check
```

Expected: all commands exit successfully with no warnings or whitespace errors.

- [ ] **Step 5: Run complete automated suites**

Run:

```bash
rtk env -u RUSTUP_TOOLCHAIN cargo test --workspace
rtk cargo test --manifest-path scripts/Cargo.toml
```

Expected: every workspace and scripts test passes.

- [ ] **Step 6: Run the repository's Apple preflight and focused live tests**

Run:

```bash
rtk bash ./scripts/apple-test-preflight.sh
rtk cargo test -p gascan-apple --test live storage -- --ignored --nocapture
rtk cargo test -p gascan-e2e --test apple_apply -- --ignored --nocapture
```

Expected: preflight passes; live tests create independently sized volumes,
install the representative tools, and remove all test-owned resources.

- [ ] **Step 7: Commit documentation and live coverage**

```bash
rtk git add README.md crates/gascan-apple/tests/live/storage.rs crates/gascan-e2e/tests/apple_common/mod.rs crates/gascan-e2e/tests/apple_apply.rs
rtk git commit -m "docs: describe managed storage policy"
```

### Task 7: Final Review and Release Readiness

**Files:**
- Review: all files changed by Tasks 1–6

**Interfaces:**
- Consumes: the completed implementation
- Produces: a review-ready feature branch; version bump and release remain separate unless explicitly requested

- [ ] **Step 1: Review the branch diff against the approved spec**

Run:

```bash
rtk git diff main...HEAD --stat
rtk git diff main...HEAD
```

Expected: every change maps to the approved storage or provisioning-diagnostic
scope; the three pre-existing `.superpowers/sdd` files are absent from the
branch diff.

- [ ] **Step 2: Re-run clean verification**

Run:

```bash
rtk cargo fmt --all -- --check
rtk env -u RUSTUP_TOOLCHAIN cargo test --workspace
rtk cargo test --manifest-path scripts/Cargo.toml
rtk git diff --check
rtk git status --short
```

Expected: all verification passes. Status shows only the three known,
unstaged `.superpowers/sdd` user files.

- [ ] **Step 3: Request code review**

Invoke `superpowers:requesting-code-review` and require separate specification
and code-quality reviews. Resolve findings through
`superpowers:receiving-code-review`, rerun the affected tests, and commit each
accepted correction separately.

- [ ] **Step 4: Complete the branch**

Invoke `superpowers:verification-before-completion`, then
`superpowers:finishing-a-development-branch`. Present the verified options for
push/PR, local integration, or retention without performing a merge or release
unless the user explicitly selects it.
