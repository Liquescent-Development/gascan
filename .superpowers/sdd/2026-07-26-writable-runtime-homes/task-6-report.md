# Task 6 Report: Publish, Exercise, and Approve the Connected Workspace Image

## Current Outcome

Task 6 remains in progress. Publication and approval are intentionally blocked
until the review fixes below receive a fresh independent approval and a new
no-cache connected image passes the complete live gate.

The first fresh build from Tasks 1-5 completed with local digest
`sha256:1412ce0d21e640e450a35622ace461a90d06d50c70ec0d21d50db420d5eae8c4`.
Its live gate exposed three stale or incomplete assumptions:

1. the host-injected user/volume smoke expected an immutable mise directory to
   be workspace-owned;
2. runtime mise pointed its system configuration at a mutable path, removing
   the reviewed immutable defaults;
3. moving Cargo and rustup homes to writable storage left rustup's command
   proxies and default settings behind in the immutable Cargo/Rust homes.

The first two fixes are committed as `01b4663` and `991715a`. The third fix is
committed as `ba938ea`, with the independent review fixes committed as
`de2f07d`. The refreshed review approved rebuilding with one minor positive
test note, closed below.

No remote candidate has been published. The approval candidate file remains
absent, and neither `images/workspace/approved-image.txt` nor connected-image
evidence has been modified.

## Rust Proxy and Default-Settings Diagnosis

The locked image contains this exact immutable Cargo command layout:

- regular executable `rustup`;
- symlinks with raw target exactly `rustup`: `cargo`, `cargo-clippy`,
  `cargo-fmt`, `cargo-miri`, `clippy-driver`, `rls`, `rust-analyzer`,
  `rust-gdb`, `rust-gdbgui`, `rust-lldb`, `rustc`, `rustdoc`, and `rustfmt`.

After Task 1 redirected `CARGO_HOME`, the writable Cargo bin was empty.
`mise current rust` still selected 1.97.0, but `mise which rustc` reported an
invalid shim. Seeding the exact command layout fixed resolution, but a neutral
direct `rustc` then showed that the writable rustup home had no default.

The immutable, non-secret rustup settings shape is:

```toml
version = "12"
default_toolchain = "1.97.0-aarch64-unknown-linux-gnu"
profile = "default"

[overrides]
```

The bootstrap now treats that strictly size-bounded file as the default source
of truth, validates the entire canonical subset without evaluation, validates
the selected source and copied toolchain commands, and atomically generates a
minimal mode-0600 writable settings file only when the user has none.

## TDD Evidence

The proxy tests were written before production changes. The focused RED run
kept two pre-existing safety tests green and failed four new contracts because
the script ignored proxy source, destination, publication, and cleanup:

```text
test result: FAILED. 2 passed; 4 failed
```

After exact allowlist, collision, atomic publication, source immutability, and
retry support:

```text
test result: ok. 6 passed; 0 failed
```

Default-settings tests were then added before implementation. Four intended
contracts failed because settings were neither validated nor published:

```text
test result: FAILED. 5 passed; 4 failed
```

After strict source/default validation, minimal publication, preservation, and
interrupted-publication cleanup:

```text
test result: ok. 9 passed; 0 failed
```

Independent review of `991715a..ba938ea` found two important gaps:

- the first parser selected a matching default line but did not validate the
  surrounding TOML;
- EXIT/INT/TERM cleanup did not reclaim staging left by SIGKILL or a process
  crash.

New tests first reproduced both gaps:

```text
test result: FAILED. 8 passed; 3 failed
```

The corrected parser is a stateful whole-file grammar for the actual immutable
subset. It requires exact ordered `version`, `default_toolchain`, and `profile`
records, followed only by an optional exact empty `[overrides]` table.
Malformed, missing, duplicate, unknown, nested, trailing-junk, unterminated,
and noncanonical cases fail.

Crash retry now scans only these exact confined prefixes:

- Rust root: `.gascan-rust-seed.`, `.gascan-rust-hash.`,
  `.gascan-rust-settings.`, `.gascan-rust-marker.`;
- Cargo bin: `.gascan-rust-proxy.`.

Each prefix has an allowed file/directory type. Unsafe basenames, symlinks, and
type mismatches fail closed. Valid stage directories are removed without
following nested symlinks. Near-match names, final paths, unrelated dotfiles,
and external symlink targets survive.

Focused GREEN after both review fixes:

```text
rtk cargo test --manifest-path scripts/Cargo.toml --test image_user_contract writable_rust_bootstrap
test result: ok. 11 passed; 0 failed

rtk cargo test --manifest-path scripts/Cargo.toml --test image_user_contract
cargo test: 28 passed

rtk cargo test --manifest-path scripts/Cargo.toml --test polyglot_image_contract
cargo test: 10 passed
```

## Live Injected Evidence

Before rebuilding, the corrected local bootstrap was safely injected through
the repository bind mount into an owner-labeled disposable container with an
owner-labeled 10 GiB tools volume. The original immutable source inode/hash
evidence was unchanged.

A network-disabled neutral-directory proof reported:

```text
mise which rustc: /home/workspace/.local/share/cargo/bin/rustc
mise current rust: 1.97.0
rustc 1.97.0 (2d8144b78 2026-07-07)
cargo 1.97.0 (c980f4866 2026-06-30)
```

Real rustup accepted the generated settings and listed the installed
components without downloading.

After the review fixes, the complete owner-scoped polyglot harness was
temporarily pointed at the bind-mounted corrected bootstrap. It accepted the
real immutable settings shape, selected every locked system default, compiled
and ran direct Rust from `/tmp` as `rust-ok`, passed the browser smoke, and
cleaned its exact container and volume. The harness was immediately restored
to the baked `/usr/local/bin/initialize-rust-home` path.

## Local Verification Before Review Fixes

```text
rtk cargo fmt --all -- --check
PASS

rtk cargo clippy --workspace --all-targets -- -D warnings
PASS

rtk cargo test --workspace
889 passed, 20 ignored, 63 filtered

rtk cargo test --manifest-path scripts/Cargo.toml
427 passed (47 suites)

rtk swift test --package-path helpers/apple-attach
11 passed

all tests/release/*-contract.sh
11 passed

rtk git diff --check
PASS
```

The first sandboxed workspace, scripts, Swift, and release-contract attempts
encountered only their known macOS PTY, local fixture-server, compiler-cache,
or Homebrew-cache permission denials. Each authoritative host-permission rerun
passed.

## Review-Fix Verification

```text
rtk cargo test --manifest-path scripts/Cargo.toml --test image_user_contract
28 passed

rtk cargo test --manifest-path scripts/Cargo.toml --test polyglot_image_contract
10 passed

rtk cargo test --manifest-path scripts/Cargo.toml
429 passed (47 suites)

shell syntax, focused rustfmt, and git diff checks
PASS
```

## Refreshed Review and Minor Closure

The read-only package
`.superpowers/sdd/2026-07-26-writable-runtime-homes/review-ba938ea-de2f07d.diff`
contained the one focused review-fix commit and 38,060 bytes of plan, diff, and
status context. The independent re-review closed both Important findings and
approved rebuilding.

Its one Minor note was that the parser's optional no-`[overrides]` branch had
no positive fixture. A test-only fixture now supplies exact canonical
version/default/profile records with no table, proves bootstrap success, and
proves the generated writable settings remain rustup's minimal empty-overrides
shape:

```text
writable_rust_bootstrap_accepts_canonical_settings_without_overrides_table
PASS

rtk cargo test --manifest-path scripts/Cargo.toml --test image_user_contract
29 passed

rtk git diff --check
PASS
```

## External-State Safety

- All diagnostic and live-proof containers and volumes used unique random
  owner tokens and repository validators before cleanup.
- Cleanup double-validated exact ownership and then proved names absent.
- Foreign Gas Can sandboxes, the builder, and user-managed volumes were not
  modified or deleted.
- The failed and superseded local image/receipt remain local evidence only.
- No GHCR tag, remote manifest, approval candidate, approval file, or release
  evidence was changed.

## Remaining Work

1. Commit the minor positive parser fixture.
2. Run a fresh no-cache connected build and the complete live gate.
3. Publish one unique digest-derived immutable GHCR candidate.
4. Pull and inspect the exact remote digest, run remote digest-pinned smokes,
   and record immutable evidence.
5. Use only the approval script to update approval/evidence.
6. Finish this report with final digests, timestamps, gate results, publication
   evidence, cleanup proof, and approval commit.
