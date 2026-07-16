# Apple container schema and snapshot-helper preflight report

## Root cause

- Native Apple `container list --all --format json` and `container inspect NAME` records identify containers with top-level `id` and `configuration.id`; they do not provide `configuration.name`. Both cleanup validators required that absent field, so cleanup failed closed with Serde `missing field name` errors.
- The connected gate invoked the reviewed `snapshot-helper-identity` boundary only inside the build script. Consequently, a missing or unsafe fixed helper was discovered after connected prefetch work instead of before network/container activity.

## TDD evidence

- RED: `native_apple_identity_shape_is_accepted_without_configuration_name` failed because `validate-owned-container` required `configuration.name`.
- RED: the missing/unsafe helper gate test reached later work because no early helper preflight existed.
- GREEN: ownership now requires exact equality of the expected name with both top-level `id` and `configuration.id`, while retaining both exact ownership-label checks. Inventory absence checks exact top-level native `id` and remains fail-closed on malformed input.
- GREEN: the live gate validates the fixed `/Library/PrivilegedHelperTools/dev.gascan.snapshot-workspace-context` path with the existing Rust identity boundary before prefetch or container activity. Tests use explicit non-live helper and identity-boundary hooks; the live path has no override or bypass.

## Verification

- `rtk cargo test --manifest-path scripts/Cargo.toml --test container_ownership --test connected_image_gate` — passed.
- `rtk cargo test --manifest-path scripts/Cargo.toml` — passed.
- `rtk proxy bash -n scripts/run-connected-image-gate.sh` — passed.
- `rtk git diff --check` — passed.

No live gate was run, no helper was installed, and no receipt, evidence PASS, or approved image was created.
