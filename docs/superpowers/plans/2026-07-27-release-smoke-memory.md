# Release Smoke Memory Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prevent the installed-release polyglot smoke from killing the Go compiler by giving its sandbox 1 GiB of memory.

**Architecture:** Keep the existing end-to-end release smoke unchanged except for its sandbox memory allocation. Add a source-contract test in the scripts test crate that couples the 1 GiB allocation to the real Go installation workload.

**Tech Stack:** Bash, Rust integration tests, Cargo

## Global Constraints

- Change only repository release tooling; do not change or republish Gascan 0.1.10.
- Keep the release-smoke CPU limit at one CPU.
- Keep the Cargo, npm, Go, Python, and Ruby installation workloads intact.
- The final installed smoke must print `PASS: installed Gas Can release smoke`.

---

### Task 1: Guard and Raise the Release-Smoke Memory Limit

**Files:**
- Create: `scripts/tests/macos_release_smoke.rs`
- Modify: `packaging/macos/release-smoke.sh:71`

**Interfaces:**
- Consumes: `packaging/macos/release-smoke.sh` as repository release-tooling source.
- Produces: a scripts-crate regression test requiring `memory = "1GiB"` while the real `go install ./go-bin` workload remains present.

- [ ] **Step 1: Write the failing source-contract test**

```rust
use std::{fs, path::PathBuf};

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("scripts has repository parent")
        .to_path_buf()
}

#[test]
fn polyglot_release_smoke_allocates_one_gibibyte() {
    let smoke =
        fs::read_to_string(repository_root().join("packaging/macos/release-smoke.sh")).unwrap();

    assert!(
        smoke.contains(r#"memory = "1GiB""#),
        "polyglot release smoke must allocate 1 GiB"
    );
    assert!(
        smoke.contains(r#"(cd "$fixture" && go install ./go-bin)"#),
        "memory contract must remain coupled to the real Go compiler workload"
    );
}
```

- [ ] **Step 2: Run the focused test and verify RED**

Run:

```bash
cargo test --manifest-path scripts/Cargo.toml --test macos_release_smoke
```

Expected: FAIL at `polyglot release smoke must allocate 1 GiB` because the script still declares `memory = "256MiB"`.

- [ ] **Step 3: Make the minimal harness change**

In `packaging/macos/release-smoke.sh`, change only:

```toml
memory = "256MiB"
```

to:

```toml
memory = "1GiB"
```

- [ ] **Step 4: Run the focused test and verify GREEN**

Run:

```bash
cargo test --manifest-path scripts/Cargo.toml --test macos_release_smoke
```

Expected: one test passes with zero failures.

- [ ] **Step 5: Run relevant repository verification**

Run:

```bash
cargo test --manifest-path scripts/Cargo.toml
cargo fmt --manifest-path scripts/Cargo.toml --all --check
git diff --check
```

Expected: the complete scripts suite passes, formatting reports no changes, and `git diff --check` exits zero.

- [ ] **Step 6: Commit the implementation**

```bash
git add scripts/tests/macos_release_smoke.rs packaging/macos/release-smoke.sh
git commit -m "test: give release smoke compiler headroom"
```

### Task 2: Publish and Prove the Harness Fix

**Files:**
- Verify: `packaging/macos/release-smoke.sh`
- Verify: `scripts/tests/macos_release_smoke.rs`

**Interfaces:**
- Consumes: the installed `/usr/local/bin/gascan` 0.1.10 package and the corrected repository smoke script.
- Produces: a merged follow-up PR and an end-to-end installed-release smoke result with no owned residue.

- [ ] **Step 1: Review the branch diff and rerun pre-push checks**

Run:

```bash
git diff origin/main...HEAD --check
cargo test --manifest-path scripts/Cargo.toml --test macos_release_smoke
```

Expected: both commands exit zero.

- [ ] **Step 2: Push and create the follow-up PR**

```bash
git push -u origin fix/release-smoke-memory
gh pr create --base main --head fix/release-smoke-memory
```

The PR must state that this is a release-harness-only correction and does not require republishing 0.1.10.

- [ ] **Step 3: Merge the reviewed PR and synchronize its exact commit**

Merge the PR after checks pass, then fetch `origin/main` and verify the merged tree contains the 1 GiB smoke limit and regression test.

- [ ] **Step 4: Run the installed-release smoke from merged source**

Run in the same interactive terminal as `sudo -v`:

```bash
./packaging/macos/release-smoke.sh
```

Expected: `PASS: installed Gas Can release smoke`.

- [ ] **Step 5: Verify smoke-owned cleanup**

Verify there are no `gate5-release-*` containers, volumes, DNS records, temporary roots, or host HTTP servers after the smoke exits.

- [ ] **Step 6: Remove merged temporary worktrees and branches**

After verifying both worktrees are clean, remove `.worktrees/writable-runtime-homes` and `.worktrees/release-0.1.10`, then remove the merged local and remote feature/release branches. Preserve the dirty primary checkout unchanged.
