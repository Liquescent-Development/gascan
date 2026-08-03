#![deny(unsafe_op_in_unsafe_fn)]

use std::os::fd::{BorrowedFd, FromRawFd as _, OwnedFd};
use std::sync::atomic::{AtomicBool, Ordering};

const STARTUP_DIAGNOSTIC_FD_ENV: &str = "GASCAN_CONTROLLER_STARTUP_FD";
static CLAIMED: AtomicBool = AtomicBool::new(false);

/// Claims the launcher's exact inherited startup-diagnostic descriptor.
///
/// This must be called once at synchronous process entry, before a runtime or
/// application thread can create or own non-standard descriptors.
pub fn take_startup_diagnostic() -> std::io::Result<Option<OwnedFd>> {
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

    // SAFETY: The trusted launcher maps this exact descriptor into the exec'd
    // process. This function is called once, before the runtime is constructed,
    // so no Rust owner or application thread can exist for it. fcntl validates
    // that it is open before ownership is claimed exactly once below.
    let flags = {
        let borrowed = unsafe { BorrowedFd::borrow_raw(raw_descriptor) };
        rustix::io::fcntl_getfd(borrowed)?
    };
    // SAFETY: The inherited descriptor was validated open above and has no
    // existing owner in this freshly exec'd process.
    let owned = unsafe { OwnedFd::from_raw_fd(raw_descriptor) };
    rustix::io::fcntl_setfd(&owned, flags | rustix::io::FdFlags::CLOEXEC)?;
    Ok(Some(owned))
}
