# Connected image integration handoff

Date: 2026-07-18

This record prepares the serial Roadmap Gate 4 run. It does not execute or
pass Gate 4, Gate 5, or the MVP.

## Frozen integration inputs

- frozen base: `917dac18fd8fcce1b9c736fdc4a5e3482f7e1e7d`
- Apple backend and safe Gate 4 harness head:
  `dbf423560acc76f98d1f045a95c0c3669e45a71f`
- Apple merge commit: `d06d619684eecb8a2c880294a4264d57e9e08b28`
- connected workspace image head:
  `f6ed3a5dff638174083edd15e0a8ef1b628aca8b`
- connected-image merge commit:
  `229c33ade5abfe9b327e0a9fc9f22e9e834e5e1d`
- integration branch: `feature/gate4-integration`
- accepted Task 7 integration head:
  `306e0b68a738ece0a86040b7ee7dd9767dba99d8`

Both histories were merged with explicit non-squashed merge commits, Apple
first and connected image second.

## Overlap, conflict, and interface review

All three relevant merge bases were the frozen base `917dac1`. The only paths
changed by both reviewed ranges were `.superpowers/sdd/progress.md`,
`crates/gascan-core/src/lib.rs`, and
`docs/superpowers/plans/2026-07-13-gascan-macos-roadmap.md`.

- The Apple merge had no conflicts.
- The connected-image merge had one content conflict in
  `.superpowers/sdd/progress.md`. The frozen base held the former integrated
  task ledger, Apple replaced it with the current Apple task ledger, and the
  connected branch appended its independent connected-task ledger. The
  resolution retains the current Apple ledger plus every connected-task entry;
  it does not duplicate the obsolete frozen ledger.
- `crates/gascan-core/src/lib.rs` merged additively: Apple exports `doctor` and
  connected image exports `gascamp`.
- The roadmap additions were complementary. The Apple capability binding and
  the connected image/status sections were both retained, then reconciled by
  this Task 7 commit.

`RuntimeBackend`, its request and ownership models, Apple command translation,
structured inspection, and lifecycle cleanup come from the Apple history. The
connected history does not change `RuntimeBackend`; it adds Gascamp/image
construction contracts and the approved image record. `PolicyCompiler` seals
the tracked approved reference into `CreateRequest.image`, and Apple command
translation forwards that exact value. The lifecycle harness uses the normal
CLI/policy path. It does not explicitly pull or rebuild an image, read an image
override from the environment, or select a fallback image.

There was no unresolved interface contradiction.

## Approved image and policy freeze

The exact approved image is
`ghcr.io/liquescent-development/gascan/workspace:d4964500a3295a33@sha256:49ba6a63ce745b7f2238e609b556776b7aab12ac0eb5f741fc47ca164dc8aeac`.

`docs/evidence/connected-workspace-image.md` is the authoritative accepted live
receipt record. It records `PASS`, `linux/arm64`, the same exact reference, and
current-run residue absent. `images/workspace/approved-image.txt` contains the
same 136 bytes with no trailing newline or other whitespace, one `@`, and a
SHA-256 digest qualifier.

The policy regression first failed against the old policy fixture, then passed
after `WORKSPACE_IMAGE` became an `include_str!` of the authoritative marker.
The marker is therefore the single policy source of truth; the reference was
not retyped into Rust.

## Platform-neutral verification

On this integration worktree:

- `cargo fmt --all -- --check`: PASS.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`: PASS.
- The final main `cargo test --workspace` rerun passed 312 tests with 12
  ignored. One earlier attempt failed the fake-runner test
  `exec_bridge_accepts_input_while_output_is_pending` with the error
  `session input is closed`. The test then passed three isolated runs and the
  complete rerun. The failure's cause was not established, and none of this is
  recorded as a product or live-runtime PASS.
- The final independent-review `gascan-e2e` run passed 58 platform-neutral
  tests with 2 live tests ignored. Focused `apple_lifecycle` and
  `apple_recovery` coverage each passed 14 platform-neutral tests with its
  single live test ignored.
- The reviewed PTY state machine owns the child through bounded readiness,
  execution, post-exit drain, and cleanup. It changes a real local terminal
  from 24 by 80 to exactly 47 rows by 132 columns, delivers `SIGWINCH`, and
  observes exact `stty size` output `47 132`. Regressions also prove that it
  does not wait for a descendant-owned PTY descriptor, returns promptly while
  preserving both errors after an injected kill failure, reaps owned children
  after other failure paths, and drains an exact 262,163-byte chatty-child
  transcript without the former artificial throttle or data loss.
- The final integration review initially found an unbounded signal PTY
  lifecycle and a signal-contract mismatch. The accepted fix bounds the
  signal child lifecycle and propagates supported `SIGINT` through a real TTY.
  For the unsupported case, it sends a real OS `SIGTERM` to the TTY-attached
  CLI and proves prompt return of the typed `unsupported_capability` error
  without delivering `SIGTERM` to the guest.
- `cargo test --manifest-path scripts/Cargo.toml`: PASS, 250 passed across 39
  suites, after reconciling merged test fixtures with the final validators and
  making their readiness and cache setup self-contained.
- `cargo clippy --manifest-path scripts/Cargo.toml --all-targets -- -D warnings`:
  PASS.
- `git diff --check`: PASS.

These are platform-neutral checks only. They are not real lifecycle,
credential-isolation, signing, notarization, or clean-host evidence.

The accepted Task 7 integration head is `306e0b6`. Its independent review
reported no Critical, Important, or Minor findings. Roadmap Gate 4, Roadmap
Gate 5, and MVP completion remain pending.

## Later serial Gate 4 run

On the supported Apple host, from the clean integration checkout, run exactly:

```sh
bash ./scripts/run-apple-e2e.sh apple_lifecycle
bash ./scripts/run-apple-e2e.sh apple_recovery
```

Run them in that order without concurrency. Preserve the harness cleanup
manifest and transcript if either command fails. The `apple_lifecycle` command
opens the CLI on a 24-by-80 PTY, changes it to exactly 47 rows by 132 columns,
sends `SIGWINCH` to the CLI, and requires the guest's `stty size` to report
exactly `47 132`. This is executable harness coverage, not live evidence until
the command above passes on the supported Apple host. Roadmap Gate 4 remains
pending until the complete real CLI lifecycle, terminal resize, and residue
checks pass. Roadmap Gate 5 and MVP completion remain pending; Gate 5 defines
MVP completion.
