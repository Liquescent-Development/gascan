//! Gas Can's client for Arca's sandbox-engine contract.
//!
//! `ArcaBackend` implements `gascan_core::runtime::RuntimeBackend` over the
//! generated client in `gascan-engine-proto`, behind [`EngineTransport`] so that
//! every mapping is testable without a live engine.
//!
//! The type mapping this crate implements is recorded in
//! `docs/superpowers/specs/2026-08-07-arca-engine-proto-design.md` §9, and the
//! decisions specific to this crate in
//! `docs/superpowers/specs/2026-08-08-gascan-arca-backend-design.md`.

mod translate;
mod transport;

pub use transport::{EngineTransport, ExecStream, LogsStream, TransportError};
