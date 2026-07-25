use gascan_core::runtime::RuntimeResource;
use gascan_core::sandbox::SandboxId;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReconcileFinding {
    UnknownOwned(RuntimeResource),
    UnknownUnowned(RuntimeResource),
    MissingOwned(SandboxId),
    OwnershipMismatch(RuntimeResource),
    SshUnavailable {
        sandbox_id: SandboxId,
        reason: String,
    },
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ReconcileReport {
    pub findings: Vec<ReconcileFinding>,
}
