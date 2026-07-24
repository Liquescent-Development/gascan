# Workstation Task 1 Review Repairs Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the four independent-review gaps in npm closure verification, generated-file publication, semantic mutation coverage, and Claude ELF validation.

**Architecture:** Keep resolution sequential so download concurrency is one, and verify every canonical npm closure URL against its locked SHA-512 with a 200 MiB per-artifact bound and 2 GiB aggregate bound. Publish the three generated files through same-directory staged files and durable backups, replacing the primary lock last and rolling every target back on any returned publication error. Extract ELF parsing into a pure validator so malformed headers and interpreter metadata can be tested without network access. Preserve normal mutable-alias update semantics while providing a separate read-only path for byte-verifying the already reviewed exact lock.

**Tech Stack:** Rust 2024, reqwest blocking client, serde JSON/TOML, tempfile, SHA-256/SHA-512, Cargo integration and unit tests.

## Global Constraints

- Prefix every shell command with `rtk`.
- Use strict red-green-refactor cycles.
- Never execute npm lifecycle scripts.
- Keep exact reviewed workstation versions and generated hashes stable.
- Preserve every unrelated and unknown primary-lock field.
- Commit these repairs separately; do not amend Task 1 commit `070f9f6`.

---

### Task 1: Verify Every npm Closure Tarball

**Files:**
- Modify: `scripts/src/bin/update-image-lock.rs`
- Test: `scripts/src/bin/update-image-lock.rs`

**Interfaces:**
- Consumes: npm lock `packages` records after missing integrities are filled.
- Produces: `verify_npm_closure_tarballs_with(packages, fetch)` and production `verify_npm_closure_tarballs(client, packages)`.

- [x] **Step 1: Write a fake-HTTP-server test with a canonical npm record whose syntactically valid SHA-512 does not match the served bytes.**
- [x] **Step 2: Run the focused unit test and confirm it fails because closure bytes are not fetched.**
- [x] **Step 3: Implement sequential canonical-URL fetching with a 200 MiB per-artifact limit and 2 GiB aggregate limit, comparing SHA-512 from every response byte-for-byte.**
- [x] **Step 4: Run the focused unit tests and confirm correct bytes pass while incorrect SRI fails with an integrity mismatch.**

### Task 2: Repair Semantic Mutation Tests

**Files:**
- Modify: `scripts/tests/update_image_lock.rs`

**Interfaces:**
- Consumes: mutated manifest/npm-lock JSON and the checked-in primary lock.
- Produces: a validation fixture whose primary aggregate hashes match the mutated generated inputs.

- [x] **Step 1: Change each semantic mutation to update the corresponding primary-lock aggregate hash and assert the intended semantic error text.**
- [x] **Step 2: Add a distinct stale-aggregate test that intentionally does not update the hash and asserts the early aggregate rejection.**
- [x] **Step 3: Run the integration tests and confirm the previously masked assertions fail at their intended semantic gates.**
- [x] **Step 4: Reorder only the necessary validation gates so closure, canonical identity, and lifecycle-bijection checks precede exact reviewed-evidence comparison.**
- [x] **Step 5: Run the integration tests and confirm every semantic assertion and the stale-hash assertion pass.**

### Task 3: Failure-Atomic Three-File Publication

**Files:**
- Modify: `scripts/src/bin/update-image-lock.rs`
- Test: `scripts/src/bin/update-image-lock.rs`

**Interfaces:**
- Consumes: validated manifest, npm-lock, and primary-lock bytes.
- Produces: `publish_generated_bundle(paths, bytes, boundary_hook)` with manifest then npm lock then primary-lock commit ordering.

- [x] **Step 1: Add a table-driven failure-injection test covering every stage-create, stage-write, stage-flush, stage-sync, backup-create, backup-write, backup-sync, backup-directory-sync, publish-rename, and directory-sync boundary.**
- [x] **Step 2: Run the test and confirm the current three independent `write_atomic` calls lack a transaction boundary.**
- [x] **Step 3: Stage, flush, sync, and validate all three temporary files before changing any target.**
- [x] **Step 4: Create and sync durable same-directory backups without removing the valid targets.**
- [x] **Step 5: Replace manifest, npm lock, and primary lock in that order; on any returned error, atomically restore exact backup bytes for all three and sync the directory.**
- [x] **Step 6: Run every failure boundary and success case, asserting exact old bytes after errors and exact new bytes after success.**
- [x] **Step 7: Document that fixed independent paths cannot provide process-crash multi-file atomicity, while returned publication errors are transactionally rolled back.**

### Task 4: Harden Claude ELF Validation

**Files:**
- Modify: `scripts/src/bin/update-image-lock.rs`
- Test: `scripts/src/bin/update-image-lock.rs`

**Interfaces:**
- Consumes: extracted `package/claude` bytes after exact tarball and binary digest verification.
- Produces: `validate_claude_elf(binary)` enforcing reviewed ELF64 Linux AArch64 structure.

- [x] **Step 1: Add focused mutations for truncation, EI_VERSION, ELF version, type, OS ABI, ABI version, header size, program-header bounds, and interpreter path.**
- [x] **Step 2: Run them and confirm the current four-byte checks accept malformed headers.**
- [x] **Step 3: Implement bounded little-endian ELF64 header/program-header parsing and exact reviewed interpreter validation when `PT_INTERP` exists.**
- [x] **Step 4: Run the focused tests and confirm the valid fixture passes and every mutation fails at the intended check.**

### Task 5: Regenerate, Verify, Review, and Commit

**Files:**
- Modify: `.superpowers/sdd/workstation-task-1-report.md` (ignored evidence only)
- Verify unchanged: `images/workspace/workstation-package.json`
- Verify unchanged: `images/workspace/workstation-package-lock.json`
- Verify unchanged: `images/workspace/versions.lock`

**Interfaces:**
- Consumes: all hardened resolver and publication behavior.
- Produces: stable generated bytes, full verification evidence, and a separate repair commit.

- [x] **Step 1: Run `--verify-existing-workstation-lock` and record verification of 156 resolved tarballs across 157 package records and 1,495,153,943 downloaded closure bytes.**
- [x] **Step 2: Confirm generated manifest/npm-lock hashes stay exact and unrelated primary-lock values remain unchanged; record mutable Pi `latest` drift to 0.82.0 without changing normal update semantics.**
- [x] **Step 3: Run focused mutation, failure-injection, ELF, and updater tests.**
- [x] **Step 4: Run the full scripts test suite, all-target clippy with `-D warnings`, targeted rustfmt check, and git diff checks.**
- [x] **Step 5: Self-review semantic test error assertions and every transaction rollback path.**
- [x] **Step 6: Append red/green/regeneration evidence and the process-crash limitation to the ignored report.**
- [x] **Step 7: Commit the repair changes separately and verify a clean worktree.**
