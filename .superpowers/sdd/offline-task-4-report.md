# Offline Workspace Image Bundles — Task 4 Report

## Outcome

Implemented the exact Gascamp source-and-Cargo-vendor producer, fail-closed evidence validation, and connected/privilege-separated Linux ARM64 CI proof. The approved commit is `f6b248c5926240856dbea83d1d2c5c90ea1c1456`; its independently inspected Git tree is `71e706057023049b8d15839cedd1fcd0b4a85968`. No bundle was published, no release was changed, and `images/workspace/versions.lock` remains in its approved pending state.

## TDD evidence

- Added `scripts/tests/gascamp_bundle.rs` before the producer existed.
- Confirmed RED with `cargo test --manifest-path scripts/Cargo.toml --test gascamp_bundle`: all ten tests failed because `produce-gascamp-bundle.sh` was absent.
- Implemented the smallest validator/producer contract and reached GREEN with 10/10 focused tests.
- The tests reject revision mismatch, exact Git-tree mismatch, dirty or extra source bytes, submodule ambiguity, altered and missing vendor crates, unlocked Git dependencies, absent Cargo checksum metadata, and registry/network-enabled Cargo configuration.

## Implementation

- `scripts/produce-gascamp-bundle.sh`
  - fetches only the approved commit from the authenticated `Liquescent-Development/gascamp` repository and verifies `FETCH_HEAD`, detached `HEAD`, the exact tree object, clean/untracked state, and absence of `.gitmodules` and gitlinks;
  - rejects every Git dependency unless its manifest has a full 40-character `rev` and the lockfile contains the matching URL/revision source;
  - exports source via `git archive`, runs `cargo vendor --locked`, and emits a parent `.cargo/config.toml` whose exact schema forces offline mode and replaces crates.io with the local vendor directory;
  - emits separate canonical manifests for source and vendor trees, binding modes, sizes, content SHA-256 values, and symlink targets; provenance independently binds both manifest digests, config bytes, revision, Git tree, platform, locked-vendor invocation, and submodule absence;
  - independently revalidates every registry lock entry against exactly one vendored crate, its package checksum, and every file in `.cargo-checksum.json`, rejecting missing/extra/altered crates or files;
  - produces a deterministic canonical manifest-first tar+zstd archive and hash/size sidecars.
- `.github/workflows/workspace-bundles.yml`
  - uses the exact SHA-pinned Ubuntu ARM64 base and only the already validated Task 2 package archive for OS tools; APT uses the local `file:` repository, disables HTTP/HTTPS methods and proxies, installs exact manifest versions with `--no-download`, audits dpkg, and performs no mutable runner APT install;
  - uses the already validated mise runtime artifact for the pinned Cargo toolchain;
  - builds twice and compares the complete evidence entry/type/link set plus every regular-file digest;
  - disables container networking and proves `cargo test --locked --offline --frozen` plus `cargo build --locked --offline --frozen --release --bin camp`;
  - removes one real vendored crate and requires a prompt offline Cargo failure;
  - revalidates as the unprivileged runner user, safely inspects/extracts the archive, binds archive contents back to the separately uploaded evidence, and emits a validation receipt;
  - uploads only short-lived CI artifacts. It does not publish or alter lock state.

## Verification

The final verification gate covered the complete scripts test suite, clippy with warnings denied, rustfmt check, every shell script with `bash -n`, YAML parsing, and `git diff --check`. See the implementing commit and task handoff for the fresh command results.

## Remaining operational concern

The connected producer and real offline build were not run from this macOS workspace. The pinned Gascamp repository is private (the stale public `gascan/gascamp` URL returns “Repository not found”), so the CI repository must define a read-only `GASCAMP_READ_TOKEN` secret with access to `Liquescent-Development/gascamp`. The producer passes it only as a transient HTTP authorization header and unsets it immediately after fetch. A real ARM64 workflow run remains the live proof that repository authorization, the validated Task 2/Task 3 artifacts, Cargo 1.97.0, full vendoring, and the network-isolated test/release build all work together. Every condition fails closed, and no Task 4 publication occurs on failure.
