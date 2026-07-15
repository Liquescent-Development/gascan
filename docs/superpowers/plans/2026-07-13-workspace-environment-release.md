# Workspace Environment and Release Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the pinned polyglot workspace image, mise and Gascamp provisioning, security acceptance suite, macOS packaging, and clean-host MVP release gate.

**Architecture:** An ARM64 OCI image supplies stable system tooling, a workspace user with guest-root sudo, mise, and bundled Gascamp. `ProvisioningPlanner` converts the manifest into explicit apply steps whose resolved versions and digests are durable; real-runtime security probes validate the host boundary before packaging.

**Tech Stack:** OCI/Dockerfile built with Apple `container`, Ubuntu LTS base by immutable digest, Rust 1.85+, mise, Bash smoke/security scripts, existing Gas Can daemon/CLI/API.

> **Approved correction (2026-07-14):** In-build apt, mise, Git, and Cargo network access is replaced by the [Offline Workspace Image Bundles Plan](./2026-07-14-offline-workspace-image-bundles.md). Connected Linux ARM64 CI produces immutable verified inputs; the Apple build is network-independent. Where the original Tasks 1, 3, or 4 describe fetching inside the Dockerfile, the correction plan governs.

## Global Constraints

- Image inputs are immutable digests or checksummed artifacts recorded in `images/workspace/versions.lock`.
- The image is native Linux ARM64; Rosetta/amd64 is deferred.
- `/workspace` is the only host bind mount.
- Persistent volumes contain tool installations, caches, and non-secret configuration only.
- Default UID/GID is the workspace user; passwordless `sudo` grants full guest root.
- Bundled Gascamp is pinned; local override must resolve beneath `/workspace/gascamp` and is labeled untrusted.
- Changed setup code or tools never execute on ordinary `up`, `run`, or `shell`; only initial creation or explicit `gascan apply` executes them.
- Security/release tests require Roadmap Gate 4; image-local work may begin after Gate 1.
- Rust production code denies unsafe code, unwraps, expects, and panics.

---

### Task 1: Add reproducible image input locking and build context

**Files:**
- Create: `images/workspace/Dockerfile`
- Create: `images/workspace/versions.toml`
- Create at execution: `images/workspace/versions.lock`
- Create: `scripts/Cargo.toml`
- Create: `scripts/src/bin/update-image-lock.rs`
- Create: `scripts/build-workspace-image.sh`
- Test: `scripts/tests/image_lock.rs`
- Modify: `.gitignore`

**Interfaces:**
- Produces: `versions.lock` containing base image digest, Ubuntu package-snapshot timestamp, mise version/URL/SHA-256, exact default runtime versions, Playwright Chromium version/URL/SHA-256, Gascamp git revision, and resulting Gas Can image tag.
- Produces: `build-workspace-image.sh`, which accepts no floating dependency versions and prints the resulting OCI digest.

- [ ] **Step 1: Write a lock completeness test**

```rust
#[test]
fn every_remote_image_input_is_immutable_and_checksummed() {
    let lock: ImageLock = toml::from_str(include_str!("../../images/workspace/versions.lock")).unwrap();
    assert!(lock.base_image.contains("@sha256:"));
    assert_eq!(lock.mise.sha256.len(), 64);
    assert_eq!(lock.playwright_chromium.sha256.len(), 64);
    assert!(lock.tools.values().all(|version| !matches!(version.as_str(), "latest" | "stable" | "lts")));
    assert_eq!(lock.gascamp.revision.len(), 40);
    assert!(!lock.workspace_tag.ends_with(":latest"));
}
```

- [ ] **Step 2: Run the test before generating the lock**

Run: `cargo test --manifest-path scripts/Cargo.toml --test image_lock`

Expected: FAIL because `versions.lock` does not exist.

- [ ] **Step 3: Implement and run the lock generator**

`versions.toml` declares `ubuntu = "24.04"`, `ubuntu_snapshot = "2026-07-13T00:00:00Z"`, `mise = "2026.5.0"`, default tool aliases, the Playwright Chromium channel, and `gascamp_revision = "f6b248c5926240856dbea83d1d2c5c90ea1c1456"`. The updater queries official release metadata, resolves the Linux ARM64 base digest, exact runtime versions, mise and browser artifact URLs, downloads artifacts to temporary files, computes SHA-256, and writes a sorted lock atomically. It rejects redirects outside approved Ubuntu/GitHub/Playwright hosts, unresolved aliases, and any non-40-character Gascamp revision. Add `.artifacts/` to `.gitignore`; build and smoke scripts use `.artifacts/workspace-image-ref` as their handoff.

Run: `cargo run --manifest-path scripts/Cargo.toml --bin update-image-lock`

Expected: creates a fully concrete `versions.lock`; no `latest`, branch name, empty hash, or mutable image reference remains. The build script records the built image reference in `.artifacts/workspace-image-ref`, which all image smoke scripts read.

- [ ] **Step 4: Build the minimal locked image**

The initial Dockerfile begins with `ARG BASE_IMAGE` and `FROM ${BASE_IMAGE}`; the build script passes only the immutable `ubuntu@sha256:...` value read from the generated lock. It configures the locked Ubuntu snapshot before noninteractive apt, installs CA certificates, and copies only verified artifacts. Run: `./scripts/build-workspace-image.sh`

Expected: exit 0 and `container image inspect --format json <tag>` reports `linux/arm64`; rerunning without input changes produces the same declared inputs.

- [ ] **Step 5: Commit immutable image inputs**

```bash
git add images/workspace scripts tests/image
git commit -m "build: lock workspace image inputs"
```

### Task 2: Create the workspace user, guest-root sudo, init, and volume layout

**Files:**
- Modify: `images/workspace/Dockerfile`
- Create: `images/workspace/bin/gascan-entrypoint`
- Create: `images/workspace/etc/sudoers.d/workspace`
- Create: `tests/image/user-and-volumes.sh`

**Interfaces:**
- Image user: `workspace`, UID/GID 1000 by default, home `/home/workspace`.
- Persistent targets: `/opt/gascan/mise`, `/home/workspace/.cache`, `/home/workspace/.config/gascan`.
- Entrypoint runs a lightweight init and waits for exec sessions without exposing a daemon socket.

- [ ] **Step 1: Write the image-local privilege test**

```bash
#!/usr/bin/env bash
set -euo pipefail
test "$(id -un)" = workspace
test "$(id -u)" = 1000
test "$(sudo -n id -u)" = 0
test "$(stat -c %U /opt/gascan/mise)" = workspace
test ! -e /run/host-services/ssh-auth.sock
test ! -e /var/run/docker.sock
```

- [ ] **Step 2: Run against the current image**

Run: `./tests/image/user-and-volumes.sh`

Expected: FAIL because the workspace user/layout is absent.

- [ ] **Step 3: Implement the user and init contract**

Install `sudo` and `tini`; create the user/group, owned directories, and mode-0440 sudoers entry `workspace ALL=(ALL:ALL) NOPASSWD: ALL`; validate it with `visudo -cf`. Set `USER workspace`, `WORKDIR /workspace`, and entrypoint `tini -- /usr/local/bin/gascan-entrypoint`. The entrypoint must use `exec` and contain no network/bootstrap behavior.

- [ ] **Step 4: Rebuild and run privilege/signal checks**

Run: `./scripts/build-workspace-image.sh && ./tests/image/user-and-volumes.sh`

Expected: PASS; SIGTERM to the container exits within five seconds and no zombie remains.

- [ ] **Step 5: Commit the guest user model**

```bash
git add images/workspace tests/image
git commit -m "feat: add privileged workspace user to image"
```

### Task 3: Install polyglot system tooling and mise

**Files:**
- Modify: `images/workspace/Dockerfile`
- Create: `images/workspace/etc/mise/config.toml`
- Create: `images/workspace/etc/profile.d/mise.sh`
- Create: `tests/image/polyglot-smoke.sh`
- Create: `tests/image/system-tools.txt`

**Interfaces:**
- Mise data root: `/opt/gascan/mise`; cache: `/home/workspace/.cache/mise`; global config contains Gas Can defaults only.
- Supported smoke matrix: Node, Python, Go, Rust, Java, Ruby, and Elixir plus Git/GitHub CLI, native compilation, and browser automation libraries.

- [ ] **Step 1: Write the smoke matrix before installing tools**

```bash
#!/usr/bin/env bash
set -euo pipefail
mise --version
for tool in node python go rust java ruby elixir; do mise ls --installed "$tool"; done
node -e 'console.log("node-ok")'
python -c 'print("python-ok")'
printf 'package main\nimport "fmt"\nfunc main(){fmt.Println("go-ok")}\n' >/tmp/main.go
go run /tmp/main.go
printf 'fn main(){println!("rust-ok");}\n' >/tmp/main.rs
rustc /tmp/main.rs -o /tmp/rust-ok && /tmp/rust-ok
printf 'class Main { public static void main(String[] a){ System.out.println("java-ok"); } }\n' >/tmp/Main.java
javac /tmp/Main.java && java -cp /tmp Main
ruby -e 'puts "ruby-ok"'
elixir -e 'IO.puts("elixir-ok")'
node /opt/gascan/tests/playwright-smoke.mjs
git --version && gh --version && cc --version
```

- [ ] **Step 2: Confirm the minimal image fails the matrix**

Run: `./tests/image/polyglot-smoke.sh`

Expected: FAIL on missing `mise` or first runtime.

- [ ] **Step 3: Install verified mise, system dependencies, and default runtimes**

Verify downloaded mise and Chromium SHA-256 values before installation. Install the reviewed package list from the locked Ubuntu snapshot with `--no-install-recommends`, clean apt metadata, configure mise paths, and run `mise install` using the exact tool versions in `versions.lock` as `workspace`. Store the same versions in `/opt/gascan/image-tool-versions.json`. Install the locked headless Chromium artifact and a minimal Playwright smoke script at `/opt/gascan/tests/playwright-smoke.mjs`.

- [ ] **Step 4: Rebuild and run smoke/cache tests**

Run: `./scripts/build-workspace-image.sh && ./tests/image/polyglot-smoke.sh`

Expected: PASS; a second container using the same named mise/cache volumes does not redownload installed versions.

- [ ] **Step 5: Commit the polyglot layer**

```bash
git add images/workspace tests/image
git commit -m "feat: ship mise polyglot workspace tools"
```

### Task 4: Bundle pinned Gascamp and support a local checkout

**Files:**
- Modify: `images/workspace/Dockerfile`
- Create: `images/workspace/bin/select-gascamp`
- Create: `tests/image/gascamp-smoke.sh`
- Create: `crates/gascan-core/src/gascamp.rs`
- Test: `crates/gascan-core/tests/gascamp_source.rs`

**Interfaces:**
- Bundled binaries: `/opt/gascan/gascamp/bin/camp` and `campd` symlink, built from locked revision.
- Produces: `GascampSource::{Bundled { revision }, Workspace { path }}` and resolver that accepts only the exact `/workspace/gascamp` subtree.
- Status reports source and `trusted = false` for workspace override.

- [ ] **Step 1: Write source-boundary and image smoke tests**

```rust
#[test]
fn local_gascamp_must_resolve_beneath_workspace() {
    assert!(resolve_gascamp("/workspace/gascamp").is_ok());
    assert!(resolve_gascamp("/workspace/repo/../gascamp").is_ok());
    assert!(resolve_gascamp("/opt/gascan/gascamp").is_err());
    assert!(resolve_gascamp("/workspace/gascamp-link-outside").is_err());
}
```

- [ ] **Step 2: Verify bundled binary and resolver are absent**

Run: `cargo test -p gascan-core --test gascamp_source && ./tests/image/gascamp-smoke.sh`

Expected: FAIL on missing resolver/binary.

- [ ] **Step 3: Build pinned Gascamp and implement selection**

Use a multi-stage Rust build at revision `f6b248c5926240856dbea83d1d2c5c90ea1c1456`, run its tests, copy the stripped binary, create `campd` symlink, and record revision. The selector emits the bundled path by default or canonicalizes the mounted override, rejects symlink escape, verifies executable `camp`, and marks workspace source untrusted in metadata.

- [ ] **Step 4: Run bundled/local smoke tests**

Run: `cargo test -p gascan-core --test gascamp_source && ./tests/image/gascamp-smoke.sh`

Expected: PASS for `camp --version`, `campd` argv0 behavior, bundled default, valid local checkout, and symlink/path escape rejection.

- [ ] **Step 5: Commit Gascamp integration**

```bash
git add images/workspace crates/gascan-core tests/image
git commit -m "feat: bundle and select pinned Gascamp"
```

### Task 5: Plan and execute explicit mise provisioning

**Files:**
- Create: `crates/gascan-core/src/provision.rs`
- Modify: `crates/gascand/src/service.rs`
- Modify: `proto/gascan/v1/gascan.proto`
- Test: `crates/gascan-core/tests/provision_plan.rs`
- Test: `crates/gascand/tests/apply_tools.rs`

**Interfaces:**
- Produces: `ProvisioningPlanner::plan(manifest, applied) -> ProvisionPlan`.
- Step variants: `WriteSafeMiseConfig`, `InstallTools`, `RunSetup`, `VerifyGascamp`, `HealthCheck`.
- `gascan apply` streams each step and persists resolved versions only after all required steps pass.

- [ ] **Step 1: Write change-detection and safe-config tests**

```rust
#[test]
fn tool_change_requires_apply_and_emits_plain_mise_config() {
    let plan = ProvisioningPlanner::plan(&manifest_tools([("node", "lts")]), &AppliedState::empty()).unwrap();
    assert_eq!(plan.steps[0], ProvisionStep::WriteSafeMiseConfig);
    let config = plan.safe_mise_toml().unwrap();
    assert!(config.contains("[tools]"));
    assert!(!config.contains("[env]"));
    assert!(!config.contains("hooks"));
}
```

- [ ] **Step 2: Verify planner tests fail**

Run: `cargo test -p gascan-core --test provision_plan && cargo test -p gascand --test apply_tools`

Expected: FAIL because provisioning types are absent.

- [ ] **Step 3: Implement deterministic planning and apply execution**

Sort tools, escape TOML through serialization rather than string interpolation, write config under Gas Can's persistent config volume, run `mise install --yes` and `mise current --json` through literal argv, and persist exact resolved versions. `up` on an existing sandbox compares desired/applied hashes and emits `apply_required` without executing changes.

- [ ] **Step 4: Run fake-backend provisioning tests**

Run: `cargo test -p gascan-core --test provision_plan && cargo test -p gascand --test apply_tools`

Expected: PASS for first create, no-op apply, changed tools, failed install, retry, safe config, progress events, and resolved-version persistence.

- [ ] **Step 5: Commit explicit tool apply**

```bash
git add crates/gascan-core crates/gascand proto
git commit -m "feat: apply mise tool provisioning explicitly"
```

### Task 6: Execute setup scripts by digest only on create or apply

**Files:**
- Modify: `crates/gascan-core/src/provision.rs`
- Modify: `crates/gascand/src/service.rs`
- Test: `crates/gascan-core/tests/setup_policy.rs`
- Test: `crates/gascan-e2e/tests/apple_apply.rs`

**Interfaces:**
- Produces: `SetupScript { canonical_relative_path, sha256 }`.
- Setup executes as `/bin/bash /workspace/<path>` with guest environment only; recorded digest updates after successful completion.

- [ ] **Step 1: Write path, digest, and non-silent-execution tests**

```rust
#[tokio::test]
async fn changed_setup_is_reported_but_not_run_by_up_or_shell() {
    let env = ApplyHarness::new().await.unwrap();
    env.write_setup("printf first > /workspace/result").await;
    env.apply().await.unwrap();
    env.write_setup("printf second > /workspace/result").await;
    env.up().await.unwrap();
    env.shell(["true"]).await.unwrap();
    assert_eq!(env.read("result").await.unwrap(), "first");
    assert!(env.status().await.unwrap().apply_required);
}
```

- [ ] **Step 2: Verify changed setup currently executes or is ignored incorrectly**

Run: `cargo test -p gascan-core --test setup_policy && cargo test -p gascan-e2e --test apple_apply -- --ignored --test-threads=1`

Expected: FAIL before setup policy is integrated.

- [ ] **Step 3: Implement canonicalization, hashing, and success-only persistence**

Resolve the manifest-relative path against the already canonical root, reject symlink escape and non-regular/non-readable files, hash bytes before mounting/execution, and recheck the mounted file digest immediately before exec to detect a race. On nonzero exit retain the previous applied digest, stop the sandbox for inspection, and include the exit code in sanitized logs.

- [ ] **Step 4: Run setup unit and live tests**

Run: `cargo test -p gascan-core --test setup_policy && cargo test -p gascan-e2e --test apple_apply -- --ignored --test-threads=1`

Expected: PASS for initial create, explicit apply, unchanged no-op, changed report, traversal/symlink/race rejection, failed setup, retry, and status metadata.

- [ ] **Step 5: Commit setup semantics**

```bash
git add crates/gascan-core crates/gascand crates/gascan-e2e/tests/apple_apply.rs
git commit -m "feat: apply workspace setup by content digest"
```

### Task 7: Build the real security acceptance suite

**Files:**
- Create: `tests/security/run.sh`
- Create: `tests/security/host-boundary.sh`
- Create: `tests/security/offline-network.sh`
- Create: `tests/security/ports.sh`
- Create: `tests/security/resources.sh`
- Create: `tests/security/fixtures/gascan.toml`

**Interfaces:**
- Consumes: a supported real backend and temporary test root.
- Produces: TAP output and nonzero exit on any violated security guarantee.
- Tests may inspect host state only through fixtures they create.

- [ ] **Step 1: Add failing adversarial probes**

```bash
#!/usr/bin/env bash
set -euo pipefail
gascan up --offline "$FIXTURE_ROOT"
deny gascan run -- test -r /Users/"$USER"/.ssh/id_ed25519
deny gascan run -- test -S /var/run/docker.sock
deny gascan run -- test -S /run/host-services/ssh-auth.sock
deny gascan run -- curl --fail --max-time 2 https://example.com
deny gascan run -- curl --fail --max-time 2 "$HOST_PROBE_URL"
allow gascan run -- sudo -n id -u
```

- [ ] **Step 2: Run the suite and capture initial violations**

Run: `./tests/security/run.sh`

Expected: initial FAIL for each missing harness or violated guarantee; cleanup trap still destroys the test sandbox and owned resources.

- [ ] **Step 3: Complete probes without weakening policy**

Test canonical mount escape, common credentials/sockets, offline DNS/IP/host routes as workspace and root, undeclared and declared loopback ports, CPU/memory/process/disk ceilings using bounded workloads, daemon socket mode/peer rejection, root guest behavior, and no secret values in logs. Fix implementation defects with unit regressions in their owning crate.

- [ ] **Step 4: Run security, Rust, and image gates**

Run: `./tests/security/run.sh && cargo test --workspace && ./tests/image/polyglot-smoke.sh`

Expected: PASS; TAP plan has no skips on a supported host and post-run runtime inspection shows no current-prefix resources.

- [ ] **Step 5: Commit security acceptance evidence**

```bash
git add tests/security crates
git commit -m "test: enforce Gas Can host security boundary"
```

### Task 8: Package binaries and execute the clean-host release gate

**Files:**
- Create: `packaging/macos/package.sh`
- Create: `packaging/macos/install.sh`
- Create: `packaging/macos/uninstall.sh`
- Create: `tests/release/clean-host.sh`
- Create: `docs/release/macos-checklist.md`
- Modify: `README.md`

**Interfaces:**
- Produces a macOS product package containing native ARM64 `gascan` and `gascand`, license, default configuration, and no Apple runtime binary.
- Installer checks prerequisites, installs binaries, and leaves daemon startup on-demand.
- Uninstaller never removes sandboxes/volumes without an explicit data-removal flag.

- [ ] **Step 1: Write package-content and clean-host tests**

```bash
#!/usr/bin/env bash
set -euo pipefail
package=$(./packaging/macos/package.sh)
pkgutil --payload-files "$package" | grep -qx './usr/local/bin/gascan'
pkgutil --payload-files "$package" | grep -qx './usr/local/bin/gascand'
! pkgutil --payload-files "$package" | grep -q '/container$'
./packaging/macos/install.sh "$package"
gascan doctor --json | jq -e '.ok == true'
```

- [ ] **Step 2: Run packaging test before scripts exist**

Run: `./tests/release/clean-host.sh`

Expected: FAIL because packaging is absent.

- [ ] **Step 3: Implement package, install, uninstall, and release workflow**

Build locked release binaries, strip them, record SHA-256 and source revision, use `pkgbuild`, and leave signing/notarization identities configurable through CI secrets without embedding them. Document Apple `container` installation/service prerequisite, exact security promise, commands, manifest, root model, network modes, data locations, and uninstall semantics.

- [ ] **Step 4: Run the complete release gate on a clean supported Mac**

Run: `./tests/release/clean-host.sh`

Expected final line: `PASS: Gas Can macOS MVP release gate`. The script installs, runs doctor, creates a multi-language sandbox, verifies mise and bundled/local Gascamp, proves offline isolation, restarts daemon/workspace, applies setup, destroys the sandbox, uninstalls binaries, and confirms no test-owned Apple resources remain.

- [ ] **Step 5: Run global verification and commit Roadmap Gate 5**

Run: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo test --workspace && ./tests/security/run.sh && ./tests/release/clean-host.sh`

Expected: all commands exit 0.

```bash
git add packaging tests/release docs/release README.md
git commit -m "release: complete Gas Can macOS MVP gate"
```
