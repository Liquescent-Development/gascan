# Offline Bundle Plan Task 5 Report

## Result

Task 5 adds the sole normal-build host network boundary and an atomic, minimal workspace context preparer without implementing Task 6's Dockerfile conversion.

- `scripts/prefetch-workspace-image.sh` is the only normal image workflow entry point that downloads artifacts or pulls a base. It requires published records for all three bundles, fetches each through reviewed artifact-class rules, revalidates warm cache bytes, refreshes the reviewed Chromium extraction, generates exact tool evidence, pulls only the locked Ubuntu digest, and validates structured `linux/arm64` inspection before context publication.
- `fetch-image-artifact` now chooses HTTPS authorities and maximum byte counts from code-owned artifact classes. Bundle fetches additionally enforce the exact locked byte count. Initial URL approval occurs before the warm-cache fast path; every redirect is approved independently. Downloads remain private temporary files until their digest and size checks succeed, so failed replacement leaves previously valid bytes intact.
- `prepare-workspace-context` requires Task 1's published typed bundle locks and validates/extracts all three archives directly into a private staging directory. Repository and legacy reviewed inputs come from explicit allowlists through capability-relative, no-follow reads. Symlinks and special files are rejected, files/directories are made read-only, and a sorted path/type/mode/size/SHA-256 manifest is written before handle-relative publication. Refresh uses the OS atomic rename-exchange operation; the old valid context remains published unless the replacement is fully assembled, then becomes cleanup input only after the swap succeeds.
- Context verification regenerates a separate expected context from the current repository, cache, and published locks, then compares canonical manifests. This binds build consumption to the current exact bundle archives and reviewed static inputs rather than trusting a self-asserted context manifest.
- `scripts/build-workspace-image.sh` performs no download or base pull. It rejects missing/pending/stale context before any Apple command, validates the exact local base's structured `linux/arm64` digest, and invokes `container build` only with the minimal context.

The context contains only `Dockerfile`, reviewed static image files, mise/Chromium/tool evidence, the three verified extracted bundle trees, and `context-manifest.tsv`. Unlisted repository/user files do not enter it.

## TDD and review

The initial context test failed because `prepare-workspace-context` did not exist. Expanded context tests then failed because assembly and the four-path CLI were absent. Artifact tests failed because typed artifact classes, exact cache validation, and atomic verified installation were absent. A later regression failed because warm-cache reuse did not yet require initial URL approval. Each behavior was implemented after its focused RED and rerun GREEN.

The final review identified stale-context refresh, incomplete directory/type identity, and derived-Chromium verification gaps. The verifier now regenerates an exact expected context from current locked inputs, canonical identity covers directories and file types/modes, prefetch atomically swaps a completed replacement, and Chromium is always re-extracted before reuse/replacement. The reviewer also noted that the existing Dockerfile remains network-capable; that conversion is deliberately excluded here because the approved plan assigns it to Task 6 and this task was explicitly constrained to no Task 6+ work.

## Verification

Fresh local verification completed successfully:

```text
cargo test --manifest-path scripts/Cargo.toml --test workspace_context --test artifact_redirect
  artifact_redirect: 5 passed
  workspace_context: 7 passed

cargo test --manifest-path scripts/Cargo.toml --all-targets
  all scripts unit and integration tests passed

cargo clippy --manifest-path scripts/Cargo.toml --all-targets -- -D warnings
  passed

cargo fmt --manifest-path scripts/Cargo.toml -- --check
  passed

bash -n scripts/*.sh
  passed

git diff --check
  passed
```

## Live caveat

`images/workspace/versions.lock` intentionally remains `publication = "pending"`, so the implementation fails before Apple invocation today. No release URLs, sizes, or hashes were fabricated, and no unavailable published bundles were downloaded. Consequently the real three published bundle assets, a live Apple `container image pull`/inspect of the exact Ubuntu digest, and a real prepared-context Apple build remain unexecuted live checks after Tasks 2-4 publication records are populated. Task 6 must convert the Dockerfile to consume these bundles without network access before an end-to-end offline Apple build can succeed.

## Parent security review follow-up

The post-commit parent review identified that a verified user-owned context path could still be changed before Apple reopened it. The build now captures the preparer's exact canonical manifest digest and passes it to a separately installed audited helper at `/usr/local/libexec/gascan/snapshot-workspace-context`. The script requires that helper to be root-owned mode `0555` and invokes it only with `sudo -n`; missing sudo privilege, helper, or exact ownership fails before any Apple command. The helper opens one source capability, rejects links/specials/extras, verifies every manifest-bound file from no-follow handles while copying, and publishes a cryptographically named root-owned read-only snapshot beneath a fixed root-owned directory. Apple receives only that snapshot. The helper revalidates its token, device/inode, ownership, modes, manifest, paths, sizes, and hashes before returning the build path and again before removing it. Adversarial tests prove that exchanging the original source cannot change snapshot bytes and mutation prevents both consumption and cleanup.

Context replacement names now use 256-bit OS randomness rather than PIDs. A sibling receipt binds the token plus old/new canonical manifest digests; recovery touches only a matching prefixed path whose complete verified manifest matches that receipt. Atomic exchange success is returned independently of best-effort old-tree cleanup. Chromium refresh likewise moved out of shell PID/move logic: the Rust extractor uses a securely random private staging directory, digest-bound exchange receipt, atomic directory exchange, safe stale recovery, and preserves the last valid tree on validation/extraction failure.

Artifact cache validation and publication now open one parent directory capability, open final files with no-follow semantics, reject symlinks/non-regular files, stream into cryptographically named handle-relative temporary files, rename through the same directory handle, and reject parent identity replacement. Failed validation or refresh removes only the exact private temporary entry and leaves the prior valid cache untouched.

The root-owned snapshot helper was not installed or exercised through real `sudo` in this workspace. No live Apple build claim is made; the fixed helper installation is an explicit prerequisite and the build intentionally fails closed while it is unavailable.

An independent follow-up review found that the helper source was hidden by a global `[Bb]in/` ignore and that the build-context path reused the locked Ubuntu snapshot variable. The helper is force-tracked in the final commit, and a regression test now keeps `ubuntu_snapshot` distinct from `build_context_snapshot`. The reviewer reported no other critical, important, or minor findings.
