#![forbid(unsafe_code)]
#![deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

mod api;
mod reconcile;
mod service;
mod socket;
mod store;

pub use api::{
    ActivityLease, ActivityTracker, ApiEventStream, Daemon, DaemonConfig, LocalApi, OperationLease,
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
