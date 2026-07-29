# Workspace Image Release Compatibility Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Publish and approve a workspace image that contains every guest helper required by Gas Can 0.1.14, and prevent future releases from using an image built from stale workspace-image inputs.

**Architecture:** A machine-readable runtime contract is the source of truth for guest helper paths shared by provisioning and the workspace image. A deterministic fingerprint covers tracked `images/workspace` inputs; image approval atomically records that fingerprint beside the immutable image pin, and release preflight rejects a source/fingerprint mismatch.

**Tech Stack:** Rust 1.85+ script tools and tests, Bash release/image orchestration, TOML contracts, Apple container 1.1.0, GHCR OCI images, Cargo, GitHub CLI, Apple Developer ID signing/notarization, Homebrew casks.

## Global Constraints

- Keep the workspace image immutable and digest-qualified as `ghcr.io/liquescent-development/gascan/workspace:<unique-tag>@sha256:<64 lowercase hex>`.
- Never overwrite a GHCR tag; derive the tag from the complete candidate digest.
- Only exact test-owned Apple containers and volumes may be cleaned.
- Image approval requires matching validated build, connected-gate candidate, and Apple-live receipts.
- The source fingerprint excludes only `images/workspace/approved-image.txt` and `images/workspace/approved-source.sha256`.
- Preserve Gas Can protocol and daemon wire compatibility.
- Release version is exactly `0.1.14`.

---

### Task 1: Define and validate the guest-helper runtime contract

**Files:**
- Create: `images/workspace/runtime-contract.toml`
- Create: `scripts/src/bin/validate-runtime-contract.rs`
- Create: `scripts/tests/runtime_contract.rs`
- Modify: `scripts/Cargo.toml`
- Modify: `scripts/run-connected-image-gate.sh`

**Interfaces:**
- Consumes: the provisioning commands in `crates/gascand/src/service.rs`, `images/workspace/Dockerfile`, and helper source files.
- Produces: `validate-runtime-contract ROOT`, exiting zero only when every declared path has an exact executable Dockerfile copy, a source file, and a provisioning reference.

- [ ] **Step 1: Write the failing validator tests**

Create fixtures in `scripts/tests/runtime_contract.rs` and invoke
`env!("CARGO_BIN_EXE_validate-runtime-contract")`. Cover:

```rust
struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
}

fn fixture(include_copy: bool, include_service_reference: bool) -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    fs::create_dir_all(root.join("images/workspace/bin")).unwrap();
    fs::create_dir_all(root.join("crates/gascand/src")).unwrap();
    fs::write(
        root.join("images/workspace/runtime-contract.toml"),
        "version = 1\n[[helpers]]\npath = \"/usr/local/bin/helper\"\nsource = \"images/workspace/bin/helper\"\n",
    ).unwrap();
    fs::write(root.join("images/workspace/bin/helper"), "#!/bin/sh\nexit 0\n").unwrap();
    fs::write(
        root.join("images/workspace/Dockerfile"),
        if include_copy {
            "COPY --chmod=0555 images/workspace/bin/helper /usr/local/bin/helper\n"
        } else {
            "FROM scratch\n"
        },
    ).unwrap();
    fs::write(
        root.join("crates/gascand/src/service.rs"),
        if include_service_reference {
            "const HELPER: &str = \"/usr/local/bin/helper\";\n"
        } else {
            "const HELPER: &str = \"/usr/local/bin/other\";\n"
        },
    ).unwrap();
    Fixture { _temp: temp, root }
}

fn validate(root: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_validate-runtime-contract"))
        .arg(root)
        .output()
        .unwrap()
}

#[test]
fn repository_runtime_contract_is_complete() {
    let output = validate(&repository_root());
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
}

#[test]
fn missing_exact_copy_or_service_reference_is_rejected() {
    for fixture in [fixture(false, true), fixture(true, false)] {
        let output = validate(&fixture.root);
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr)
            .contains("/usr/local/bin/helper"));
    }
}
```

- [ ] **Step 2: Run the focused test and verify RED**

Run:

```sh
rtk cargo test --locked --manifest-path scripts/Cargo.toml --test runtime_contract -- --nocapture
```

Expected: compilation fails because the validator binary does not exist.

- [ ] **Step 3: Add the exact runtime contract**

Create `images/workspace/runtime-contract.toml`:

```toml
version = 1

[[helpers]]
path = "/usr/local/bin/configure-shell-home"
source = "images/workspace/bin/configure-shell-home"

[[helpers]]
path = "/usr/local/bin/initialize-rust-home"
source = "images/workspace/bin/initialize-rust-home"

[[helpers]]
path = "/usr/local/bin/configure-workstation-home"
source = "images/workspace/bin/configure-workstation-home"

[[helpers]]
path = "/usr/local/bin/select-gascamp"
source = "images/workspace/bin/select-gascamp"

[[helpers]]
path = "/usr/local/bin/mise"
source = ".artifacts/mise-linux-arm64"
```

- [ ] **Step 4: Implement the minimal validator**

Add a `[[bin]]` entry named `validate-runtime-contract`. Parse the contract
with `toml`, reject versions other than `1`, duplicate paths/sources,
non-absolute destinations, and unsafe relative sources. For each helper:

```rust
let copy = format!("COPY --chmod=0555 {} {}", helper.source, helper.path);
if !dockerfile.lines().any(|line| line.trim() == copy) {
    return Err(format!("Dockerfile does not install {}", helper.path));
}
if !service.contains(&format!("\"{}\"", helper.path)) {
    return Err(format!("provisioning does not reference {}", helper.path));
}
```

Require repository-backed sources to be regular, non-symlink files. Permit the
single generated `.artifacts/mise-linux-arm64` source only when its exact COPY
line exists.

- [ ] **Step 5: Make the connected gate validate before building**

In `scripts/run-connected-image-gate.sh`, add before prefetch/build:

```bash
run_tool validate-runtime-contract "$root"
```

- [ ] **Step 6: Run focused and existing image-tool tests**

Run:

```sh
rtk cargo test --locked --manifest-path scripts/Cargo.toml --test runtime_contract -- --nocapture
rtk cargo test --locked --manifest-path scripts/Cargo.toml
rtk bash tests/image/shell-home-root-contract.sh .
```

Expected: all pass.

- [ ] **Step 7: Commit**

```sh
rtk git add images/workspace/runtime-contract.toml scripts/Cargo.toml \
  scripts/src/bin/validate-runtime-contract.rs scripts/tests/runtime_contract.rs \
  scripts/run-connected-image-gate.sh
rtk git commit -S -m "build: validate workspace runtime helper contract"
```

---

### Task 2: Add deterministic workspace-image source fingerprinting

**Files:**
- Create: `scripts/workspace-image-source-digest.sh`
- Create: `scripts/tests/workspace_image_source_digest.rs`
- Modify: `scripts/Cargo.toml`

**Interfaces:**
- Consumes: tracked files under `images/workspace/`.
- Produces: `workspace-image-source-digest.sh ROOT`, printing exactly one lowercase 64-character SHA-256 plus newline.

- [ ] **Step 1: Write failing fingerprint tests**

Create temporary Git repositories and assert:

```rust
fn source_fixture() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    fs::create_dir_all(root.join("images/workspace/bin")).unwrap();
    fs::write(root.join("images/workspace/bin/helper"), "one\n").unwrap();
    fs::write(root.join("images/workspace/approved-image.txt"), "image\n").unwrap();
    fs::write(root.join("images/workspace/approved-source.sha256"), format!("{}\n", "0".repeat(64))).unwrap();
    Command::new("git").args(["init", "-q"]).current_dir(&root).status().unwrap();
    Command::new("git").args(["add", "."]).current_dir(&root).status().unwrap();
    (temp, root)
}

fn source_digest(root: &Path) -> Output {
    Command::new("bash")
        .arg(repository_root().join("scripts/workspace-image-source-digest.sh"))
        .arg(root)
        .output()
        .unwrap()
}

#[test]
fn digest_is_stable_and_changes_with_image_source() {
    let (_temp, root) = source_fixture();
    let first = source_digest(&root);
    let second = source_digest(&root);
    assert!(first.status.success());
    assert_eq!(first.stdout, second.stdout);
    fs::write(root.join("images/workspace/bin/helper"), "two\n").unwrap();
    let changed = source_digest(&root);
    assert!(changed.status.success());
    assert_ne!(first.stdout, changed.stdout);
}

#[test]
fn approval_outputs_do_not_change_the_digest() {
    let (_temp, root) = source_fixture();
    let first = source_digest(&root);
    fs::write(root.join("images/workspace/approved-image.txt"), "replacement\n").unwrap();
    fs::write(root.join("images/workspace/approved-source.sha256"), format!("{}\n", "f".repeat(64))).unwrap();
    assert_eq!(first.stdout, source_digest(&root).stdout);
}

#[test]
fn unsafe_or_empty_source_tree_is_rejected() {
    let (_temp, root) = source_fixture();
    fs::remove_file(root.join("images/workspace/bin/helper")).unwrap();
    std::os::unix::fs::symlink(
        root.join("images/workspace/approved-image.txt"),
        root.join("images/workspace/bin/helper"),
    ).unwrap();
    assert!(!source_digest(&root).status.success());

    Command::new("git")
        .args(["rm", "--cached", "images/workspace/bin/helper"])
        .current_dir(&root)
        .status()
        .unwrap();
    assert!(!source_digest(&root).status.success());
}
```

- [ ] **Step 2: Run the test and verify RED**

Run:

```sh
rtk cargo test --locked --manifest-path scripts/Cargo.toml \
  --test workspace_image_source_digest -- --nocapture
```

Expected: failures because the fingerprint command is absent.

- [ ] **Step 3: Implement the fingerprint command**

The Bash command must:

1. canonicalize `ROOT`;
2. use `git -C "$root" ls-files -z -- images/workspace`;
3. skip exactly the two approval outputs;
4. reject symlinks, non-regular files, tabs, newlines, and an empty selection;
5. feed `path<TAB>sha256<LF>` records in Git index order into
   `shasum -a 256`; and
6. print only the digest.

Use a private temporary record file with `umask 077` and an EXIT trap rather
than a pipeline whose producer status can be lost.

- [ ] **Step 4: Run focused tests and verify the real tree**

Run:

```sh
rtk cargo test --locked --manifest-path scripts/Cargo.toml \
  --test workspace_image_source_digest -- --nocapture
rtk bash scripts/workspace-image-source-digest.sh .
```

Expected: tests pass and the command prints one lowercase 64-character digest.

- [ ] **Step 5: Commit**

```sh
rtk git add scripts/workspace-image-source-digest.sh \
  scripts/tests/workspace_image_source_digest.rs scripts/Cargo.toml
rtk git commit -S -m "build: fingerprint workspace image sources"
```

---

### Task 3: Make image approval atomically publish the source fingerprint

**Files:**
- Modify: `scripts/approve-connected-workspace-image.sh`
- Modify: `scripts/tests/connected_image_approval.rs`

**Interfaces:**
- Consumes: the source-digest command from Task 2 and the existing three matching image receipts.
- Produces: an atomic approval triple: image pin, evidence document, and `images/workspace/approved-source.sha256`.

- [ ] **Step 1: Extend approval tests and verify RED**

Update fixtures with a stub source-digest command printing 64 `b` characters.
Assert successful approval writes:

```text
bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
```

to `approved-source.sha256`, includes it in the evidence, preserves its prior
mode, and restores all three previous files for FAIL, INT, and TERM at every
publication boundary.

Run:

```sh
rtk cargo test --locked --manifest-path scripts/Cargo.toml \
  --test connected_image_approval -- --nocapture
```

Expected: failures because approval still publishes only two files.

- [ ] **Step 2: Implement atomic triple publication**

Resolve the digest command from
`${GASCAN_APPROVAL_SOURCE_DIGEST_COMMAND:-"$root/scripts/workspace-image-source-digest.sh"}`.
Validate its output against `^[0-9a-f]{64}$`. Stage, back up, publish, and
rollback `approved-source.sha256` using the same ownership/mode-preserving
pattern as the existing image and evidence files. Add explicit test boundaries
before and after fingerprint replacement.

- [ ] **Step 3: Run focused and complete script tests**

Run:

```sh
rtk cargo test --locked --manifest-path scripts/Cargo.toml \
  --test connected_image_approval -- --nocapture
rtk cargo test --locked --manifest-path scripts/Cargo.toml
```

Expected: all pass.

- [ ] **Step 4: Commit**

```sh
rtk git add scripts/approve-connected-workspace-image.sh \
  scripts/tests/connected_image_approval.rs
rtk git commit -S -m "build: bind image approval to source fingerprint"
```

---

### Task 4: Reject stale approved images during release preflight

**Files:**
- Modify: `packaging/macos/release-common.sh`
- Modify: `packaging/macos/release-gates.sh`
- Modify: `packaging/macos/release.sh`
- Modify: `tests/release/source-input-contract.sh`
- Modify: `tests/release/release-script-contract.sh`

**Interfaces:**
- Consumes: `images/workspace/approved-source.sha256` and the Task 2 digest command.
- Produces: `gascan_gate_workspace_image_source REPO`, returning 65 with an actionable rebuild/approval message on absence, malformed content, or mismatch.

- [ ] **Step 1: Add failing release-contract cases**

In `tests/release/source-input-contract.sh`, seed
`images/workspace/approved-source.sha256` as a tracked input and include it in
`classes`. In `tests/release/release-script-contract.sh`, add:

```bash
printf '%064d\n' 0 >"$fixture/images/workspace/approved-source.sha256"
source_digest="$fixture/scripts/workspace-image-source-digest.sh"
cat >"$source_digest" <<'EOF_DIGEST'
#!/bin/sh
printf '%064d\n' 0
EOF_DIGEST
chmod 0755 "$source_digest"
gascan_gate_workspace_image_source "$fixture"

printf '%064d\n' 1 >"$fixture/images/workspace/approved-source.sha256"
if gascan_gate_workspace_image_source "$fixture" 2>"$fixture/stale-error"; then
  printf 'stale workspace image source fingerprint passed\n' >&2
  exit 1
fi
grep -Fq 'rebuild, live-test, and approve' "$fixture/stale-error"
```

Add parallel missing and non-hex cases, and record the gate in the existing
release mutation-scan stub so deleting its call makes the contract fail.

- [ ] **Step 2: Run contracts and verify RED**

Run:

```sh
rtk bash tests/release/source-input-contract.sh
rtk bash tests/release/release-script-contract.sh
```

Expected: failures because the new approval file and gate are not enforced.

- [ ] **Step 3: Implement the release gate**

Add:

```bash
gascan_gate_workspace_image_source() {
  local repo=$1 expected observed
  expected=$(tr -d '\n' <"$repo/images/workspace/approved-source.sha256") || return 65
  [[ $expected =~ ^[0-9a-f]{64}$ ]] || {
    printf 'approved workspace image source fingerprint is invalid\n' >&2
    return 65
  }
  observed=$("$repo/scripts/workspace-image-source-digest.sh" "$repo") || return 65
  [[ $observed == "$expected" ]] || {
    printf 'approved workspace image is stale; rebuild, live-test, and approve the current image\n' >&2
    return 65
  }
}
```

Call it after version/source-cleanliness checks and before GitHub publication
checks. Freeze the full `images/workspace` tree and the digest command as
release inputs.

- [ ] **Step 4: Run all release contracts**

Run:

```sh
rtk bash -c 'set -euo pipefail; for c in tests/release/*-contract.sh; do bash "$c"; done'
```

Expected: all 13 contracts pass.

- [ ] **Step 5: Commit**

```sh
rtk git add packaging/macos/release-common.sh packaging/macos/release-gates.sh \
  packaging/macos/release.sh tests/release/source-input-contract.sh \
  tests/release/release-script-contract.sh
rtk git commit -S -m "release: reject stale workspace image approvals"
```

---

### Task 5: Build, publish, live-test, and approve the corrected image

**Files:**
- Modify through approval: `images/workspace/approved-image.txt`
- Create through approval: `images/workspace/approved-source.sha256`
- Modify through approval: `docs/evidence/connected-workspace-image.md`
- Generated ignored receipts: `.artifacts/`

**Interfaces:**
- Consumes: Tasks 1–4 and the repository connected-image workflow.
- Produces: one public digest-qualified GHCR image and three matching committed approval records.

- [ ] **Step 1: Run Apple preflight and inventory exact residue**

```sh
rtk bash ./scripts/apple-test-preflight.sh
rtk container list --all --format json
rtk container volume list --format json
```

Expected: preflight passes; do not remove foreign resources.

- [ ] **Step 2: Build and run the local connected gate**

```sh
rtk bash ./scripts/run-connected-image-gate.sh
```

Expected: every image smoke passes, including
`test -x /usr/local/bin/configure-shell-home`, and the final line is a local
digest-qualified candidate.

- [ ] **Step 3: Publish the unique candidate**

Run this block without manually typing a tag or digest:

```sh
rtk bash -c '
set -euo pipefail
receipt=.artifacts/workspace-image-build.json
reference_file=.artifacts/workspace-image-ref
local_reference=$(jq -er .reference "$receipt")
local_tag=${local_reference%@*}
digest=${local_reference##*@}
digest_hex=${digest#sha256:}
test ${#digest_hex} -eq 64
locked_tag=$(awk -F " = " '"'"'$1 == "workspace_tag" {
  gsub(/^"|"$/, "", $2); print $2
}'"'"' images/workspace/versions.lock)
locked_tag=${locked_tag#gascan-workspace:}
remote_tag=ghcr.io/liquescent-development/gascan/workspace:${locked_tag}-${digest_hex}
remote_reference=$remote_tag@$digest
container image tag "$local_tag" "$remote_tag"
headers=$(mktemp .artifacts/.workspace-registry-headers.XXXXXX)
receipt_tmp=$(mktemp .artifacts/.workspace-image-build.public.XXXXXX)
reference_tmp=$(mktemp .artifacts/.workspace-image-ref.public.XXXXXX)
trap '"'"'rm -f "$headers" "$receipt_tmp" "$reference_tmp"'"'"' EXIT
token=$(curl --fail --silent --show-error \
  "https://ghcr.io/token?scope=repository:liquescent-development/gascan/workspace:pull" |
  jq -er .token)
status=$(curl --silent --show-error --output /dev/null \
  --dump-header "$headers" --write-out "%{http_code}" \
  --header "Authorization: Bearer $token" \
  --header "Accept: application/vnd.oci.image.manifest.v1+json, application/vnd.oci.image.index.v1+json" \
  "https://ghcr.io/v2/liquescent-development/gascan/workspace/manifests/${remote_tag##*:}")
case $status in
  200)
    existing=$(awk '"'"'tolower($1) == "docker-content-digest:" {
      gsub(/\r/, "", $2); print $2
    }'"'"' "$headers")
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
printf "%s" "$inspect" |
  cargo run --quiet --locked --offline --manifest-path scripts/Cargo.toml \
    --bin validate-connected-build -- "$remote_tag" >/dev/null
jq --arg reference "$remote_reference" --arg tag "$remote_tag" \
  ".reference = \$reference | .tag = \$tag" "$receipt" >"$receipt_tmp"
printf "%s\n" "$remote_reference" >"$reference_tmp"
bash scripts/validate-connected-image-receipt.sh \
  "$reference_tmp" "$receipt_tmp" >/dev/null
mv -f "$receipt_tmp" "$receipt"
mv -f "$reference_tmp" "$reference_file"
rm -f "$headers"
trap - EXIT
'
```

- [ ] **Step 4: Re-run the public-image gate and Apple apply suite**

```sh
rtk bash ./scripts/run-connected-image-gate.sh --prebuilt
rtk env GASCAN_E2E_CANDIDATE_IMAGE_FILE=.artifacts/connected-workspace-image-candidate.txt \
  bash ./scripts/run-apple-e2e.sh apple_apply
```

Expected: all ignored live apply tests pass and scoped cleanup leaves no
test-owned container or volume.

- [ ] **Step 5: Verify all receipts agree**

```sh
rtk cat .artifacts/connected-workspace-image-candidate.txt
rtk cat .artifacts/connected-workspace-image-apple-live.txt
rtk bash ./scripts/validate-connected-image-receipt.sh \
  .artifacts/workspace-image-ref .artifacts/workspace-image-build.json
```

Expected: byte-identical public digest-qualified references.

- [ ] **Step 6: Approve the image**

```sh
rtk bash ./scripts/approve-connected-workspace-image.sh
rtk bash ./scripts/workspace-image-source-digest.sh .
rtk cat images/workspace/approved-source.sha256
```

Expected: the two fingerprints match exactly.

- [ ] **Step 7: Commit the approval**

```sh
rtk git add images/workspace/approved-image.txt \
  images/workspace/approved-source.sha256 \
  docs/evidence/connected-workspace-image.md
rtk git commit -S -m "build: approve shell-compatible workspace image"
```

---

### Task 6: Prepare and verify Gas Can 0.1.14

**Files:**
- Modify: six `crates/*/Cargo.toml`
- Modify: `Cargo.lock`
- Modify: `README.md`
- Modify: `docs/release/macos-checklist.md`

**Interfaces:**
- Consumes: the approved corrected image from Task 5.
- Produces: a clean, signed, review-ready 0.1.14 branch.

- [ ] **Step 1: Bump exactly the standard nine files**

Replace `0.1.13` with `0.1.14` in the six workspace crate manifests, update
the six workspace package records in root `Cargo.lock`, and update current
release references in README and the macOS checklist. Do not change
`scripts/Cargo.lock` or historical specs.

- [ ] **Step 2: Commit before clone-based contracts**

```sh
rtk git add Cargo.lock README.md crates/*/Cargo.toml docs/release/macos-checklist.md
rtk git commit -S -m "release: prepare Gas Can 0.1.14"
```

- [ ] **Step 3: Run full verification**

```sh
rtk cargo fmt --all -- --check
rtk cargo clippy --locked --workspace --all-targets -- -D warnings
rtk cargo test --locked --workspace --all-targets
rtk cargo test --locked --manifest-path scripts/Cargo.toml
rtk bash -c 'set -euo pipefail; for c in tests/release/*-contract.sh; do bash "$c"; done'
rtk git diff --check origin/main..HEAD
rtk git status --short --branch
```

Expected: zero failures and a clean branch.

- [ ] **Step 4: Request independent code review**

Review `origin/main..HEAD` against the approved design. Fix every Critical and
Important finding, rerun affected tests, and request follow-up review until the
assessment is ready to merge.

- [ ] **Step 5: Push and create the PR**

```sh
rtk git push -u origin fix/workspace-image-shell-helper
rtk gh pr create --base main --head fix/workspace-image-shell-helper \
  --title "Bind releases to compatible workspace images"
```

Include the new public image digest, Apple-live evidence, test counts, and
release-contract results in the PR body.

- [ ] **Step 6: Squash merge**

After forge checks and review:

```sh
rtk gh pr merge --squash --delete-branch
```

Record the exact merge commit and verify `origin/main` points to it.

---

### Task 7: Sign, publish, and verify the 0.1.14 release

**Files:**
- No source edits.
- External outputs: signed Git tag, GitHub release assets, notarized package, Homebrew tap commit.

**Interfaces:**
- Consumes: exact Task 6 squash-merge commit.
- Produces: public Gas Can 0.1.14 and a fetchable Homebrew cask.

- [ ] **Step 1: Create and push the signed tag**

```sh
rtk git tag -s v0.1.14 -m "Gas Can 0.1.14"
rtk git verify-tag v0.1.14
rtk git push origin v0.1.14
```

Require `v0.1.14^{}` and `origin/main` to equal the exact PR merge commit.

- [ ] **Step 2: Run read-only release preflight**

```sh
rtk ./packaging/macos/release.sh 0.1.14 --check \
  --codesign-identity "Developer ID Application: Liquescent Development LLC (Z548WR4TF8)" \
  --installer-identity "Developer ID Installer: Liquescent Development LLC (Z548WR4TF8)" \
  --notary-profile AC_PASSWORD \
  --tap /Users/kiener/code/homebrew-tap
```

Expected: all preconditions and cask style pass.

- [ ] **Step 3: Run the live release**

Run the same command without `--check`. Expected: Developer ID signatures,
accepted Apple notarization, successful stapling, a public GitHub release with
exactly three assets, and a pushed Homebrew cask commit.

- [ ] **Step 4: Verify publication**

```sh
rtk gh release view v0.1.14 --repo Liquescent-Development/gascan \
  --json isDraft,isPrerelease,tagName,targetCommitish,assets,url
rtk git verify-tag v0.1.14
rtk brew update
rtk brew info --cask liquescent-development/tap/gascan
rtk brew fetch --force --cask liquescent-development/tap/gascan
```

Expected: public non-prerelease, three uploaded assets, matching signed target,
and Homebrew fetches cask 0.1.14 with the published package SHA-256.

- [ ] **Step 5: Verify the original user path after upgrade**

After installing 0.1.14:

```sh
rtk gascan --version
rtk gascan up /Users/kiener/code
rtk gascan shell --sandbox code
```

Expected: version `0.1.14`, provisioning completes without
`configure-shell-home: command not found`, and the configured managed Starship
prompt starts in Bash.
