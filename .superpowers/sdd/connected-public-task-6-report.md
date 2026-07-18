# Connected Public Task 6 implementation report

## Scope

Implemented only the platform-neutral correction to the connected image gate harness. The real connected image gate was not run. No live Apple, image, Gate 4, or Gate 5 evidence was created or claimed, and `images/workspace/approved-image.txt` remains absent.

## TDD RED

First RED command:

```sh
rtk cargo test --manifest-path scripts/Cargo.toml --test connected_image_gate
```

Observed result: exit 101; 10 failed and 6 passed. The successful harness path failed with `connected image gate: GASCAMP_READ_TOKEN_FILE is required`, proving the obsolete token-file precondition prevented anonymous operation.

Second focused RED extended the credential boundary:

```sh
rtk proxy cargo test --manifest-path scripts/Cargo.toml --test connected_image_gate gate_rejects_obsolete_credential_input_before_work -- --exact
```

Observed result: exit 101; 0 passed and 1 failed. `GITHUB_TOKEN` was not rejected before connected work, proving the controller needed the same fail-closed authentication-input boundary as the public build entrypoint.

## TDD GREEN

Focused GREEN command:

```sh
rtk proxy cargo test --manifest-path scripts/Cargo.toml --test connected_image_gate gate_rejects_obsolete_credential_input_before_work -- --exact
```

Observed result: exit 0; 1 passed, 0 failed. The harness rejected `GASCAMP_READ_TOKEN_FILE`, `GITHUB_TOKEN`, `DOCKER_AUTH_CONFIG`, and `CUSTOM_BUILD_CREDENTIAL` before prefetch/build activity.

Required platform-neutral verification:

```sh
rtk proxy cargo test --manifest-path scripts/Cargo.toml --test connected_image_gate --test image_user_contract --test polyglot_image_contract
rtk bash -n scripts/run-connected-image-gate.sh tests/image/user-and-volumes.sh tests/image/polyglot-smoke.sh tests/image/gascamp-smoke.sh
rtk git diff --check
```

Observed result: exit 0. Test counts were 16/16 connected gate, 4/4 image user contract, and 4/4 polyglot image contract. Shell syntax validation and diff whitespace validation passed.

## Files changed

- `scripts/run-connected-image-gate.sh`: removed the obsolete token-file requirement, secret-file metadata/canonicalization assumptions, and token propagation into the build; added fail-closed rejection of authentication inputs before connected work.
- `scripts/tests/connected_image_gate.rs`: made the fake anonymous build independent of secret files and added credential-boundary coverage while retaining the cleanup, receipt, inventory, signal, residue, and atomic-publication matrix.
- `docs/evidence/connected-workspace-image.md`: retained `PENDING` status and now describes the pending anonymous live run without a credential prerequisite.

## Self-review

- Existing ownership validation still immediately precedes every authoritative stop/delete mutation.
- Cleanup remains bounded, signal-safe, and fail-closed; authoritative inventory absence remains required when inspect cannot prove absence.
- Exact receipt/reference, digest, and `linux/arm64` validation remain unchanged.
- Evidence and approval publication remain staged, residue-gated, atomic, and rollback-safe.
- Unrelated and foreign resource protections remain covered by the passing harness.
- Obsolete credential inputs are rejected without printing their values and before prefetch/build activity.
- No smoke script behavior needed correction; their immutable digest-qualified reference validation and shared owner-token behavior remain covered by the passing contract suite.
- The unrelated pre-existing `.superpowers/sdd/progress.md` modification was not changed or staged.

## Live-evidence disclaimer

The real connected image gate was deliberately not run. This report contains platform-neutral fake-controller test evidence only. It does not establish an Apple-host build, a real image digest, live smoke results, Gate 4 PASS, or Gate 5 PASS. No `images/workspace/approved-image.txt` was created.
