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
//!
//! A [`StartupCode`] and not a `&str`, so that being on the list is a property
//! of the type rather than of an assertion. It began as a `debug_assert!` at
//! the writer, which is compiled out of exactly the builds users run: an
//! unlisted code then passed every provenance check on the read side and was
//! dropped by the whitelist without a trace, and the daemon's cause reached the
//! user as a readiness timeout 150 seconds later. There is no longer a way to
//! write a code that the reader will not accept.

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

/// A code the reader is guaranteed to accept.
///
/// The constructor is private, so the only values that exist are the ones
/// [`ACCEPTED_CODES`] lists. A writer cannot emit a code the reader will drop,
/// in any build profile -- which is what a `debug_assert!` could not promise.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StartupCode(&'static str);

impl StartupCode {
    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }

    /// The code a diagnostic line carries, if the reader accepts it.
    ///
    /// The read side's whitelist check. `None` is a line that is discarded.
    #[must_use]
    pub fn from_wire(code: &str) -> Option<Self> {
        ACCEPTED_CODES
            .into_iter()
            .find(|accepted| *accepted == code)
            .map(Self)
    }
}

impl std::fmt::Display for StartupCode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.0)
    }
}

/// The typed form of each code above, which is what writers hand to the channel.
pub mod code {
    use super::StartupCode;

    pub const CONTROLLER_STATE_INVALID: StartupCode = StartupCode(super::CONTROLLER_STATE_INVALID);
    pub const CONTROLLER_STATE_UNSAFE: StartupCode = StartupCode(super::CONTROLLER_STATE_UNSAFE);
    pub const CONTROLLER_STATE_CONFLICT: StartupCode =
        StartupCode(super::CONTROLLER_STATE_CONFLICT);
    pub const CONTROLLER_STATE_MIGRATION_FAILED: StartupCode =
        StartupCode(super::CONTROLLER_STATE_MIGRATION_FAILED);
    pub const ENGINE_ENVIRONMENT_INCOMPLETE: StartupCode =
        StartupCode(super::ENGINE_ENVIRONMENT_INCOMPLETE);
    pub const ENGINE_ARTIFACTS_UNAVAILABLE: StartupCode =
        StartupCode(super::ENGINE_ARTIFACTS_UNAVAILABLE);
    pub const ENGINE_SOCKET_FOREIGN: StartupCode = StartupCode(super::ENGINE_SOCKET_FOREIGN);
    pub const ENGINE_NOT_LISTENING: StartupCode = StartupCode(super::ENGINE_NOT_LISTENING);
    pub const ENGINE_EXITED: StartupCode = StartupCode(super::ENGINE_EXITED);
    pub const ENGINE_SUPERVISION_IO: StartupCode = StartupCode(super::ENGINE_SUPERVISION_IO);
    pub const ENGINE_TRANSPORT_UNAVAILABLE: StartupCode =
        StartupCode(super::ENGINE_TRANSPORT_UNAVAILABLE);
}

/// The most a diagnostic message may be, once the reader has bounded it.
///
/// The file itself is capped at 64 KiB, but that cap DISCARDS an over-long
/// diagnostic whole, which turns a named cause back into a readiness timeout.
/// This one truncates instead, so a long message still says what happened.
pub const MAX_MESSAGE_BYTES: usize = 4096;

/// A message from the channel, made safe to print.
///
/// **The whitelist bounds the code and nothing bounded the message.** It is
/// assembled from `io::Error` and `EngineError` Display output, which embeds
/// paths and OS error strings, so an environment that names a socket
/// `/tmp/x\x1b[2J...` reaches the CLI's stderr as escape sequences that clear
/// the screen and paint an attacker-chosen line where the error should be.
/// Newlines let one diagnostic render as several lines of apparent CLI output.
///
/// Sanitized at the READER, beside the whitelist, because the reader is the one
/// that does not trust the writer. C0 and C1 control characters -- including
/// `\n`, `\r` and ESC -- become a space; the result is truncated to
/// [`MAX_MESSAGE_BYTES`] on a character boundary.
#[must_use]
pub fn sanitize_message(message: &str) -> String {
    let mut sanitized = String::with_capacity(message.len().min(MAX_MESSAGE_BYTES));
    for character in message.chars() {
        if sanitized.len() >= MAX_MESSAGE_BYTES {
            break;
        }
        // C0 (including DEL) and C1, which is where ESC and every cursor
        // control lives. Anything else is ordinary text, including non-ASCII.
        if character.is_control() || ('\u{80}'..='\u{9f}').contains(&character) {
            sanitized.push(' ');
        } else {
            sanitized.push(character);
        }
    }
    sanitized
}

/// Whether the read side will surface a diagnostic carrying this code.
#[must_use]
pub fn is_accepted(code: &str) -> bool {
    StartupCode::from_wire(code).is_some()
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

#[cfg(test)]
mod sanitizer_tests {
    use super::{MAX_MESSAGE_BYTES, StartupCode, sanitize_message};

    /// **An escape sequence in a path must not reach the terminal.**
    ///
    /// `EngineError::ForeignSocket` embeds `path.display()`, so a socket path
    /// chosen by whatever set the daemon's environment is attacker-controlled
    /// text on a whitelisted code. Unsanitized it clears the screen and paints
    /// its own line where the error should be.
    #[test]
    fn control_characters_do_not_survive() {
        let hostile = "/tmp/x\u{1b}[2J\u{1b}[1;1H  All checks passed.\u{1b}[?25l/e.sock";
        let sanitized = sanitize_message(hostile);
        assert!(!sanitized.contains('\u{1b}'), "{sanitized:?}");
        assert!(sanitized.contains("/tmp/x"), "the real path was lost");
        for message in ["a\nb", "a\rb", "a\u{7f}b", "a\u{9b}b"] {
            let sanitized = sanitize_message(message);
            assert_eq!(sanitized, "a b", "{message:?}");
        }
    }

    /// Ordinary text, including non-ASCII, is left alone.
    #[test]
    fn printable_text_is_unchanged() {
        for message in ["plain", "/Users/naïve/Library/Application Support", "→ ok"] {
            assert_eq!(sanitize_message(message), message);
        }
    }

    /// **Truncated, not discarded.** The file cap drops an over-long diagnostic
    /// whole, which turns the named cause back into a readiness timeout; this
    /// bound keeps the beginning, which is where the cause is.
    #[test]
    fn an_over_long_message_is_truncated_on_a_character_boundary() {
        let long = "é".repeat(MAX_MESSAGE_BYTES);
        let sanitized = sanitize_message(&long);
        assert!(sanitized.len() <= MAX_MESSAGE_BYTES);
        assert!(sanitized.starts_with('é'));
        assert!(!sanitized.is_empty());
    }

    /// Only the listed codes exist, and `from_wire` is the only way in.
    #[test]
    fn an_unlisted_code_has_no_representation() {
        assert_eq!(
            StartupCode::from_wire(super::ENGINE_EXITED).map(StartupCode::as_str),
            Some(super::ENGINE_EXITED)
        );
        for code in ["", "engine", "controller_state", "engine_offline_proven"] {
            assert_eq!(StartupCode::from_wire(code), None, "{code} was accepted");
        }
    }

    /// Every typed constant is one the reader accepts, and they are distinct.
    #[test]
    fn every_typed_code_round_trips_through_the_wire() {
        let typed = [
            super::code::CONTROLLER_STATE_INVALID,
            super::code::CONTROLLER_STATE_UNSAFE,
            super::code::CONTROLLER_STATE_CONFLICT,
            super::code::CONTROLLER_STATE_MIGRATION_FAILED,
            super::code::ENGINE_ENVIRONMENT_INCOMPLETE,
            super::code::ENGINE_ARTIFACTS_UNAVAILABLE,
            super::code::ENGINE_SOCKET_FOREIGN,
            super::code::ENGINE_NOT_LISTENING,
            super::code::ENGINE_EXITED,
            super::code::ENGINE_SUPERVISION_IO,
            super::code::ENGINE_TRANSPORT_UNAVAILABLE,
        ];
        assert_eq!(typed.len(), super::ACCEPTED_CODES.len());
        for code in typed {
            assert_eq!(StartupCode::from_wire(code.as_str()), Some(code));
        }
    }
}
