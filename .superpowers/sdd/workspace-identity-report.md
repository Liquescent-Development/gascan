# Workspace identity collision correction

## Root cause

The exact pinned Ubuntu base already owns UID and GID 1000 as the canonical
`ubuntu` account and group. Creating a second `workspace` identity at those
IDs therefore fails before the final image can be assembled.

## TDD record

- RED: `image_user_contract` failed because the migration helper and its
  Dockerfile invocation did not exist.
- GREEN: the final stage now invokes a narrow helper which accepts only the
  exact pinned Ubuntu UID/GID 1000 identity, rejects aliases and pre-existing
  workspace identities, renames/moves it with `usermod` and `groupmod`, and
  verifies the exact resulting passwd/group records and home layout.
- The helper forbids non-unique IDs and does not delete/recreate identities.

## Verification

- `cargo test --manifest-path scripts/Cargo.toml --test image_user_contract`
- `cargo test --manifest-path scripts/Cargo.toml --test connected_dockerfile`
- `cargo test --manifest-path scripts/Cargo.toml --test polyglot_image_contract`
- `cargo test --manifest-path scripts/Cargo.toml -- --test-threads=1`
- `cargo clippy --manifest-path scripts/Cargo.toml --test image_user_contract -- -D warnings`
- `shellcheck images/workspace/bin/migrate-workspace-identity`
- `bash -n images/workspace/bin/migrate-workspace-identity`
- `git diff --check`

The repository-wide `cargo fmt --check` remains blocked by pre-existing
formatting drift in unrelated test files. The changed Rust test was formatted
and checked independently.

No live image gate, privileged helper operation, evidence publication, or
approval-marker mutation was performed by this task.

## Review correction

- RED: the behavioral fixture failed before the isolated migration core
  existed.
- GREEN: production uses a fixed-argument wrapper while tests invoke the core
  with explicit fixture files and command paths. Exact success, command order,
  post-state rejection, and twelve prevalidation failures are executed.
- Home validation uses Linux `stat -c '%F:%u:%g'` and rejects missing paths,
  links, non-directories, wrong ownership, and any existing or dangling-link
  destination before either mutation command can run. The resulting home is
  revalidated with the same exact type and numeric ownership contract.

## Connected context follow-up

- Live RED: the sealed connected context omitted the newly introduced
  `images/workspace/libexec/migrate-workspace-identity-core`, so BuildKit could
  not satisfy its Dockerfile `COPY`.
- Test RED reproduced the omission by deriving local, non-stage Dockerfile
  `COPY` sources and requiring exact sealed bytes and executable modes.
- GREEN: connected assembly now reviews and seals the `libexec` tree alongside
  the existing `bin`, `etc`, and `tests` trees; the explicit repository file
  contract also includes both migration files.
- No live build, helper operation, evidence publication, or approval mutation
  was performed for this correction.

### Cross-contract review correction

The contract test now copies and parses the actual repository Dockerfile. It
classifies the three generated artifact sources explicitly, derives repository
sources and `--chmod` modes from each non-stage `COPY`, and compares sealed
bytes and modes. Connected assembly independently parses the sealed Dockerfile
and rejects unsafe syntax or any source absent from the staged context. A
hypothetical unsealed local `COPY` proves this fails before publication.

The follow-up now uses one shared production COPY parser in both preparation
and tests. It handles case-insensitive instructions, leading spaces, flags,
multi-source shell form, and stage copies structurally, while rejecting tabs,
quoting/JSON, continuations, unknown flags, and malformed operands. Sealed
source modes are derived from source executability (0444/0555), independently
of Docker destination `--chmod`; generated artifacts remain explicitly mapped.

Escape directives are now explicitly unsupported and rejected across case and
leading-space variants, including both backtick and default backslash forms;
an end-to-end mutation proves they cannot hide an unsealed multiline COPY.
A fixture-local repository directory COPY now passes through real assembly and
recursively verifies exact descendants and normalized 0444/0555 modes. Nested
symlink, FIFO special-file, and token-like filename mutations each fail without
publishing a context.

The final parser edge is closed by allowing only ordinary spaces as leading
whitespace. Tabs anywhere, and other leading ASCII whitespace such as form
feed, are rejected before directive/comment recognition. Parser and full
preparation mutations cover tab-prefixed backtick/backslash escape directives,
including case variants, and prove no context publication.
