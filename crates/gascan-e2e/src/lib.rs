#![forbid(unsafe_code)]

//! Real-process acceptance tests for the local control plane.
//!
//! The helpers here exist because this crate's tests drive child processes, and
//! a child that fails says why on its own streams. An `assert!` that names only
//! `status.success()` throws that away and reports nothing, which is how a
//! failure in these binaries has repeatedly arrived as a bare line number.
//! Every test binary in this crate defines its own `Environment`, so the
//! description has to live here to be shared rather than copied.

use std::process::{ExitStatus, Output};

/// Say whether the child exited, was signalled, or produced no status at all.
///
/// `ExitStatus`'s `Debug` form prints the raw wait status, so a plain exit code
/// 1 reads as `unix_wait_status(256)`.
#[must_use]
pub fn describe_status(status: &ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt as _;
    match (status.code(), status.signal()) {
        (Some(code), _) => format!("exit code {code}"),
        (None, Some(signal)) => format!("killed by signal {signal}"),
        (None, None) => "no exit status".to_owned(),
    }
}

/// Describe how a child ended *and* what it printed on both streams.
///
/// Both streams, always: a report written to stdout is invisible to an
/// assertion that quotes only stderr, which is exactly how
/// `doctor_human_output_names_each_check` once failed with an empty message.
#[must_use]
pub fn describe_output(output: &Output) -> String {
    format!(
        "{}, stdout={}, stderr={}",
        describe_status(&output.status),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// Require that a child succeeded, reporting its streams if it did not, and
/// hand the output back so callers can keep inspecting it.
///
/// Taking and returning the `Output` is what lets a bare
/// `assert!(thing.status.success())` become `succeeded(thing)` without the
/// caller having to name a temporary just to build a message.
#[must_use]
#[track_caller]
pub fn succeeded(output: Output) -> Output {
    assert!(output.status.success(), "{}", describe_output(&output));
    output
}

/// Require that a child succeeded when only its status was captured.
///
/// Prefer [`succeeded`]: a status alone cannot say what the child printed, so
/// this reports strictly less. It exists for the sites that never piped the
/// streams in the first place.
#[track_caller]
pub fn status_succeeded(status: &ExitStatus) {
    assert!(status.success(), "{}", describe_status(status));
}
