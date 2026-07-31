# Task 5 Implementer Report

## Status

Complete. The workspace image now installs a no-secret developer-home helper
that manages persistent Git identity, protocol, SSH signing/authentication key,
per-host OpenSSH stanzas, and the versioned onboarding receipt. The Rust
adapter invokes only that helper through `GuestRunner`, uses discrete argv with
no environment or stdin, and validates bounded status JSON.

The production API remains crate-private. As explicitly permitted by the Task
5 brief, fake-runner adapter tests live in `src/configure/tests.rs` under the
existing `#[cfg(test)]` module rather than the inaccessible external
`crates/gascan/tests/configure_git.rs` target. No public test seam was added.

## Files

- `images/workspace/bin/configure-developer-home`
- `images/workspace/Dockerfile`
- `images/workspace/tests/workstation-contract.sh`
- `scripts/tests/image_user_contract.rs`
- `crates/gascan/src/configure/git.rs`
- `crates/gascan/src/configure/mod.rs`
- `crates/gascan/src/configure/tests.rs`
- `crates/gascan/Cargo.toml`
- `Cargo.lock`

## TDD evidence

Initial helper RED:

- `cargo test --manifest-path scripts/Cargo.toml configure_developer_home`
- Failed six focused contracts because the helper and Docker installation were
  absent.

Initial adapter RED:

- `cargo test -p gascan configure_git`
- Failed because `crates/gascan/src/configure/git.rs` did not exist. A
  test-only `Eq` derive incompatibility exposed in the same compile was
  corrected before implementation.

Additional security RED/GREEN checkpoints:

- Generated key validation initially rejected macOS `ssh-keygen -y` output
  because that implementation retained the public comment; the validator now
  accepts the two-field or three-field public form while comparing algorithm
  and key body.
- A direct `/dev/fd/DIR/STAGE` hardening attempt failed on macOS because its
  descriptor filesystem cannot traverse directory descriptors. The portable
  implementation uses `fchdir` into validated directory fds, invokes tools on
  relative staging paths, and restores cwd.
- The descriptor-anchoring test failed against absolute-path key generation,
  then passed after the `fchdir` implementation.
- The protocol atomicity test failed while protocol occupied a second file,
  then passed after the protocol marker moved into the same atomically
  published Git config that contains exactly six Git fields.
- The exclusive-key-stage test failed while keygen owned a fixed leaf path,
  then passed after generation moved into an exclusively created mode-0700
  staging directory opened by fd.
- The first-use SSH test failed while `ssh-keyscan` prepopulated
  `known_hosts`; after coordinator correction, `ssh-host` writes only sorted
  identity stanzas and leaves normal OpenSSH first-use display/recording to
  Task 6.
- The HTTPS test failed because an explicit `ssh-host` call was accepted;
  HTTPS state now rejects it before mutation.
- The selector/request mismatch test failed after recording one guest call;
  mismatch now returns a stable redacted error before any guest mutation.
- Malformed padded Ed25519 status initially passed; the adapter now performs
  canonical base64 and exact SSH wire-blob validation and binds the key comment
  to the request sandbox ID.
- A barrier-driven real-process concurrency test reproduced a key-stage
  collision and possible lost SSH-host update. A bounded exclusive `flock` on
  the already-open managed Git directory now spans state validation and the
  full operation; concurrent Git and host tests pass.

## Implementation and security contract

- The helper walks `HOME/.config/gascan/git/ssh` through opened directory fds
  with `O_NOFOLLOW`, then validates the Task 2 directory markers, owner, group,
  and mode before mutation.
- Every helper-owned leaf is opened without following links and checked for
  regular type, exact owner/group/mode, link count one, and bounded size.
  FIFOs, symlinks, wrong ownership, permissive modes, hard links, malformed
  keys/config/receipts, partial pairs, and stale stages fail without repair or
  deletion.
- Keygen runs exactly passwordless Ed25519 generation inside an exclusive
  same-volume staging directory. Both staged files and key correspondence are
  validated and fsynced before publication. A valid sandbox-scoped pair is
  reused unchanged.
- Git is written through `git config --file` staging and contains exactly
  `user.name`, `user.email`, `gpg.format=ssh`, `user.signingkey`,
  `commit.gpgsign=true`, and `tag.gpgsign=true`. The SSH/HTTPS protocol marker
  is a Git comment, so status and all six Git values commit in one rename.
- SSH hostnames use strict lowercase DNS validation. Managed stanzas are sorted
  and idempotent, choose the persistent key, and set `IdentitiesOnly yes`.
  They deliberately do not alter `StrictHostKeyChecking` and do not create
  `known_hosts`, preserving normal visible first-use verification for Task 6.
- Receipt writes are exclusive-staged, fd-validated, fsynced, and atomically
  published with only the exact versioned `complete` or `declined` line.
- Status is capped at 64 KiB and contains only identity, protocol, public key,
  fingerprint, and receipt state. Unknown arguments are rejected generically;
  no token or private-key bytes are accepted or emitted.
- All operations take a bounded exclusive lock on the validated Git directory
  fd, preventing stale concurrent snapshots without creating another state
  file.

## Final verification

- `cargo test --manifest-path scripts/Cargo.toml --test image_user_contract`:
  59 passed.
- `cargo test --manifest-path scripts/Cargo.toml --test connected_dockerfile`:
  31 passed.
- `cargo test -p gascan configure_git`: 5 passed.
- `cargo test -p gascan configure_ssh_host`: 1 passed.
- `cargo test -p gascan` with the established host/process access required by
  daemon attestation tests: 237 passed across six suites.
- `cargo clippy -p gascan --tests`: zero errors; only expected unused/dead-code
  warnings for the crate-private onboarding foundation consumed by later tasks.
- `cargo fmt --all -- --check`, `git diff --check`, Python syntax compilation,
  and `sh -n images/workspace/tests/workstation-contract.sh`: passed.

The first restricted-sandbox full `gascan` package run remained silent for
more than six minutes and was stopped with status 130. Re-running the identical
command with the established host/process access completed in seconds with the
237-test result above.

Running `images/workspace/tests/workstation-contract.sh` directly on the macOS
checkout stops at `locked version evidence is unavailable`. That contract
requires the assembled Linux image's `/opt/gascan` evidence and therefore
cannot be completed source-only. Its syntax and the Docker/image source
contracts pass; assembled-image execution remains the downstream image gate.

## Review disposition

Independent security review found and drove fixes for first-use host-key
behavior, selector/request mismatch, protocol coupling, key staging, identity
validation, Ed25519 parsing, and whole-operation serialization. The bounded
fix-round review reported no remaining Critical or Important issue and marked
the code ready to commit.

The coordinator clarified that the earlier prompt phrase requesting
`ssh-keyscan` publication was erroneous: the approved written Task 5 plan owns
only SSH stanzas, while Task 6 owns visible `ssh -T git@HOST` fingerprint
display and recording. The tests now explicitly prove Task 5 does not
prepopulate `known_hosts`.

## Concerns

POSIX cannot atomically rename the two required fixed regular key leaves as a
single operation. The helper follows the written contract by validating and
fsyncing both staged products before either rename; interruption between the
two publishes leaves a partial pair that is rejected fail-closed on the next
run rather than deleted or silently repaired.

The full workstation contract is assembled-image-only as described above. No
other functional concern remains.
