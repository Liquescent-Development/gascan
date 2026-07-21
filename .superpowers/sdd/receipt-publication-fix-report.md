# Connected receipt publication regression report

## Scope

Diagnose the failing `fake_runner_failure_matrix_cleans_snapshot_and_never_commits_an_invalid_pair` test without changing the privileged-helper allowlist or weakening receipt identity validation.

## Root cause

The `fail_ref` case did not reach either receipt publication move. The Apple-native inspect fixture constructed these identities:

- image `id`: `99…99`
- configuration descriptor digest: `sha256:77…77`
- variant digest: `sha256:99…99`

`validate-connected-build` correctly rejected the mismatched image ID and descriptor before publication. The seeded old reference and seeded old JSON receipt therefore remained a valid pair. The test's final validator invocation accepted that valid old pair, and the assertion incorrectly reported it as an accepted old-reference/new-JSON pair.

This was stale test-fixture mechanics introduced while adapting the fake Apple inspect schema. Production publication and receipt validation were not the cause.

## RED evidence

The exact focused command failed consistently at the stale-pair assertion. Temporary diagnostic instrumentation showed:

- stderr: `image id and immutable descriptor digest must match`
- retained reference digest: `aa…aa`
- retained JSON digest: `aa…aa`
- no `mv` publication calls in the fake-runner log

## Fix

- Make the fake native inspect record internally valid by matching its descriptor digest to image ID `99…99` while retaining an independent valid variant digest `77…77`.
- Log fake `mv` calls.
- In `fail_ref`, prove reference publication was attempted, the retained reference is the old `aa…aa` identity, and the retained JSON is the new `99…99` identity before asserting that the real validator rejects the mixed pair.

No production code or validation contract changed.

## GREEN evidence

`rtk cargo test --manifest-path scripts/Cargo.toml --test connected_image_build fake_runner_failure_matrix_cleans_snapshot_and_never_commits_an_invalid_pair -- --exact`

Result: 1 passed, 0 failed.

Additional fresh verification:

- `rtk cargo test --manifest-path scripts/Cargo.toml --test connected_image_build`: 9 passed.
- `rtk cargo test --manifest-path scripts/Cargo.toml --test connected_image_gate`: passed.
- `rtk cargo test --manifest-path scripts/Cargo.toml`: full scripts suite passed.
- `rtk git diff --check`: passed.

`cargo fmt --check` is not repository-clean because pre-existing files outside this change are unformatted. The changed Rust test was formatted directly with `rustfmt`; unrelated formatting was not retained.
