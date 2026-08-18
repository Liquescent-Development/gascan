//! The codes a daemon may report through the startup diagnostic channel.
//!
//! The CLI hands a daemon it spawns an already-unlinked file descriptor, and a
//! daemon that fails before it can serve writes one line into it so the CLI can
//! say *why* instead of reporting a readiness timeout. The read side validates
//! that file on owner uid, mode, `nlink == 0`, a size bound, an owner-token
//! match, **and a closed whitelist of codes**.
//!
//! That whitelist lives here, in a crate `gascan` and `gascand` both depend on
//! while neither depends on the other, for the reason [`crate::backend`] gives:
//! two processes that must agree cannot each keep their own copy. Before this
//! module the four controller codes were spelled twice -- once in
//! `ControllerStateError::code()` and once in a `matches!` in the reader -- and
//! a code added to the writer but not the reader is a diagnostic that is
//! written, validated, and then silently discarded.
//!
//! **The whitelist being closed is the feature.** It bounds what a process
//! writing to that descriptor can make the CLI print, so widening it is a
//! deliberate act: a new code belongs here, with a reason, or nowhere.

/// A controller state path that is not absolute and normal.
pub const CONTROLLER_STATE_INVALID: &str = "controller_state_invalid";
/// A controller state path whose ownership, type or mode is not this user's.
pub const CONTROLLER_STATE_UNSAFE: &str = "controller_state_unsafe";
/// Two active controller databases that differ, which Gas Can will not merge.
pub const CONTROLLER_STATE_CONFLICT: &str = "controller_state_conflict";
/// The legacy-to-durable migration failed or the store would not open.
pub const CONTROLLER_STATE_MIGRATION_FAILED: &str = "controller_state_migration_failed";

/// A variable the Arca backend requires was not set.
///
/// The engine's executable, socket and state root are all undefaulted, so their
/// absence is this error naming the variable rather than a guessed path. Before
/// this code existed that message went to a `Stdio::null()` stderr and the user
/// saw a generic readiness timeout.
pub const ENGINE_ENVIRONMENT_INCOMPLETE: &str = "engine_environment_incomplete";
/// The engine's fetched boot artifacts could not be located.
pub const ENGINE_ARTIFACTS_UNAVAILABLE: &str = "engine_artifacts_unavailable";
/// The engine socket exists but belongs to another user.
pub const ENGINE_SOCKET_FOREIGN: &str = "engine_socket_foreign";
/// The engine was spawned and never bound its socket.
pub const ENGINE_NOT_LISTENING: &str = "engine_not_listening";
/// The engine exited before it listened, carrying its exit status.
pub const ENGINE_EXITED: &str = "engine_exited";
/// Supervising the engine failed on I/O -- it could not be spawned, or its
/// socket could not be inspected.
pub const ENGINE_SUPERVISION_IO: &str = "engine_supervision_io";
/// The engine bound its socket but the daemon could not open a channel on it.
pub const ENGINE_TRANSPORT_UNAVAILABLE: &str = "engine_transport_unavailable";

/// Every code the read side accepts. A line carrying anything else is dropped.
pub const ACCEPTED_CODES: [&str; 11] = [
    CONTROLLER_STATE_INVALID,
    CONTROLLER_STATE_UNSAFE,
    CONTROLLER_STATE_CONFLICT,
    CONTROLLER_STATE_MIGRATION_FAILED,
    ENGINE_ENVIRONMENT_INCOMPLETE,
    ENGINE_ARTIFACTS_UNAVAILABLE,
    ENGINE_SOCKET_FOREIGN,
    ENGINE_NOT_LISTENING,
    ENGINE_EXITED,
    ENGINE_SUPERVISION_IO,
    ENGINE_TRANSPORT_UNAVAILABLE,
];

/// Whether the read side will surface a diagnostic carrying this code.
#[must_use]
pub fn is_accepted(code: &str) -> bool {
    ACCEPTED_CODES.contains(&code)
}

#[cfg(test)]
mod tests {
    use super::{ACCEPTED_CODES, is_accepted};

    /// A duplicate in the table is a code that was renamed in one place and
    /// left behind in another, and the array length would still typecheck.
    #[test]
    fn the_accepted_codes_are_distinct() {
        let mut sorted = ACCEPTED_CODES;
        sorted.sort_unstable();
        for pair in sorted.windows(2) {
            assert_ne!(pair[0], pair[1], "duplicate startup diagnostic code");
        }
    }

    /// The whitelist is closed, and a test that only checked membership would
    /// pass just as well if it accepted everything.
    #[test]
    fn an_unlisted_code_is_not_accepted() {
        assert!(is_accepted(ACCEPTED_CODES[0]));
        for code in ["", "engine", "controller_state", "engine_offline_proven"] {
            assert!(!is_accepted(code), "{code} must not be accepted");
        }
    }
}
