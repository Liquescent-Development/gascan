# Connected Build Diagnostics Implementation Report

## Scope

Added bounded, private, sanitized diagnostics at the connected `container build` boundary and safe mise version metadata on lock mismatch. No live gate, helper installation, evidence publication, or gate claim was performed.

## RED

- `connected_image_build::fake_runner_failure_matrix_cleans_snapshot_and_never_commits_an_invalid_pair` failed because the build transcript was discarded.
- `connected_dockerfile::dockerfile_prints_safe_mise_version_metadata_only_when_the_lock_comparison_fails` failed because mismatch metadata was absent.

## GREEN

- Fake failures now prove safe output is emitted, forbidden authentication-like output is rejected without echo, output is bounded with a truncation marker, the original statuses 81/82/83 are preserved, and temporary diagnostics are removed.
- The build drains the complete CLI stream while retaining at most 128 KiB plus one truncation byte in a mode `0600`, exclusively-created `.artifacts` temporary file. A separate private marker records forbidden patterns found anywhere in the complete stream.
- Success removes diagnostic files immediately. Failure and signals remove them through the existing cleanup trap before snapshot cleanup.
- The Dockerfile emits only actual and expected resolved version JSON when their comparison fails.

## Verification

- `bash -n scripts/build-connected-workspace-image.sh`
- Focused connected build and Dockerfile tests: 14 passed.
- Connected image and connected workspace filtered suites: passed.
- Full `scripts/Cargo.toml` test suite: passed.
- `cargo fmt --check` remains blocked by pre-existing formatting drift in unrelated test files; the two touched Rust tests were formatted directly, then unrelated churn was removed.

## Security and publication

- Public connected builds accept no credential inputs.
- Diagnostics matching authorization, bearer, token, secret, password, or credential boundaries are not emitted.
- No diagnostic transcript is retained or published on either success or failure.
- Image receipt and reference publication ordering is unchanged.
