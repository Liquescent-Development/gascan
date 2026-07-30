#![forbid(unsafe_code)]
#![deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

#[cfg(debug_assertions)]
pub const TEST_FAKE_BACKEND_ENV: &str = "GASCAN_TEST_FAKE_BACKEND";
pub const TEST_ERROR_DIAGNOSTICS_ENV: &str = "GASCAN_TEST_ERROR_DIAGNOSTICS";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackendSelection {
    Apple,
    #[cfg(debug_assertions)]
    Fake,
}

#[cfg(debug_assertions)]
pub const fn backend_selection(fake_requested: bool) -> BackendSelection {
    if fake_requested {
        BackendSelection::Fake
    } else {
        BackendSelection::Apple
    }
}

#[cfg(not(debug_assertions))]
pub const fn backend_selection(_fake_requested: bool) -> BackendSelection {
    BackendSelection::Apple
}

mod api;
mod doctor;
mod reconcile;
mod service;
mod socket;
mod ssh;
mod store;

pub use api::{
    ActivityLease, ActivityTracker, ApiEventStream, Daemon, DaemonConfig, ErrorDiagnostics,
    OperationLease, SandboxApi,
};
pub use doctor::{SshDoctorFacts, ssh_doctor_facts, ssh_doctor_facts_for_paths};
pub use socket::{OwnedSocket, PeerUid, PeerUidMismatch, SocketPaths, validate_peer_uid};
pub use ssh::{
    ActiveSsh, HostIdentity, ManagedSshHost, PortReservation, PreparedSshCreate, PreparedSshFiles,
    PublishedSshSnapshot, SshConfigCommitError, SshConfigCommitFault, SshError, SshManager,
    SshPaths, SshReadinessPolicy, commit_openssh_files, ensure_host_identity,
    prepare_openssh_files, publish_openssh_files, readiness_ssh_args,
};

pub use reconcile::{ReconcileFinding, ReconcileReport};
pub use service::{
    DoctorCompleter, DoctorState, NoopProvisioner, Operation, ProvisionRequest,
    ProvisionResolution, Provisioner, SandboxService, ServiceError, StorageCapacityChange,
    UpRequest,
};
pub use store::{
    ActualState, DesiredState, ImageResolution, OperationEvent, OperationId, OperationKind,
    OperationRecord, OperationStatus, SandboxRecord, SetupResolution, SshResolution,
    StorageResolution, Store, StoreError, ToolResolution,
};
