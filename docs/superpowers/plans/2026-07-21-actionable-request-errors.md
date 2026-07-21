# Actionable Request Errors Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `gascan up .` work and make request-validation failures say what
is wrong instead of `daemon error: invalid_request`.

**Architecture:** The CLI resolves project roots to absolute paths before
sending them, because a relative path names the *client's* directory and the
daemon runs elsewhere; the daemon keeps rejecting relative roots as defense in
depth. Validation failures keep their stable code in the gRPC status message,
where clients already read it, and carry the human cause in the status details
using the `v1::Error` fields the proto already reserves.

**Tech Stack:** Rust (edition 2024), tonic 0.12.3, prost, clap, tokio.

Design: `docs/superpowers/specs/2026-07-21-actionable-request-errors-design.md`

## Global Constraints

- Build and test with `--locked`. The one permitted `Cargo.lock` change is the
  `tempfile` dev-dependency edge added in Task 3, Step 2; no package version may
  move and no new package may appear.
- Add no new runtime dependency. `bytes::Bytes` is reachable as
  `tonic::codegen::Bytes`; `prost` is already a dependency of `gascan-proto`.
  `tempfile` is test-only and never ships in the package payload.
- No proto schema change. `v1::Error { code, message, details }` already exists.
- Additive API only: new codes join `error_code::ALL`; `API_MINOR` goes 0 -> 1.
- `tonic::Status::message()` must keep carrying the stable code for every
  existing code. Detail travels only in status details.
- `cargo clippy --locked --workspace --all-targets -- -D warnings` and
  `cargo fmt --check` must pass at every commit.
- Scope is request validation for `up` and `apply` only. Do not reclassify
  runtime, policy, or sandbox errors.

---

## File Structure

| File | Responsibility |
| --- | --- |
| `crates/gascan-proto/src/lib.rs` | Two new codes, `API_MINOR` bump, and `error_detail` encode/decode shared by both sides |
| `crates/gascan-proto/tests/api_compatibility.rs` | Codes stay public and unique; detail round-trips and degrades |
| `crates/gascand/src/api.rs` | `RequestError` carrying code + cause; `spec_for_root` stops discarding causes |
| `crates/gascan/src/cli.rs` | `resolve_project_root`, used by `up` and `apply` |
| `crates/gascan/src/client.rs` | Render the cause when details are present |

Encoding lives in `gascan-proto` so the daemon and CLI share one implementation
and one round-trip test, and `prost` stays an implementation detail rather than
leaking into the CLI's dependencies.

---

### Task 1: Shared error detail and new codes

**Files:**
- Modify: `crates/gascan-proto/src/lib.rs`
- Test: `crates/gascan-proto/tests/api_compatibility.rs`

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `gascan_proto::error_code::INVALID_MANIFEST: &str` = `"invalid_manifest"`
  - `gascan_proto::error_code::INVALID_PROJECT_ROOT: &str` = `"invalid_project_root"`
  - `gascan_proto::error_detail::encode(code: &str, message: &str) -> Vec<u8>`
  - `gascan_proto::error_detail::decode_message(details: &[u8]) -> Option<String>`

- [ ] **Step 1: Write the failing tests**

Append to `crates/gascan-proto/tests/api_compatibility.rs`:

```rust
#[test]
fn request_validation_codes_are_public_and_unique() {
    let codes = gascan_proto::error_code::ALL;
    assert!(codes.contains(&"invalid_manifest"));
    assert!(codes.contains(&"invalid_project_root"));
    assert_eq!(
        codes.len(),
        codes.iter().copied().collect::<HashSet<_>>().len()
    );
}

#[test]
fn error_detail_round_trips_the_human_cause() {
    let encoded = gascan_proto::error_detail::encode(
        gascan_proto::error_code::INVALID_MANIFEST,
        "unknown variant `kiener`, expected `workspace` or `root`",
    );
    assert_eq!(
        gascan_proto::error_detail::decode_message(&encoded).as_deref(),
        Some("unknown variant `kiener`, expected `workspace` or `root`")
    );
}

#[test]
fn error_detail_degrades_instead_of_failing() {
    // Absent details: an older daemon sends none, and the caller must fall back
    // to the stable code rather than error.
    assert_eq!(gascan_proto::error_detail::decode_message(&[]), None);
    // Truncated: field 1 is a length-5 string with no bytes following.
    assert_eq!(gascan_proto::error_detail::decode_message(&[0x0a, 0x05]), None);
    // Well-formed but empty message: nothing useful to show.
    let empty = gascan_proto::error_detail::encode("invalid_manifest", "");
    assert_eq!(gascan_proto::error_detail::decode_message(&empty), None);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --locked -p gascan-proto --test api_compatibility 2>&1 | tail -20`
Expected: FAIL. `error[E0433]: failed to resolve: could not find 'error_detail' in 'gascan_proto'`.

- [ ] **Step 3: Add the codes and bump the minor version**

In `crates/gascan-proto/src/lib.rs`, change line 8:

```rust
pub const API_MINOR: u32 = 1;
```

Inside `pub mod error_code`, after the `INVALID_REQUEST` constant, add:

```rust
    /// A manifest could not be parsed or is not valid for its project root.
    pub const INVALID_MANIFEST: &str = "invalid_manifest";
    /// A project root is empty, relative, missing, or not a directory.
    pub const INVALID_PROJECT_ROOT: &str = "invalid_project_root";
```

In the same module, add both to `ALL`, immediately after `INVALID_REQUEST`:

```rust
    pub const ALL: &[&str] = &[
        INCOMPATIBLE_API_MAJOR,
        INVALID_REQUEST,
        INVALID_MANIFEST,
        INVALID_PROJECT_ROOT,
        DISK_CONTROL_UNSUPPORTED,
        SANDBOX_NOT_FOUND,
        OPERATION_CONFLICT,
        BACKEND_UNAVAILABLE,
        INTERNAL,
        EMPTY_SESSION_TOKEN,
        UNKNOWN_SESSION_TOKEN,
        EXPIRED_SESSION_TOKEN,
        SESSION_TOKEN_MISMATCH,
    ];
```

- [ ] **Step 4: Add the `error_detail` module**

In `crates/gascan-proto/src/lib.rs`, after the `error_code` module closes, add:

```rust
/// Encoding for the human-readable cause that travels beside a stable code.
///
/// The stable code stays in the gRPC status message, where every existing
/// client already reads it. The cause travels in the status details, so adding
/// one cannot change what an older client sees, and a client that does not
/// understand details keeps working unchanged.
pub mod error_detail {
    use prost::Message as _;

    /// Encode a code and human cause for `tonic::Status::with_details`.
    #[must_use]
    pub fn encode(code: &str, message: &str) -> Vec<u8> {
        super::v1::Error {
            code: code.to_owned(),
            message: message.to_owned(),
            details: Vec::new(),
        }
        .encode_to_vec()
    }

    /// Recover the human cause produced by [`encode`].
    ///
    /// Returns `None` for absent, malformed, or empty details so a caller can
    /// fall back to the stable code. A malformed detail must never be worse
    /// than no detail at all.
    #[must_use]
    pub fn decode_message(details: &[u8]) -> Option<String> {
        if details.is_empty() {
            return None;
        }
        let error = super::v1::Error::decode(details).ok()?;
        (!error.message.is_empty()).then_some(error.message)
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --locked -p gascan-proto 2>&1 | tail -20`
Expected: PASS. All `api_compatibility` tests pass, including the pre-existing
`public_error_codes_are_stable_and_unique`.

- [ ] **Step 6: Verify lints and formatting**

Run: `cargo clippy --locked -p gascan-proto --all-targets -- -D warnings && cargo fmt --check`
Expected: exit 0, no output.

- [ ] **Step 7: Commit**

```bash
git add crates/gascan-proto/src/lib.rs crates/gascan-proto/tests/api_compatibility.rs
git commit -m "feat: carry a human cause beside stable request-error codes

Add invalid_manifest and invalid_project_root, and an error_detail helper that
encodes a cause into the v1::Error fields the proto already reserves. Both
sides share one implementation so the round-trip has a single test, and prost
stays an implementation detail of gascan-proto.

Additive: existing codes keep their position in the status message, so API_MINOR
moves 0 -> 1 rather than breaking v1."
```

---

### Task 2: The daemon stops discarding causes

**Files:**
- Modify: `crates/gascand/src/api.rs` (`spec_for_root` at line 865; call sites at 1352 and 1386; existing test `project_roots_are_absolute_and_manifest_bound` at ~2331)
- Test: `crates/gascand/src/api.rs` (`mod tests` at line 1672)

**Interfaces:**
- Consumes: `gascan_proto::error_code::{INVALID_MANIFEST, INVALID_PROJECT_ROOT, INTERNAL}`, `gascan_proto::error_detail::encode`.
- Produces: `spec_for_root(project_root: String) -> Result<SandboxSpec, RequestError>`, where `RequestError::status(self) -> tonic::Status`.

`ApiInputError` is deliberately left alone. It stays `Copy` and keeps serving
`selector_id`, `timestamp_millis`, and `argv_from_wire` unchanged; only
`spec_for_root` moves to the richer type.

- [ ] **Step 1: Write the failing tests**

Add inside `mod tests` in `crates/gascand/src/api.rs`:

```rust
    #[tokio::test]
    async fn invalid_manifest_reports_its_cause() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        std::fs::write(
            directory.path().join("gascan.toml"),
            "version = 1\nuser = \"kiener\"\n",
        )?;
        let root = directory
            .path()
            .to_str()
            .ok_or("non-UTF-8 fixture")?
            .to_owned();
        let status = spec_for_root(root)
            .await
            .err()
            .ok_or("an unknown user mode must be rejected")?
            .status();
        assert_eq!(status.message(), error_code::INVALID_MANIFEST);
        let cause = gascan_proto::error_detail::decode_message(status.details())
            .ok_or("the status must carry a human cause")?;
        assert!(
            cause.contains("kiener"),
            "the cause must quote the rejected value: {cause}"
        );
        assert!(
            cause.contains("workspace") && cause.contains("root"),
            "the cause must name the accepted values: {cause}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn project_root_rejections_report_their_cause()
    -> Result<(), Box<dyn std::error::Error>> {
        for root in ["", "relative"] {
            let status = spec_for_root(root.to_owned())
                .await
                .err()
                .ok_or("a non-absolute project root must be rejected")?
                .status();
            assert_eq!(status.message(), error_code::INVALID_PROJECT_ROOT);
            assert!(
                gascan_proto::error_detail::decode_message(status.details()).is_some(),
                "a rejected project root must explain itself"
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn resolving_a_root_does_not_change_sandbox_identity()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let root = directory
            .path()
            .to_str()
            .ok_or("non-UTF-8 fixture")?
            .to_owned();
        let plain = spec_for_root(root.clone())
            .await
            .map_err(|_| std::io::Error::other("default manifest rejected"))?;
        let dotted = spec_for_root(format!("{root}/."))
            .await
            .map_err(|_| std::io::Error::other("default manifest rejected"))?;
        assert_eq!(plain.id(), dotted.id());
        Ok(())
    }
```

- [ ] **Step 2: Update the existing test that asserts the old type**

Replace the two `ApiInputError::Invalid` assertions in
`project_roots_are_absolute_and_manifest_bound` so it checks the code rather
than the variant:

```rust
        assert_eq!(
            spec_for_root(String::new())
                .await
                .err()
                .ok_or("an empty project root must be rejected")?
                .status()
                .message(),
            error_code::INVALID_PROJECT_ROOT
        );
        assert_eq!(
            spec_for_root("relative".to_owned())
                .await
                .err()
                .ok_or("a relative project root must be rejected")?
                .status()
                .message(),
            error_code::INVALID_PROJECT_ROOT
        );
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test --locked -p gascand --lib api 2>&1 | tail -20`
Expected: FAIL to compile with `cannot find type 'RequestError'` (the new tests
call `.status()` on the error returned by `spec_for_root`).

- [ ] **Step 4: Add `RequestError`**

In `crates/gascand/src/api.rs`, immediately after the `impl ApiInputError`
block (which ends at line 620), add:

```rust
/// A rejected request: a stable code plus the cause to show the operator.
///
/// The code stays in the gRPC status message, where clients already read it,
/// and the cause travels in the status details. Splitting them this way is what
/// lets the daemon explain a failure without changing the wire contract an
/// existing client depends on.
struct RequestError {
    grpc: tonic::Code,
    code: &'static str,
    cause: Option<String>,
}

impl RequestError {
    fn invalid(code: &'static str, cause: impl Into<String>) -> Self {
        Self {
            grpc: tonic::Code::InvalidArgument,
            code,
            cause: Some(cause.into()),
        }
    }

    fn internal() -> Self {
        Self {
            grpc: tonic::Code::Internal,
            code: error_code::INTERNAL,
            cause: None,
        }
    }

    fn status(self) -> tonic::Status {
        let Some(cause) = self.cause else {
            return tonic::Status::new(self.grpc, self.code);
        };
        tonic::Status::with_details(
            self.grpc,
            self.code,
            tonic::codegen::Bytes::from(gascan_proto::error_detail::encode(self.code, &cause)),
        )
    }
}
```

- [ ] **Step 5: Rewrite `spec_for_root` to preserve every cause**

Replace the whole `spec_for_root` function (line 865 to the end of its body)
with:

```rust
async fn spec_for_root(project_root: String) -> Result<SandboxSpec, RequestError> {
    if project_root.is_empty() {
        return Err(RequestError::invalid(
            error_code::INVALID_PROJECT_ROOT,
            "project root must not be empty",
        ));
    }
    let root = Utf8PathBuf::from(project_root);
    if !root.is_absolute() {
        // Relative roots are resolved by the client, which knows its own
        // working directory. The daemon's does not match, so it refuses rather
        // than guessing.
        return Err(RequestError::invalid(
            error_code::INVALID_PROJECT_ROOT,
            format!("project root must be an absolute path, but `{root}` is relative"),
        ));
    }
    tokio::task::spawn_blocking(move || {
        let manifest = Manifest::load(&root).map_err(|error| {
            RequestError::invalid(
                error_code::INVALID_MANIFEST,
                format!("cannot use {}: {error}", root.join("gascan.toml")),
            )
        })?;
        let name = manifest
            .name()
            .map(ToOwned::to_owned)
            .or_else(|| root.file_name().map(ToOwned::to_owned))
            .ok_or_else(|| {
                RequestError::invalid(
                    error_code::INVALID_PROJECT_ROOT,
                    format!("cannot derive a sandbox name from `{root}`; set `name` in gascan.toml"),
                )
            })?;
        SandboxSpec::from_root(&name, &root, manifest).map_err(|error| match error {
            gascan_core::sandbox::SandboxError::InvalidManifest(inner) => RequestError::invalid(
                error_code::INVALID_MANIFEST,
                format!("manifest is not valid for `{root}`: {inner}"),
            ),
            other => RequestError::invalid(
                error_code::INVALID_PROJECT_ROOT,
                format!("cannot use `{root}` as a project root: {other}"),
            ),
        })
    })
    .await
    .map_err(|_| RequestError::internal())?
}
```

- [ ] **Step 6: Update the two call sites**

At line 1352 (`up`) and line 1386 (`apply`), change the mapper:

```rust
        let spec = spec_for_root(request.into_inner().project_root)
            .await
            .map_err(RequestError::status)?;
```

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test --locked -p gascand 2>&1 | tail -20`
Expected: PASS, 0 failed. If `gascan_core::sandbox::SandboxError` is not in
scope, add it to the existing `use gascan_core::sandbox::{...}` import rather
than using the full path.

- [ ] **Step 8: Verify lints and formatting**

Run: `cargo clippy --locked -p gascand --all-targets -- -D warnings && cargo fmt --check`
Expected: exit 0, no output.

- [ ] **Step 9: Commit**

```bash
git add crates/gascand/src/api.rs
git commit -m "fix: report why a request was rejected instead of invalid_request

spec_for_root mapped three distinct failures through map_err(|_| Invalid),
so a bad manifest and a relative path were indistinguishable at the CLI. Each
failure now carries its own code and the cause the daemon already had.

ApiInputError is untouched and stays Copy for its other three callers."
```

---

### Task 3: The CLI resolves project roots

**Files:**
- Modify: `crates/gascan/src/cli.rs` (`Up` at 135-145, `Apply` at 146-160)
- Test: `crates/gascan/src/cli.rs` (new `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `CliError::Usage(String)`, already defined at line 76 and already
  mapped to `EXIT_USAGE` (64).
- Produces: `fn resolve_project_root(project_root: &str) -> Result<String, CliError>`.

- [ ] **Step 1: Write the failing tests**

Append to `crates/gascan/src/cli.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::resolve_project_root;

    #[test]
    fn relative_roots_resolve_against_this_process() -> Result<(), Box<dyn std::error::Error>> {
        let resolved = resolve_project_root(".")?;
        assert_eq!(
            std::path::Path::new(&resolved),
            std::env::current_dir()?.canonicalize()?
        );
        assert!(std::path::Path::new(&resolved).is_absolute());
        Ok(())
    }

    #[test]
    fn absolute_roots_survive_resolution() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let canonical = directory.path().canonicalize()?;
        let resolved = resolve_project_root(canonical.to_str().ok_or("non-UTF-8 fixture")?)?;
        assert_eq!(std::path::Path::new(&resolved), canonical);
        Ok(())
    }

    #[test]
    fn dot_segments_and_trailing_slashes_normalize()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let canonical = directory.path().canonicalize()?;
        let base = canonical.to_str().ok_or("non-UTF-8 fixture")?;
        for variant in [format!("{base}/"), format!("{base}/."), format!("{base}/./")] {
            assert_eq!(
                std::path::Path::new(&resolve_project_root(&variant)?),
                canonical,
                "variant {variant} must normalize"
            );
        }
        Ok(())
    }

    #[test]
    fn parent_and_nested_segments_resolve() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let canonical = directory.path().canonicalize()?;
        std::fs::create_dir(canonical.join("nested"))?;
        let base = canonical.to_str().ok_or("non-UTF-8 fixture")?;

        // A nested relative segment.
        assert_eq!(
            std::path::Path::new(&resolve_project_root(&format!("{base}/nested"))?),
            canonical.join("nested")
        );
        // A parent segment that climbs back out of it.
        assert_eq!(
            std::path::Path::new(&resolve_project_root(&format!("{base}/nested/.."))?),
            canonical
        );
        Ok(())
    }

    #[test]
    fn a_symlinked_root_resolves_to_its_target() -> Result<(), Box<dyn std::error::Error>> {
        // The daemon canonicalizes too, so the client must agree with it about
        // which directory a symlink names; otherwise the same project could
        // produce two sandbox identities.
        let directory = tempfile::tempdir()?;
        let canonical = directory.path().canonicalize()?;
        let target = canonical.join("project");
        std::fs::create_dir(&target)?;
        let link = canonical.join("link");
        std::os::unix::fs::symlink(&target, &link)?;

        let resolved = resolve_project_root(link.to_str().ok_or("non-UTF-8 fixture")?)?;
        assert_eq!(std::path::Path::new(&resolved), target);
        Ok(())
    }

    #[test]
    fn a_missing_root_fails_here_rather_than_at_the_daemon() {
        let error = resolve_project_root("/definitely/not/a/real/project/root")
            .expect_err("a missing root must be rejected");
        assert_eq!(error.exit_code(), super::EXIT_USAGE);
        assert!(
            format!("{error}").contains("/definitely/not/a/real/project/root"),
            "the message must name the offending path"
        );
    }

    #[test]
    fn an_empty_root_is_rejected() {
        let error = resolve_project_root("").expect_err("an empty root must be rejected");
        assert_eq!(error.exit_code(), super::EXIT_USAGE);
    }
}
```

- [ ] **Step 2: Add `tempfile` as a dev-dependency of the CLI crate**

`crates/gascan/Cargo.toml` has no `[dev-dependencies]` section. Add one after
`[dependencies]`. `tempfile` is *not* a workspace dependency — `gascand`
declares it directly — so match that exactly:

```toml
[dev-dependencies]
tempfile = "3"
```

This adds a dependency edge to `Cargo.lock`, so `--locked` will refuse until the
lock is refreshed. Refresh it and confirm nothing else moved:

```bash
cargo check --workspace --all-targets
git diff --stat Cargo.lock
git diff Cargo.lock
```

Expected: the only change is `tempfile` appearing in the `gascan` package's
dependency list. No package version anywhere may change, and no new package may
appear — `tempfile` and its transitive dependencies are already in the lock via
`gascand`. If any version moves, stop: that is dependency drift, not this
change.

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test --locked -p gascan 2>&1 | tail -20`
Expected: FAIL to compile with `cannot find function 'resolve_project_root'`.

- [ ] **Step 4: Implement `resolve_project_root`**

In `crates/gascan/src/cli.rs`, add above the `execute` function:

```rust
/// Resolve a project root to the absolute path the daemon requires.
///
/// A relative path names a directory relative to *this* process. The daemon
/// runs with a different working directory, so resolving there would mount the
/// wrong directory; resolution has to happen on this side. The daemon still
/// rejects a relative root, and that check stays: it is the boundary, not a
/// fallback for this function.
fn resolve_project_root(project_root: &str) -> Result<String, CliError> {
    if project_root.is_empty() {
        return Err(CliError::Usage(
            "project root must not be empty".to_owned(),
        ));
    }
    let resolved = std::fs::canonicalize(project_root).map_err(|error| {
        CliError::Usage(format!("cannot use `{project_root}` as a project root: {error}"))
    })?;
    resolved
        .to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| CliError::Usage(format!("project root `{project_root}` is not valid UTF-8")))
}
```

- [ ] **Step 5: Use it in `up` and `apply`**

Replace the `Command::Up` arm:

```rust
        Command::Up { project_root, json } => {
            let project_root = resolve_project_root(&project_root)?;
            operation(
                client
                    .api
                    .up(v1::UpRequest { project_root })
                    .await?
                    .into_inner(),
                json,
            )
            .await
        }
```

Replace the `Command::Apply` arm. `apply` has the same hole today: an omitted
argument becomes `current_dir()`, but a relative argument is sent unresolved.

```rust
        Command::Apply { project_root, json } => {
            let root = match project_root {
                Some(root) => resolve_project_root(&root)?,
                None => resolve_project_root(".")?,
            };
            operation(
                client
                    .api
                    .apply(v1::ApplyRequest { project_root: root })
                    .await?
                    .into_inner(),
                json,
            )
            .await
        }
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test --locked -p gascan 2>&1 | tail -20`
Expected: PASS, 0 failed.

- [ ] **Step 7: Verify lints and formatting**

Run: `cargo clippy --locked -p gascan --all-targets -- -D warnings && cargo fmt --check`
Expected: exit 0, no output.

- [ ] **Step 8: Commit**

```bash
git add crates/gascan/src/cli.rs crates/gascan/Cargo.toml
git commit -m "fix: resolve project roots before sending them to the daemon

up sent its argument verbatim, so 'gascan up .' was rejected as a relative
root. Resolution belongs on this side: a relative path names this process's
directory, and the daemon's differs, so resolving there would mount the wrong
one. apply had the same hole for an explicit relative argument.

The daemon's absolute-path check is unchanged and is now defense in depth."
```

---

### Task 4: The CLI shows the cause

**Files:**
- Modify: `crates/gascan/src/client.rs:23` (the `Rpc` arm of `Display`)
- Test: `crates/gascan/src/client.rs` (existing `#[cfg(test)] mod` at line 202)

**Interfaces:**
- Consumes: `gascan_proto::error_detail::decode_message` from Task 1.
- Produces: no new public surface.

- [ ] **Step 1: Write the failing tests**

Add inside the existing `#[cfg(test)] mod` in `crates/gascan/src/client.rs`:

```rust
    #[test]
    fn rpc_errors_show_the_cause_when_the_daemon_sends_one() {
        let details = gascan_proto::error_detail::encode(
            gascan_proto::error_code::INVALID_MANIFEST,
            "unknown variant `kiener`, expected `workspace` or `root`",
        );
        let status = tonic::Status::with_details(
            tonic::Code::InvalidArgument,
            gascan_proto::error_code::INVALID_MANIFEST,
            tonic::codegen::Bytes::from(details),
        );
        let rendered = format!("{}", super::ClientError::Rpc(Box::new(status)));
        assert!(
            rendered.contains("unknown variant `kiener`"),
            "the cause must reach the operator: {rendered}"
        );
    }

    #[test]
    fn rpc_errors_fall_back_to_the_code_without_details() {
        let status = tonic::Status::invalid_argument(gascan_proto::error_code::INVALID_REQUEST);
        let rendered = format!("{}", super::ClientError::Rpc(Box::new(status)));
        assert_eq!(rendered, "daemon error: invalid_request");
    }

    #[test]
    fn malformed_details_never_panic_and_fall_back() {
        let status = tonic::Status::with_details(
            tonic::Code::InvalidArgument,
            gascan_proto::error_code::INVALID_REQUEST,
            tonic::codegen::Bytes::from_static(&[0x0a, 0x05]),
        );
        let rendered = format!("{}", super::ClientError::Rpc(Box::new(status)));
        assert_eq!(rendered, "daemon error: invalid_request");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --locked -p gascan 2>&1 | tail -20`
Expected: FAIL on `rpc_errors_show_the_cause_when_the_daemon_sends_one` with
`assertion failed: the cause must reach the operator: daemon error: invalid_manifest`.
The other two pass already, which is the point: current behavior must not change
when there are no usable details.

- [ ] **Step 3: Render the cause**

In `crates/gascan/src/client.rs`, replace the `Rpc` arm at line 23:

```rust
            Self::Rpc(error) => {
                match gascan_proto::error_detail::decode_message(error.details()) {
                    Some(cause) => write!(formatter, "error: {cause}"),
                    None => write!(formatter, "daemon error: {}", error.message()),
                }
            }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --locked -p gascan 2>&1 | tail -20`
Expected: PASS, 0 failed.

- [ ] **Step 5: Verify the whole workspace still passes**

Run: `cargo test --locked --workspace 2>&1 | tail -5`
Expected: 0 failed. The total passing count rises above the previous 451 by the
number of tests added in Tasks 1-4.

Run: `for c in tests/release/*-contract.sh; do bash "$c" >/dev/null 2>&1 || echo "FAIL $c"; done`
Expected: no output. All 10 release contracts still pass.

- [ ] **Step 6: Verify lints and formatting across the workspace**

Run: `cargo clippy --locked --workspace --all-targets -- -D warnings && cargo fmt --check`
Expected: exit 0, no output.

- [ ] **Step 7: Commit**

```bash
git add crates/gascan/src/client.rs
git commit -m "fix: print the daemon's cause instead of its bare error code

The CLI printed the stable code and nothing else, so a user saw
'daemon error: invalid_request' for a mistyped manifest field. When the daemon
sends a cause it is now shown; without one, output is byte-for-byte what it was,
so an old daemon and a new CLI still agree."
```

---

## Manual verification

After Task 4, confirm the two reported defects are actually gone. This needs a
built CLI and a running daemon, so it is not part of the automated suite.

```bash
cargo build --locked --workspace
cd /tmp && mkdir -p gascan-manual && cd gascan-manual
cp /usr/local/share/gascan/default-gascan.toml gascan.toml
```

1. Break the manifest with `user = "kiener"`, then run the built
   `gascan up .`. Expect a message naming the file, the rejected value, and
   `workspace` or `root` — not `invalid_request`.
2. Restore `user = "workspace"` and run `gascan up .` again. Expect it to
   resolve and proceed rather than fail on the relative path.
3. Run `gascan up /definitely/not/real`. Expect exit code 64 and a message
   naming the path, with no daemon round-trip.

---

## Release note

`crates/` and `packaging/` are release inputs, so landing this means the next
release is a new version. Do not attempt to republish an existing tag; follow
`docs/release/macos-checklist.md`.
