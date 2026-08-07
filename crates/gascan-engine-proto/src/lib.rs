#![forbid(unsafe_code)]

//! Generated client for Arca's sandbox-engine contract, `arca.engine.v1`.
//!
//! The contract is defined in Arca, at `proto/arca/engine/v1/engine.proto`, and
//! reaches this crate through the signed pin in `engine/arca-pin.json` rather
//! than through a checked-in copy. Arca owns the wire protocol; Gas Can owns the
//! behavioural specification.
//!
//! This crate is generated surface and nothing else. The translation between
//! these types and Gas Can's `RuntimeBackend` types is `gascan-arca`'s, and the
//! mapping it implements is recorded in
//! `docs/superpowers/specs/2026-08-07-arca-engine-proto-design.md` §9.
//!
//! Only a client is generated. Arca serves this contract.

/// Serialised `FileDescriptorSet` for `arca.engine.v1`.
///
/// Exposed so the service surface can be asserted against the descriptor rather
/// than against the Rust types alone: a generator that emits an empty module
/// still exits 0, and a shrunken service still compiles for any caller that does
/// not use the missing method.
pub const FILE_DESCRIPTOR_SET: &[u8] =
    tonic::include_file_descriptor_set!("arca_engine_descriptor");

/// Version 1 of the sandbox-engine contract.
///
/// The major version is the package path: a breaking change is a new package,
/// never an edit to this one.
pub mod v1 {
    tonic::include_proto!("arca.engine.v1");
}
