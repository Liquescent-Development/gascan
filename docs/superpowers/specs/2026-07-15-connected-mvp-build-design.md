# Connected MVP Workspace Build Design

## Decision

The macOS MVP uses Apple `container build` as a connected build. It does not
require a network-disabled builder VM, and publication of the three offline
ARM64 input bundles is not a prerequisite for Roadmap Gates 4 or 5.

The image remains reproducible enough for an MVP through digest-pinned base
images, authenticated package metadata, pinned tool versions, verified
downloads, locked Rust dependencies, exact image contracts, and recorded image
digests. The existing offline-bundle implementation is preserved as deferred
hardening rather than deleted or represented as completed release work.

## Evidence Behind the Decision

The initial conclusion that Apple build VMs lacked public egress was caused by
a local firewall, not an Apple Containerization limitation. The investigation
on 2026-07-15 established the following sequence:

1. Builder DNS resolved public names through Apple's VM gateway resolver.
2. Builder TCP connections on ports 80 and 443 timed out while the local
   firewall was enabled.
3. After the firewall was corrected, a runtime container fetched
   `https://example.com`, and the builder connected to
   `ports.ubuntu.com:443`.
4. The remaining HTTPS apt failure was a bootstrap property of the minimal
   Ubuntu image: it did not yet contain `ca-certificates`.
5. A strict builder test using Ubuntu's signed HTTP apt metadata to install
   `ca-certificates` and `curl`, followed by an HTTPS request, completed
   successfully in 12.4 seconds and produced image manifest list
   `sha256:1f2a353c47bd187f7590503718fe3be9f9d69f15f2ab281a29d52d5ad5afa84c`.

This digest identifies diagnostic output only. It is not a Gas Can workspace
image or release artifact.

## MVP Build Boundary

The connected build may access only sources declared by the workspace image
lock and Dockerfile:

- the Ubuntu base image by immutable digest;
- Ubuntu repositories authenticated by the Ubuntu archive keyring;
- mise and runtime artifacts pinned by version and, where available, digest;
- the pinned Chromium artifact by digest;
- locked Gascamp source and dependency inputs supplied without persisting a
  private credential in an image layer or build output.

The build must fail if a required version, digest, platform, image-user,
sudo/init, volume, browser, runtime, or Gascamp contract is not satisfied. A
successful build records the final `linux/arm64` image reference and digest.

## Gascamp Credential Boundary

Private Gascamp access uses Apple BuildKit's secret mount. Apple
Containerization 1.1.0 requires the secret source to be a descendant of the
host context directory, so the build orchestrator copies the validated
owner-only token into a fresh `0700` temporary context under a fixed private
path with mode `0600`. That path is excluded by `.dockerignore` and supplied
only through `--secret`; it is not part of the transmitted Docker build
context.

The original token remains outside the repository, and both it and the staged
copy must be regular files owned by the current UID with mode `0600`. The
staged token must never be referenced by `COPY`, passed through `ARG` or `ENV`,
persisted in a layer, printed in logs, included in evidence, or left after the
build. Tests must prove exclusion from the transmitted context and absence
from command text, transcripts, image history, and the exported final
filesystem. Cleanup must cover success, failure, `INT`, and `TERM`.

This is the approved MVP boundary. A connected CI-produced, digest-verified
source artifact remains a future alternative if the local builder boundary
proves insufficient.

## Deferred Offline Build

Commits through `9025c56` implement and test much of an offline bundle path:
immutable bundle validation and producers, verified host prefetch, privileged
snapshot hardening, network-independent Dockerfile assembly, and a fail-closed
offline gate scaffold. This work is retained on `feature/provisioning`.

It is not on the MVP critical path. Its `publication = "pending"` lock and
PENDING evidence are accurate. Completing it later requires publishing the
Ubuntu package, mise runtime, and Gascamp source/vendor bundles and running its
cold/warm/corruption evidence gate. Delaying that work does not weaken runtime
sandbox isolation because image-building and running agent workloads are
separate trust boundaries.

## Roadmap and Handoff Records

The coordinated roadmap is the authoritative gate summary. The canonical
handoff document records branch heads, accepted commits, unfinished work,
decisions, and restart procedure. Chat transcripts and ignored SDD ledgers are
not authoritative restart state.

Every future gate transition must update both durable records in the same
integration change. Gate 5 remains the definition of macOS MVP completion.

## Acceptance Criteria for the Next Plan

The connected workspace image plan must:

1. replace the offline-only build invocation with a connected, locked build;
2. preserve the reviewed workspace user, guest-root sudo, tini, volumes, mise,
   Chromium, Gascamp selector, and exact inventory contracts;
3. prove private Gascamp credentials are excluded from the transmitted build
   context and leave no temporary-file, layer, command-text, log, filesystem,
   or evidence residue;
4. build and smoke-test a digest-qualified `linux/arm64` image on Apple
   Containerization 1.1;
5. hand that exact image reference to the reviewed Gate 4 lifecycle harness;
6. update durable evidence without claiming Gate 4 until the complete real CLI
   lifecycle passes.
