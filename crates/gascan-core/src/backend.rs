//! Which runtime backend a daemon runs, and which one a client expects.
//!
//! This lives in `gascan-core` rather than in `gascand` because two crates that
//! do not depend on each other have to agree on it. `gascand` constructs the
//! backend it was asked for; `gascan` records that choice in the daemon instance
//! record and refuses to talk to a daemon running a different one. If each
//! derived its own answer from the environment, the two could disagree without
//! anything noticing -- which is precisely the failure the instance record's
//! backend field exists to make impossible.

/// The backends a daemon can run.
///
/// `Apple` and `Arca` are both release variants. `Fake` is not: it is
/// `#[cfg(debug_assertions)]` so that a release binary cannot be talked into a
/// runtime that fabricates its answers, and `Arca` must not inherit that
/// treatment -- it is a real engine and a shipped configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackendSelection {
    Apple,
    Arca,
    #[cfg(debug_assertions)]
    Fake,
}

/// Both backend environment variables were set at once.
///
/// A precedence rule was considered and rejected. Whichever order it chose, a
/// user who set both would get a daemon on one backend while believing they had
/// asked for the other, and the instance record would faithfully agree with the
/// daemon -- so the mismatch check downstream could never catch it. Refusing is
/// the only answer that cannot be silently wrong.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AmbiguousBackend;

impl std::fmt::Display for AmbiguousBackend {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(
            "more than one backend was requested; set at most one of \
             GASCAN_ARCA_BACKEND or GASCAN_TEST_FAKE_BACKEND",
        )
    }
}

impl std::error::Error for AmbiguousBackend {}

/// The environment variable that selects the Arca engine backend.
///
/// Named here beside `BackendSelection` and not in `gascand`, for the reason
/// this module exists: the client reads it to know what it expects and the
/// daemon reads it to know what to build, and a second spelling in either place
/// would be a selection the two disagree about.
pub const ARCA_BACKEND_ENV: &str = "GASCAN_ARCA_BACKEND";

/// The engine socket the Arca backend dials.
///
/// Undefaulted deliberately. Its absence under `GASCAN_ARCA_BACKEND` is a
/// startup error naming the variable, not a guessed path: a default would make
/// a typo in the socket path indistinguishable from not having set it, and the
/// engine's state root is Arca's to choose rather than Gas Can's to assume.
pub const ENGINE_SOCKET_ENV: &str = "GASCAN_ENGINE_SOCKET";

/// The state root the engine is given when this daemon spawns it.
///
/// Undefaulted for the reason [`ENGINE_SOCKET_ENV`] states and one more that is
/// specific to it. The supervisor dials before it spawns, so a daemon routinely
/// ADOPTS an engine some earlier process started and pointed at a state root of
/// that process's choosing. A guessed default would be right only when this
/// daemon happened to be the one that spawned, and on every other start it would
/// name a second, empty store that nothing reconciles against the first --
/// sandboxes that exist reported as missing, with no error anywhere.
///
/// Distinct from the kernel and the vminit layout, which are NOT environment
/// variables: those are Gas Can's own installation, written and verified by
/// `gascan engine fetch` at paths `ArtifactPaths` owns, and read from there by
/// both the spawn and the doctor fact so the two cannot describe different
/// files.
pub const ENGINE_STATE_ROOT_ENV: &str = "GASCAN_ENGINE_STATE_ROOT";

/// The engine executable the supervisor spawns when nothing is listening.
///
/// Undefaulted for the same reason as the socket, and for one more: the `.pkg`
/// carries no engine payload -- `packaging/macos/package.sh` states that the
/// engine is a build gate and not a payload -- so there is no installed path to
/// default to. Guessing one would produce "no such file" from a path the user
/// never chose.
pub const ENGINE_BIN_ENV: &str = "GASCAN_ENGINE_BIN";

/// How long a daemon waits for an engine it spawned to bind its socket.
///
/// **Measured, not chosen.** `crates/gascan-arca/tests/live/common/mod.rs`'s
/// `await_socket` records that "a binary's first execution is far slower than
/// its later ones: a freshly built `arca-engine` measured 997ms on a fresh
/// inode against 10ms warm, and freshly linked test binaries on the same
/// machine took ~50s each to start under load. **30s failed on a cold
/// engine.**" That tier settled on 120s. The daemon's own bound was 20s, which
/// is under a start this repository had already measured as failing at 30s, so
/// a correctly configured host with a cold engine failed `gascan up`.
///
/// A bound, not a retry policy: it exists so a hung engine surfaces as an error
/// naming the socket rather than as a daemon that never finishes starting. The
/// child is checked for having exited on every tick, so a dead engine ends the
/// wait long before this does -- which is what keeps widening it cheap.
pub const ENGINE_READINESS: std::time::Duration = std::time::Duration::from_secs(120);

/// How long a client waits for a daemon that must bring an engine up first.
///
/// **It MUST exceed [`ENGINE_READINESS`], and `readiness_bounds_are_ordered`
/// asserts it.** The client's bound was 15s against the daemon's 20s, so the
/// client always abandoned first and `EngineError::NotListening` -- the daemon's
/// own error, the one that names the socket it waited on -- could not reach a
/// user by construction. What a user saw instead was a generic
/// `SupervisorError::Readiness`.
///
/// Both constants live here, in a crate `gascan` and `gascand` both depend on
/// while neither depends on the other, for the reason [`ARCA_BACKEND_ENV`]
/// gives: two processes that must agree cannot each keep their own copy.
///
/// **This is not the bound for an Apple-backed daemon**, which starts no engine
/// and whose 15s default stays as it is. Applying this to every backend would
/// make an Apple daemon that never becomes healthy take two and a half minutes
/// to say so.
pub const ENGINE_BACKED_DAEMON_READINESS: std::time::Duration = std::time::Duration::from_secs(150);

/// The environment variable that selects the fabricating test runtime.
///
/// `#[cfg(debug_assertions)]` here as well as at every read of it, so that the
/// name does not even exist in a release build.
#[cfg(debug_assertions)]
pub const FAKE_BACKEND_ENV: &str = "GASCAN_TEST_FAKE_BACKEND";

impl BackendSelection {
    /// The stable name recorded in the daemon instance record.
    ///
    /// Spelled out rather than derived from `Debug`, because this string is
    /// persisted and compared across processes: a `Debug` rename would silently
    /// change what an already-written record means.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Apple => "apple",
            Self::Arca => "arca",
            #[cfg(debug_assertions)]
            Self::Fake => "fake",
        }
    }
}

/// Resolves the backend from what was requested.
///
/// Takes booleans rather than reading the environment itself so that the
/// decision is testable without mutating process state -- `std::env::set_var` is
/// unsafe and global, and a selection rule tested through it is a rule tested
/// once per process.
///
/// `fake_requested` is forced false in release builds by the caller, which is
/// where the `#[cfg]` for the environment variable already lives.
pub fn backend_selection(
    fake_requested: bool,
    arca_requested: bool,
) -> Result<BackendSelection, AmbiguousBackend> {
    match (fake_requested, arca_requested) {
        (true, true) => Err(AmbiguousBackend),
        #[cfg(debug_assertions)]
        (true, false) => Ok(BackendSelection::Fake),
        #[cfg(not(debug_assertions))]
        (true, false) => Ok(BackendSelection::Apple),
        (false, true) => Ok(BackendSelection::Arca),
        (false, false) => Ok(BackendSelection::Apple),
    }
}

/// Resolves the backend from this process's environment.
///
/// **The one place the environment is read.** The daemon calls it to decide
/// which backend to construct and the client calls it to decide which daemon it
/// is willing to talk to; if either grew its own copy of this, the two could
/// disagree about what was asked for while both being internally consistent --
/// and the daemon's instance record would faithfully record the daemon's
/// answer, so the mismatch check could never see it.
///
/// `GASCAN_TEST_FAKE_BACKEND` is not consulted at all in a release build, which
/// is what keeps `Fake` unreachable there rather than merely unselected.
pub fn backend_from_environment() -> Result<BackendSelection, AmbiguousBackend> {
    #[cfg(debug_assertions)]
    let fake_requested = std::env::var_os(FAKE_BACKEND_ENV).is_some();
    #[cfg(not(debug_assertions))]
    let fake_requested = false;
    backend_selection(fake_requested, std::env::var_os(ARCA_BACKEND_ENV).is_some())
}

#[cfg(test)]
mod readiness_tests {
    use super::{ENGINE_BACKED_DAEMON_READINESS, ENGINE_READINESS};

    /// **The client must outlast the daemon it is waiting on.**
    ///
    /// If it does not, the daemon's own error is produced for a client that has
    /// already gone, and every engine startup failure reaches the user as a
    /// generic readiness timeout instead of one naming the socket. That was the
    /// state of the two bounds when this test was written: the client's 15s
    /// against the daemon's 20s.
    ///
    /// The margin is asserted, not just the ordering. Equal bounds race, and a
    /// margin under the poll interval is the same as none.
    #[test]
    fn readiness_bounds_are_ordered() {
        assert!(
            ENGINE_BACKED_DAEMON_READINESS > ENGINE_READINESS,
            "the client ({ENGINE_BACKED_DAEMON_READINESS:?}) must outlast the daemon's \
             engine wait ({ENGINE_READINESS:?}), or the daemon's specific error is produced \
             for a client that has already abandoned it"
        );
        assert!(
            ENGINE_BACKED_DAEMON_READINESS - ENGINE_READINESS >= std::time::Duration::from_secs(10),
            "the margin between the two bounds is under 10s, which a slow tick can close"
        );
    }
}
