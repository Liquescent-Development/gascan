# Native Apple container schema / early snapshot-helper preflight review

## Findings

### Important — test hooks can bypass the early live helper preflight while targeting the live checkout

`root` is taken verbatim from `GASCAN_GATE_TEST_ROOT`, while `tool_root` is canonicalized with `pwd -P`; the live/test decision then uses raw string equality ([scripts/run-connected-image-gate.sh:4](../../scripts/run-connected-image-gate.sh), [scripts/run-connected-image-gate.sh:5](../../scripts/run-connected-image-gate.sh), [scripts/run-connected-image-gate.sh:65](../../scripts/run-connected-image-gate.sh)). A caller can set `GASCAN_GATE_TEST_ROOT="$tool_root/."` (or a symlink spelling of the same directory), making the comparison false although all subsequent scripts, evidence, and approval paths resolve into the live checkout. That activates `GASCAN_GATE_TEST_SNAPSHOT_HELPER` and `GASCAN_GATE_TEST_HELPER_IDENTITY_BIN` ([scripts/run-connected-image-gate.sh:68](../../scripts/run-connected-image-gate.sh)) and permits prefetch to begin before the fixed helper is checked. The build script later checks the fixed helper ([scripts/build-connected-workspace-image.sh:64](../../scripts/build-connected-workspace-image.sh)), but that is after prefetch, so it does not satisfy the early-preflight invariant. Canonicalize/validate the selected root before choosing the branch, and add a regression test using an alias of the repository root.

### Minor — the inventory-present regression fixture still uses the obsolete schema

The `present` case emits only `configuration.name` ([scripts/tests/connected_image_gate.rs:589](../../scripts/tests/connected_image_gate.rs)). It now fails closed because top-level `id` is missing, so the test can pass without proving that a native Apple record whose top-level `id` exactly matches is detected as present. Update this fixture (or add a direct inventory-validator test) with top-level `id`; retain separate missing/malformed cases.

## Spec compliance

**Needs fixes.** The schema implementation itself is compliant: inventory absence compares exact top-level `id` and malformed input fails closed ([scripts/src/bin/validate-container-inventory.rs:7](../../scripts/src/bin/validate-container-inventory.rs), [scripts/src/bin/validate-container-inventory.rs:19](../../scripts/src/bin/validate-container-inventory.rs)); ownership requires one record, exact equality of top-level `id` and `configuration.id`, and both exact labels ([scripts/src/bin/validate-owned-container.rs:31](../../scripts/src/bin/validate-owned-container.rs), [scripts/src/bin/validate-owned-container.rs:37](../../scripts/src/bin/validate-owned-container.rs), [scripts/src/bin/validate-owned-container.rs:40](../../scripts/src/bin/validate-owned-container.rs)). Missing fields, malformed JSON, multiple inspect records, mismatched identities, and foreign labels fail closed. Cleanup remains bounded and revalidates ownership before mutation ([scripts/run-connected-image-gate.sh:77](../../scripts/run-connected-image-gate.sh), [scripts/run-connected-image-gate.sh:94](../../scripts/run-connected-image-gate.sh)). However, the Important live/test path-alias bypass violates the explicit production-root and early-preflight requirements.

## Task quality

**Needs fixes.** The ownership tests cover native shape, both identity mismatches, foreign owner, empty and ambiguous inspect output ([scripts/tests/container_ownership.rs:23](../../scripts/tests/container_ownership.rs), [scripts/tests/container_ownership.rs:42](../../scripts/tests/container_ownership.rs)). The helper failure test proves ordering in an isolated fixture ([scripts/tests/connected_image_gate.rs:148](../../scripts/tests/connected_image_gate.rs)), but does not exercise the production-root alias boundary. Rustfmt-only changes are mechanical and introduce no identified behavior change. No live gate was run during this review, and this review makes no receipt, evidence-PASS, or image-approval claim.

## Focused verification

- `rtk cargo test --manifest-path scripts/Cargo.toml --test container_ownership native_apple_identity_shape_is_accepted_without_configuration_name`
- `rtk cargo test --manifest-path scripts/Cargo.toml --test connected_image_gate missing_or_unsafe_snapshot_helper_fails_before_prefetch_or_container_activity`
- Combined command exited 0; the filtered output reported the ownership test passing. No broad suite was rerun.

## Review-finding correction

- RED: dot-segment and symlink aliases of the live checkout both made the raw-string root comparison select the non-live branch and execute the supplied test identity hook.
- GREEN: the configured root is now resolved with physical-path semantics before live/test classification. Any alias resolving to the live checkout ignores test hooks and enters the fixed `/Library/PrivilegedHelperTools/dev.gascan.snapshot-workspace-context` existence/identity preflight before sudo, prefetch, or container activity. Legitimate isolated fixture roots still use their explicit hooks.
- The cleanup inventory presence fixture now uses the observed native top-level `id` plus `configuration.id` shape and asserts exact current-token presence is rejected with `exact container remains in inventory`, rather than merely accepting any nonzero failure.
- `rtk cargo test --manifest-path scripts/Cargo.toml --test connected_image_gate` — passed after the correction.
- `rtk cargo test --manifest-path scripts/Cargo.toml` — passed after the correction.
- No live gate was run and no evidence, receipt, or approval marker was created.
