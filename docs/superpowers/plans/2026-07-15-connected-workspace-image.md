# Connected Workspace Image Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

> **Roadmap role:** This is a focused Plan 4 continuation addendum, not a new
> product plan, phase, or parallel roadmap. It replaces the offline-only image
> assumption inside the already-approved Workspace Environment and Release
> Plan after the 2026-07-15 firewall diagnosis. It must be read with that plan
> and the coordinated roadmap.

> **Execution state:** Durable planning only. No task in this document has
> started. The user selected subagent-driven execution for the next session
> that explicitly resumes implementation; do not infer authorization to begin
> merely from discovering this file.

**Goal:** Build and smoke-test the locked Gas Can `linux/arm64` workspace image with Apple Containerization 1.1 using connected public acquisition and a non-persistent private Gascamp build secret, then hand its exact digest-qualified reference to Roadmap Gate 4.

**Architecture:** Keep the reviewed offline-bundle path as a deferred, separate entrypoint, but make the MVP entrypoint a connected build. The host verifies small immutable artifacts and the base image; the Dockerfile uses signed Ubuntu repositories, exact mise configuration, locked runtime versions, and a BuildKit secret mount for the pinned private Gascamp revision. A live image gate accepts only the reference emitted by structured image inspection and runs all existing owner-scoped smoke tests before producing evidence.

**Tech Stack:** Rust 1.95+ test utilities, Bash with `set -euo pipefail`, Apple `container` CLI 1.1.0, BuildKit Dockerfile secret mounts, Ubuntu 24.04 ARM64, mise 2026.5.0, Cargo locked builds, Git, TOML, jq.

## Global Constraints

- The target is Apple silicon macOS 26+ with Apple `container` CLI exactly compatible with the Gate 2 fixture; current reviewed controller version is 1.1.0.
- The image platform is exactly `linux/arm64`; the Ubuntu base is selected by immutable digest, never by a mutable tag.
- Builder egress denial is not an MVP requirement. Runtime offline isolation remains mandatory and unchanged.
- The MVP build must not require `workspace_bundles.publication = "published"` or any offline bundle.
- The existing offline bundle producers, validators, snapshot helper, and PENDING evidence remain truthful deferred work; do not delete their commits or claim their gate.
- Ubuntu package metadata is authenticated by the Ubuntu archive keyring. Bootstrap may use the distribution's signed HTTP apt source; general artifact downloads use HTTPS and locked digests where the upstream publishes stable bytes.
- Every mise tool version exactly matches `images/workspace/versions.lock` and `images/workspace/etc/mise/config.toml`; `latest`, `stable`, `lts`, and wildcard selectors are forbidden.
- Gascamp is exactly revision `f6b248c5926240856dbea83d1d2c5c90ea1c1456`. Cargo commands use `--locked`. The private read token originates in an owner-only file outside the repository. For Apple Containerization 1.1.0, Gas Can stages a `0600` copy beneath a fresh `0700` temporary host context and supplies that copy only through a BuildKit secret mount.
- The staged secret path is excluded by `.dockerignore` and must not enter the transmitted Docker build context. The token value must never appear in Dockerfile arguments, environment declarations, transmitted context content, image history, image filesystem, build transcript, evidence, or process command text constructed by Gas Can.
- The final image default is `workspace:workspace` UID/GID 1000. Guest root remains available through the reviewed passwordless sudo policy. tini, volumes, Chromium, selector, ownership, and read-only bundled-tool contracts remain unchanged.
- Every live test creates exact token-owned names, validates ownership before mutation, handles `INT` and `TERM`, bounds waits, and proves no current-token resource remains.
- Gate 4 remains pending until its complete real CLI lifecycle passes; a successful image gate is not Gate 4 evidence.

---

### Task 1: Prove Apple BuildKit secret mounts and non-retention

**Files:**
- Create: `scripts/probe-apple-build-secret.sh`
- Create: `scripts/tests/apple_build_secret.rs`
- Create: `docs/evidence/apple-build-secret.md`

**Interfaces:**
- Consumes: `GASCAN_TEST_SECRET_FILE`, an absolute current-UID regular file outside the repository with mode `0600` and a synthetic test value.
- Produces: a fresh `0700` host context containing a `0600` staged copy at `.build-secrets/gascamp_read_token`, a `.dockerignore` entry that excludes `.build-secrets`, reviewed command shape `container build --secret id=gascamp_read_token,src=$staged_secret`, and evidence that the value is absent from the transmitted context while `/run/secrets/gascamp_read_token` exists only for the mounted `RUN` instruction.

- [ ] **Step 1: Write the failing secret-probe contract**

Create `scripts/tests/apple_build_secret.rs` with tests that require all of these literal safeguards in the probe:

```rust
#[test]
fn probe_requires_private_external_file_and_checks_non_retention() {
    let probe = include_str!("../probe-apple-build-secret.sh");
    for required in [
        "test \"$uid\" = \"$(stat -f %u \"$secret\")\"",
        "test \"600\" = \"$(stat -f %Lp \"$secret\")\"",
        "printf '%s\\n' '.build-secrets' >\"$context/.dockerignore\"",
        "--secret \"id=gascamp_read_token,src=$staged_secret\"",
        "RUN --mount=type=secret,id=gascamp_read_token,required=true",
        "test ! -e /run/secrets/gascamp_read_token",
        "container image inspect --format json",
    ] {
        assert!(probe.contains(required), "missing secret safeguard: {required}");
    }
    assert!(probe.contains("EXPECTED_SECRET_SHA256"));
    for forbidden in ["GASCAMP_READ_TOKEN=", "ENV GASCAMP", "ARG GASCAMP"] {
        assert!(!probe.contains(forbidden), "secret-bearing channel: {forbidden}");
    }
}
```

Add a fake `container` test that records arguments, returns a structured inspect fixture, models `.dockerignore` processing, and asserts the synthetic secret value never occurs in argv, stdout, stderr, the generated Dockerfile, or the retained transmitted-context fixture. It must also prove that the staged source is beneath the private host context and is removed by cleanup.

- [ ] **Step 2: Run the focused test and verify RED**

Run:

```sh
cargo test --manifest-path scripts/Cargo.toml --test apple_build_secret
```

Expected: FAIL because `scripts/probe-apple-build-secret.sh` does not exist.

- [ ] **Step 3: Implement the fail-closed probe**

The script must:

1. canonicalize the supplied path and reject repository descendants;
2. verify current UID ownership, `0600`, regular-file type, and one nonempty line;
3. create a private `0700` temporary context;
4. copy the secret to `.build-secrets/gascamp_read_token` with mode `0600`, write an exact `.dockerignore` exclusion for `.build-secrets`, and reject a symlink or ownership/mode mismatch after staging;
5. prove the staged secret is absent from a separately captured representation of the transmitted context before invoking the builder;
6. build a two-step synthetic Dockerfile whose first step compares a SHA-256 supplied separately from the secret and whose second step proves the mount is absent;
7. inspect the resulting image structurally;
8. search the transmitted-context fixture, captured transcript, image history/inspect JSON, and an exported stopped test container filesystem for the synthetic value;
9. clean only its token-owned container, image tag, staged secret, and private context through traps on success, failure, `INT`, and `TERM`.

The host context directory itself contains the staged secret because Apple
BuildKit 0.12.0 rejects external `src` paths. Acceptance depends on proving
that `.dockerignore` prevents the staged path from entering the transmitted
Docker build context; merely placing the file beneath the host context is not
sufficient evidence.

The Dockerfile fragment is:

```Dockerfile
FROM ubuntu@sha256:7f622ca8766bccb22f04242ecb6f19f770b2f08827dc4b8c707de5e78a6da7ab
ARG EXPECTED_SECRET_SHA256
RUN --mount=type=secret,id=gascamp_read_token,required=true \
    test "$(sha256sum /run/secrets/gascamp_read_token | cut -d' ' -f1)" = "$EXPECTED_SECRET_SHA256"
RUN test ! -e /run/secrets/gascamp_read_token
```

- [ ] **Step 4: Run unit and live proof**

Run:

```sh
cargo test --manifest-path scripts/Cargo.toml --test apple_build_secret
secret_file=$(mktemp /tmp/gascan-secret-probe.XXXXXX)
chmod 0600 "$secret_file"
openssl rand -hex 32 >"$secret_file"
GASCAN_TEST_SECRET_FILE="$secret_file" ./scripts/probe-apple-build-secret.sh
rm -f "$secret_file"
```

Expected: tests PASS; live probe records `PASS` without printing the secret; no probe container remains.

- [ ] **Step 5: Record sanitized evidence and commit**

Record Apple CLI version, macOS version, architecture, probe image digest, checks performed, and cleanup result in `docs/evidence/apple-build-secret.md`. Do not record the secret or absolute private path.

```sh
git add scripts/probe-apple-build-secret.sh scripts/tests/apple_build_secret.rs docs/evidence/apple-build-secret.md
git commit -m "test: prove Apple build secret isolation"
```

### Task 2: Define the connected lock and acquisition boundary

**Files:**
- Modify: `images/workspace/versions.lock`
- Create: `scripts/prefetch-connected-workspace-image.sh`
- Modify: `scripts/src/bin/prepare-workspace-context.rs`
- Modify: `scripts/tests/image_lock.rs`
- Create: `scripts/tests/connected_workspace_context.rs`

**Interfaces:**
- Consumes: the immutable base-image digest, mise and Chromium URL/digest records, exact tool versions, and Gascamp revision from `versions.lock`.
- Produces: `.artifacts/connected-workspace-context` plus a canonical manifest digest; it contains reviewed public artifacts and source files but no offline bundles or private token.

- [ ] **Step 1: Write RED lock and context tests**

Add a top-level mode without changing the deferred bundle record:

```toml
workspace_build_mode = "connected"
```

The lock test must accept exactly `connected`, reject other values, and continue validating `workspace_bundles.publication = "pending"` as deferred state. The context test must require this exact allowlist:

```rust
let required = [
    "Dockerfile",
    ".artifacts/mise-linux-arm64",
    ".artifacts/playwright-chromium-reviewed",
    ".artifacts/expected-tool-versions.json",
    "images/workspace/bin",
    "images/workspace/etc",
    "images/workspace/tests",
    "images/workspace/versions.lock",
    "tests/image/system-tools.txt",
];
```

Assert that `bundles/`, `.git/`, token-like filenames, symlinks, sockets, and files outside the allowlist fail before context publication.

- [ ] **Step 2: Verify RED**

Run:

```sh
cargo test --manifest-path scripts/Cargo.toml --test image_lock --test connected_workspace_context
```

Expected: FAIL because connected mode and context preparation are absent.

- [ ] **Step 3: Implement connected prefetch**

`scripts/prefetch-connected-workspace-image.sh` must:

- parse the lock through Rust/TOML code rather than shell substring trust;
- call the existing bounded `fetch-image-artifact` for mise and Chromium;
- run `extract-reviewed-chromium` and `validate-tool-versions`;
- pull and structurally inspect the exact base image;
- invoke `prepare-workspace-context --mode connected --replace`;
- atomically publish the context and print only its canonical manifest digest;
- never read or require `GASCAMP_READ_TOKEN_FILE`.

Extend `prepare-workspace-context` with an explicit `Connected` mode rather than weakening the offline allowlist.

- [ ] **Step 4: Run focused and regression tests**

```sh
cargo test --manifest-path scripts/Cargo.toml --test image_lock --test connected_workspace_context --test workspace_context --test artifact_redirect
bash -n scripts/prefetch-connected-workspace-image.sh
```

Expected: PASS; existing offline context tests remain PASS.

- [ ] **Step 5: Commit**

```sh
git add images/workspace/versions.lock scripts/prefetch-connected-workspace-image.sh scripts/src/bin/prepare-workspace-context.rs scripts/tests/image_lock.rs scripts/tests/connected_workspace_context.rs
git commit -m "build: prepare locked connected image context"
```

### Task 3: Restore connected Ubuntu and mise assembly

**Files:**
- Modify: `images/workspace/Dockerfile`
- Replace: `scripts/tests/offline_dockerfile.rs` with `scripts/tests/connected_dockerfile.rs`
- Modify: `scripts/tests/polyglot_image_contract.rs`
- Modify: `scripts/tests/image_user_contract.rs`
- Modify: `tests/image/system-tools.txt`

**Interfaces:**
- Consumes: the Task 2 minimal connected context.
- Produces: a `workspace-base` stage with exact system tools, mise, seven exact runtimes, Chromium inputs, sudo/tini, and no Gascamp credential handling.

- [ ] **Step 1: Write the failing connected Dockerfile contract**

The new test must require:

```rust
for required in [
    "FROM ${BASE_IMAGE} AS workspace-base",
    "apt-get -o Acquire::Retries=0 update",
    "install --yes --no-install-recommends",
    "rm -rf /var/lib/apt/lists/*",
    "COPY --chmod=0555 .artifacts/mise-linux-arm64 /usr/local/bin/mise",
    "mise install --yes",
    "mise current --json",
    "cmp /tmp/resolved-tool-versions.json /tmp/expected-tool-versions.json",
] {
    assert!(dockerfile.contains(required), "missing connected contract: {required}");
}
for forbidden in [
    "bundles/ubuntu_packages",
    "bundles/mise_runtimes",
    "Dir::Bin::methods=/nonexistent",
    "apt-get upgrade",
    "latest",
] {
    assert!(!dockerfile.contains(forbidden), "deferred/unlocked path: {forbidden}");
}
```

Parse `tests/image/system-tools.txt` and require the Dockerfile to install that exact sorted unique set, with no inline unreviewed package names.

- [ ] **Step 2: Verify RED**

```sh
cargo test --manifest-path scripts/Cargo.toml --test connected_dockerfile --test polyglot_image_contract --test image_user_contract
```

Expected: FAIL on the existing offline stages.

- [ ] **Step 3: Implement `workspace-base`**

Use the immutable `BASE_IMAGE`. Copy `system-tools.txt`, run signed apt update/install with retries zero, verify every requested package is installed, then remove apt lists. Copy the digest-verified mise binary and exact config, run `mise install --yes`, record `mise current --json`, normalize it through jq, compare it to `.artifacts/expected-tool-versions.json`, and store the root-owned `0444` evidence file.

Use this package-install shape so the reviewed file is the sole package list:

```Dockerfile
COPY --chmod=0444 tests/image/system-tools.txt /tmp/system-tools.txt
RUN test "$(LC_ALL=C sort -u /tmp/system-tools.txt | wc -l | tr -d ' ')" = "$(wc -l < /tmp/system-tools.txt | tr -d ' ')" \
    && apt-get -o Acquire::Retries=0 update \
    && DEBIAN_FRONTEND=noninteractive xargs apt-get \
         -o Acquire::Retries=0 install --yes --no-install-recommends </tmp/system-tools.txt \
    && while IFS= read -r package; do dpkg-query -W -f='${db:Status-Status}\n' "$package" | grep -Fx installed; done </tmp/system-tools.txt \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/* /tmp/system-tools.txt
```

Do not use shell-generated version selectors. `mise install` consumes only `/etc/mise/config.toml`, whose exact values are already cross-checked against the lock.

- [ ] **Step 4: Preserve final-user contracts and run tests**

Keep the existing UID/GID, sudoers validation, tini entrypoint, volume declarations, Chromium copy/read-only behavior, and `workspace` ownership of the mise volume root.

```sh
cargo test --manifest-path scripts/Cargo.toml --test connected_dockerfile --test polyglot_image_contract --test image_user_contract
cargo test --manifest-path scripts/Cargo.toml
```

Expected: PASS.

- [ ] **Step 5: Commit**

```sh
git add images/workspace/Dockerfile scripts/tests/connected_dockerfile.rs scripts/tests/offline_dockerfile.rs scripts/tests/polyglot_image_contract.rs scripts/tests/image_user_contract.rs tests/image/system-tools.txt
git commit -m "build: assemble connected polyglot workspace base"
```

### Task 4: Build pinned private Gascamp without retaining credentials

**Files:**
- Modify: `images/workspace/Dockerfile`
- Create: `scripts/tests/gascamp_build_secret.rs`
- Modify: `tests/image/gascamp-smoke.sh`
- Modify: `scripts/tests/polyglot_image_contract.rs`

**Interfaces:**
- Consumes: BuildKit secret `gascamp_read_token` and locked 40-character `GASCAMP_REVISION` build argument containing no credential.
- Produces: `/opt/gascan/gascamp/bin/camp`, relative `campd` symlink, and root-owned read-only `/opt/gascan/gascamp/REVISION`; no source, `.git`, Cargo cache, or secret enters the final stage.

- [ ] **Step 1: Write RED credential-boundary tests**

Require the Gascamp builder to contain:

```rust
for required in [
    "RUN --mount=type=secret,id=gascamp_read_token,required=true",
    "https://github.com/Liquescent-Development/gascamp.git",
    "git rev-parse HEAD",
    "$GASCAMP_REVISION",
    "cargo test --locked",
    "cargo build --locked --release --bin camp",
    "COPY --from=gascamp-builder /out /opt/gascan/gascamp",
] {
    assert!(dockerfile.contains(required), "missing Gascamp boundary: {required}");
}
for forbidden in [
    "ARG GASCAMP_READ_TOKEN",
    "ENV GASCAMP_READ_TOKEN",
    "COPY .git",
    "COPY --from=gascamp-builder /root",
    "bundles/gascamp_source_vendor",
] {
    assert!(!dockerfile.contains(forbidden), "credential/source leak: {forbidden}");
}
```

The test must also reject a clone URL containing `@github.com` so a token cannot be interpolated into the URL. Require a Git credential helper that reads `/run/secrets/gascamp_read_token` at credential-request time and emits the fixed username `x-access-token` without placing the token in the Dockerfile or command argument.

- [ ] **Step 2: Verify RED**

```sh
cargo test --manifest-path scripts/Cargo.toml --test gascamp_build_secret
```

Expected: FAIL on the offline bundle copy.

- [ ] **Step 3: Implement the secret-mounted builder stage**

Within one secret-mounted `RUN` instruction:

1. install a temporary credential-helper script whose source contains only the secret-file path, never its content;
2. clone without embedding credentials in the URL;
3. detach at the exact `GASCAMP_REVISION` and verify `git rev-parse HEAD` equals it;
4. remove `.git` and the helper before compilation output is copied;
5. run exact locked tests and release build using the mise-installed Cargo;
6. strip and install only `camp`, `campd`, and `REVISION` beneath `/out`;
7. make `/out` read-only.

Use a cache directory only if it is a BuildKit cache mount and no credential can be written to it. The minimal MVP implementation should omit the cache mount.

Use this credential and fetch shape; the helper source contains only the fixed
secret mount path, and the Git command contains no token:

```Dockerfile
ARG GASCAMP_REVISION
RUN --mount=type=secret,id=gascamp_read_token,required=true \
    set -eu; \
    install -d -m 0700 /tmp/gascamp; \
    printf '%s\n' \
      '#!/bin/sh' \
      'case "${1:-get}" in' \
      '  get)' \
      "    printf '%s\\n' 'username=x-access-token'" \
      "    printf 'password=%s\\n' \"\$(cat /run/secrets/gascamp_read_token)\"" \
      '    ;;' \
      'esac' >/tmp/gascamp-credential; \
    chmod 0700 /tmp/gascamp-credential; \
    cd /tmp/gascamp; \
    git init; \
    git remote add origin https://github.com/Liquescent-Development/gascamp.git; \
    git -c credential.helper=/tmp/gascamp-credential -c credential.useHttpPath=true \
         fetch --depth=1 origin "$GASCAMP_REVISION"; \
    git checkout --detach FETCH_HEAD; \
    test "$(git rev-parse HEAD)" = "$GASCAMP_REVISION"; \
    rm -rf .git /tmp/gascamp-credential; \
    cargo test --locked; \
    cargo build --locked --release --bin camp
```

A test must reject a Dockerfile where the secret-mounted instruction ends
before `git fetch`.

- [ ] **Step 4: Run structural tests**

```sh
cargo test --manifest-path scripts/Cargo.toml --test gascamp_build_secret --test polyglot_image_contract
bash -n tests/image/gascamp-smoke.sh
```

Expected: PASS.

- [ ] **Step 5: Commit**

```sh
git add images/workspace/Dockerfile scripts/tests/gascamp_build_secret.rs scripts/tests/polyglot_image_contract.rs tests/image/gascamp-smoke.sh
git commit -m "build: compile pinned Gascamp with build secret"
```

### Task 5: Add the connected build orchestrator and exact reference receipt

**Files:**
- Create: `scripts/build-connected-workspace-image.sh`
- Create: `scripts/src/bin/validate-connected-build.rs`
- Create: `scripts/tests/connected_image_build.rs`
- Modify: `scripts/build-workspace-image.sh`
- Modify: `scripts/tests/image_lock.rs`

**Interfaces:**
- Consumes: Task 2 context and canonical context digest, the sealed public snapshot returned by the unchanged reviewed helper, exact local base inspection, and `GASCAMP_READ_TOKEN_FILE` validated and privately staged by Task 1 rules.
- Produces: atomic `.artifacts/workspace-image-ref` matching exactly `^gascan-workspace:[a-z0-9._-]+@sha256:[0-9a-f]{64}$` and `.artifacts/workspace-image-build.json` containing platform, lock digest, context digest, image digest, Apple version, and sanitized timestamps/status.

- [ ] **Step 1: Write failing fake-runner tests**

Cover these cases with a fake `container` executable:

- missing, relative, foreign-owned, group/world-readable, symlink, empty, or repository-contained source secret file fails before staging or `container build`;
- staged-secret creation, permission, `.dockerignore`, or transmitted-context exclusion failure prevents `container build`;
- the privileged helper is invoked only through `create`, `path`, and `finish`; it never receives the source or staged secret path;
- the sealed public snapshot is copied into a separate current-UID-owned `0700` wrapper, and its public manifest must equal the Task 2 context digest before and after build;
- source-secret validation and copying use one no-follow file descriptor so a pathname swap cannot change the validated bytes;
- context verification failure or changed post-build digest fails without publishing a reference;
- build invocation has exactly one `--secret`, exact `BASE_IMAGE`, exact `GASCAMP_REVISION`, `--arch arm64`, and no token value;
- structured inspect must report `linux/arm64` and the exact built tag;
- malformed/mutable/mismatched image output fails;
- JSON is atomically published first and the reference atomically published last as the commit marker; interruption between them must not expose an accepted new reference with stale or missing JSON;
- stdout, stderr, argv, receipts, and retained files do not contain a synthetic token.

The expected invocation slice is:

```rust
let required = [
    "build", "--arch", "arm64",
    "--secret", "id=gascamp_read_token,src=/private/wrapper/.build-secrets/gascamp_read_token",
    "--build-arg", "BASE_IMAGE=ubuntu@sha256:7f622ca8766bccb22f04242ecb6f19f770b2f08827dc4b8c707de5e78a6da7ab",
    "--build-arg", "GASCAMP_REVISION=f6b248c5926240856dbea83d1d2c5c90ea1c1456",
];
```

- [ ] **Step 2: Verify RED**

```sh
cargo test --manifest-path scripts/Cargo.toml --test connected_image_build
```

Expected: FAIL because the connected orchestrator and validator do not exist.

- [ ] **Step 3: Implement orchestration**

Make `scripts/build-workspace-image.sh` a mode dispatcher driven by the exact lock value. For `connected`, exec `build-connected-workspace-image.sh`; retain the old implementation under `scripts/build-offline-workspace-image.sh` for deferred use, but do not allow `auto` fallback between modes.

The connected script must use the already-reviewed privileged snapshot helper
only to create, locate, and finish a sealed public snapshot. It must create a
separate unprivileged `0700` wrapper, copy the sealed public snapshot and stage
the validated external secret beneath that wrapper through descriptor-safe
Rust code, pass only the staged wrapper path to BuildKit, and reverify the
public manifest, staged secret, and secret exclusion after build. The helper
and sudoers contract remain unchanged and never receive a credential path.

Publish the fully validated JSON receipt first and the reference file last.
The reference is the commit marker; every consumer must reject receipt pairs
whose tag, image digest, context digest, or lock digest disagree. The script
must remove the wrapper and finish the privileged public snapshot through
bounded traps. It must never capture or print the secret.

- [ ] **Step 4: Run focused and full scripts suites**

```sh
cargo test --manifest-path scripts/Cargo.toml --test connected_image_build --test image_lock
cargo test --manifest-path scripts/Cargo.toml
cargo clippy --manifest-path scripts/Cargo.toml --all-targets -- -D warnings
cargo fmt --manifest-path scripts/Cargo.toml --all -- --check
bash -n scripts/build-workspace-image.sh scripts/build-connected-workspace-image.sh scripts/build-offline-workspace-image.sh
```

Expected: PASS.

- [ ] **Step 5: Commit**

```sh
git add scripts/build-workspace-image.sh scripts/build-connected-workspace-image.sh scripts/build-offline-workspace-image.sh scripts/src/bin/validate-connected-build.rs scripts/tests/connected_image_build.rs scripts/tests/image_lock.rs
git commit -m "build: orchestrate connected workspace image"
```

### Task 6: Run the real connected image gate

**Files:**
- Create: `scripts/run-connected-image-gate.sh`
- Create: `scripts/tests/connected_image_gate.rs`
- Create: `docs/evidence/connected-workspace-image.md`
- Create after live PASS: `images/workspace/approved-image.txt`
- Modify: `tests/image/user-and-volumes.sh`
- Modify: `tests/image/polyglot-smoke.sh`
- Modify: `tests/image/gascamp-smoke.sh`

**Interfaces:**
- Consumes: exact build receipts and a current-run 128-bit lowercase hex owner token.
- Produces: sanitized evidence for one real connected build and all three live smokes, including exact image reference/digest/platform and no current-token residue.

- [ ] **Step 1: Write RED fail-closed gate tests**

Using a fake `container`, require:

- a new owner token for every run and exact predictable names for all three smokes;
- cleanup validates ownership before every stop/delete;
- `INT` and `TERM` exit nonzero after bounded cleanup;
- build failure, missing/malformed receipt, platform mismatch, smoke failure, or residue prevents evidence publication;
- all smoke scripts receive the same exact digest-qualified reference file and owner token;
- final inventory proves all exact current-token names absent;
- unrelated and foreign resources are untouched;
- evidence is atomically replaced only after the final residue check.
- `images/workspace/approved-image.txt` is absent on failure and, after a live
  PASS, contains exactly the inspected digest-qualified reference with no
  trailing newline.

- [ ] **Step 2: Verify RED**

```sh
cargo test --manifest-path scripts/Cargo.toml --test connected_image_gate
```

Expected: FAIL because the gate does not exist.

- [ ] **Step 3: Implement the gate and strengthen smokes**

The gate sequence is fixed:

1. validate controller and secret-file preconditions;
2. run connected prefetch;
3. run connected build;
4. validate both receipts and exact `linux/arm64` inspection;
5. run user/volume, polyglot/browser, and Gascamp smokes with one owner token;
6. prove no exact current-token container remains;
7. write sanitized evidence.

Each smoke must reject a reference without `@sha256:` plus 64 lowercase hex characters and must not reread a mutable default tag.

- [ ] **Step 4: Run platform-neutral verification**

```sh
cargo test --manifest-path scripts/Cargo.toml --test connected_image_gate --test image_user_contract --test polyglot_image_contract
bash -n scripts/run-connected-image-gate.sh tests/image/user-and-volumes.sh tests/image/polyglot-smoke.sh tests/image/gascamp-smoke.sh
git diff --check
```

Expected: PASS.

- [ ] **Step 5: Run the real gate**

Create the private token file outside the repository, mode `0600`, then run:

```sh
sudo -v
GASCAMP_READ_TOKEN_FILE=/absolute/private/path/gascamp-read-token \
  ./scripts/run-connected-image-gate.sh
```

Expected: one connected ARM64 build; all three smokes PASS; evidence records a digest-qualified image and no current-token residue. If credentials, registry access, Apple runtime, or network policy fail, record the exact blocker and do not create PASS evidence.

- [ ] **Step 6: Independently review evidence and commit**

Review must compare the evidence image reference and
`images/workspace/approved-image.txt` byte-for-byte to structured live
inspection and verify the secret value is absent from the repository, build
transcript, image history, and exported final filesystem.

```sh
git add scripts/run-connected-image-gate.sh scripts/tests/connected_image_gate.rs tests/image docs/evidence/connected-workspace-image.md images/workspace/approved-image.txt
git commit -m "test: prove connected workspace image"
```

### Task 7: Prepare exact Gate 4 integration handoff

**Files:**
- Create worktree: `.worktrees/gate4-integration` from `feature/macos-mvp` at `917dac1`
- Merge: approved Apple backend head `dbf4235`
- Merge: approved connected-image head produced by Task 6
- Modify: `crates/gascan-core/src/policy.rs`
- Modify: `crates/gascan-core/tests/policy.rs`
- Consume: `images/workspace/approved-image.txt` created by Task 6
- Modify: `docs/status/macos-mvp-handoff.md`
- Modify: `docs/superpowers/plans/2026-07-13-gascan-macos-roadmap.md`
- Create: `docs/evidence/connected-image-handoff.md`
- Test: `crates/gascan-e2e/tests/apple_lifecycle.rs`

**Interfaces:**
- Consumes: Task 6 exact image reference and approved Apple Gate 4 harness at `dbf4235`.
- Produces: an integration branch whose frozen policy image equals Task 6's exact locally built reference and reviewed instructions for running Gate 4; does not itself claim Gate 4.

- [ ] **Step 1: Write the reference-handoff regression**

Create the isolated integration worktree using the `superpowers:using-git-worktrees` skill, merge the two reviewed feature heads without squashing their evidence, and resolve overlaps through explicit joint review.

Replace the old locked GHCR fixture in `crates/gascan-core/src/policy.rs` with
the committed Task 6 receipt. Add a policy regression that asserts the request
uses those exact bytes and a digest-qualified reference:

```rust
#[test]
fn image_reference_is_the_gate_approved_connected_image() {
    let request = compile_workspace_request();
    let approved = include_str!("../../../images/workspace/approved-image.txt");
    assert_eq!(request.image(), approved);
    assert!(request.image().contains("@sha256:"));
    assert_eq!(request.image().matches('@').count(), 1);
    assert!(!request.image().chars().any(|ch| ch.is_ascii_whitespace()));
}
```

Task 6 must already have committed the exact receipt; absence is a hard error.

- [ ] **Step 2: Run and verify RED**

```sh
cargo test -p gascan-core --test policy image_reference_is_the_gate_approved_connected_image
```

Expected: FAIL because policy still contains the old GHCR fixture.

- [ ] **Step 3: Freeze the exact connected image in policy**

Set the policy constant without copying or retyping the reference:

```rust
const WORKSPACE_IMAGE: &str =
    include_str!("../../../images/workspace/approved-image.txt");
```

Before committing, inspect that reference again and require the same digest and `linux/arm64`.
The Gate 4 harness must not pull, rebuild, read an environment override, or
choose a fallback image.

- [ ] **Step 4: Reconcile durable status**

Update the handoff and roadmap with:

- connected image commit and evidence path;
- exact digest-qualified image reference;
- Apple backend head `dbf4235`;
- integration branch starting point `917dac1` and both merge commits;
- Gate 4 still `pending` until the full lifecycle runs;
- next action: create an integration worktree, merge reviewed branches, run full verification, then execute Gate 4 serially.

- [ ] **Step 5: Run final plan verification and commit**

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo test --manifest-path scripts/Cargo.toml
cargo clippy --manifest-path scripts/Cargo.toml --all-targets -- -D warnings
git diff --check
```

Expected: all platform-neutral verification PASS; live connected-image evidence remains valid; Gate 4 remains explicitly pending.

```sh
git add crates/gascan-core/src/policy.rs crates/gascan-core/tests/policy.rs docs/status/macos-mvp-handoff.md docs/superpowers/plans/2026-07-13-gascan-macos-roadmap.md docs/evidence/connected-image-handoff.md
git commit -m "docs: hand connected image to Gate 4"
```

## Completion Boundary

This plan is complete only when Tasks 1–7 are independently reviewed, the real connected image gate has passed, its exact image reference is durably recorded, and the Gate 4 harness can consume that reference without fallback. Completion of this plan does not mean Roadmap Gate 4 or the macOS MVP has passed.
