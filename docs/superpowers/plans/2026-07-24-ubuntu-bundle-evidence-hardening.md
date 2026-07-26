# Ubuntu Bundle Evidence Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Ubuntu bundle publication fail unless exact roots, signed metadata, cache bytes, configured packages, and reviewed command mappings are independently proven.

**Architecture:** The producer and verifier retain separate signed-metadata and command-evidence implementations. A producer-only cache helper stages and publishes bytes across an explicit validation boundary, while CI repeats the offline install and command comparison inside the pinned networkless ARM64 image.

**Tech Stack:** Bash, Python 3, `apt-get`, `dpkg-deb`, `dpkg-query`, Rust integration tests, GitHub Actions.

## Global Constraints

- Use `ubuntu@sha256:7f622ca8766bccb22f04242ecb6f19f770b2f08827dc4b8c707de5e78a6da7ab`.
- Use Linux ARM64 and the reviewed `2026-07-13T00:00:00Z` Ubuntu snapshot.
- Keep `APT::Install-Recommends=false` and `--no-install-recommends`.
- Do not add capabilities, privileged mode, devices, host credentials, or network access to runtime validation.
- Producer and independent verifier implementations must remain separate.
- Implementation follows test-first RED/GREEN cycles and lands in a new commit without amending `792c98f`.

---

### Task 1: Pin the Complete Root Input

**Files:**
- Modify: `scripts/tests/connected_dockerfile.rs`

**Interfaces:**
- Consumes: `tests/image/system-tools.txt`
- Produces: a byte-exact trusted root-list regression contract

- [ ] **Step 1: Replace inclusion-only assertions with a failing exact-list assertion**

```rust
const EXPECTED_SYSTEM_TOOLS: &str = "autoconf\nbind9-dnsutils\n...\nzstd\n";
assert_eq!(package_text, EXPECTED_SYSTEM_TOOLS);
```

- [ ] **Step 2: Run the targeted test and prove the mutation contract fails**

Run: `rtk cargo test --manifest-path scripts/Cargo.toml --test connected_dockerfile dockerfile_installs_exactly_the_sorted_unique_reviewed_package_list`

Expected: RED until the exact trusted list replaces permissive inclusion checks.

- [ ] **Step 3: Complete the exact list and remove redundant subset/exclusion loops**

```rust
assert_eq!(
    package_text, EXPECTED_SYSTEM_TOOLS,
    "reviewed Ubuntu root package set changed"
);
```

- [ ] **Step 4: Run the targeted suite**

Run: `rtk cargo test --manifest-path scripts/Cargo.toml --test connected_dockerfile`

Expected: 13 tests pass.

### Task 2: Enforce Signed-Index Equivalence

**Files:**
- Modify: `scripts/produce-ubuntu-package-bundle.sh`
- Modify: `scripts/verify-ubuntu-debian-evidence.py`
- Modify: `scripts/tests/ubuntu_package_bundle.rs`

**Interfaces:**
- Consumes: signed `Packages.xz` stanzas
- Produces: one full-field canonical record per `(Package, Version, Architecture)`

- [ ] **Step 1: Add failing mutation tests**

```rust
#[test] fn rejects_same_index_duplicate_pva_with_changed_filename_and_hash() { /* re-sign mutation */ }
#[test] fn rejects_cross_index_changed_depends() { /* signed second index */ }
#[test] fn rejects_cross_index_changed_provides() { /* signed second index */ }
#[test] fn rejects_cross_index_changed_multi_arch() { /* signed second index */ }
#[test] fn rejects_cross_index_changed_unknown_field() { /* signed second index */ }
```

- [ ] **Step 2: Run the bundle suite and confirm each new test is RED**

Run: `rtk cargo test --manifest-path scripts/Cargo.toml --test ubuntu_package_bundle signed`

Expected: conflicting records are accepted by the current file-identity deduplication.

- [ ] **Step 3: Implement producer normalization**

```python
group = (item["Package"], item["Version"], item["Architecture"])
canonical = tuple(sorted(item.items()))
if group in source_groups:
    fail("ambiguous signed package group in one index")
if group in groups and groups[group] != canonical:
    fail("conflicting signed package metadata across indexes")
```

- [ ] **Step 4: Implement the same semantics independently**

Apply a separate full-field implementation in `selected_packages()` in
`scripts/verify-ubuntu-debian-evidence.py`; do not import producer code.

- [ ] **Step 5: Run the targeted suite GREEN**

Run: `rtk cargo test --manifest-path scripts/Cargo.toml --test ubuntu_package_bundle`

Expected: all bundle tests pass, including identical cross-index republication.

### Task 3: Move Cache Bytes Behind Signed Validation

**Files:**
- Create: `scripts/ubuntu-package-cache.py`
- Modify: `scripts/produce-ubuntu-package-bundle.sh`
- Modify: `scripts/tests/ubuntu_package_bundle.rs`

**Interfaces:**
- Consumes: verified signed indexes, shared cache, private APT archive directory
- Produces: validated private staging and atomic validated publication

- [ ] **Step 1: Add behavioral temporary-cache tests**

```rust
#[test] fn poisoned_shared_cache_entry_is_not_staged() { /* fake dpkg-deb + bytes */ }
#[test] fn invalid_private_download_publishes_nothing() { /* preexisting cache unchanged */ }
#[test] fn valid_shared_cache_entry_is_reused_privately() { /* bytes copied */ }
#[test] fn interrupted_atomic_publish_leaves_no_destination_or_temp() { /* injected stop */ }
```

- [ ] **Step 2: Run those tests and confirm RED because the helper is absent**

Run: `rtk cargo test --manifest-path scripts/Cargo.toml --test ubuntu_package_bundle cache`

Expected: helper invocation fails or current producer-order assertion detects cache-before-signature use.

- [ ] **Step 3: Implement `stage`**

```python
record = signed_record_for(decoded_filename, package, version, arch)
if size(path) == record.size and sha256(path) == record.sha256:
    shutil.copyfile(path, private_archives / path.name)
```

Reject same-index/cross-index ambiguity before examining any cache candidate.
Never configure APT to read the shared directory directly.

- [ ] **Step 4: Implement validate-all-then-`publish`**

```python
validated = [validate(path) for path in private_archives.glob("*.deb")]
for source, destination in validated:
    temporary = destination.with_name("." + destination.name + ".tmp-" + nonce)
    copy_flush_fsync(source, temporary)
    os.replace(temporary, destination)
```

Validate every candidate before the first rename. Clean temporary files on
failure and support deterministic interruption injection for the regression.

- [ ] **Step 5: Reorder producer stages**

Fetch and verify signed releases/indexes first; invoke cache `stage`; run
private APT download; invoke cache `publish`; only then assemble repository
evidence.

- [ ] **Step 6: Run cache and bundle suites GREEN**

Run: `rtk cargo test --manifest-path scripts/Cargo.toml --test ubuntu_package_bundle`

Expected: all cache boundary tests and existing bundle tests pass.

### Task 4: Add Behavioral Command Evidence

**Files:**
- Create: `scripts/verify-ubuntu-command-evidence.sh`
- Modify: `scripts/produce-ubuntu-package-bundle.sh`
- Modify: `scripts/verify-ubuntu-debian-evidence.py`
- Modify: `scripts/tests/ubuntu_package_bundle.rs`
- Modify: `scripts/tests/image_user_contract.rs`

**Interfaces:**
- Consumes: exact installed package database plus `package-manifest.tsv`
- Produces: canonical `command-providers.tsv`

- [ ] **Step 1: Add missing/wrong/missing-Pico mutation tests**

```rust
#[test] fn rejects_missing_command_evidence() { /* delete dig line */ }
#[test] fn rejects_wrong_command_provider_or_path() { /* mutate provider/path */ }
#[test] fn independent_runtime_check_rejects_missing_pico_alternative() { /* fake PATH */ }
```

- [ ] **Step 2: Confirm RED**

Run: `rtk cargo test --manifest-path scripts/Cargo.toml --test ubuntu_package_bundle command`

Expected: missing file or mutation is currently not checked.

- [ ] **Step 3: Implement the producer writer after exact offline installation**

```bash
while IFS=$'\t' read -r command package; do
  path=$(command -v "$command")
  test -x "$path"
  resolved=$(readlink -f "$path")
  # Require exact installed manifest version/architecture and dpkg ownership.
  printf '%s\t%s\t%s\n' "$command" "$package" "$path"
done | LC_ALL=C sort -u >"$evidence/command-providers.tsv"
```

Install the exact local repository closure with all HTTP/HTTPS methods disabled,
`--no-download`, and `--no-install-recommends`; then require `dpkg --audit`.

- [ ] **Step 4: Implement the independent comparator**

`verify-ubuntu-command-evidence.sh` independently repeats exact manifest
version/architecture checks, executable resolution, resolved ownership, and
Pico alternative resolution, then compares canonical bytes.

- [ ] **Step 5: Make structural evidence verification require the canonical file**

The producer's embedded verifier validates exact required command/provider
pairs, absolute paths, unique canonical ordering, and no extra lines.

- [ ] **Step 6: Run command and bundle suites GREEN**

Run: `rtk cargo test --manifest-path scripts/Cargo.toml --test ubuntu_package_bundle`

Expected: mutations fail and the canonical fixture passes.

### Task 5: Independently Repeat Runtime Proof in CI

**Files:**
- Modify: `.github/workflows/workspace-bundles.yml`
- Modify: `scripts/tests/ubuntu_package_bundle.rs`

**Interfaces:**
- Consumes: extracted bundle evidence and repository source
- Produces: pinned offline independent validation before artifact promotion

- [ ] **Step 1: Add a failing workflow contract test**

Require the Ubuntu validation job to contain the exact pinned image,
`--platform linux/arm64`, `--network none`, read-only mounts, `timeout`,
local-only APT methods, exact manifest installation, `dpkg --audit`, and
`verify-ubuntu-command-evidence.sh`; reject credential mounts and privileges.

- [ ] **Step 2: Run the workflow test RED**

Run: `rtk cargo test --manifest-path scripts/Cargo.toml --test ubuntu_package_bundle workflow`

Expected: current host-only validator lacks the containerized command proof.

- [ ] **Step 3: Implement bounded offline container validation**

```bash
timeout --signal=KILL 300s docker run --rm --network none --platform linux/arm64 \
  --mount "type=bind,source=$GITHUB_WORKSPACE,target=/src,readonly" \
  --mount "type=bind,source=$RUNNER_TEMP/validated-evidence,target=/evidence,readonly" \
  "$image" bash -ceu 'install exact local packages; dpkg --audit; verify commands'
```

- [ ] **Step 4: Run workflow and bundle suite GREEN**

Run: `rtk cargo test --manifest-path scripts/Cargo.toml --test ubuntu_package_bundle`

Expected: workflow contract and all bundle regressions pass.

### Task 6: Regenerate, Verify, Report, and Commit

**Files:**
- Modify: `.superpowers/sdd/workstation-task-2-report.md` (ignored report only)
- Commit: all reviewed implementation/test/workflow files; do not amend prior commits

**Interfaces:**
- Consumes: validated content-scoped cache
- Produces: clean regenerated archive evidence and separate follow-up commit

- [ ] **Step 1: Regenerate in the exact pinned ARM64 environment**

Use the task-scoped cache and the pinned image. Require successful producer
exit, matching archive hash/size sidecars, and no invalid cache publication.

- [ ] **Step 2: Independently validate in a fresh pinned offline container**

Extract the archive; install exact manifest packages with network disabled;
require `dpkg --audit`; run signed/dependency verification and independent
command evidence comparison.

- [ ] **Step 3: Run focused and full verification**

```bash
rtk cargo test --manifest-path scripts/Cargo.toml --test connected_dockerfile
rtk cargo test --manifest-path scripts/Cargo.toml --test image_user_contract
rtk cargo test --manifest-path scripts/Cargo.toml --test ubuntu_package_bundle
rtk cargo test --manifest-path scripts/Cargo.toml
rtk cargo clippy --manifest-path scripts/Cargo.toml --all-targets --all-features -- -D warnings
rtk bash -n scripts/produce-ubuntu-package-bundle.sh
rtk bash -n scripts/verify-ubuntu-command-evidence.sh
rtk python3 -c 'compile(open("scripts/ubuntu-package-cache.py").read(), "scripts/ubuntu-package-cache.py", "exec")'
rtk python3 -c 'compile(open("scripts/verify-ubuntu-debian-evidence.py").read(), "scripts/verify-ubuntu-debian-evidence.py", "exec")'
rtk git diff --check
```

- [ ] **Step 4: Append behavioral evidence and approved file-list expansion**

Record RED/GREEN mutations, cache behavior, signed-index equivalence, pinned
offline command paths/providers (including Pico), archive hash/size/count,
workflow expansion approval, and final suite counts.

- [ ] **Step 5: Self-review behavior versus source-substring assertions**

Confirm cache-order and runtime-command requirements are exercised by processes
and temporary files. Source/workflow substring checks may only guard static CI
wiring, not substitute for producer/verifier behavioral tests.

- [ ] **Step 6: Create a separate implementation commit**

```bash
rtk git add .github/workflows/workspace-bundles.yml scripts/produce-ubuntu-package-bundle.sh \
  scripts/ubuntu-package-cache.py scripts/verify-ubuntu-command-evidence.sh \
  scripts/verify-ubuntu-debian-evidence.py scripts/tests/connected_dockerfile.rs \
  scripts/tests/image_user_contract.rs scripts/tests/ubuntu_package_bundle.rs
rtk git commit -m "fix: harden Ubuntu bundle evidence"
```
