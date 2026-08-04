#![deny(unsafe_code, unsafe_op_in_unsafe_fn)]

use std::os::fd::{FromRawFd as _, OwnedFd};
use std::sync::atomic::{AtomicBool, Ordering};

const STARTUP_DIAGNOSTIC_FD_ENV: &str = "GASCAN_CONTROLLER_STARTUP_FD";
static CLAIMED: AtomicBool = AtomicBool::new(false);

/// Claims the launcher's exact inherited startup-diagnostic descriptor.
///
/// This must be called once at synchronous process entry, before a runtime or
/// application thread can create or own non-standard descriptors.
///
/// Calling the ownership-transfer API without acknowledging its I/O-safety
/// preconditions must not compile:
///
/// ```compile_fail
/// let _ = gascan_inherited_fd::take_startup_diagnostic();
/// ```
///
/// # Safety
///
/// Call this only once at synchronous process entry, before starting threads or
/// constructing libraries that can open, close, or own non-standard file
/// descriptors. If the environment value names an open descriptor, it must be
/// a uniquely owned descriptor inherited across `exec`; no Rust I/O type may
/// already own it, and nothing may concurrently close or reassign it. Invalid
/// and stale descriptor numbers are permitted and return `EBADF` before an fd
/// wrapper is constructed. Calls after the first are rejected by the ownership
/// guard before the descriptor is inspected.
#[allow(unsafe_code)]
pub unsafe fn take_startup_diagnostic() -> std::io::Result<Option<OwnedFd>> {
    let Some(raw_descriptor) = std::env::var_os(STARTUP_DIAGNOSTIC_FD_ENV) else {
        return Ok(None);
    };
    if CLAIMED.swap(true, Ordering::AcqRel) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "controller startup diagnostic descriptor was already claimed",
        ));
    }
    let raw_descriptor = raw_descriptor
        .into_string()
        .map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "controller startup diagnostic descriptor must be valid UTF-8",
            )
        })?
        .parse::<i32>()
        .map_err(|error| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("controller startup diagnostic descriptor must be an integer: {error}"),
            )
        })?;
    if raw_descriptor < 3 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "controller startup diagnostic descriptor must not be standard I/O",
        ));
    }

    validate_raw_descriptor(raw_descriptor)?;
    // SAFETY: The caller guarantees exclusive ownership of any valid inherited
    // descriptor, and validate_raw_descriptor returned before this point for
    // EBADF. The one-time guard prevents a second ownership transfer.
    let owned = unsafe { OwnedFd::from_raw_fd(raw_descriptor) };
    let flags = rustix::io::fcntl_getfd(&owned)?;
    rustix::io::fcntl_setfd(&owned, flags | rustix::io::FdFlags::CLOEXEC)?;
    Ok(Some(owned))
}

#[allow(unsafe_code)]
fn validate_raw_descriptor(raw_descriptor: i32) -> std::io::Result<()> {
    // SAFETY: POSIX fcntl(F_GETFD) accepts an arbitrary integer descriptor and
    // reports EBADF for an invalid/stale value; it does not dereference memory
    // or assume Rust I/O-safety invariants.
    if unsafe { libc::fcntl(raw_descriptor, libc::F_GETFD) } == -1 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{STARTUP_DIAGNOSTIC_FD_ENV, take_startup_diagnostic};
    use std::os::fd::{AsRawFd as _, IntoRawFd as _};

    type TestResult = Result<(), Box<dyn std::error::Error>>;
    const CHILD_MODE: &str = "GASCAN_INHERITED_FD_TEST_CHILD";

    fn run_isolated(test_name: &str, mode: &str) -> TestResult {
        if std::env::var(CHILD_MODE).as_deref() == Ok(mode) {
            return Ok(());
        }
        let status = std::process::Command::new(std::env::current_exe()?)
            .args(["--exact", test_name, "--nocapture"])
            .env(CHILD_MODE, mode)
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("isolated inherited-fd test failed: {status}").into())
        }
    }

    #[test]
    #[allow(unsafe_code)]
    fn stale_descriptor_returns_ebadf_before_fd_wrapper_construction() -> TestResult {
        const MODE: &str = "stale";
        if std::env::var(CHILD_MODE).as_deref() != Ok(MODE) {
            return run_isolated(
                "tests::stale_descriptor_returns_ebadf_before_fd_wrapper_construction",
                MODE,
            );
        }
        // SAFETY: The isolated test process has no application threads.
        unsafe {
            std::env::set_var(STARTUP_DIAGNOSTIC_FD_ENV, i32::MAX.to_string());
        }
        // SAFETY: The isolated process is single-threaded at this boundary; a
        // stale descriptor is explicitly accepted by the API contract.
        let error = unsafe { take_startup_diagnostic() }
            .err()
            .ok_or("stale descriptor was accepted")?;
        assert_eq!(
            error.raw_os_error(),
            Some(rustix::io::Errno::BADF.raw_os_error())
        );
        Ok(())
    }

    #[test]
    #[allow(unsafe_code)]
    fn exact_descriptor_is_owned_once_and_repeat_is_rejected() -> TestResult {
        const MODE: &str = "repeat";
        if std::env::var(CHILD_MODE).as_deref() != Ok(MODE) {
            return run_isolated(
                "tests::exact_descriptor_is_owned_once_and_repeat_is_rejected",
                MODE,
            );
        }
        let raw_descriptor = std::fs::File::open("/dev/null")?.into_raw_fd();
        // SAFETY: The isolated test process has no application threads.
        unsafe {
            std::env::set_var(STARTUP_DIAGNOSTIC_FD_ENV, raw_descriptor.to_string());
        }
        // SAFETY: into_raw_fd transferred the descriptor without an owner, and
        // this isolated process has no application threads.
        let owned =
            unsafe { take_startup_diagnostic() }?.ok_or("valid descriptor was not claimed")?;
        assert_eq!(owned.as_raw_fd(), raw_descriptor);
        // SAFETY: Repeat calls are rejected by CLAIMED before descriptor access.
        let repeat = unsafe { take_startup_diagnostic() }
            .err()
            .ok_or("descriptor ownership was transferred twice")?;
        assert_eq!(repeat.kind(), std::io::ErrorKind::AlreadyExists);
        drop(owned);
        Ok(())
    }
}
