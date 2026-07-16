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

## Independent review correction

The first implementation was rejected because its shell scanner wrote unsanitized bytes before completing its scan, did not match opaque known credential values, and could fail open if the scanner failed.

### RED

- The new sanitizer test initially failed to compile because the reviewed binary did not exist.
- The exact-bound test then failed because the first helper invocation retained 128 KiB rather than the required 128 KiB plus one truncation byte.
- The existing Dockerfile test failed when the comparison changed to quiet mode, proving the contract was exercised.

### GREEN

- `sanitize-build-output` drains the complete stream into a bounded in-memory candidate, scans keywords and every nonempty value belonging to a credential-policy environment name, and only creates a diagnostic after a clean EOF.
- Sensitive output returns status 42 without ever creating an artifact. Scanner, input, create, and write failures also create or retain nothing.
- Output creation is `create_new`, mode `0600`, and rejects existing files and symlinks. The exact maximum is 128 KiB plus one byte so truncation can be proven without unbounded storage.
- Tests cover early sensitive output followed by a 1 MB tail, sensitive output after 128 KiB, opaque credential values across all policy name families, exact bounds, symlink/existing-file refusal, scanner failure injection, original build-status preservation, truncation, success cleanup, TERM cleanup, and diagnostic removal before privileged snapshot finish.
- Diagnostic security failure deliberately takes precedence with status 1. When sanitization succeeds, the original container status is preserved.
- The Dockerfile uses quiet comparison. Executed behavior tests prove matching JSON is silent and mismatched JSON emits exactly the actual and expected documents, without labels or stderr, then fails.

### Final verification

- Focused sanitizer, connected-build, and Dockerfile suites: 19 passed.
- Full `scripts/Cargo.toml` suite: passed.
- Shell syntax and `git diff --check`: passed.
- No live gate, helper installation, image evidence, or gate claim was performed.
