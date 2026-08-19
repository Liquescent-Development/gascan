//! The shared daemon file protocol is pinned to its literal values here.
//!
//! Moving the six values into one crate makes both halves of the daemon
//! lifecycle agree with each other, but agreement is not the whole contract:
//! these are values a `gascan` from one release reads off a directory a
//! `gascand` from another release wrote, and neither crate's own suite can
//! notice a change that both halves make together. MEASURED on 2026-08-18
//! while making that move: with `INSTANCE_TOMBSTONE_MODE` changed from `0o200`
//! to `0o220`, `cargo test -p gascand --lib` reported `111 passed; 0 failed`,
//! because every assertion in it compares against the constant rather than
//! against `0o200`. `cargo test -p gascan --lib` failed 11 tests only because
//! its fixtures happen to spell the mode out.
//!
//! So each value is restated once here, deliberately, as the second place an
//! edit has to reach. This is the same guard
//! `crates/gascan-proto/tests/api_compatibility.rs` puts on the two of these
//! values that are also published API.
//!
//! A failure here is not a bug in this test. It means someone changed the
//! on-disk protocol, and the question to answer before updating the literal is
//! what happens when the new writer meets the old reader.

use gascan_core::daemon_protocol::{
    DIRECTORY_MODE, INSTANCE_NAME, INSTANCE_STAGING_PURPOSE, INSTANCE_TOMBSTONE_MODE,
    LIFECYCLE_LOCK_NAME, PRIVATE_FILE_MODE, RECLAIM_STAGING_PURPOSE, SOCKET_NAME,
};

#[test]
fn the_runtime_directory_is_owner_only() {
    assert_eq!(DIRECTORY_MODE, 0o700);
}

#[test]
fn every_published_object_is_owner_only() {
    assert_eq!(PRIVATE_FILE_MODE, 0o600);
}

#[test]
fn the_inert_instance_record_is_write_only() {
    assert_eq!(INSTANCE_TOMBSTONE_MODE, 0o200);
}

/// The inert face and the published face have to be distinguishable by mode
/// alone, because that is the only thing a reader holding a `stat` has to
/// separate them by before it looks at size.
#[test]
fn the_two_faces_of_the_instance_record_differ_by_mode() {
    assert_ne!(INSTANCE_TOMBSTONE_MODE, PRIVATE_FILE_MODE);
}

#[test]
fn the_protocol_file_names_are_stable() {
    assert_eq!(SOCKET_NAME, "gascand.sock");
    assert_eq!(INSTANCE_NAME, "daemon-instance.json");
    assert_eq!(LIFECYCLE_LOCK_NAME, "daemon-lifecycle.lock");
}

/// Two processes now stage files in the daemon's runtime directory, so the
/// prefixes are protocol rather than private detail: `gascand`'s sweeper
/// matches both, and a prefix that changed on one side alone would either
/// orphan files forever or sweep a live one.
#[test]
fn the_staging_prefixes_are_stable_and_distinct() {
    assert_eq!(INSTANCE_STAGING_PURPOSE, "instance");
    assert_eq!(RECLAIM_STAGING_PURPOSE, "reclaim");
    assert_ne!(INSTANCE_STAGING_PURPOSE, RECLAIM_STAGING_PURPOSE);
}
