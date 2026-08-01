### Task 10: Document, verify, review, and merge the feature

**Files:**
- Modify: `README.md`
- Modify: `packaging/macos/release-smoke.sh`
- Modify: `scripts/tests/macos_release_smoke.rs`

**Interfaces:**
- Produces documented `gascan configure`, `configure git`, `configure gh`, and
  `configure glab` workflows including `--hostname`, `--token-stdin`, SSH/HTTPS,
  enterprise hosts, focused retries, persistence/security, offline behavior,
  and destroy cleanup.
- Produces release smoke coverage for imported Git identity, SSH signing,
  signed commit/tag creation, nested Starship, and credential persistence using
  fake forge CLIs and no real tokens.
- Produces `GASCAN_RELEASE_GASCAND`, defaulting to `/usr/local/bin/gascand`, so
  branch smoke attests to and shuts down the daemon matching the tested CLI.

- [ ] **Step 1: Add failing README and release-smoke contracts**

Require every public command/flag and the complete security/persistence model.
Add a smoke fixture that configures Git with fake forge CLIs and verifies a
signed commit and tag. Prove daemon attestation and shutdown use
`GASCAN_RELEASE_GASCAND`, never a hard-coded installed daemon.

- [ ] **Step 2: Update quickstart and reference**

Quickstart must show `gascan up .`, the optional first-use developer setup, and
focused retry commands. Explain host-global import, hidden token input,
global-only defaults, native credential files, no Gas Can vault, per-sandbox
key revocation, GitHub's separate auth/signing registrations, GitLab
`auth_and_signing`, enterprise hostnames, SSH/HTTPS tradeoffs, offline behavior,
destroy cleanup, and `git log --show-signature -1` verification.

- [ ] **Step 3: Run formatting and focused suites**

Run the exact Task 10 Step 3 commands from
`docs/superpowers/plans/2026-07-30-developer-onboarding.md`. If the direct-host
workstation contract is invalid because it requires sealed build context,
prove that from the contract and use the authoritative prebuilt/public image
gate already accepted in Task 9; do not weaken or bypass image verification.

- [ ] **Step 4: Run full workspace and release contracts**

Run the exact Task 10 Step 4 commands, including branch-built `gascan` and
`gascand` release smoke. The smoke must not read or transmit real host forge
tokens. Preserve all unrelated user sandboxes/worktrees.

- [ ] **Step 5: Commit documentation and smoke coverage**

Stage only the three planned files plus directly necessary contract fixtures;
run cached diff checks and commit `docs: explain developer onboarding`.

- [ ] **Step 6: Two-stage independent review**

First review against the approved design/plan, then independently review code
quality and security. Fix every valid Critical/Important finding with a failing
regression; resolve Minor findings that affect correctness/security or user UX.

- [ ] **Step 7: Final branch verification**

Use `superpowers:verification-before-completion` and run the exact Task 10 Step
7 commands. Require a clean branch and fresh passing evidence.

- [ ] **Step 8: Push, create, verify, and squash-merge the feature PR**

Push `feat/developer-onboarding`, create the feature PR with a reviewed body,
watch required checks, and squash-merge/delete the remote branch. Record PR URL
and squash commit. Do not delete this active worktree until the 0.1.17 release
is complete.

**Controller boundary:** The implementer stops after the documentation/smoke
commit and clean report. The controller owns independent review, final branch
verification, remote push/PR/checks/merge, and recording the squash commit.
