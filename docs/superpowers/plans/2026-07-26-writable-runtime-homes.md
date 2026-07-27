# Writable Runtime Homes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give every bundled development tool conventional writable, persistent user storage; reject unsafe reuse of the previous mount layout; and ship the fix as Gas Can 0.1.10.

**Architecture:** Expand the three existing named-volume boundaries to `~/.local`, `~/.cache`, and `~/.config`, then centralize every runtime home and `PATH` entry in `gascan-core` policy so container creation and provisioning agree. Seed the immutable image's Rust toolchain into the writable tools volume with an idempotent guest script, version the persisted storage resolution to prevent old-volume mis-mounts, and prove the result through unit, service, image-contract, live Apple, package, and installed-release tests.

**Tech Stack:** Rust 1.95 workspace, Tokio, tonic/protobuf, SQLite, Bash/POSIX shell, Dockerfile-compatible Apple `container` image builds, mise, rustup/Cargo, npm, Go, Python/pip, RubyGems, Mix/Hex/rebar, GitHub CLI, and the repository macOS release driver.

## Global Constraints

- Bundled programs remain immutable fallbacks below `/opt/gascan`.
- Managed storage layout version 2 mounts `tools` at `/home/workspace/.local`, `cache` at `/home/workspace/.cache`, and `config` at `/home/workspace/.config`.
- Existing pre-version-2 sandboxes must fail with explicit `gascan destroy --yes` then `gascan up` guidance; Gas Can must never silently reuse their contents at the new targets.
- `tools`, `cache`, and `config` retain independent capacities from `gascan.toml`; no whole-home volume is introduced.
- Normal user-write destinations must resolve below `/home/workspace/.local`, `/home/workspace/.cache`, or `/home/workspace/.config`, never below `/opt/gascan`.
- User executable directories precede immutable defaults on `PATH`.
- The bundled Rust seed is copied locally into the tools volume without network access, without root ownership, and without overwriting existing user toolchains or settings.
- Existing user content in a valid version-2 volume is never recursively chowned or deleted.
- No host credentials, SSH agent, Docker socket, arbitrary host path, or host home is forwarded.
- The release target is exactly `0.1.10`.
- Every shell command in this repository is invoked through `rtk`.
- The user's dirty primary checkout is never modified, stashed, reset, or cleaned.

---

## File Structure

### Runtime policy and storage

- `crates/gascan-core/src/policy.rs`: owns canonical managed roots, language-tool homes, exact container `PATH`, guest environment, and volume targets.
- `crates/gascan-core/tests/policy.rs`: proves exact targets, capacities, environment values, containment, and ordering.
- `crates/gascand/src/service.rs`: validates layout version 2, initializes mounted roots, invokes the Rust bootstrap, and supplies the same environment to mise provisioning.
- `crates/gascand/src/api.rs`: exposes layout incompatibility as a failed precondition and a status apply requirement.
- `crates/gascand/tests/lifecycle.rs`: proves old layouts fail before runtime calls and remain destroyable.
- `crates/gascand/tests/apply_tools.rs`: proves exact guest initialization and provisioning argv.

### Progress protocol and presentation

- `proto/gascan/v1/gascan.proto`: adds the stable `INITIALIZE_RUNTIME_HOME` provisioning step at numeric value 6.
- `crates/gascan-core/src/provision.rs`: names the new internal provisioning step.
- `crates/gascand/src/api.rs`: maps stored step text to the protobuf enum.
- `crates/gascan/src/presentation.rs`: renders “Preparing writable tool storage” and old-layout recreation guidance.
- Existing protocol, provision-plan, API, and presentation tests: prove additive enum compatibility and human/JSON behavior.

### Workspace image

- `images/workspace/Dockerfile`: keeps `/opt/gascan` homes only during image construction, sets final runtime homes, creates broad home roots, installs the bootstrap script, and declares the version-2 volumes.
- `images/workspace/bin/configure-workstation-home`: creates only marked application directories below the broad managed roots.
- `images/workspace/bin/initialize-rust-home`: new focused, fail-closed, idempotent Rust seed copier.
- `images/workspace/etc/profile.d/mise.sh`: preserves the exact policy `PATH` and runtime-home defaults for interactive/SSH shells.
- `images/workspace/tests/workstation-contract.sh`: audits writable destinations, mount containment, ownership, and `PATH`.
- `tests/image/workstation-smoke.sh`: executes local package-manager write/install smoke cases in the live image.
- `scripts/tests/connected_dockerfile.rs`, `scripts/tests/image_user_contract.rs`, and `scripts/tests/connected_image_gate.rs`: enforce source-level image and gate contracts.

### Documentation, release, and regression cleanup

- `README.md`: documents storage roots, migration, Rust seed cost, and common package-manager workflows.
- `docs/release/macos-checklist.md`: records the version-2 recreation requirement and installed-release checks.
- `packaging/macos/release-smoke.sh`: proves representative writable homes in the installed package.
- `crates/gascan/src/cli.rs`: corrects the stale SSH test that expects accepted mode `0755` to fail.
- Six `crates/*/Cargo.toml`, root `Cargo.lock`, `README.md`, and `docs/release/macos-checklist.md`: patch-version bump to 0.1.10 on the release branch.

---

### Task 1: Centralize Version-2 Volume Roots and Runtime Homes

**Files:**
- Modify: `crates/gascan-core/src/policy.rs`
- Modify: `crates/gascan-core/tests/policy.rs`
- Modify: `crates/gascan-apple/tests/attach_configuration.rs`
- Modify: `crates/gascan-apple/tests/fixtures/translate-create-offline.json`

**Interfaces:**
- Produces constants `TOOLS_ROOT`, `CACHE_ROOT`, `CONFIG_ROOT`, `CARGO_HOME`, `RUSTUP_HOME`, `NPM_CACHE_DIR`, `GO_PATH`, and `CONTAINER_PATH`.
- Produces `guest_environment(ssh_enabled, control)` entries consumed by container creation and Task 3 provisioning.
- Produces version-2 `RuntimeVolume` targets consumed by Task 2 storage validation and Task 4 image contracts.

- [ ] **Step 1: Add failing policy assertions for exact version-2 targets**

Update the existing volume test to require:

```rust
assert_eq!(
    request
        .volumes()
        .iter()
        .map(|volume| volume.target.as_str())
        .collect::<Vec<_>>(),
    [
        "/home/workspace/.local",
        "/home/workspace/.cache",
        "/home/workspace/.config",
    ]
);
```

Retain the existing exact independent capacities and ownership assertions.

- [ ] **Step 2: Add failing environment and `PATH` assertions**

Assert these exact mappings:

```rust
let expected = [
    ("XDG_DATA_HOME", "/home/workspace/.local/share"),
    ("XDG_CACHE_HOME", "/home/workspace/.cache"),
    ("XDG_CONFIG_HOME", "/home/workspace/.config"),
    ("CARGO_HOME", "/home/workspace/.local/share/cargo"),
    ("MISE_CARGO_HOME", "/home/workspace/.local/share/cargo"),
    ("RUSTUP_HOME", "/home/workspace/.local/share/rustup"),
    ("MISE_RUSTUP_HOME", "/home/workspace/.local/share/rustup"),
    ("NPM_CONFIG_PREFIX", "/home/workspace/.local"),
    ("NPM_CONFIG_CACHE", "/home/workspace/.cache/npm"),
    ("GOPATH", "/home/workspace/.local/share/go"),
    ("GOCACHE", "/home/workspace/.cache/go-build"),
    ("GOMODCACHE", "/home/workspace/.cache/go-mod"),
    ("PYTHONUSERBASE", "/home/workspace/.local"),
    ("GEM_HOME", "/home/workspace/.local/share/gem"),
    ("MIX_HOME", "/home/workspace/.local/share/mix"),
    ("HEX_HOME", "/home/workspace/.local/share/hex"),
    ("REBAR_CACHE_DIR", "/home/workspace/.cache/rebar3"),
];
for (name, value) in expected {
    assert_eq!(request.environment().get(name).map(String::as_str), Some(value));
}
assert_eq!(
    request.environment().get("PATH").map(String::as_str),
    Some(concat!(
        "/home/workspace/.local/bin:",
        "/home/workspace/.local/share/cargo/bin:",
        "/home/workspace/.local/share/go/bin:",
        "/home/workspace/.local/share/gem/bin:",
        "/home/workspace/.local/share/mise/shims:",
        "/opt/gascan/mise/shims:",
        "/usr/local/sbin:/usr/local/bin:",
        "/opt/gascan/workstation/bin:",
        "/usr/sbin:/usr/bin:/sbin:/bin"
    ))
);
```

- [ ] **Step 3: Run the policy and Apple translation tests to verify failure**

Run:

```bash
rtk cargo test -p gascan-core --test policy
rtk cargo test -p gascan-apple --test attach_configuration
```

Expected: failures show old leaf volume targets, absent tool-home variables, and the old `PATH`.

- [ ] **Step 4: Implement canonical constants and environment entries**

In `policy.rs`, define exact public constants and build `CONTAINER_PATH` from one literal:

```rust
pub const TOOLS_ROOT: &str = "/home/workspace/.local";
pub const CACHE_ROOT: &str = "/home/workspace/.cache";
pub const CONFIG_ROOT: &str = "/home/workspace/.config";
pub const CARGO_HOME: &str = "/home/workspace/.local/share/cargo";
pub const RUSTUP_HOME: &str = "/home/workspace/.local/share/rustup";
pub const GO_PATH: &str = "/home/workspace/.local/share/go";
pub const CONTAINER_PATH: &str = concat!(
    "/home/workspace/.local/bin:",
    "/home/workspace/.local/share/cargo/bin:",
    "/home/workspace/.local/share/go/bin:",
    "/home/workspace/.local/share/gem/bin:",
    "/home/workspace/.local/share/mise/shims:",
    "/opt/gascan/mise/shims:",
    "/usr/local/sbin:/usr/local/bin:",
    "/opt/gascan/workstation/bin:",
    "/usr/sbin:/usr/bin:/sbin:/bin"
);
```

Add every environment variable from Step 2 to `guest_environment`. Change only the three volume targets; retain volume names, capacities, labels, and ownership.

- [ ] **Step 5: Update Apple request fixtures without weakening exact comparison**

Regenerate the expected environment and volume target entries in
`translate-create-offline.json`; do not normalize or subset the fixture.

- [ ] **Step 6: Run focused tests**

Run:

```bash
rtk cargo test -p gascan-core --test policy
rtk cargo test -p gascan-apple --test attach_configuration
rtk cargo test -p gascan-apple
```

Expected: all pass.

- [ ] **Step 7: Commit**

```bash
rtk git add crates/gascan-core/src/policy.rs crates/gascan-core/tests/policy.rs crates/gascan-apple/tests/attach_configuration.rs crates/gascan-apple/tests/fixtures/translate-create-offline.json
rtk git commit -m "fix: route runtime homes into managed storage"
```

---

### Task 2: Reject Legacy Volume Layouts Before Runtime Mutation

**Files:**
- Modify: `crates/gascand/src/service.rs`
- Modify: `crates/gascand/src/api.rs`
- Modify: `crates/gascan-proto/src/lib.rs`
- Modify: `crates/gascand/tests/lifecycle.rs`
- Modify: inline API tests in `crates/gascand/src/api.rs`
- Modify: `crates/gascan/src/presentation.rs`
- Modify: presentation tests in `crates/gascan/src/presentation.rs`

**Interfaces:**
- Consumes Task 1's exact `TOOLS_ROOT`, `CACHE_ROOT`, and `CONFIG_ROOT`.
- Produces `const STORAGE_LAYOUT_VERSION: u32 = 2`.
- Produces `ServiceError::StorageLayoutRequiresRecreate { recorded: Option<u32> }`.
- Produces status requirement reason `storage_layout_changed`, consumed by human and JSON CLI rendering.

- [ ] **Step 1: Write failing lifecycle tests for version 1 and absent versions**

For each legacy record, capture runtime call count, call `up` and `apply`, and assert:

```rust
assert_eq!(error.code(), "storage_layout_requires_recreate");
assert_eq!(
    error.to_string(),
    "managed storage layout changed; run `gascan destroy --yes` and then `gascan up`"
);
assert_eq!(runtime.calls().await.len(), before);
```

Also call `destroy` and assert all three existing named volumes are still removed.

- [ ] **Step 2: Write failing status/API tests**

Build a record with `StorageResolution::new(1, ...)` and assert:

```rust
assert_eq!(
    status.apply_requirements,
    [v1::ApplyRequirement {
        reason: "storage_layout_changed".to_owned(),
        current: "1".to_owned(),
        requested: "2".to_owned(),
    }]
);
```

For an absent resolution, use `"unknown"` as `current`. If the image also
changed, assert both requirements are present in deterministic
`storage_layout_changed`, then `image_changed` order.

- [ ] **Step 3: Run focused tests to verify failure**

Run:

```bash
rtk cargo test -p gascand apply_rejects_legacy_storage_resolution
rtk cargo test -p gascand storage_layout
rtk cargo test -p gascan storage_layout
```

Expected: tests fail because storage version 1 is still accepted as the only valid capacity record and status reports only image changes.

- [ ] **Step 4: Store layout version 2 and separate layout from capacity errors**

Implement:

```rust
const STORAGE_LAYOUT_VERSION: u32 = 2;

fn storage_resolution(requested: StorageCapacities) -> StorageResolution {
    StorageResolution::new(
        STORAGE_LAYOUT_VERSION,
        json!({
            "tools_bytes": requested[0].1,
            "cache_bytes": requested[1].1,
            "config_bytes": requested[2].1,
        }),
    )
}
```

At the beginning of storage validation, return
`StorageLayoutRequiresRecreate` unless the resolution version is exactly 2.
Only then compare capacities and retain the existing
`StorageChangeRequiresRecreate` detail structure.

Change `requested_storage_from_volumes` to match Task 1's broad roots.

- [ ] **Step 5: Map the new error through API and CLI status**

Add `STORAGE_LAYOUT_REQUIRES_RECREATE` to the protocol error-code constants.
Return tonic `FailedPrecondition` with stable code
`storage_layout_requires_recreate` and structured details:

```json
{
  "reason": "storage_layout_changed",
  "recorded_layout": 1,
  "requested_layout": 2,
  "recovery": "run `gascan destroy --yes` and then `gascan up`"
}
```

Teach `wire_status` to append the version requirement, and teach human status
rendering to print:

```text
Recreation required
  Managed storage layout  1 → 2
  Run gascan destroy --yes, then gascan up
```

- [ ] **Step 6: Run focused and store compatibility tests**

Run:

```bash
rtk cargo test -p gascand lifecycle
rtk cargo test -p gascand store
rtk cargo test -p gascand api
rtk cargo test -p gascan presentation
```

Expected: all pass; no SQLite migration is added because the existing
resolution version column carries layout version 2.

- [ ] **Step 7: Commit**

```bash
rtk git add crates/gascand/src/service.rs crates/gascand/src/api.rs crates/gascand/tests/lifecycle.rs crates/gascan-proto/src/lib.rs crates/gascan/src/presentation.rs
rtk git commit -m "fix: require recreation for storage layout v2"
```

Include any inline API/presentation test files modified by Step 5 in the same
commit.

---

### Task 3: Add Idempotent Writable Rust Bootstrap and Progress

**Files:**
- Create: `images/workspace/bin/initialize-rust-home`
- Modify: `images/workspace/Dockerfile`
- Modify: `proto/gascan/v1/gascan.proto`
- Modify: `crates/gascan-core/src/provision.rs`
- Modify: `crates/gascand/src/service.rs`
- Modify: `crates/gascand/src/api.rs`
- Modify: `crates/gascan/src/presentation.rs`
- Modify: `scripts/tests/image_user_contract.rs`
- Modify: `crates/gascand/tests/apply_tools.rs`
- Modify: existing protocol/provision/API/presentation tests

**Interfaces:**
- Consumes Task 1's `CARGO_HOME`, `RUSTUP_HOME`, and exact runtime environment.
- Produces executable `/usr/local/bin/initialize-rust-home`.
- Produces protobuf `PROVISION_STEP_INITIALIZE_RUNTIME_HOME = 6` and Rust enum `ProvisionStep::InitializeRuntimeHome`.
- Produces guest action name `initialize_rust_home`.

- [ ] **Step 1: Write failing script contract tests**

Create fixtures with a source Rust home containing:

```text
toolchains/1.97.0-aarch64-unknown-linux-gnu/bin/cargo
update-hashes/1.97.0-aarch64-unknown-linux-gnu
```

Run the script with test-only positional source and destination roots and
assert:

- first run copies the toolchain as the invoking user;
- second run changes no inode/content in the published toolchain;
- an existing user toolchain sentinel survives;
- a newly added source toolchain is added without replacing the first;
- `.gascan-rust-seed.*` staging residue is removed on retry;
- symlink/non-directory destination collisions fail nonzero;
- no source file is changed.

- [ ] **Step 2: Write failing provisioning argv and progress tests**

Require this order before mise installation:

```rust
[
    "initialize_managed_volume_roots",
    "initialize_rust_home",
    "initialize_workstation_home",
    "reset_safe_mise_workdir",
    "create_safe_mise_workdir",
    "write_safe_mise_config",
    "install_tools",
]
```

Require emitted protobuf step 6 and human message
`Preparing writable tool storage`.

- [ ] **Step 3: Run focused tests to verify failure**

Run:

```bash
rtk cargo test --manifest-path scripts/Cargo.toml image_user_contract
rtk cargo test -p gascand --test apply_tools
rtk cargo test -p gascan-core --test provision_plan
rtk cargo test -p gascan presentation
```

Expected: failures show the missing script, action, enum, and message.

- [ ] **Step 4: Implement the bootstrap script**

Use POSIX shell with `set -eu`. Defaults:

```sh
source_root=${1:-/opt/gascan/mise/rustup}
destination_root=${2:-/home/workspace/.local/share/rustup}
```

For each direct child of `"$source_root/toolchains"`:

1. Reject a source symlink or non-directory.
2. Skip an existing destination directory.
3. Reject an existing destination symlink or non-directory.
4. Create a mode-0700 staging directory named
   `.gascan-rust-seed.$toolchain.$$` below the destination root.
5. Copy contents with `cp -R "$source/." "$stage/"` without `-a`.
6. Verify the staged `bin/cargo` and `bin/rustc` are executable.
7. Publish with
   `mv --no-clobber --no-target-directory "$stage" "$destination/toolchains/$toolchain"`
   so a concurrently created final name is never replaced or treated as a
   directory target.
8. Copy the matching regular update-hash file through a mode-0600 temporary
   file and atomic rename only when absent.

Write a regular mode-0600
`$destination_root/.gascan-bundled-toolchains-v1` marker containing sorted
toolchain names. Trap EXIT/INT/TERM to remove only the script's recorded
staging path.

- [ ] **Step 5: Add progress step 6 without renumbering existing values**

Append:

```protobuf
PROVISION_STEP_INITIALIZE_RUNTIME_HOME = 6;
```

Map it to `ProvisionStep::InitializeRuntimeHome`,
`"initialize_runtime_home"`, API enum conversion, and
`"Preparing writable tool storage"`.

Do not add it to the manifest-dependent `ProvisionPlan`; runtime-home
initialization runs on every create/apply path before the plan because it is
idempotent and repairs interrupted bootstrap state.

- [ ] **Step 6: Invoke the bootstrap after volume ownership initialization**

Change `initialize_managed_volume_roots` to create the tools and cache roots:

```rust
[
    "/usr/bin/sudo", "-n", "/usr/bin/install", "-d",
    "-o", "workspace", "-g", "workspace", "-m", "0700",
    TOOLS_ROOT, CACHE_ROOT,
]
```

Retain a separate root command that creates `CONFIG_ROOT` as
`root:workspace` mode `1770`. This sticky group-writable boundary allows
ordinary XDG configuration while preventing the workspace user from replacing
root-owned Gas Can SSH state. Then emit `InitializeRuntimeHome` and execute:

```rust
[
    "/usr/bin/env",
    format!("HOME={WORKSPACE_HOME}"),
    format!("CARGO_HOME={CARGO_HOME}"),
    format!("RUSTUP_HOME={RUSTUP_HOME}"),
    "/usr/local/bin/initialize-rust-home".to_owned(),
]
```

Run it as `workspace`, before `configure-workstation-home` and before any mise
command.

- [ ] **Step 7: Supply the complete Task 1 environment to `mise_command`**

Add `CARGO_HOME`, `RUSTUP_HOME`, `MISE_CARGO_HOME`,
`MISE_RUSTUP_HOME`, XDG variables, package-manager variables, and exact
`PATH` to the `/usr/bin/env` argv. Assert provisioning and normal runtime maps
are identical for each shared key.

- [ ] **Step 8: Run focused tests**

Run:

```bash
rtk cargo test --manifest-path scripts/Cargo.toml image_user_contract
rtk cargo test -p gascan-core --test provision_plan
rtk cargo test -p gascand --test apply_tools
rtk cargo test -p gascand api
rtk cargo test -p gascan presentation
```

Expected: all pass.

- [ ] **Step 9: Commit**

```bash
rtk git add images/workspace/bin/initialize-rust-home images/workspace/Dockerfile proto/gascan/v1/gascan.proto crates/gascan-core/src/provision.rs crates/gascand/src/service.rs crates/gascand/src/api.rs crates/gascan/src/presentation.rs scripts/tests/image_user_contract.rs crates/gascand/tests/apply_tools.rs
rtk git commit -m "fix: seed writable Rust toolchains"
```

Include generated protobuf outputs only if this repository tracks them.

---

### Task 4: Enforce Writable Defaults in the Final Image

**Files:**
- Modify: `images/workspace/Dockerfile`
- Modify: `images/workspace/bin/configure-workstation-home`
- Modify: `images/workspace/etc/profile.d/mise.sh`
- Modify: `images/workspace/tests/workstation-contract.sh`
- Modify: `tests/image/workstation-smoke.sh`
- Modify: `scripts/tests/connected_dockerfile.rs`
- Modify: `scripts/tests/image_user_contract.rs`
- Modify: `scripts/tests/connected_image_gate.rs`

**Interfaces:**
- Consumes Task 1's exact paths and Task 3's bootstrap command.
- Produces a final image whose effective runtime environment matches production policy.
- Produces live write-smoke markers consumed by the connected image gate.

- [ ] **Step 1: Replace the incorrect persistent-build-home tests**

Change `assert_persistent_rustup_homes` into a two-phase assertion:

```rust
assert_env_before_first_install("CARGO_HOME", "/opt/gascan/mise/cargo");
assert_env_before_first_install("RUSTUP_HOME", "/opt/gascan/mise/rustup");
assert_effective_env("CARGO_HOME", "/home/workspace/.local/share/cargo");
assert_effective_env("RUSTUP_HOME", "/home/workspace/.local/share/rustup");
```

Require final `VOLUME` targets to be exactly the three version-2 roots and
require the bootstrap script to be copied mode 0555.

- [ ] **Step 2: Add failing workstation contract assertions**

The contract must inspect tool-reported destinations:

```sh
test "$(rustup show home)" = "$RUSTUP_HOME"
test "$(npm config get prefix)" = "$NPM_CONFIG_PREFIX"
test "$(npm config get cache)" = "$NPM_CONFIG_CACHE"
test "$(go env GOPATH)" = "$GOPATH"
test "$(go env GOCACHE)" = "$GOCACHE"
test "$(go env GOMODCACHE)" = "$GOMODCACHE"
test "$(python -m site --user-base)" = "$PYTHONUSERBASE"
test "$(gem env home)" = "$GEM_HOME"
```

For each destination, resolve it with `realpath -m`, assert containment below
one managed root, assert its nearest existing parent is writable by UID 1000,
and assert `/opt/gascan` is never the result. Assert every install bin
directory appears before `/opt/gascan/mise/shims` on `PATH`.

- [ ] **Step 3: Add failing local-package install smoke cases**

Build network-independent local fixtures under `/tmp` and execute:

```sh
cargo run --manifest-path "$fixture/rust-app/Cargo.toml"
cargo install --path "$fixture/rust-bin"
npm install --global "$fixture/npm-bin"
go install "$fixture/go-bin"
python -m pip install --user --no-deps "$fixture/python-bin"
gem install --local "$fixture/ruby-bin.gem"
```

Assert each installed command resolves below `/home/workspace/.local` and
runs. Add a crates.io dependency fetch case separately to the networked live
gate so the original Cargo registry-cache failure is reproduced.

The networked case uses an exact dependency
`cfg-if = "=1.0.4"`, verifies Cargo creates registry state below
`$CARGO_HOME`, and runs `rustup component add rust-src` followed by
`rustup component list --installed`. The raw-image smoke invokes
`/usr/local/bin/initialize-rust-home` after mounting the tools volume and
before either Rust command.

- [ ] **Step 4: Run source-level image tests to verify failure**

Run:

```bash
rtk cargo test --manifest-path scripts/Cargo.toml connected_dockerfile
rtk cargo test --manifest-path scripts/Cargo.toml image_user_contract
rtk cargo test --manifest-path scripts/Cargo.toml connected_image_gate
```

Expected: failures show old final `ENV`, old volume leaves, incomplete
workstation configuration, and absent write-smoke commands.

- [ ] **Step 5: Set final image environment and broad roots**

Keep build-stage `/opt/gascan` `ENV` declarations before `mise install`. In the
final stage:

- create `/home/workspace/.local` and `.cache` as `workspace:workspace` mode
  0700;
- create `/home/workspace/.config` as `root:workspace` mode 1770 and verify
  the workspace user can create its own child directory but cannot replace a
  root-owned Gas Can child;
- change `VOLUME` to those roots;
- set every Task 1 runtime variable explicitly;
- set the exact `PATH`;
- retain immutable `/opt/gascan` ownership and modes.

Update `configure-workstation-home` preflight/marker logic to operate safely
inside the broad roots without requiring root-owned `.config/gascan`.

- [ ] **Step 6: Keep interactive and SSH shells identical**

Update `mise.sh` defaults for all shared environment keys and build the exact
Task 1 `PATH` without duplicating entries on repeated sourcing. Verify both
`bash -lc` and native SSH run the same `CARGO_HOME`, `RUSTUP_HOME`, XDG roots,
and command resolution.

- [ ] **Step 7: Run source-level image and shell tests**

Run:

```bash
rtk cargo test --manifest-path scripts/Cargo.toml connected_dockerfile
rtk cargo test --manifest-path scripts/Cargo.toml image_user_contract
rtk cargo test --manifest-path scripts/Cargo.toml connected_image_gate
```

Expected: all pass.

- [ ] **Step 8: Commit**

```bash
rtk git add images/workspace/Dockerfile images/workspace/bin/configure-workstation-home images/workspace/etc/profile.d/mise.sh images/workspace/tests/workstation-contract.sh tests/image/workstation-smoke.sh scripts/tests/connected_dockerfile.rs scripts/tests/image_user_contract.rs scripts/tests/connected_image_gate.rs
rtk git commit -m "test: enforce writable workstation defaults"
```

---

### Task 5: Document Migration and Restore a Green Local Baseline

**Files:**
- Modify: `README.md`
- Modify: `docs/release/macos-checklist.md`
- Modify: `packaging/macos/release-smoke.sh`
- Modify: `crates/gascan/src/cli.rs`
- Test: all workspace and script suites

**Interfaces:**
- Consumes Tasks 1–4 behavior.
- Produces user-facing upgrade guidance and installed-release write proof.

- [ ] **Step 1: Correct the stale SSH unit test**

The test `optional_include_offer_failure_preserves_successful_up_result`
currently creates mode `0755`, which is intentionally accepted after the SSH
permission compatibility release. Change its unsafe fixture to mode `0775`
and keep the expected warning unchanged. Add or retain a neighboring test
showing `0755` succeeds without a warning.

- [ ] **Step 2: Update README storage and migration guidance**

Document this exact pre-0.1.10 upgrade sequence:

```bash
gascan destroy --yes
gascan up .
```

Explain that the new tools volume receives an approximately 1.5 GiB local
copy of the bundled Rust toolchain, and show independent sizing:

```toml
[storage]
tools = "20GiB"
cache = "10GiB"
config = "2GiB"
```

Add copyable examples for:

```bash
cargo run
rustup component add rust-src
npm install -g typescript
go install golang.org/x/tools/gopls@latest
python -m pip install --user ruff
gem install bundler
```

State that project-specific dependency versions should still be declared in
project files or `[tools]`, not installed globally by Gas Can.

- [ ] **Step 3: Extend installed-release smoke**

After the release sandbox starts, assert exact version-2 mount targets,
writable homes, `cargo run` with a crates.io dependency, local
`cargo install --path`, one npm/Go/Python/Ruby local install, and XDG config
creation. Use release-smoke-owned temporary fixtures and preserve its exact
cleanup guarantees.

- [ ] **Step 4: Run format, lint, workspace, and script suites**

Run:

```bash
rtk cargo fmt --all -- --check
rtk cargo clippy --workspace --all-targets -- -D warnings
rtk cargo test --workspace
rtk cargo test --manifest-path scripts/Cargo.toml
rtk swift test --package-path helpers/attach
rtk git diff --check
```

Then run every release contract:

```bash
rtk bash -c 'for contract in tests/release/*-contract.sh; do bash "$contract"; done'
```

Expected: all pass. Specifically confirm the formerly failing SSH test passes.

- [ ] **Step 5: Commit**

```bash
rtk git add README.md docs/release/macos-checklist.md packaging/macos/release-smoke.sh crates/gascan/src/cli.rs
rtk git commit -m "docs: explain writable sandbox storage"
```

---

### Task 6: Publish, Exercise, and Approve the Connected Workspace Image

**Files:**
- Modify through approval script: `images/workspace/approved-image.txt`
- Modify through approval script: `docs/evidence/connected-workspace-image.md`
- Generated local artifacts under ignored `.artifacts/`

**Interfaces:**
- Consumes the complete Tasks 1–5 source tree.
- Produces the digest-qualified approved workspace image used by policy and release packaging.

- [ ] **Step 1: Run Apple preflight and check for owned residue**

Run:

```bash
rtk bash ./scripts/apple-test-preflight.sh
rtk container list --all --format json
rtk container volume list --format json
```

Do not delete ambiguous or foreign resources. Clean only exact test-owned
resources through the repository cleanup path.

- [ ] **Step 2: Build and run the local connected image gate**

Run:

```bash
rtk bash ./scripts/run-connected-image-gate.sh
```

Expected final output: one digest-qualified candidate reference. Require the
new networked Cargo dependency case to download successfully and every
package-manager write smoke to pass.

- [ ] **Step 3: Publish a unique immutable GHCR candidate**

Read the validated local receipt. Derive a never-reused remote tag from the
locked workspace tag plus the complete 64-character image digest. Execute one
reviewed shell block so no digest is typed or guessed:

```bash
rtk bash -c '
set -euo pipefail
receipt=.artifacts/workspace-image-build.json
reference_file=.artifacts/workspace-image-ref
local_reference=$(jq -er .reference "$receipt")
local_tag=${local_reference%@*}
digest=${local_reference##*@}
digest_hex=${digest#sha256:}
test ${#digest_hex} -eq 64
locked_tag=$(awk -F " = " '"'"'$1 == "workspace_tag" { gsub(/^"|"$/, "", $2); print $2 }'"'"' images/workspace/versions.lock)
locked_tag=${locked_tag#gascan-workspace:}
remote_tag=ghcr.io/liquescent-development/gascan/workspace:${locked_tag}-${digest_hex}
remote_reference=$remote_tag@$digest
container image tag "$local_tag" "$remote_tag"
headers=$(mktemp .artifacts/.workspace-registry-headers.XXXXXX)
receipt_tmp=$(mktemp .artifacts/.workspace-image-build.public.XXXXXX)
reference_tmp=$(mktemp .artifacts/.workspace-image-ref.public.XXXXXX)
trap '"'"'rm -f "$headers" "$receipt_tmp" "$reference_tmp"'"'"' EXIT
token=$(curl --fail --silent --show-error "https://ghcr.io/token?scope=repository:liquescent-development/gascan/workspace:pull" | jq -er .token)
status=$(curl --silent --show-error --output /dev/null --dump-header "$headers" --write-out "%{http_code}" \
  --header "Authorization: Bearer $token" \
  --header "Accept: application/vnd.oci.image.manifest.v1+json, application/vnd.oci.image.index.v1+json" \
  "https://ghcr.io/v2/liquescent-development/gascan/workspace/manifests/${remote_tag##*:}")
case $status in
  200)
    existing=$(awk '"'"'tolower($1) == "docker-content-digest:" { gsub(/\r/, "", $2); print $2 }'"'"' "$headers")
    test "$existing" = "$digest"
    ;;
  404)
    container image push --platform linux/arm64 "$remote_tag"
    ;;
  *)
    printf "unexpected GHCR manifest status: %s\n" "$status" >&2
    exit 1
    ;;
esac
container image pull "$remote_reference"
inspect=$(container image inspect "$remote_tag")
printf "%s" "$inspect" | cargo run --quiet --locked --offline --manifest-path scripts/Cargo.toml --bin validate-connected-build -- "$remote_tag" >/dev/null
jq --arg reference "$remote_reference" --arg tag "$remote_tag" ".reference = \$reference | .tag = \$tag" "$receipt" >"$receipt_tmp"
printf "%s\n" "$remote_reference" >"$reference_tmp"
bash scripts/validate-connected-image-receipt.sh "$reference_tmp" "$receipt_tmp" >/dev/null
mv -f "$receipt_tmp" "$receipt"
mv -f "$reference_tmp" "$reference_file"
rm -f "$headers"
trap - EXIT
'
```

The public manifest query requires an existing tag's descriptor digest to
equal the local digest and skips the push in that case; any difference fails
closed. This guarantees the process never overwrites an existing registry
tag. After publication, the pull, structured inspection, and receipt
validation above must all agree on the exact digest.

- [ ] **Step 4: Re-run the gate and full Apple apply suite against the public digest**

Run:

```bash
rtk bash ./scripts/run-connected-image-gate.sh --prebuilt
rtk env GASCAN_E2E_CANDIDATE_IMAGE_FILE=.artifacts/connected-workspace-image-candidate.txt bash ./scripts/run-apple-e2e.sh apple_apply
```

Expected: the candidate file and Apple-live acceptance file contain the same
public GHCR digest-qualified reference, all apply/write checks pass, and scoped
cleanup reports no test-owned residue.

- [ ] **Step 5: Inspect live evidence before approval**

Verify:

```bash
rtk cat .artifacts/connected-workspace-image-candidate.txt
rtk cat .artifacts/connected-workspace-image-apple-live.txt
rtk cat .artifacts/workspace-image-build.json
rtk bash ./scripts/validate-connected-image-receipt.sh .artifacts/workspace-image-ref .artifacts/workspace-image-build.json
```

The candidate, Apple-live receipt, and validated public build reference must
match exactly.

- [ ] **Step 6: Approve the image**

Run:

```bash
rtk bash ./scripts/approve-connected-workspace-image.sh
```

Expected: it atomically updates only the approved image reference and evidence
document with the matching digest.

- [ ] **Step 7: Re-run policy and full local verification against the approved pin**

Run:

```bash
rtk cargo test --workspace
rtk cargo test --manifest-path scripts/Cargo.toml
rtk cargo clippy --workspace --all-targets -- -D warnings
rtk git diff --check
```

Expected: all pass.

- [ ] **Step 8: Commit the approved image**

```bash
rtk git add images/workspace/approved-image.txt docs/evidence/connected-workspace-image.md
rtk git commit -m "build: approve writable workspace image"
```

---

### Task 7: Review and Merge the Feature Pull Request

**Files:**
- No new source files; this task verifies and publishes the feature branch.

**Interfaces:**
- Consumes all implementation commits and the approved image digest.
- Produces a merged `origin/main` suitable for a release-version branch.

- [ ] **Step 1: Verify branch cleanliness and commit scope**

Run:

```bash
rtk git status --short
rtk git log --oneline origin/main..HEAD
rtk git diff --check origin/main...HEAD
```

Expected: clean worktree, only intended commits, no changes to the dirty
primary checkout.

- [ ] **Step 2: Use verification-before-completion and request code review**

Run the complete commands from Task 5 Step 4 once more from the final commit.
Review specifically for:

- legacy volume corruption paths;
- unsafe Rust bootstrap replacement or symlink traversal;
- environment disagreement between normal exec, SSH, mise provisioning, and
  final Dockerfile;
- credentials printed by tests;
- test-owned Apple resource cleanup.

- [ ] **Step 3: Push and create the PR**

```bash
rtk git push -u origin fix/writable-runtime-homes
rtk gh pr create --base main --head fix/writable-runtime-homes \
  --title "Fix writable sandbox tool storage" \
  --body "Moves managed storage to conventional user roots, adds a safe writable Rust bootstrap, rejects legacy mount layouts, audits all bundled defaults, and records live Apple image-gate evidence."
```

The PR body summarizes the root cause, version-2 migration, Rust seed cost,
default-tool audit, live image evidence, and exact verification commands.

- [ ] **Step 4: Wait for checks and merge**

Run:

```bash
rtk gh pr checks --watch
rtk gh pr merge --squash
```

Expected: all required checks pass and GitHub reports the PR merged.

- [ ] **Step 5: Sync without touching the primary checkout**

Fetch `origin/main` in the isolated worktree and create the release branch
directly from it. Do not check out or rebase the user's dirty primary branch.
After switching away from the feature branch, delete its remote ref explicitly
and retain the local branch only until the temporary worktree is removed.

---

### Task 8: Bump to 0.1.10, Tag, Publish, and Verify

**Files:**
- Modify: six `crates/*/Cargo.toml` workspace package versions
- Modify: `Cargo.lock`
- Modify: `README.md`
- Modify: `docs/release/macos-checklist.md`
- Do not modify: `scripts/Cargo.lock`
- Do not modify: `tests/release/release-script-contract.sh`

**Interfaces:**
- Consumes merged feature commit on `origin/main`.
- Produces merged version-bump PR, signed `v0.1.10`, notarized package, GitHub release, Homebrew cask update, and installed-release smoke evidence.

- [ ] **Step 1: Create the release branch from refreshed `origin/main`**

```bash
rtk git fetch origin
rtk git switch -c release/0.1.10 origin/main
```

- [ ] **Step 2: Bump exactly the release-owned version references**

Set each workspace crate version to `0.1.10`, update `README.md` and
`docs/release/macos-checklist.md` references from `0.1.9` to `0.1.10`, then
run:

```bash
rtk cargo update --workspace --offline
```

Verify `scripts/Cargo.lock` and release contract fixture arguments are
unchanged.

- [ ] **Step 3: Commit before running release contracts**

```bash
rtk git add Cargo.toml Cargo.lock crates README.md docs/release/macos-checklist.md
rtk git commit -m "release: prepare Gas Can 0.1.10"
```

- [ ] **Step 4: Verify the bump**

Run:

```bash
rtk cargo metadata --locked --no-deps --format-version 1
rtk cargo check --locked --workspace --all-targets
rtk bash -c 'for contract in tests/release/*-contract.sh; do bash "$contract"; done'
```

Use `jq` on the metadata output to assert every Gas Can workspace package is
`0.1.10`.

- [ ] **Step 5: Create and merge the version PR**

```bash
rtk git push -u origin release/0.1.10
rtk gh pr create --base main --head release/0.1.10 \
  --title "Release Gas Can 0.1.10" \
  --body "Bumps the six Gas Can workspace crates and user-facing release references to 0.1.10 after the writable-runtime-home fix."
rtk gh pr checks --watch
rtk gh pr merge --squash
```

- [ ] **Step 6: Create the signed provenance tag from exact remote main**

In a clean release worktree:

```bash
rtk git fetch origin
rtk git switch --detach origin/main
rtk git tag -s v0.1.10 -m "Gas Can 0.1.10"
rtk git verify-tag v0.1.10
rtk git push origin v0.1.10
```

Assert the tag target equals `origin/main` and that no `v0.1.10` release
already exists before pushing.

- [ ] **Step 7: Run the non-mutating release check**

```bash
rtk bash ./packaging/macos/release.sh 0.1.10 --check
```

Expected: `all release preconditions pass for 0.1.10`.

- [ ] **Step 8: Publish**

Run:

```bash
rtk bash ./packaging/macos/release.sh 0.1.10
```

Do not create, move, delete, or recreate the signed tag during recovery. If a
publish interruption leaves a draft, inspect it before using the documented
draft-only recovery.

- [ ] **Step 9: Verify the public and installed release**

Run:

```bash
rtk gh release view v0.1.10 --json isDraft,tagName,url,assets
rtk brew update
rtk brew upgrade --cask gascan
```

Require `isDraft=false`, the package/checksum/build-manifest assets, Gas Can
`--version` equal to `0.1.10`, and the complete updated
`packaging/macos/release-smoke.sh` PASS line.

- [ ] **Step 10: Clean only the temporary branch/worktree**

Delete the merged feature and release remote refs explicitly, detach the
temporary worktree from both local branches, and remove the isolated worktree.
Only after the worktree is absent, delete its two local branches. Do not alter
the user's dirty `feat/default-ssh-workstation` checkout or its uncommitted
files.
