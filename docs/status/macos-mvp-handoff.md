# Gas Can macOS MVP Handoff

Last reconciled: 2026-07-15

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
- The earlier builder-egress failure was caused by a local firewall. A strict
  apt-bootstrap-plus-HTTPS diagnostic build passed after correction.
- Offline ARM64 bundles are deferred hardening and are not a Gate 4 prerequisite.
- Private Gascamp credentials must never enter the transmitted Docker build
  context or persist in layers, logs, evidence, or temporary host files after
  cleanup.
- For the MVP on Apple Containerization 1.1.0, the validated external Gascamp
  token is copied into a fresh private host context, excluded from the
  transmitted Docker context by `.dockerignore`, mounted only as a BuildKit
  secret, and removed through bounded cleanup. This supersedes the infeasible
  external-`src` command shape while preserving non-retention requirements.

## Roadmap Status

| Phase or gate | Status | Durable evidence |
|---|---|---|
| Phase 0 / Gate 1 | Passed and integrated | `48a7a18` |
| Phase 1 Apple feasibility / Gate 2 | Passed and integrated | `6bedef8`, `docs/feasibility/apple-container-report.md` |
| Phase 1 core control plane / Gate 3 | Passed and integrated | `7c7d083`, integration record `917dac1` |
| Phase 2 Apple backend implementation | Implemented and reviewed on feature branch; not integrated | head `dbf4235` |
| Phase 2 workspace image | Offline implementation exists; connected MVP Tasks 1–4 are approved; Tasks 5–7 remain | provisioning head includes `b03add2`, `docs/evidence/apple-build-secret.md`, and the connected-build addendum |
| Gate 4 real lifecycle | Pending; harness approved but no complete real lifecycle evidence | harness `dbf4235` |
| Phase 3 security, packaging, release | Not started as an integrated phase | blocked by Gate 4 |
| Gate 5 clean-host release | Pending | no evidence |

Only Gates 1, 2, and 3 have passed. Gate 5 is the definition of MVP completion.

## Branch and Worktree Inventory

The recorded locations and accepted heads are:

| Worktree | Branch | Accepted head or integration point | Purpose |
|---|---|---|---|
| `.worktrees/macos-mvp` | `feature/macos-mvp` | `917dac1` | integration branch through Gate 3 |
| `.worktrees/apple-backend` | `feature/apple-backend` | `dbf4235` | reviewed Plan 3 implementation and Gate 4 harness |
| `.worktrees/provisioning` | `feature/provisioning` | `9025c56` before this documentation change | image, Gascamp, offline bundles, and image gates |

Do not merge feature branches wholesale without reviewing their merge base and
overlap. The provisioning branch contains a deliberately deferred offline
Dockerfile path that must be converted for the connected MVP before Gate 4.

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

The harness is approved; Gate 4 itself is not passed.

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
- `b03add2`: approved structural pinned-Gascamp secret-mounted builder and
  final `/out` credential/source boundary; no real private build was run.

These offline commits are reviewed assets, not completed publication or live
image evidence. `images/workspace/versions.lock` still says
`publication = "pending"`.

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

1. Execute Task 5 of
   `docs/superpowers/plans/2026-07-15-connected-workspace-image.md`, then
   continue task by task with independent review gates. Tasks 1–4 are complete
   and must not be redispatched. The user has preselected subagent-driven
   execution; do not re-plan the program.
2. Convert the image path from mandatory offline bundles to connected locked
   acquisition while preserving all reviewed image contracts.
3. Select and prove the private Gascamp credential boundary.
4. Build the real ARM64 workspace image and record its exact digest.
5. Integrate the Apple backend and connected image work into
   `feature/macos-mvp` with conflict review and full verification.
6. Inventory Plan 4 Tasks 4–6 against their plan; do not infer their completion
   from the Gascamp selector commit alone.
7. Run the complete Gate 4 real lifecycle: `up`, `shell`, `run`, `apply`,
   `down`, restart, reconciliation, and `destroy`, including PTY, signals,
   exact exits, and residue checks.
8. Complete Plan 4 security acceptance, packaging, installation, and clean-host
   release work.
9. Run and record Gate 5.

## Fresh-Session Restart Procedure

From the repository root:

```sh
git status --short
git worktree list
git -C .worktrees/macos-mvp log -1 --oneline
git -C .worktrees/apple-backend log -1 --oneline
git -C .worktrees/provisioning log -1 --oneline
```

Then read, in order:

1. this handoff;
2. `docs/superpowers/plans/2026-07-13-gascan-macos-roadmap.md`;
3. `docs/superpowers/specs/2026-07-15-connected-mvp-build-design.md`;
4. `docs/superpowers/plans/2026-07-15-connected-workspace-image.md`;
5. the relevant task plan before modifying its branch.

If the fresh session is asked to continue implementation, use
`superpowers:subagent-driven-development` on Task 1 of the connected-build Plan
4 addendum. If the fresh session is asked only for status, report from this
document and do not dispatch implementation agents.

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
