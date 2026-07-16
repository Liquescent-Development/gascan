# Connected helper path compatibility report

## Root cause

The connected image orchestrator seals `.artifacts/connected-workspace-context`, but
`validate_caller_source` accepted only the reviewed offline basename
`.artifacts/workspace-context`. The privileged helper therefore rejected the connected
context before creating a snapshot.

## RED

Added helper-level contract tests for both reviewed context names and the surrounding
security boundary. Before the implementation change:

```text
tests::reviewed_workspace_context_names_are_accepted ... FAILED
connected-workspace-context
test result: FAILED. 0 passed; 1 failed
```

The existing offline name remained accepted in the same test.

## GREEN

Changed the basename check to an explicit two-entry allowlist:

- `workspace-context`
- `connected-workspace-context`

Canonical-path, `.artifacts` parent, directory type, symlink, and caller-ownership
checks remain unchanged. Tests cover rejection of an arbitrary sibling name, a
non-`.artifacts` parent, a symlink alias, a mismatched caller UID, and a regular file.

Verification:

- `rtk cargo test --manifest-path scripts/Cargo.toml --bin snapshot-workspace-context` — 12 passed.
- `rtk rustfmt --edition 2024 --check scripts/src/bin/snapshot-workspace-context.rs` — passed.
- `rtk cargo test --manifest-path scripts/Cargo.toml` — helper and relevant contracts
  passed, but the pre-existing `connected_image_build` interruption assertion failed:
  `interrupted old-reference/new-JSON pair was accepted`.
- `rtk git diff --check` — passed.

The full-suite failure is outside this helper-path change and reproduces when its test
is run alone. The standalone `workspace_snapshot` integration target also contains an
unrelated stale assertion: it expects the privileged snapshot contract directly in
`scripts/build-workspace-image.sh`, while that dispatcher does not contain the asserted
implementation. When invoked alone, that pre-existing assertion fails. No helper was
installed and no live gate or evidence publication was attempted.

## Review correction

The first alias test used the disallowed basename `source-alias`, so the allowlist alone
could reject it without exercising canonical-path protection. The corrected RED test
creates `.artifacts/connected-workspace-context` as a symlink, proves its basename and
parent satisfy the explicit contract, proves canonicalization resolves elsewhere, and
expects the canonical/symlink-specific rejection. The test failed against the first
implementation because it returned the combined allowlist error. GREEN separates that
diagnostic without weakening any check; the valid-name alias is now rejected explicitly
because its canonical path differs.
