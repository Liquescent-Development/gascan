# Apple container inspect compatibility fix

## Scope and root cause

The connected image flow had two coupled compatibility defects:

1. `prefetch-connected-workspace-image.sh`, `build-connected-workspace-image.sh`, and `run-connected-image-gate.sh` invoked `container image inspect --format json`. The installed Apple container CLI accepts only `container image inspect [--debug] <images>...` and emits JSON without a format option.
2. The validators and fake fixtures modeled an obsolete inspect schema. Native Apple output uses an unprefixed 64-hex top-level `id`. For a pulled digest-qualified base, `configuration.descriptor.digest` is the local index identity while the locked source digest is `variants[0].digest`. The exact `linux/arm64` variant must therefore be used for lock comparison. For a locally built tag, the immutable digest-qualified receipt is the validated local descriptor digest, with its unprefixed `id` required to match.

Affected connected call paths were independently traced through prefetch, build, receipt publication, and the final gate re-inspection. Offline paths were not changed because they are preserved reviewed work and are outside this connected correction.

## TDD evidence

### RED cycle 1

Tests and fake CLIs were changed first to require native syntax and the real schema:

- `image_inspect.rs` expected the sole `linux/arm64` variant digest and rejected the obsolete missing-variant-digest schema.
- Connected prefetch/build/gate fixtures rejected extra inspect arguments and emitted unprefixed IDs, descriptor digests, and variant digests.
- The connected build validator fixture used an unprefixed ID and a distinct valid variant digest.

Command:

`cargo test --manifest-path scripts/Cargo.toml --test image_inspect --test connected_workspace_context --test connected_image_build --test connected_image_gate`

Observed RED: three connected-build failures, including rejection of the native built-image schema and fake runners failing because production still supplied `--format json`. These were behavior failures after correcting one fixture-format compilation error; they were not test setup errors.

### GREEN cycle 1

Production changes removed `--format json` from all three connected call paths, made `validate-image-inspect` return the sole `linux/arm64` variant digest, and made `validate-connected-build` accept only an unprefixed ID matching the immutable descriptor digest while retaining exact tag, platform, and digest validation.

The focused four-test-target command then exited 0.

### RED/GREEN cycle 2

The final gate fixture was hardened so its built-image variant digest differs from its local descriptor digest. The focused success test failed with `inspection digest differs from receipt`, proving the gate incorrectly reused the base-source validator for a built-image receipt.

Command:

`cargo test --manifest-path scripts/Cargo.toml --test connected_image_gate successful_gate_uses_one_reference_and_token_then_publishes_atomically`

Production was then changed so gate re-inspection uses `validate-connected-build` with the exact tag extracted from the digest-qualified receipt. The focused success test and the complete four-target focused suite both exited 0.

## Resulting contract

- Connected inspect calls use native Apple JSON output with no unsupported format option.
- Base images: exactly one record and one `linux/arm64` variant; return the valid lowercase SHA-256 variant digest for byte-for-byte comparison with the lock.
- Built images: exact immutable build tag; exactly one `linux/arm64` variant with a valid digest; valid local descriptor SHA-256; unprefixed ID must equal the descriptor hex; receipt/reference use the descriptor digest.
- Mutable tags, malformed/ambiguous records, old prefixed IDs, missing/invalid variant digests, wrong platforms, mismatched IDs/descriptors, and receipt identity mismatches remain rejected.
- Cleanup ownership and atomic publication code was not weakened or bypassed.

## Verification

- Focused connected suite: exit 0.
- Full `scripts/Cargo.toml` test suite: exit 0.
- `bash -n` for all three changed connected shell scripts: exit 0.
- `cargo fmt --manifest-path scripts/Cargo.toml -- --check`: exit 0.
- `git diff --check`: exit 0.

The live gate was not run. No live evidence, build receipt, or `approved-image.txt` was created, and no Gate 4 or Gate 5 claim is made.
