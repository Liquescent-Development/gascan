# Locked Default Workstation Image Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make every approved workspace image contain the guaranteed editors, coding agents, forge clients, languages, build tools, and non-privileged diagnostics at exact locked versions.

**Architecture:** Extend the existing image-input resolver and connected/offline context pipeline with exact workstation artifacts, expand the reviewed Ubuntu package set, and assemble everything into the immutable image. Mutable project tools remain additive and earlier in `PATH`; third-party configuration and cache paths point at managed sandbox volumes.

**Tech Stack:** Ubuntu 24.04 ARM64 snapshot bundle, Rust image tooling, mise system runtimes, a lock-derived npm dependency cache, official GitHub/GitLab release artifacts, Dockerfile multi-stage build, connected and offline image gates.

**Prerequisite:** Complete `docs/superpowers/plans/2026-07-23-image-resolution-upgrades.md`.

**Design:** `docs/superpowers/specs/2026-07-23-default-ssh-workstation-design.md`

## Global Constraints

- The image must be complete before project provisioning and without sandbox network access.
- `versions.toml` may express update intent; `versions.lock` contains every exact resolved version, URL, digest, and platform.
- Docker builds and sandbox startup never resolve `latest`, `stable`, `lts`, tags, redirects to unrecorded assets, or mutable registry metadata.
- Ubuntu packages come only from the existing dated snapshot and reviewed package bundle.
- External tools come only from official upstream artifacts or official package registries with verified SHA-256 evidence.
- No installer is piped into a shell.
- No host or third-party credential may enter the build context or image.
- Guaranteed commands and exact versions are release-blocking image-gate assertions.
- User `[tools]` shims remain earlier in `PATH` and may override immutable defaults.
- Diagnostic packages receive no additional runtime capabilities.

---

### Task 1: Lock Official Workstation Artifacts

**Files:**
- Modify: `images/workspace/versions.toml`
- Modify: `images/workspace/versions.lock`
- Create: `images/workspace/workstation-package.json`
- Create: `images/workspace/workstation-package-lock.json`
- Modify: `scripts/src/bin/update-image-lock.rs`
- Modify: `scripts/tests/image_lock.rs`
- Modify: `scripts/tests/tool_versions.rs`
- Modify: `scripts/tests/update_image_lock.rs`

**Interfaces:**
- Produces: `[workstation]` update intent
- Produces: `[workstation_artifacts.<name>] { version, url, sha256, kind }`
- Produces: `images/workspace/workstation-package.json` and
  `images/workspace/workstation-package-lock.json`
- Produces exact records for `claude`, `codex`, `pi`, `herdr`, `glab`, and
  `neovim`

- [ ] **Step 1: Write failing lock-schema tests**

Add:

```rust
const REQUIRED: &[&str] = &[
    "claude",
    "codex",
    "pi",
    "herdr",
    "glab",
    "neovim",
];
```

For every record assert:

- Nonempty concrete version without `latest`, `stable`, `lts`, or wildcard.
- HTTPS URL on the tool's official registry/release host.
- 64-character lowercase SHA-256.
- Platform is `linux-arm64`.
- Kind is one of `npm_tgz`, `tar_zst`, or `tar_gz`.

Mutation tests must reject an unknown host, redirect outside the allowlist,
missing digest, duplicate tool, wrong architecture, and mutable release URL.

- [ ] **Step 2: Run scripts tests and confirm RED**

Run:

```bash
rtk cargo test --manifest-path scripts/Cargo.toml --test image_lock
rtk cargo test --manifest-path scripts/Cargo.toml --test update_image_lock
```

Expected: workstation lock fields are absent.

- [ ] **Step 3: Add update intent**

Add an explicit source section:

```toml
[workstation]
claude = "latest"
codex = "latest"
pi = "latest"
herdr = "latest"
glab = "latest"
neovim = "0.11"
```

These values are resolver input only. They must not be copied into Docker,
release evidence, or the final lock.

- [ ] **Step 4: Resolve official immutable artifacts**

Extend the updater with:

```rust
#[derive(Serialize)]
struct WorkstationArtifact {
    version: String,
    url: String,
    sha256: String,
    platform: String,
    kind: String,
}
```

Resolution rules:

- Claude Code: official `@anthropic-ai/claude-code` npm tarball.
- Codex: official `@openai/codex` npm tarball.
- Pi: official `@earendil-works/pi-coding-agent` npm tarball.
- Herdr: exact Linux ARM64 asset from
  `github.com/ogulcancelik/herdr/releases`.
- GitLab CLI: exact Linux ARM64 asset from the official GitLab CLI release.
- Neovim: exact Linux ARM64 release archive from `github.com/neovim/neovim`.

Fetch registry/release metadata, select an exact version, then fetch the exact
asset bytes and compute SHA-256 before writing the lock. Record the final asset
URL, not a `latest` URL. Extend the allowlist only with official npm, GitHub,
GitLab, and their documented asset CDN hosts.

For the three npm tools, write a generated package manifest containing exact
versions and an npm lockfile containing the complete transitive dependency
closure and integrity values. Reject lifecycle scripts in every resolved
package unless a package-specific, reviewed exception and sandboxed build step
is added. The lock updater must prove the three top-level versions agree with
the workstation artifact records.

- [ ] **Step 5: Generate and validate the real lock**

Run:

```bash
rtk cargo run --manifest-path scripts/Cargo.toml --bin update-image-lock
rtk cargo test --manifest-path scripts/Cargo.toml --test image_lock
rtk cargo test --manifest-path scripts/Cargo.toml --test tool_versions
rtk cargo test --manifest-path scripts/Cargo.toml --test update_image_lock
```

Expected: the updater writes six concrete workstation records and every test
passes. Review the complete lock diff; no unrelated runtime version may change
without explicit acceptance.

- [ ] **Step 6: Commit**

```bash
rtk git add images/workspace/versions.toml images/workspace/versions.lock \
  images/workspace/workstation-package.json \
  images/workspace/workstation-package-lock.json \
  scripts/src/bin/update-image-lock.rs scripts/tests/image_lock.rs \
  scripts/tests/tool_versions.rs scripts/tests/update_image_lock.rs
rtk git commit -m "build: lock workstation tool artifacts"
```

### Task 2: Expand the Reviewed Ubuntu Developer Package Set

**Files:**
- Modify: `tests/image/system-tools.txt`
- Modify: `images/workspace/bundles/ubuntu-packages.toml`
- Modify: `images/workspace/versions.lock`
- Modify: `scripts/tests/connected_dockerfile.rs`
- Modify: `scripts/tests/image_user_contract.rs`
- Modify: `scripts/tests/ubuntu_package_bundle.rs`

**Interfaces:**
- Consumes: existing Ubuntu snapshot bundle pipeline
- Produces: reviewed packages for editors, OpenSSH, and diagnostics

- [ ] **Step 1: Write the failing exact-package contract**

Require this sorted set in addition to existing packages:

```text
bind9-dnsutils
emacs-nox
fd-find
fzf
iproute2
iputils-ping
less
lsof
nano
net-tools
netcat-openbsd
openssh-client
openssh-server
procps
psmisc
ripgrep
rsync
tmux
traceroute
tree
vim
wget
```

Neovim, Glab, and coding agents are locked external artifacts and must not be
accepted as Ubuntu substitutes.

Add command mapping assertions:

```rust
[
    ("bind9-dnsutils", &["dig", "nslookup"]),
    ("iproute2", &["ip", "ss"]),
    ("iputils-ping", &["ping"]),
    ("net-tools", &["ifconfig", "netstat"]),
    ("procps", &["ps", "top"]),
    ("psmisc", &["pstree"]),
    ("nano", &["nano", "pico"]),
]
```

- [ ] **Step 2: Run package-contract tests and confirm RED**

Run:

```bash
rtk cargo test --manifest-path scripts/Cargo.toml --test connected_dockerfile
rtk cargo test --manifest-path scripts/Cargo.toml --test image_user_contract
rtk cargo test --manifest-path scripts/Cargo.toml --test ubuntu_package_bundle
```

Expected: required packages are missing and the package input hash differs.

- [ ] **Step 3: Update the package input and immutable bundle record**

Add the exact sorted package names, regenerate the Ubuntu package bundle from
the reviewed snapshot, and update:

```toml
system_packages_sha256 = "<sha256 of tests/image/system-tools.txt>"
```

in both the package-bundle record and image lock. The producer must retain
`--no-install-recommends`.

- [ ] **Step 4: Verify command availability without capabilities**

Add image contract checks that every command resolves, while explicitly
asserting the Dockerfile does not add `CAP_NET_RAW`, `CAP_NET_ADMIN`,
`--privileged`, device mounts, or packet-capture packages.

- [ ] **Step 5: Run all package and Dockerfile tests**

Run:

```bash
rtk cargo test --manifest-path scripts/Cargo.toml --test connected_dockerfile
rtk cargo test --manifest-path scripts/Cargo.toml --test image_user_contract
rtk cargo test --manifest-path scripts/Cargo.toml --test ubuntu_package_bundle
```

Expected: all tests pass with the new exact package hash.

- [ ] **Step 6: Commit**

```bash
rtk git add tests/image/system-tools.txt images/workspace/bundles/ubuntu-packages.toml \
  images/workspace/versions.lock scripts/tests/connected_dockerfile.rs \
  scripts/tests/image_user_contract.rs scripts/tests/ubuntu_package_bundle.rs
rtk git commit -m "build: add workstation system packages"
```

### Task 3: Prefetch and Verify Workstation Artifacts

**Files:**
- Modify: `scripts/prefetch-connected-workspace-image.sh`
- Modify: `scripts/src/bin/prepare-workspace-context.rs`
- Modify: `scripts/tests/connected_workspace_context.rs`
- Modify: `scripts/tests/connected_image_build.rs`
- Modify: `scripts/tests/workspace_context.rs`

**Interfaces:**
- Consumes: `[workstation_artifacts]` lock records
- Produces: `.artifacts/connected-workspace-context/workstation/npm-cache/`
- Produces: `.artifacts/connected-workspace-context/workstation/<native-name>`
- Produces exact context-manifest entries with source digest and destination

- [ ] **Step 1: Write failing context-manifest tests**

Assert the connected context contains the three native archives:

```text
workstation/herdr.tar.zst
workstation/glab.tar.gz
workstation/neovim.tar.gz
```

It must also contain the generated npm manifests and an npm cache whose
tarballs are exactly the dependency closure in
`workstation-package-lock.json`. Every native file must match its lock
SHA-256; every npm cache entry must match its lockfile integrity. Reject
absent, extra, symlinked, hard-linked, wrong-sized, wrong-digest, or
path-traversal content.

- [ ] **Step 2: Run context tests and confirm RED**

Run:

```bash
rtk cargo test --manifest-path scripts/Cargo.toml --test connected_workspace_context
rtk cargo test --manifest-path scripts/Cargo.toml --test workspace_context
```

Expected: workstation context entries are absent.

- [ ] **Step 3: Add bounded prefetch**

Download each exact locked native URL and npm lockfile dependency URL with the
existing redirect/host policy and:

```text
connect timeout: 15 seconds
overall timeout: 120 seconds per artifact
maximum artifact size: explicit per-record bound
temporary mode: 0600
final mode: 0444
```

Verify native SHA-256 and npm Subresource Integrity before atomic rename. Build
the npm cache in the connected prefetch context using the exact generated
lockfile with lifecycle scripts disabled. Never invoke an artifact during
prefetch.

- [ ] **Step 4: Extend context preparation**

Copy only validated regular files into the canonical `workstation/` paths and
include them in the context digest. Connected-context verification must fail if
the lock changes after prefetch.

- [ ] **Step 5: Run context and build-input tests**

Run:

```bash
rtk cargo test --manifest-path scripts/Cargo.toml --test connected_workspace_context
rtk cargo test --manifest-path scripts/Cargo.toml --test workspace_context
rtk cargo test --manifest-path scripts/Cargo.toml --test connected_image_build
```

Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
rtk git add scripts/prefetch-connected-workspace-image.sh \
  scripts/src/bin/prepare-workspace-context.rs \
  scripts/tests/connected_workspace_context.rs scripts/tests/connected_image_build.rs \
  scripts/tests/workspace_context.rs
rtk git commit -m "build: prefetch locked workstation artifacts"
```

### Task 4: Assemble the Immutable Workstation Layer

**Files:**
- Modify: `images/workspace/Dockerfile`
- Create: `images/workspace/bin/install-workstation-artifacts`
- Create: `images/workspace/bin/configure-workstation-home`
- Modify: `images/workspace/etc/profile.d/mise.sh`
- Modify: `scripts/tests/connected_dockerfile.rs`
- Modify: `scripts/tests/image_user_contract.rs`
- Modify: `scripts/tests/polyglot_image_contract.rs`

**Interfaces:**
- Consumes: verified `workstation/*` context artifacts
- Produces: guaranteed commands and persistent config/cache path contract

- [ ] **Step 1: Write failing Dockerfile and installer tests**

Assert:

- npm packages are installed from the complete locked local cache with
  `npm ci --offline --ignore-scripts`; no dependency may be resolved or
  downloaded during the Docker build.
- Herdr, Glab, Neovim archives are extracted only after path/type validation.
- Installed files are root-owned, non-writable, and beneath `/opt/gascan` or
  `/usr/local/bin`.
- `fd` resolves to reviewed `fdfind`.
- `pico` resolves to reviewed Nano.
- No curl, wget, npm registry, GitHub, or GitLab access occurs in Docker build.

- [ ] **Step 2: Run Dockerfile contracts and confirm RED**

Run:

```bash
rtk cargo test --manifest-path scripts/Cargo.toml --test connected_dockerfile
rtk cargo test --manifest-path scripts/Cargo.toml --test image_user_contract
```

Expected: installer scripts and guaranteed commands do not exist.

- [ ] **Step 3: Add a strict artifact installer**

`install-workstation-artifacts` accepts the lock-derived expected versions as
arguments and:

1. Rejects archives containing absolute paths, `..`, devices, sockets, or
   escaping symlinks.
2. Runs `npm ci --offline --ignore-scripts` against the generated package and
   lock files using only the verified local cache, then copies the resulting
   immutable dependency tree and command shims into the system prefix.
3. Installs the three native ARM64 commands from validated archives.
4. Creates only reviewed compatibility links for `fd` and `pico`.
5. Verifies exact `--version` output for all six external tools.
6. Removes package-manager cache and temporary artifacts.

The script uses `set -eu` and never evaluates artifact-provided shell text.

- [ ] **Step 4: Configure persistent third-party paths**

`configure-workstation-home` creates image-owned links or directories so:

```text
~/.claude  -> ~/.config/gascan/agents/claude
~/.codex   -> ~/.config/gascan/agents/codex
~/.pi      -> ~/.config/gascan/agents/pi
```

Use documented configuration-directory environment variables for Herdr, `gh`,
and `glab` where supported. Route caches/logs to
`/home/workspace/.cache/<tool>`. Do not create credentials or placeholder
tokens.

The script must be idempotent and refuse a preexisting non-owned file at a
managed link path.

- [ ] **Step 5: Wire the Dockerfile**

Copy verified artifacts and scripts, run installation as root, run path
configuration after workspace identity migration, and enforce:

```text
root ownership for immutable tools
workspace ownership for empty managed target directories
0555 executables
0444 metadata
no group/other-writable files
```

Update `CONTAINER_PATH` only if necessary; mutable mise shims remain first.

- [ ] **Step 6: Run static image contracts**

Run:

```bash
rtk cargo test --manifest-path scripts/Cargo.toml --test connected_dockerfile
rtk cargo test --manifest-path scripts/Cargo.toml --test image_user_contract
rtk cargo test --manifest-path scripts/Cargo.toml --test polyglot_image_contract
```

Expected: all tests pass.

- [ ] **Step 7: Commit**

```bash
rtk git add images/workspace/Dockerfile images/workspace/bin/install-workstation-artifacts \
  images/workspace/bin/configure-workstation-home images/workspace/etc/profile.d/mise.sh \
  scripts/tests/connected_dockerfile.rs scripts/tests/image_user_contract.rs \
  scripts/tests/polyglot_image_contract.rs
rtk git commit -m "feat: assemble default workstation image"
```

### Task 5: Add Exact Image and Live Workstation Gates

**Files:**
- Create: `images/workspace/tests/workstation-contract.sh`
- Modify: `scripts/run-connected-image-gate.sh`
- Modify: `scripts/tests/connected_image_gate.rs`
- Modify: `crates/gascan-e2e/tests/apple_apply.rs`
- Modify: `crates/gascan-e2e/tests/apple_common/mod.rs`
- Modify: `images/workspace/approved-image.txt`
- Modify: `images/workspace/versions.lock`
- Modify: `docs/evidence/connected-workspace-image.md`
- Modify: `README.md`

**Interfaces:**
- Consumes: complete workstation image
- Produces: exact release-blocking command/version and persistence evidence

- [ ] **Step 1: Add a credential-free workstation smoke**

The guest script must verify the exact lock-derived inventory and invoke:

```text
vim --version
nvim --version
emacs --version
pico --version
claude --version
codex --version
pi --version
herdr --version
go version
rustc --version
cargo --version
gh --version
glab --version
git --version
ip -Version
ss --version
ping -V
ifconfig --version
netstat --version
dig -v
traceroute --version
nc -h
rg --version
fd --version
fzf --version
tmux -V
```

Normalize only documented formatting differences. Never accept merely nonempty
output for a version-locked command.

- [ ] **Step 2: Add path and credential assertions**

Prove:

- Every agent/forge config path resolves inside the config volume.
- Cache paths resolve inside the cache volume.
- No host home, `.ssh`, agent token, forge token, Docker socket, or host
  keychain path is readable.
- Default tools run in an offline sandbox.
- User `[tools]` can override one immutable mise runtime without mutating
  `/opt/gascan`.

- [ ] **Step 3: Run static and scripts verification**

Run:

```bash
rtk cargo fmt --all -- --check
rtk cargo clippy --workspace --all-targets --all-features -- -D warnings
rtk cargo test --workspace
rtk cargo test --manifest-path scripts/Cargo.toml
rtk git diff --check
```

Expected: all tests pass.

- [ ] **Step 4: Build and run connected image gate**

Run:

```bash
rtk cargo run --manifest-path scripts/Cargo.toml --bin update-image-lock
rtk bash ./scripts/prefetch-connected-workspace-image.sh
rtk bash ./scripts/build-connected-workspace-image.sh
rtk bash ./scripts/run-connected-image-gate.sh
```

Expected: the gate publishes no approval until exact workstation smoke,
polyglot smoke, credential isolation, context digest, and cleanup pass.

- [ ] **Step 5: Run live Apple workstation acceptance**

Run:

```bash
rtk bash ./scripts/apple-test-preflight.sh
rtk cargo test -p gascan-e2e --test apple_apply workstation -- --ignored --nocapture
```

Expected: a new offline sandbox runs every guaranteed command with no download,
then a networked sandbox authenticates no accounts but retains synthetic
configuration sentinels across down/up and image replacement. Exact test-owned
resource inventory is empty afterward.

- [ ] **Step 6: Update README**

Add the guaranteed command list, exact version-discovery commands, immutable
default versus `[tools]` override behavior, credential persistence/destruction,
and the statement that installed diagnostics do not add capabilities.

- [ ] **Step 7: Commit**

```bash
rtk git add images/workspace/tests/workstation-contract.sh \
  scripts/run-connected-image-gate.sh scripts/tests/connected_image_gate.rs \
  crates/gascan-e2e/tests/apple_apply.rs crates/gascan-e2e/tests/apple_common/mod.rs \
  README.md images/workspace/approved-image.txt images/workspace/versions.lock \
  docs/evidence/connected-workspace-image.md
rtk git commit -m "feat: approve default workstation image"
```
