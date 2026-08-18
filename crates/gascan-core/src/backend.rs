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
