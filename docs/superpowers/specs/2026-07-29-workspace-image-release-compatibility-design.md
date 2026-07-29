# Workspace image release compatibility design

## Problem

Gas Can 0.1.13 invokes `/usr/local/bin/configure-shell-home` while its approved
workspace-image pin still names an image built before that helper existed. The
CLI therefore passes Doctor and creates a sandbox, but initial provisioning
fails with `command not found`.

The immediate defect is a stale approved image. The systemic defect is that a
release can prove that its source tree and package are clean without proving
that the approved immutable image was built from the current workspace-image
inputs.

## Outcome

Gas Can 0.1.14 will pin a newly built, public, Apple-live-tested workspace
image containing the native-shell and managed-prompt assets. Release preflight
will reject any future source tree whose approved image predates a change to
the workspace-image inputs.

## Image contract

Add a machine-readable runtime contract under `images/workspace/` naming the
privileged guest helpers invoked by provisioning. Contract tests will require:

- every contracted absolute helper path is invoked by the provisioning plan;
- every invoked privileged image helper is present in the contract;
- the Dockerfile copies each contracted helper into the exact path with an
  executable mode; and
- the corresponding source file exists and passes its existing syntax and
  behavioral contracts.

This catches a new provisioning dependency that was not added to the image
source before an image is built.

## Approved-source fingerprint

Add one deterministic source-fingerprint command shared by image approval and
release preflight. It hashes the sorted relative paths and contents of the
tracked workspace-image source tree, excluding only the approved image
reference and the fingerprint output itself. The runtime contract is inside
that tree and is therefore included automatically.

`approve-connected-workspace-image.sh` will atomically publish three related
records only after the existing build receipt, connected image gate, and Apple
live receipt agree:

1. the immutable GHCR image reference;
2. the connected-image evidence document; and
3. the approved source fingerprint.

Release preflight will recompute the fingerprint and fail with an actionable
message if it differs. Changing a helper, Dockerfile instruction, prompt
configuration, package lock, or other tracked image input will therefore
require a new approved image before another Gas Can release.

## Image publication

From the clean feature branch:

1. Prefetch and build the current connected workspace image without cache.
2. Run the complete connected image gate locally.
3. Publish it to a never-reused GHCR tag derived from the complete image
   digest.
4. Pull and structurally verify the public digest.
5. Re-run the prebuilt connected gate and the full Apple `apple_apply` suite
   against that public digest.
6. Confirm candidate, build, and Apple-live receipts match exactly.
7. Run the approval script to update the pin, evidence, and fingerprint.

Existing cleanup ownership rules remain unchanged; only exact test-owned
containers and volumes may be removed.

## Release and validation

Automated tests will cover runtime-contract completeness, deterministic
fingerprinting, atomic approval rollback, and stale-fingerprint release
rejection. The full Rust workspace, scripts workspace, image contracts, release
contracts, connected image gate, and Apple apply suite must pass.

After review, the change will be merged through a pull request. Gas Can will be
bumped to 0.1.14, signed and tagged at the exact squash-merge commit, then
packaged, Developer ID signed, Apple notarized, published to GitHub, and
released through the Homebrew tap. A final Homebrew fetch must resolve 0.1.14
and its published checksum.

The failed 0.1.13 sandbox left no managed sandbox inventory, so no migration or
backward-compatibility handling is required.
