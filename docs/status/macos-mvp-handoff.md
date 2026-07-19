# Gas Can macOS MVP Handoff

Last reconciled: 2026-07-19

This is the canonical restart document for a fresh agent or session. Read it
with `docs/superpowers/plans/2026-07-13-gascan-macos-roadmap.md` before changing
code. Verify branch heads rather than assuming the paths below still exist.

The 2026-07-15 documentation work made current state durable; it did not start
a new implementation effort. The connected workspace image document is a
focused continuation addendum to existing Plan 4. Addendum Task 1 exposed and
then accommodated an Apple BuildKit 0.12.0 restriction: secret `src` paths
must be descendants of the host context directory. The revised probe passed
and was independently approved through `44bb3b2`.

## Product Boundary

Gas Can is a secure sandbox for agentic coding. On macOS it uses Apple's
lightweight Linux VMs and always runs the user workspace inside a container.
The CLI surface is `gascan up <code-root>`, `gascan shell`, `gascan run`,
`gascan apply`, `gascan down`, and `gascan destroy`; a future GUI connects to
the same daemon API. Only the canonical code root is mounted. The workspace
user is non-root, with deliberate guest-root access through sudo. mise provides
the polyglot toolchain.

## Authoritative Decisions

- macOS first; Linux/Firecracker is later.
- Always use a container inside the lightweight VM.
- CPU and memory limits are supported on Apple 1.1. Explicit disk and process
  limits fail closed because the selected API cannot enforce them.
- Root is allowed inside the sandbox through sudo; the image default user
  remains `workspace`.
- Runtime offline mode uses Apple's no-network container configuration and was
  proven in Gate 2. This is separate from image-builder networking.
- The MVP workspace image is a connected, locked build. Deliberate builder-VM
  network isolation is not required.
- Image construction is development and release work. End-user distribution
  consumes a prebuilt image; the actual distribution packaging remains Gate 5
  work.
- The user approved the successfully built connected workspace image recorded
  below as the prebuilt MVP image input while Apple builder reliability is
  repaired separately.
- The earlier builder-egress failure was caused by a local firewall. A strict
  apt-bootstrap-plus-HTTPS diagnostic build passed after correction.
- Offline ARM64 bundles are deferred hardening and are not a Gate 4 prerequisite.
- Gascamp is publicly readable at
  `https://github.com/Liquescent-Development/gascamp.git`; anonymous
  `git ls-remote` succeeded on 2026-07-15. The MVP build uses no Gascamp
  token, credential file, credential helper, authentication header, or
  BuildKit secret.
- The reviewed Apple BuildKit secret probe remains capability evidence but is
  not an MVP image-build or Gate 4 prerequisite.
- Apple BuildKit does not transfer the root-owned privileged snapshot payload
  into its build context, even when its host manifest is valid. The connected
  MVP therefore builds directly from the caller-owned Task 2 context after
  canonical manifest verification before and after the build. This accepts a
  trusted-local-caller assumption for transient in-build mutation; a
  transferable sealed context remains deferred hardening. The helper and
  offline path are retained unchanged.

## Roadmap Status

| Phase or gate | Status | Durable evidence |
|---|---|---|
| Phase 0 / Gate 1 | Passed and integrated | `48a7a18` |
| Phase 1 Apple feasibility / Gate 2 | Passed and integrated | `6bedef8`, `docs/feasibility/apple-container-report.md` |
| Phase 1 core control plane / Gate 3 | Passed and integrated | `7c7d083`, integration record `917dac1` |
| Phase 2 Apple backend implementation | Implemented, reviewed, integrated, and accepted through Gate 4 | accepted integration head `a475f8c` |
| Phase 2 workspace image | Connected ARM64 prebuilt image accepted and frozen into integrated policy | image head `f6ed3a5`, merge `229c33a`, `docs/evidence/connected-image-handoff.md` |
| Gate 4 real lifecycle | Passed on the supported live Apple platform | accepted implementation head `a475f8c`; exact serial evidence below |
| Phase 3 security, packaging, release | Not started as an integrated phase | Gate 4 prerequisite satisfied; Gate 5 work remains |
| Gate 5 clean-host release | Pending | no evidence |

Gates 1 through 4 have passed. Gate 5 remains pending and is the definition of
MVP completion; the MVP is not complete.

## Branch and Worktree Inventory

The recorded locations and accepted heads are:

| Worktree | Branch | Accepted head or integration point | Purpose |
|---|---|---|---|
| `.worktrees/macos-mvp` | `feature/macos-mvp` | `917dac1` | integration branch through Gate 3 |
| `.worktrees/apple-backend` | `feature/apple-backend` | `dbf4235` | reviewed Plan 3 implementation and Gate 4 harness |
| `.worktrees/provisioning` | `feature/provisioning` | `f6ed3a5` | accepted connected image, Gascamp, offline bundles, and image gates |
| `.worktrees/gate4-integration` | `feature/gate4-integration` | accepted Gate 4 implementation head `a475f8c` | Gate 4 integration and accepted live lifecycle evidence from frozen base `917dac1` |

The Task 7 feature merges were reviewed from their shared frozen base rather
than accepted wholesale. The deliberately deferred offline path remains
separate from the connected MVP input and is not a Gate 4 prerequisite.

## Accepted Implementation Milestones

### Integrated foundation

- `48a7a18`: Gate 1 probe seam freeze.
- `6bedef8`: Gate 2 Apple feasibility evidence.
- `7c7d083`: Gate 3 fake-backend end-to-end evidence.
- `917dac1`: integrated Gate 3 roadmap evidence.

### Apple backend branch

- `b2b98ac`: Apple request translation and macOS resource contract.
- `e3a2291`: structured inspection and ownership classification.
- `ce6b5b0`: lifecycle reconciliation and cleanup.
- `745c516`: bounded, ordered attach bridge.
- `109a7a3`: production backend selection and evidence-bearing doctor.
- `dbf4235`: approved Gate 4 harness cleanup and teardown safety.

This branch head is historical input to the accepted Gate 4 integration.

### Task 7 integration

- `d06d619`: explicit non-squashed merge of reviewed Apple head `dbf4235`.
- `229c33a`: explicit non-squashed merge of accepted connected-image head
  `f6ed3a5`, with the progress-ledger conflict resolved by preserving both
  current histories.
- `6a81545`: freezes
  `images/workspace/approved-image.txt` into policy and records the reviewed
  handoff in `docs/evidence/connected-image-handoff.md`.
- `b09f573` through `271db68`: add and harden the real CLI PTY-resize path.
  The accepted bounded state machine owns the child throughout execution,
  changes the PTY from 24 by 80 to exactly 47 rows by 132 columns, sends
  `SIGWINCH`, and requires the guest to report `47 132`. Reviewed regressions
  cover bounded cleanup after failures, a descendant retaining PTY
  descriptors, a forced kill failure, and lossless bounded-batch draining of
  a 262,163-byte chatty-child transcript.
- `306e0b6`: resolves the final integration review's signal-path findings with
  a bounded PTY lifecycle. The reviewed harness now exercises
  contract-correct `SIGINT` propagation through a real TTY. It also sends a
  real OS `SIGTERM` to the TTY-attached CLI and proves that the CLI promptly
  returns its typed `unsupported_capability` error without delivering the
  unsupported signal to the guest.

Task 7 is accepted at `306e0b6`. Its platform-neutral harness evidence did not
by itself pass Gate 4; the later serial live runs recorded below did.

### Gate 4 acceptance

Gate 4 passed on 2026-07-19 at accepted implementation head
`a475f8c7e1e1c955ea28279c5f711ee2b8c8f2ac`. The exact required commands ran
serially on the same live platform:

1. `bash ./scripts/run-apple-e2e.sh apple_lifecycle` exited 0.
   `cli_lifecycle_survives_daemon_and_host_state_changes ... ok`; 1 passed,
   0 failed, 0 ignored, 26 filtered out; 6.77 seconds.
2. Only after lifecycle passed,
   `bash ./scripts/run-apple-e2e.sh apple_recovery` exited 0.
   `cli_recovers_from_stale_daemon_metadata_and_runtime_truth ... ok`;
   1 passed, 0 failed, 0 ignored, 26 filtered out; 6.53 seconds.

Both runs reported macOS 26.5.1 on arm64, Apple `container` 1.1.0 release at
commit `5973b9cc626a3e7a499bb316a958237ebe14e2ed`, and
`container-apiserver` 1.1.0 at the same commit. After both passed, read-only
inventory checks found no IDs or names containing `gate4` in
`container list --all --format json` or `container volume list --format json`;
`/private/tmp/gascan-gate4-501` was absent or empty. Unrelated pre-existing
Apple resources were preserved.

The accepted implementation includes these independently clean-reviewed
corrections after the previously recorded Task 7 head:

- `8cc59c3`: safe protocol-v2 per-exec terminal/locale environment overlay.
- `a686344`: exact raw Apple guest PTY CRLF harness expectation.
- `a475f8c`: bounded PTY resize readiness diagnostics.

This is Gate 4 evidence only. It does not claim distribution,
signing/notarization, clean-host installation, Gate 5, or MVP completion.

### Provisioning and offline branch

- `c99bbaf`: Gascamp source selector implementation.
- `809796e`: immutable bundle contract.
- `db988c3`: Ubuntu ARM64 package producer.
- `a6f3cf1`: mise runtime producer.
- `b22247f`: Gascamp source/vendor producer.
- `144615a`: privileged snapshot helper hardening.
- `65e49fe`: network-independent Dockerfile assembly.
- `9025c56`: truthful PENDING offline image gate scaffold.
- `44bb3b2`: approved Apple BuildKit staged-secret isolation probe and live
  non-retention evidence, including bounded ownership-checked cleanup.
- `1878744`: approved connected lock, public acquisition boundary, and atomic
  minimal context preparation with offline mode preserved.
- `7a429cb`: approved connected Ubuntu and mise polyglot base assembly with
  exact reviewed package input and preserved final-user contracts.
- `b03add2` and `61fd8d9`: historical reviewed implementations based on the
  since-corrected private-repository assumption. Their Gascamp secret and
  wrapper paths are superseded; `61fd8d9`'s structured inspection, cleanup,
  and receipt protections remain in the accepted implementation. The helper
  and offline snapshot remain deferred hardening rather than part of the
  connected MVP path.
- `5ae9567`: records the corrected public Gascamp source boundary.
- `321f87f`: completes the accepted simplification to an anonymous fetch of
  the exact pinned public Gascamp revision, without a token, credential file,
  credential helper, authentication header, or BuildKit secret.
- `30dd514`: independently approved platform-neutral connected image gate
  harness with transactional publication, bounded ownership-checked cleanup,
  real-smoke fake-controller coverage, and authoritative residue inventory.
  This is not a live image PASS.

These offline commits are reviewed assets, not completed publication or live
image evidence. `images/workspace/versions.lock` still says
`publication = "pending"`.

**Accepted prebuilt MVP image input.** On 2026-07-18 the connected workspace
image gate passed on Apple Container 26.5.1 using the exact public GHCR index
`ghcr.io/liquescent-development/gascan/workspace:d4964500a3295a33@sha256:49ba6a63ce745b7f2238e609b556776b7aab12ac0eb5f741fc47ca164dc8aeac`.
Anonymous public registry inspection and pull, all three image smokes, and
current-run cleanup passed. The authoritative tracked records are
`docs/evidence/connected-workspace-image.md` and
`images/workspace/approved-image.txt`.

Apple builder context-streaming reliability remains separately tracked by
[`Liquescent-Development/gascan#1`](https://github.com/Liquescent-Development/gascan/issues/1)
and does not invalidate the accepted prebuilt image. The issue records the
default 2 GiB builder SIGKILL, success with 4 CPUs and 4 GiB, the reused-builder
`demux channel full` / invalid tar header failure, and Docker/OCI-import
investigation. Offline bundle publication and builder-VM network isolation
remain deferred and are not MVP blockers.
Roadmap Gate 4 passed with the exact serial live lifecycle and recovery
evidence recorded above. Roadmap Gate 5 remains pending and remains the
definition of MVP completion. This evidence does not claim Phase 3 completion,
distribution, signing/notarization, clean-host installation, Gate 5, or MVP
completion.

## Verified Environmental Facts

- Controller: Apple silicon macOS 26.5.1, Apple `container` CLI 1.1.0 during
  Plan 3 live doctor verification.
- The operator network permits DNS only through `10.10.10.53`; Apple VMs see a
  gateway resolver such as `192.168.64.1` or `192.168.66.1` that forwards
  successfully.
- Gate 2 runtime-network tests passed 9/9 in the final full run. Public HTTP
  probes were diagnostic; the owned host endpoint and structured empty network
  configuration were the isolation proof.
- On 2026-07-15 a local firewall blocked Apple VM TCP while DNS continued to
  work. After correction, runtime HTTPS and builder TCP 443 worked.
- The final strict diagnostic builder installed `ca-certificates` and `curl`
  through signed Ubuntu apt metadata and fetched `https://example.com`.

Do not encode the operator DNS IP into Gas Can product policy.

## Current Unfinished Work

The connected image was built from the anonymous public Gascamp source at the
exact pinned revision and approved as the prebuilt MVP input. The build uses
the caller-owned verified context directly because Apple BuildKit omits the
root-owned snapshot payload; the helper remains only deferred/offline
hardening.

1. Continue investigating Apple builder reliability separately under issue #1;
   do not make end-user distribution rebuild the image.
2. Complete Plan 4 security acceptance, packaging, installation, and clean-host
   release work.
3. Run and record Gate 5. Until it passes, the MVP is not complete.

## Fresh-Session Restart Procedure

From the repository root:

```sh
git status --short
git worktree list
git -C .worktrees/macos-mvp log -1 --oneline
git -C .worktrees/apple-backend log -1 --oneline
git -C .worktrees/provisioning log -1 --oneline
git -C .worktrees/gate4-integration log -1 --oneline
```

Then read, in order:

1. this handoff;
2. `docs/superpowers/plans/2026-07-13-gascan-macos-roadmap.md`;
3. `docs/superpowers/specs/2026-07-15-connected-mvp-build-design.md`;
4. `docs/superpowers/plans/2026-07-15-connected-workspace-image.md`;
5. the relevant task plan before modifying its branch.

If the fresh session is asked to continue implementation, start from accepted
Gate 4 head `a475f8c` and proceed only with explicitly authorized Phase 3 or
Gate 5 work. If the fresh session is asked only for status, report from this
document and do not run live gates.

Before claiming any gate, run the roadmap's program-level verification and the
gate-specific live suite. Never infer Gate 4 from harness tests or Gate 5 from
unit tests.

## Deferred Work

- Publish and exercise the three offline ARM64 bundles.
- Remove the deliberate PENDING stop and re-enable the cold/warm/corruption
  offline image gate through a reviewed future change.
- Add Linux/Firecracker support.
- Add a GUI client over the daemon API.
- Consider builder egress denial as defense-in-depth if Apple exposes a stable
  supported control; it is not an MVP requirement.
