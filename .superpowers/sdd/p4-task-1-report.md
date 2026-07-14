# Plan 4 Task 1 report

Status: **READY FOR REVIEW**

## Scope

Implemented image Track C Task 1 only in the isolated `workspace-image` worktree. No `gascand`, Plan 2, or root workspace manifest paths were modified. `scripts/Cargo.toml` declares its own workspace, so the shared root `Cargo.toml` did not require a change.

## TDD evidence

- RED: `cargo test --manifest-path scripts/Cargo.toml --test image_lock` failed because `images/workspace/versions.lock` did not exist.
- GREEN: the generated concrete lock passes the completeness and checksum-shape tests.
- RED/GREEN: mise's published `./filename` checksum format initially failed resolution; a focused parser regression now passes.
- RED/GREEN: unresolved `rust@stable` was rejected; Rust stable now resolves through the official tagged channel manifest and has a parser regression.
- Playwright's tagged manifest contains unrelated versionless entries; the parser regression proves they are ignored while Chromium still requires `browserVersion` and a numeric revision.
- Review RED/GREEN: the build script lacked artifact download deadlines/host checks; a static regression now requires 15-second connect and 120-second overall deadlines, visible progress, HTTPS-only redirects, and approved initial/final hosts.
- Review RED/GREEN: post-build inspect was text-scraped. Structured validator tests now prove exactly one Linux/ARM64 image succeeds and mismatched, malformed, empty, or ambiguous inspect output fails closed.
- Rereview RED/GREEN: curl could validate only the initial and final redirect hosts. The build now uses a bounded streaming Rust downloader whose reqwest redirect policy validates HTTPS and the approved host set before every hop, caps redirects at five, and preserves connect/overall deadlines, byte progress, atomic output, and SHA-256 verification. An injectable fake-transport regression proves an unapproved intermediate is rejected after one approved contact and before a second contact occurs.

## Delivered interfaces

- `images/workspace/versions.toml` contains the reviewed aliases and source inputs.
- `images/workspace/versions.lock` pins the Ubuntu Linux ARM64 digest, snapshot timestamp, verified mise ARM64 URL/SHA-256, seven exact runtime versions, Playwright Chromium version/revision/URL/SHA-256, Gascamp revision, and content-derived image tag.
- `update-image-lock` validates approved redirect hosts, verifies mise against its published SHA-256 list, applies 15-second connection and 120-second request deadlines, applies 60-second mise resolver deadlines, rejects unresolved aliases, and writes the lock atomically.
- `images/workspace/Dockerfile` starts with `ARG BASE_IMAGE` / `FROM ${BASE_IMAGE}`, configures the locked Ubuntu snapshot, installs CA certificates noninteractively, and copies only build-script-verified artifacts.
- `scripts/build-workspace-image.sh` rejects mutable base/tag inputs; downloads both artifacts through the per-hop-validating helper with bounded, visible HTTPS-only transfers; verifies their hashes; builds Linux ARM64; and writes `.artifacts/workspace-image-ref` only after the structured inspect validator proves a unique Linux/ARM64 variant and valid OCI index digest.
- `.artifacts/` is ignored as the image/smoke handoff directory.

## Concrete lock highlights

```text
ubuntu@sha256:7f622ca8766bccb22f04242ecb6f19f770b2f08827dc4b8c707de5e78a6da7ab
mise 2026.5.0 sha256:fba7c8a383cf3c59eb5a9995d5299fd2c78eba7eb1daace48d75fe491362f79a
Playwright Chromium 149.0.7827.55 revision 1228 sha256:ec044b50ed065adeb4c5ffdb42d1529901cbaf897cdf542bfef8af01d6e0cc79
gascan-workspace:d4964500a3295a33
```

## Verification

```text
cargo test --manifest-path scripts/Cargo.toml
10 passed; 0 failed

cargo clippy --manifest-path scripts/Cargo.toml --all-targets -- -D warnings
exit 0

bash -n scripts/build-workspace-image.sh
exit 0

git diff --check
exit 0
```

The generator completed with bounded progress output and produced the lock. Per controller instruction, the privileged/live `container build` step was not run; image build/inspect evidence remains pending review-time execution on the supported runtime.

The unchanged root workspace suite was attempted as an extra isolation check. Its unprivileged run stopped before compilation because `index.crates.io` DNS resolution was unavailable; the escalated retry was interrupted externally after roughly 340 seconds. This is not counted as Task 1 verification and no root workspace failure was observed.
