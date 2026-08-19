//! The on-disk protocol of the daemon's runtime directory, shared by the
//! process that writes it and the process that reads it.
//!
//! `gascand` creates these paths and `gascan` classifies them, from different
//! processes in different crates, and every value here has to be the same on
//! both sides or the classification is wrong rather than merely different.
//! Each one used to be declared twice, once in `crates/gascand/src/socket.rs`
//! and once in `crates/gascan/src/daemon.rs`. `0o600` was not even spelled the
//! same way in the two -- `SOCKET_MODE` against `FILE_MODE` -- so a grep for
//! either name found one copy and reported it as the only one.
//!
//! Nothing here is a default that a caller may override, and one edit here is
//! now the only way to change any of it: renaming or removing any of the six
//! stops both `gascan` and `gascand` from compiling. MEASURED on 2026-08-18 by
//! renaming each of the six in turn and running `cargo check -p gascan
//! --all-targets` and `cargo check -p gascand --all-targets`: twelve of twelve
//! failed to compile.
//!
//! Changing a value, by contrast, is *not* a compile error -- both crates
//! simply agree on the new one, which is the point, and is also why neither
//! crate's suite reliably notices. `tests/daemon_protocol.rs` is where that is
//! caught; it records the measurement.
//!
//! # The three faces of the instance record
//!
//! The instance record ([`INSTANCE_NAME`]) is the strongest coupling in this
//! protocol and the one least visible from either side alone: `gascand`
//! asserts it in its own tests and `gascan` consumes it in a classifier, with
//! nothing between them. The rule is that its path shows a reader exactly
//! three faces:
//!
//! | face | mode | size | meaning |
//! |---|---|---|---|
//! | absent | -- | -- | no daemon has published here |
//! | inert | [`INSTANCE_TOMBSTONE_MODE`] | `0` | staged, or retired |
//! | published | [`PRIVATE_FILE_MODE`] | non-empty | a live daemon's record |
//!
//! `(INSTANCE_TOMBSTONE_MODE, non-empty)` is the illegal fourth: a record
//! written but never published, which `gascan`'s `validate_file_stat` turns
//! into a terminal `DaemonState::Unsafe`. `gascand` keeps it off the path by
//! building every next state under a staging name (see
//! `INSTANCE_STAGING_PURPOSE` in `crates/gascand/src/socket.rs`, which is
//! that crate's alone) and renaming it into place.
//!
//! **One in-tree producer still violates the rule and is not fixed.**
//! `retire_held_record` in `crates/gascan/src/daemon.rs` `fchmod`s a
//! *published* record to [`INSTANCE_TOMBSTONE_MODE`] and only then truncates
//! it, in place at the destination, on the path where a previous daemon was
//! `SIGKILL`ed and the next `gascan start` reclaims the record. Fixing it is a
//! change to the reclaim protocol rather than a mechanical one, because
//! `validate_retired_tombstone` requires the held descriptor's inode to still
//! be at the name, and a rename unlinks it.
//!
//! # A stat is about the path only when `st_nlink == 1`
//!
//! `lstat` is not atomic across resolving a name and reading the inode's
//! attributes, so an observer can resolve a name to a file an instant before
//! that file is renamed away and still read its attributes afterwards. Such a
//! read describes an inode that is no longer at the name. Every classifier
//! here therefore requires `st_nlink == 1` before it treats a mode-and-size
//! pair as a state of the path at all, and every writer that both truncates
//! and chmods an outgoing record truncates first, so that a torn read is
//! `(0600, 0)` or `(0200, 0)` -- neither of which is the illegal fourth face.

/// Mode of the daemon's runtime directory: owner-only, `0700`.
///
/// Enforced on creation and re-asserted on every traversal, because `mkdirat`'s
/// mode argument is masked by the umask. Also stated in the published API as
/// `gascan_proto::SOCKET_DIRECTORY_MODE`, and bound to it by a `const` assertion
/// in `crates/gascand/src/socket.rs`, so the two cannot drift apart.
pub const DIRECTORY_MODE: u16 = 0o700;

/// Mode of every published object in that directory: owner-only, `0600`.
///
/// It covers the socket, the published instance record, the lifecycle lock and
/// the startup diagnostic. It is named for the permission rather than for any
/// one of them, because it was named for one of them in each crate --
/// `SOCKET_MODE` in `gascand`, `FILE_MODE` in `gascan` -- and neither name was
/// true of all four, which is why neither grep found the other. Also stated in
/// the published API as `gascan_proto::SOCKET_MODE`, and bound to it by a
/// `const` assertion in `crates/gascand/src/socket.rs`.
pub const PRIVATE_FILE_MODE: u16 = 0o600;

/// Mode of the instance record while it is inert: write-only, `0200`, and
/// empty.
///
/// Both the staged record before publication and the tombstone left after
/// retirement wear it. A reader that finds this mode with a non-empty file has
/// found the illegal fourth face described in the module documentation.
pub const INSTANCE_TOMBSTONE_MODE: u16 = 0o200;

/// The daemon's listening socket, relative to the runtime directory.
pub const SOCKET_NAME: &str = "gascand.sock";

/// The daemon's instance record, relative to the runtime directory.
pub const INSTANCE_NAME: &str = "daemon-instance.json";

/// The lifecycle lock serialising start and stop, relative to the runtime
/// directory.
pub const LIFECYCLE_LOCK_NAME: &str = "daemon-lifecycle.lock";
