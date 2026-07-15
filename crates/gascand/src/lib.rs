#![forbid(unsafe_code)]
#![deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

pub const TEST_FAKE_BACKEND_ENV: &str = "GASCAN_TEST_FAKE_BACKEND";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackendSelection {
    Apple,
    Fake,
}

pub const fn backend_selection(fake_requested: bool) -> BackendSelection {
    if cfg!(debug_assertions) && fake_requested {
        BackendSelection::Fake
    } else {
        BackendSelection::Apple
    }
}

mod api;
mod reconcile;
mod service;
mod socket;
mod store;

pub use api::{
    ActivityLease, ActivityTracker, ApiEventStream, Daemon, DaemonConfig, OperationLease,
    SandboxApi,
};
pub use socket::{OwnedSocket, PeerUid, PeerUidMismatch, SocketPaths, validate_peer_uid};

pub use reconcile::{ReconcileFinding, ReconcileReport};
pub use service::{
    NoopProvisioner, Operation, ProvisionRequest, ProvisionResolution, Provisioner, SandboxService,
    ServiceError, UpRequest,
};
pub use store::{
    ActualState, DesiredState, ImageResolution, OperationEvent, OperationId, OperationKind,
    OperationRecord, OperationStatus, SandboxRecord, SetupResolution, Store, StoreError,
    ToolResolution,
};
