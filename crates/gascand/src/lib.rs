#![forbid(unsafe_code)]
#![deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

#[cfg(debug_assertions)]
pub use gascan_core::backend::FAKE_BACKEND_ENV as TEST_FAKE_BACKEND_ENV;
pub const TEST_ERROR_DIAGNOSTICS_ENV: &str = "GASCAN_TEST_ERROR_DIAGNOSTICS";

// The selection moved to gascan-core when a second release backend arrived.
// `gascan` does not depend on `gascand`, and the client now has to know which
// backend it expects in order to refuse a daemon running another -- so the rule
// has to sit where both can reach it. Re-exported here because this is still
// where the daemon's callers look for it.
pub use gascan_core::backend::{
    ARCA_BACKEND_ENV, AmbiguousBackend, BackendSelection, ENGINE_BIN_ENV, ENGINE_SOCKET_ENV,
    backend_from_environment, backend_selection,
};

mod api;
mod controller_state;
mod doctor;
mod engine;
mod reconcile;
mod service;
mod socket;
mod ssh;
mod store;

pub use api::{
    ActivityLease, ActivityTracker, ApiEventStream, Daemon, DaemonConfig, ErrorDiagnostics,
    OperationLease, SandboxApi,
};
pub use controller_state::{
    ControllerStateError, ControllerStatePaths, MigrationFault, open_controller_store,
    open_controller_store_with_fault,
};
pub use doctor::{SshDoctorFacts, ssh_doctor_facts, ssh_doctor_facts_for_paths};
pub use engine::{
    EngineError, EngineLaunch, EngineReadiness, EngineSpawner, SpawnedEngine, TokioEngineSpawner,
    ensure_engine,
};
pub use socket::{OwnedSocket, PeerUid, PeerUidMismatch, SocketPaths, validate_peer_uid};
pub use ssh::{
    ActiveSsh, GenerationCleanup, HostIdentity, ManagedSshHost, PortReservation, PreparedSshCreate,
    PreparedSshFiles, PublishedSshSnapshot, SshConfigCommitError, SshConfigCommitFault, SshError,
    SshManager, SshPaths, SshReadinessOptions, SshReadinessPolicy, commit_openssh_files,
    commit_openssh_files_with_cleanup_fault, commit_openssh_files_with_fault, ensure_host_identity,
    prepare_openssh_files, prune_known_hosts_generations, publish_openssh_files,
    readiness_ssh_args,
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
