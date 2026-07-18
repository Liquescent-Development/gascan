# Offline Workspace Image Bundles Implementation Plan

> **Status as of 2026-07-15:** Deferred hardening, not a macOS MVP
> prerequisite. Tasks 1–6 and the PENDING Task 7 scaffold have reviewed
> implementations on `feature/provisioning`; the three bundles are not
> published and no live offline image evidence exists. The connected MVP build
> decision is recorded in
> `docs/superpowers/specs/2026-07-15-connected-mvp-build-design.md`. Do not
> resume this plan or treat `9025c56` as Gate evidence without an explicit
> roadmap decision.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Produce the locked Linux ARM64 workspace inputs in connected CI, verify them on macOS, and make Apple `container build` a network-independent assembly step.

**Architecture:** A connected Linux ARM64 GitHub Actions workflow produces three deterministic artifacts: an Ubuntu package closure, a complete mise runtime tree, and exact Gascamp source plus Cargo vendor content. The macOS build downloads immutable release assets through the existing bounded fetcher, validates outer and inner manifests, assembles an allowlisted temporary context, proves the exact base digest is local, and invokes an offline-only Dockerfile.

**Tech Stack:** Rust 1.85+ image tooling, GitHub Actions ARM64 runners, Ubuntu snapshot metadata and `dpkg`, mise 2026.5.0, Cargo vendor, tar/zstd archives, Apple `container` 1.1.0.

## Global Constraints

- `container build` performs no DNS, HTTP, Git, package resolution, mise installation, Cargo fetch, or implicit base-image pull.
- Every bundle targets `linux/arm64`, has a lowercase SHA-256, byte size, canonical sorted inner manifest, and producer provenance.
- Ubuntu packages come only from `2026-07-13T00:00:00Z`; signed metadata is verified with the Ubuntu archive key before dependency resolution.
- Mise is exactly `2026.5.0` and installs exactly Node, Python, Go, Rust, Java, Ruby, and Elixir at the versions in `images/workspace/versions.lock`.
- Gascamp source is exactly commit `f6b248c5926240856dbea83d1d2c5c90ea1c1456`; Cargo builds use `--locked --offline --frozen`.
- Archives reject traversal, absolute paths, duplicate entries, device nodes, and symlink or hardlink escape before extraction.
- The Apple build context contains only an explicit allowlist; repository contents and user artifacts are never sent wholesale.
- Missing, corrupt, wrong-platform, unsigned, or incomplete inputs fail before `container build`.
- Production Rust denies unsafe code, unwraps, expects, and panics.

---

### Task 1: Define and validate the immutable bundle contract

**Files:**
- Modify: `images/workspace/versions.lock`
- Create: `scripts/src/bundle.rs`
- Create: `scripts/src/bin/validate-workspace-bundle.rs`
- Modify: `scripts/src/lib.rs`
- Modify: `scripts/Cargo.toml`
- Test: `scripts/tests/workspace_bundle.rs`
- Test: `scripts/tests/image_lock.rs`

**Interfaces:**
- Produces: `BundleLock { url, sha256, size, media_type, platform }` entries named `ubuntu_packages`, `mise_runtimes`, and `gascamp_source_vendor`.
- Produces: `validate_bundle(lock, archive, destination) -> Result<BundleEvidence, BundleError>`.

- [ ] **Step 1: Add failing schema and adversarial archive tests**

Require all three records, `platform = "linux/arm64"`, media type `application/vnd.gascan.workspace-bundle.v1+tar.zstd`, exact size, and lowercase 64-hex hashes. Test traversal, absolute paths, duplicates, device nodes, escaping links, truncation, extra/missing manifest entries, and per-file hash mismatch.

- [ ] **Step 2: Verify RED**

Run: `cargo test --manifest-path scripts/Cargo.toml --test workspace_bundle --test image_lock`

Expected: FAIL because bundle records and validator are absent.

- [ ] **Step 3: Implement the typed manifest and fail-closed extractor**

Use `tar` and `zstd` crates with no shell extraction. Parse `bundle-manifest.json` first, require sorted unique relative UTF-8 paths, regular files/directories or confined relative symlinks only, stream-hash every regular file, and atomically rename a completed temporary destination.

- [ ] **Step 4: Verify GREEN and lint**

Run: `cargo test --manifest-path scripts/Cargo.toml --test workspace_bundle --test image_lock && cargo clippy --manifest-path scripts/Cargo.toml --all-targets -- -D warnings`

Expected: PASS; every malformed fixture returns a stable typed error.

- [ ] **Step 5: Commit the bundle contract**

```bash
git add images/workspace/versions.lock scripts
git commit -m "build: define verified workspace bundle contract"
```

### Task 2: Produce the signed Ubuntu ARM64 package closure in CI

**Files:**
- Create: `images/workspace/bundles/ubuntu-packages.toml`
- Create: `scripts/produce-ubuntu-package-bundle.sh`
- Create: `.github/workflows/workspace-bundles.yml`
- Test: `scripts/tests/ubuntu_package_bundle.rs`

**Interfaces:**
- Consumes: exact snapshot timestamp, base digest, `tests/image/system-tools.txt`, and builder packages `build-essential`, `ca-certificates`, `git`, `libssl-dev`, `pkg-config`.
- Produces: deterministic local apt repository plus signed-metadata evidence and exact package/version/architecture/SHA-256 manifest.

- [ ] **Step 1: Add fixture tests for closure verification**

Test wrong signing-key fingerprint, invalid InRelease signature, hash mismatch, non-ARM64 package, missing dependency, version ambiguity, inclusion of Recommends, and nondeterministic ordering.

- [ ] **Step 2: Verify RED**

Run: `cargo test --manifest-path scripts/Cargo.toml --test ubuntu_package_bundle`

Expected: FAIL because producer evidence validation is absent.

- [ ] **Step 3: Implement the connected ARM64 producer**

Run only on `ubuntu-24.04-arm` in GitHub Actions. Verify snapshot InRelease with `/usr/share/keyrings/ubuntu-archive-keyring.gpg`, resolve the union closure using `--no-install-recommends`, download every `.deb`, verify Packages hashes, generate a local signed-index-preserving repository, and normalize archive uid/gid/mtime/order.

- [ ] **Step 4: Publish immutable workflow artifacts**

The workflow uploads `ubuntu-packages-linux-arm64.tar.zst`, its SHA-256, byte size, inner manifest, and provenance. Release publication is keyed by the `versions.lock` input digest; existing assets with different bytes fail rather than overwrite.

- [ ] **Step 5: Verify producer fixtures and shell syntax**

Run: `cargo test --manifest-path scripts/Cargo.toml --test ubuntu_package_bundle && bash -n scripts/produce-ubuntu-package-bundle.sh`

Expected: PASS.

- [ ] **Step 6: Commit the package producer**

```bash
git add .github images/workspace/bundles scripts
git commit -m "build: produce locked Ubuntu package bundle"
```

### Task 3: Produce the exact mise runtime tree in CI

**Files:**
- Create: `scripts/produce-mise-runtime-bundle.sh`
- Create: `scripts/tests/mise_runtime_bundle.rs`
- Modify: `.github/workflows/workspace-bundles.yml`

**Interfaces:**
- Produces: `/opt/gascan/mise` archive, exact seven-key `mise current --json`, canonical file manifest, config/mise/base digests, and upstream artifact provenance.

- [ ] **Step 1: Add failing provenance and tree tests**

Reject wrong platform, mise/config digest mismatch, missing or extra tool, version mismatch, missing executable, writable root-owned evidence, unsafe archive entry, and unsorted manifest.

- [ ] **Step 2: Verify RED**

Run: `cargo test --manifest-path scripts/Cargo.toml --test mise_runtime_bundle`

Expected: FAIL because runtime bundle validation is absent.

- [ ] **Step 3: Implement the connected ARM64 producer**

Use the verified mise 2026.5.0 binary and exact config, set `MISE_DATA_DIR=/opt/gascan/mise`, install only the seven locked tools, capture `mise current --json`, remove download caches, normalize ownership/modes/timestamps, and emit provenance for every backend download.

- [ ] **Step 4: Publish and verify deterministically**

Run producer twice from the same base digest and compare canonical manifests and archive hashes before publication.

- [ ] **Step 5: Run tests and commit**

Run: `cargo test --manifest-path scripts/Cargo.toml --test mise_runtime_bundle && bash -n scripts/produce-mise-runtime-bundle.sh`

```bash
git add .github scripts
git commit -m "build: produce locked mise runtime bundle"
```

### Task 4: Produce exact Gascamp source and Cargo vendor content

**Files:**
- Create: `scripts/produce-gascamp-bundle.sh`
- Create: `scripts/tests/gascamp_bundle.rs`
- Modify: `.github/workflows/workspace-bundles.yml`

**Interfaces:**
- Produces: canonical exact-commit source tree, `cargo vendor --locked` closure, source replacement config, tree digest, inner manifest, and provenance.

- [ ] **Step 1: Add failing source/vendor tests**

Reject commit/tree mismatch, dirty or extra source, submodule ambiguity, altered or missing vendored crate, unlocked Git dependency, absent Cargo checksum, and registry-enabled Cargo config.

- [ ] **Step 2: Verify RED**

Run: `cargo test --manifest-path scripts/Cargo.toml --test gascamp_bundle`

Expected: FAIL because Gascamp bundle validation is absent.

- [ ] **Step 3: Implement the connected producer**

Fetch only commit `f6b248c5926240856dbea83d1d2c5c90ea1c1456`, verify HEAD and clean tree, reject submodules unless individually locked, run `cargo vendor --locked`, generate `.cargo/config.toml` replacing crates-io with `vendor`, and normalize the archive.

- [ ] **Step 4: Prove offline completeness**

In the connected ARM64 job, disable network after bundle creation and run `cargo test --locked --offline --frozen` and `cargo build --locked --offline --frozen --release --bin camp`. Remove one vendored crate in a negative fixture and require prompt failure without network access.

- [ ] **Step 5: Run tests and commit**

Run: `cargo test --manifest-path scripts/Cargo.toml --test gascamp_bundle && bash -n scripts/produce-gascamp-bundle.sh`

```bash
git add .github scripts
git commit -m "build: vendor pinned Gascamp for offline builds"
```

### Task 5: Prefetch verified bundles and assemble a minimal context

**Files:**
- Create: `scripts/src/bin/prepare-workspace-context.rs`
- Create: `scripts/prefetch-workspace-image.sh`
- Modify: `scripts/build-workspace-image.sh`
- Test: `scripts/tests/workspace_context.rs`
- Test: `scripts/tests/artifact_redirect.rs`

**Interfaces:**
- Produces: `.artifacts/workspace-context/` containing only Dockerfile, reviewed static files, expected tool evidence, and three verified extracted bundles.
- Build command consumes cache only; `prefetch-workspace-image.sh` is the sole normal-build network entry point.
- Prefetch pulls only the exact locked base digest through Apple `container image pull`, then requires structured `linux/arm64` digest inspection before publishing the prepared context.

- [ ] **Step 1: Add failing cache/context tests**

Test missing/corrupt bundle, wrong size/hash/platform, cached corruption, failed refresh preserving prior valid cache, unapproved redirect, extra repository file, symlink in context, and nondeterministic manifest.

- [ ] **Step 2: Verify RED**

Run: `cargo test --manifest-path scripts/Cargo.toml --test workspace_context --test artifact_redirect`

Expected: FAIL because the preparer and bundle fetch contract are absent.

- [ ] **Step 3: Implement bounded prefetch and atomic context assembly**

Extend the existing fetcher with code-owned host allowlists and exact size limits. Revalidate warm-cache bytes on every use. Assemble a new temporary directory from an explicit path allowlist, write a sorted context manifest, make files read-only where applicable, then atomically publish it.

Pull `ubuntu@sha256:7f622ca8766bccb22f04242ecb6f19f770b2f08827dc4b8c707de5e78a6da7ab` during explicit prefetch and validate its structured inspect result with the existing image-inspect validator. Never pull a tag or allow `container build` to acquire the base implicitly.

- [ ] **Step 4: Make build fail before Apple invocation on incomplete input**

Remove all downloading from `build-workspace-image.sh`; require the verified context and exact locally inspected `ubuntu@sha256:7f622ca8766bccb22f04242ecb6f19f770b2f08827dc4b8c707de5e78a6da7ab`. A missing base or bundle exits before `container build`.

- [ ] **Step 5: Verify and commit**

Run: `cargo test --manifest-path scripts/Cargo.toml --test workspace_context --test artifact_redirect && bash -n scripts/prefetch-workspace-image.sh scripts/build-workspace-image.sh`

```bash
git add scripts
git commit -m "build: prepare verified offline image context"
```

### Task 6: Convert the Dockerfile to network-independent assembly

**Files:**
- Modify: `images/workspace/Dockerfile`
- Modify: `scripts/tests/image_lock.rs`
- Modify: `scripts/tests/polyglot_image_contract.rs`
- Modify: `scripts/tests/image_user_contract.rs`
- Test: `scripts/tests/offline_dockerfile.rs`

**Interfaces:**
- Consumes: only the minimal prepared context.
- Produces: final ARM64 image with exact package inventory, runtime evidence, and offline-built Gascamp.

- [ ] **Step 1: Add a failing no-network Dockerfile contract**

Reject `http://`, `https://`, `apt-get update`, remote apt sources, `git fetch`, `git clone`, `mise install`, `cargo fetch`, `curl`, and `wget`. Require local package installation with network methods disabled, copied mise tree, and `cargo test/build --locked --offline --frozen`.

- [ ] **Step 2: Verify RED**

Run: `cargo test --manifest-path scripts/Cargo.toml --test offline_dockerfile`

Expected: FAIL on current in-build network operations.

- [ ] **Step 3: Implement offline assembly**

Install only copied verified `.deb` files with remote sources removed and apt acquire retries zero; copy the verified mise tree and compare exact tool evidence; copy Gascamp source/vendor content and run offline frozen tests/build; retain workspace user, sudo, tini, Chromium, and selector contracts.

- [ ] **Step 4: Verify structural and tooling suites**

Run: `cargo test --manifest-path scripts/Cargo.toml && cargo clippy --manifest-path scripts/Cargo.toml --all-targets -- -D warnings`

Expected: PASS with no network-capable Dockerfile token.

- [ ] **Step 5: Commit offline Dockerfile**

```bash
git add images/workspace scripts/tests
git commit -m "build: assemble workspace image without network"
```

### Task 7: Prove cold and warm offline builds on Apple

**Files:**
- Create: `scripts/run-offline-image-gate.sh`
- Create: `docs/evidence/offline-image-build.md`
- Modify: `tests/image/user-and-volumes.sh`
- Modify: `tests/image/polyglot-smoke.sh`
- Modify: `tests/image/gascamp-smoke.sh`

**Interfaces:**
- Produces: sanitized Gate evidence for cold host prefetch, network-independent Apple assembly, warm-cache no-fetch rebuild, image digest/platform, and all live smokes.

- [ ] **Step 1: Add fail-closed gate harness tests**

Require cleanup ownership tokens, exact image-reference handoff, nonzero exit on missing evidence, and no current-prefix containers after failure or signal.

- [ ] **Step 2: Prefetch from an empty host cache**

Run: `./scripts/prefetch-workspace-image.sh`

Expected: every artifact is downloaded once, verified, and recorded; no Apple build begins during prefetch.

- [ ] **Step 3: Build with public VM egress unavailable**

Run: `./scripts/run-offline-image-gate.sh cold`

Expected: image build passes without network-capable steps; platform is `linux/arm64`; user, volume, seven-runtime, browser, `camp`, `campd`, and selector smokes pass.

- [ ] **Step 4: Prove warm-cache no-fetch behavior and corruption failure**

Run: `./scripts/run-offline-image-gate.sh warm`

Expected: no host HTTP request, unchanged artifact hashes, successful rebuild. Corrupt each bundle fixture in turn and expect failure before Apple build invocation.

- [ ] **Step 5: Record evidence and commit**

```bash
git add scripts tests/image docs/evidence/offline-image-build.md
git commit -m "test: prove network-independent workspace image build"
```
