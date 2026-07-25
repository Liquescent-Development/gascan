# Workstation Formal Review Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the four remaining formal-review gaps without changing the sealed workstation image context or its approved public digest.

**Architecture:** Split candidate validation from explicit post-live approval, expose candidate selection only through E2E test-support plumbing, broaden immutable and command coverage in host/E2E tests, and exercise real credential-free tool state where safe. The published `fc3c17...` image remains valid because it subsequently passed Apple live acceptance; this work hardens future releases.

**Tech Stack:** Bash gate orchestration, Rust script tests, Rust Apple E2E harness, Apple container runtime, immutable GHCR ARM64 image.

## Global Constraints

- Preserve the reviewed Pi/protobuf lock; do not acquire or review drifted bytes.
- `--verify-existing-workstation-lock` is the release gate.
- Preserve connected acquisition for legacy image layers as a separate accepted boundary.
- The workstation layer remains sealed and offline.
- Do not change any file inside the sealed image context.
- Do not rebuild or republish unless a sealed-context file must change; stop first if that becomes necessary.
- Never authenticate or consume host credentials.
- Implement one strict RED/GREEN cycle at a time.
- Do not begin Managed SSH.

---

### Task 1: Candidate validation and explicit approval

**Files:**
- Modify: `scripts/run-connected-image-gate.sh`
- Create: `scripts/approve-connected-workspace-image.sh`
- Modify: `scripts/run-apple-e2e.sh`
- Modify: `scripts/tests/connected_image_gate.rs`
- Modify: `scripts/tests/apple_e2e_runner.rs`
- Modify: `crates/gascan-core/src/policy.rs`
- Modify: `crates/gascand/src/service.rs`
- Modify: `crates/gascand/src/main.rs`
- Modify: `crates/gascan-e2e/Cargo.toml`
- Modify: `crates/gascan-e2e/tests/apple_common/mod.rs`

**Interfaces:**
- Produces: candidate evidence under `.artifacts`, never tracked approval.
- Produces: an Apple-live receipt bound to the exact candidate image.
- Consumes: matching candidate and live receipts in an explicit approval script.

- [ ] Write focused tests proving successful candidate validation does not modify approval/final evidence and every failure or interruption leaves them unchanged.
- [ ] Run focused tests and capture the expected approval-ordering RED.
- [ ] Make the connected gate publish only atomic candidate evidence.
- [ ] Add E2E-only candidate image injection and an exact post-suite live receipt.
- [ ] Add explicit approval publication that rejects missing, malformed, failed, or mismatched live receipts and rolls back every publication boundary.
- [ ] Run focused tests and capture GREEN.

### Task 2: Full immutable override proof

**Files:**
- Modify: `crates/gascan-e2e/tests/apple_apply.rs`

**Interfaces:**
- Consumes: recursive metadata and content snapshot of `/opt/gascan`.
- Produces: equality proof before and after mutable mise override.

- [ ] Write a focused static test requiring the live probe to cover all regular files under `/opt/gascan`.
- [ ] Run it and capture RED against the workstation-only probe.
- [ ] Change both live snapshots to hash all regular `/opt/gascan` files with deterministic relative paths, metadata, and contents.
- [ ] Run the focused test and capture GREEN.

### Task 3: Real credential-free agent and forge state

**Files:**
- Modify: `crates/gascan-e2e/tests/apple_apply.rs`

**Interfaces:**
- Produces: safe unauthenticated command/config/cache/log state for Claude, Codex, Pi, Herdr, gh, and glab inside managed volumes.
- Produces: down/up and predecessor/candidate replacement persistence proof.

- [ ] Write focused static coverage for real safe tool invocations and managed path assertions; explicitly identify any tool without a safe write command.
- [ ] Run it and capture RED against sentinel-only behavior.
- [ ] Seed and probe real credential-free state through the tools where supported, retaining a sentinel only for a documented unsupported case.
- [ ] Run focused tests and capture GREEN.

### Task 4: Every advertised diagnostic command

**Files:**
- Modify: `tests/image/workstation-smoke.sh`
- Modify: `scripts/tests/connected_image_gate.rs`

**Interfaces:**
- Produces: deterministic offline-safe host-side live assertions for `nslookup`, `curl`, `wget`, `rsync`, `lsof`, `file`, `jq`, `ps`, `top`, `pstree`, `tree`, and `less`.

- [ ] Write a focused static test requiring all advertised diagnostic invocations.
- [ ] Run it and capture RED.
- [ ] Add bounded deterministic invocations to the host-side smoke only.
- [ ] Run focused tests and capture GREEN.

### Task 5: Verification, evidence, and commit

**Files:**
- Modify: `.superpowers/sdd/workstation-task-5-report.md` (ignored)

**Interfaces:**
- Produces: review-ready RED/GREEN and final verification evidence.

- [ ] Run targeted tests after each task.
- [ ] Run all impacted workspace and scripts suites, formatting, clippy, shell syntax, and diff checks.
- [ ] Run the public-digest Apple 5/5 suite because candidate selection and E2E persistence behavior changed.
- [ ] Confirm the sealed-context digest and approved public image remain unchanged.
- [ ] Append binding decisions, RED/GREEN evidence, final commands, outcomes, and concerns to the ignored report.
- [ ] Commit with a separate clear subject.
