use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DoctorStatus {
    Pass,
    Warning,
    Fail,
    Unknown,
}

impl DoctorStatus {
    pub const fn is_available(self) -> bool {
        matches!(self, Self::Pass | Self::Warning)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DoctorCheckRole {
    ReadinessPrerequisite,
    OperationalDiagnostic,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DoctorCheckId {
    HostArchitecture,
    HostMacos,
    RuntimeCli,
    RuntimeVersion,
    RuntimeService,
    RuntimeKernel,
    RuntimeSchema,
    StorageState,
    StorageImages,
    WorkspaceAccess,
    RuntimeBindMounts,
    RuntimeNamedVolumes,
    RuntimeTty,
    RuntimeSignals,
    RuntimeLoopbackPublish,
    RuntimeResourceLimits,
    RuntimeOffline,
    SshClient,
    SshIdentity,
    SshConfig,
    SshNativePublish,
}

impl DoctorCheckId {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::HostArchitecture => "host.architecture",
            Self::HostMacos => "host.macos",
            Self::RuntimeCli => "runtime.cli",
            Self::RuntimeVersion => "runtime.version",
            Self::RuntimeService => "runtime.service",
            Self::RuntimeKernel => "runtime.kernel",
            Self::RuntimeSchema => "runtime.schema",
            Self::StorageState => "storage.state",
            Self::StorageImages => "storage.images",
            Self::WorkspaceAccess => "workspace.access",
            Self::RuntimeBindMounts => "runtime.bind_mounts",
            Self::RuntimeNamedVolumes => "runtime.named_volumes",
            Self::RuntimeTty => "runtime.tty",
            Self::RuntimeSignals => "runtime.signals",
            Self::RuntimeLoopbackPublish => "runtime.loopback_publish",
            Self::RuntimeResourceLimits => "runtime.resource_limits",
            Self::RuntimeOffline => "runtime.offline",
            Self::SshClient => "ssh.client",
            Self::SshIdentity => "ssh.identity",
            Self::SshConfig => "ssh.config",
            Self::SshNativePublish => "ssh.native_publish",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "host.architecture" => Self::HostArchitecture,
            "host.macos" => Self::HostMacos,
            "runtime.cli" => Self::RuntimeCli,
            "runtime.version" => Self::RuntimeVersion,
            "runtime.service" => Self::RuntimeService,
            "runtime.kernel" => Self::RuntimeKernel,
            "runtime.schema" => Self::RuntimeSchema,
            "storage.state" => Self::StorageState,
            "storage.images" => Self::StorageImages,
            "workspace.access" => Self::WorkspaceAccess,
            "runtime.bind_mounts" => Self::RuntimeBindMounts,
            "runtime.named_volumes" => Self::RuntimeNamedVolumes,
            "runtime.tty" => Self::RuntimeTty,
            "runtime.signals" => Self::RuntimeSignals,
            "runtime.loopback_publish" => Self::RuntimeLoopbackPublish,
            "runtime.resource_limits" => Self::RuntimeResourceLimits,
            "runtime.offline" => Self::RuntimeOffline,
            "ssh.client" => Self::SshClient,
            "ssh.identity" => Self::SshIdentity,
            "ssh.config" => Self::SshConfig,
            "ssh.native_publish" => Self::SshNativePublish,
            _ => return None,
        })
    }

    pub const fn role(self) -> DoctorCheckRole {
        match self {
            Self::WorkspaceAccess | Self::SshIdentity | Self::SshConfig => {
                DoctorCheckRole::OperationalDiagnostic
            }
            _ => DoctorCheckRole::ReadinessPrerequisite,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DoctorFact {
    pub status: DoctorStatus,
    pub detail: String,
    pub remedy: Option<String>,
}

impl DoctorFact {
    pub fn pass(detail: impl Into<String>) -> Self {
        Self {
            status: DoctorStatus::Pass,
            detail: detail.into(),
            remedy: None,
        }
    }
    pub fn warning(detail: impl Into<String>) -> Self {
        Self {
            status: DoctorStatus::Warning,
            detail: detail.into(),
            remedy: None,
        }
    }
    pub fn fail(detail: impl Into<String>) -> Self {
        Self {
            status: DoctorStatus::Fail,
            detail: detail.into(),
            remedy: None,
        }
    }
    pub fn unknown(detail: impl Into<String>) -> Self {
        Self {
            status: DoctorStatus::Unknown,
            detail: detail.into(),
            remedy: None,
        }
    }
    pub fn with_remedy(mut self, remedy: impl Into<String>) -> Self {
        self.remedy = Some(remedy.into());
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DoctorFacts {
    pub architecture: DoctorFact,
    pub macos: DoctorFact,
    pub cli: DoctorFact,
    pub version: DoctorFact,
    pub service: DoctorFact,
    pub kernel: DoctorFact,
    pub schema: DoctorFact,
    pub state_storage: DoctorFact,
    pub image_storage: DoctorFact,
    pub workspace: DoctorFact,
    pub bind_mounts: DoctorFact,
    pub named_volumes: DoctorFact,
    pub tty: DoctorFact,
    pub signals: DoctorFact,
    pub loopback_publish: DoctorFact,
    pub resource_limits: DoctorFact,
    pub offline: DoctorFact,
    pub ssh_client: DoctorFact,
    pub ssh_identity: DoctorFact,
    pub ssh_config: DoctorFact,
    pub ssh_native_publish: DoctorFact,
}

impl DoctorFacts {
    pub fn unavailable(detail: impl Into<String>) -> Self {
        let detail = detail.into();
        let fact = || DoctorFact::unknown(detail.clone());
        Self {
            architecture: fact(),
            macos: fact(),
            cli: fact(),
            version: fact(),
            service: fact(),
            kernel: fact(),
            schema: fact(),
            state_storage: fact(),
            image_storage: fact(),
            workspace: fact(),
            bind_mounts: fact(),
            named_volumes: fact(),
            tty: fact(),
            signals: fact(),
            loopback_publish: fact(),
            resource_limits: fact(),
            offline: fact(),
            ssh_client: fact(),
            ssh_identity: fact(),
            ssh_config: fact(),
            ssh_native_publish: fact(),
        }
    }
    #[cfg(debug_assertions)]
    #[doc(hidden)]
    pub fn all_supported_for_tests() -> Self {
        let pass = || DoctorFact::pass("verified test evidence");
        Self {
            architecture: pass(),
            macos: pass(),
            cli: pass(),
            version: pass(),
            service: pass(),
            kernel: pass(),
            schema: pass(),
            state_storage: pass(),
            image_storage: pass(),
            workspace: pass(),
            bind_mounts: pass(),
            named_volumes: pass(),
            tty: pass(),
            signals: pass(),
            loopback_publish: pass(),
            resource_limits: pass(),
            offline: pass(),
            ssh_client: pass(),
            ssh_identity: pass(),
            ssh_config: pass(),
            ssh_native_publish: pass(),
        }
    }

    pub fn into_report(self) -> DoctorReport {
        let entries = [
            (
                DoctorCheckId::HostArchitecture,
                self.architecture,
                "run gascan on Apple silicon",
            ),
            (
                DoctorCheckId::HostMacos,
                self.macos,
                "upgrade this host to macOS 26 or newer",
            ),
            (
                DoctorCheckId::RuntimeCli,
                self.cli,
                "install Apple container 1.1.0 in PATH",
            ),
            (
                DoctorCheckId::RuntimeVersion,
                self.version,
                "install the supported Apple container 1.1.0 release",
            ),
            (
                DoctorCheckId::RuntimeService,
                self.service,
                "run `container system start` and retry",
            ),
            (
                DoctorCheckId::RuntimeKernel,
                self.kernel,
                "run `container system start`, install its recommended kernel, and retry",
            ),
            (
                DoctorCheckId::RuntimeSchema,
                self.schema,
                "install matching Apple container 1.1.0 CLI and service components",
            ),
            (
                DoctorCheckId::StorageState,
                self.state_storage,
                "free disk space in the Apple container application root",
            ),
            (
                DoctorCheckId::StorageImages,
                self.image_storage,
                "free disk space on the Apple application/state/image filesystem",
            ),
            (
                DoctorCheckId::WorkspaceAccess,
                self.workspace,
                "grant gascan read/write access to the canonical workspace",
            ),
            (
                DoctorCheckId::RuntimeBindMounts,
                self.bind_mounts,
                "install a supported Apple container release with bind-mount support",
            ),
            (
                DoctorCheckId::RuntimeNamedVolumes,
                self.named_volumes,
                "install a supported Apple container release with named-volume support",
            ),
            (
                DoctorCheckId::RuntimeTty,
                self.tty,
                "install a supported Apple container release with TTY support",
            ),
            (
                DoctorCheckId::RuntimeSignals,
                self.signals,
                "install a supported Apple container release with signal support",
            ),
            (
                DoctorCheckId::RuntimeLoopbackPublish,
                self.loopback_publish,
                "install a supported Apple container release with loopback publication support",
            ),
            (
                DoctorCheckId::RuntimeResourceLimits,
                self.resource_limits,
                "install a supported Apple container release with resource-limit support",
            ),
            (
                DoctorCheckId::RuntimeOffline,
                self.offline,
                "install a supported Apple container release with proven offline isolation",
            ),
            (
                DoctorCheckId::SshClient,
                self.ssh_client,
                "install the system OpenSSH client at /usr/bin/ssh",
            ),
            (
                DoctorCheckId::SshIdentity,
                self.ssh_identity,
                "restore the matching managed SSH identity and safe permissions; otherwise destroy and recreate affected sandboxes",
            ),
            (
                DoctorCheckId::SshConfig,
                self.ssh_config,
                "remove unsafe generated SSH config state, then run `gascan up`",
            ),
            (
                DoctorCheckId::SshNativePublish,
                self.ssh_native_publish,
                "install a supported Apple container release with loopback publication support",
            ),
        ];
        DoctorReport {
            checks: entries
                .into_iter()
                .map(|(id, fact, default_remedy)| DoctorCheck {
                    id: id.as_str().to_owned(),
                    status: fact.status,
                    detail: fact.detail,
                    remedy: fact.remedy.unwrap_or_else(|| default_remedy.to_owned()),
                })
                .collect(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DoctorCheck {
    pub id: String,
    pub status: DoctorStatus,
    pub detail: String,
    pub remedy: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DoctorReport {
    pub checks: Vec<DoctorCheck>,
}

impl DoctorReport {
    pub fn check(&self, id: &str) -> Option<&DoctorCheck> {
        self.checks.iter().find(|check| check.id == id)
    }
    pub fn is_ready(&self) -> bool {
        self.checks.iter().all(|check| check.status.is_available())
    }

    pub fn runtime_readiness_failure(&self) -> Option<&DoctorCheck> {
        self.checks.iter().find(|check| {
            matches!(check.status, DoctorStatus::Fail | DoctorStatus::Unknown)
                && DoctorCheckId::from_name(&check.id)
                    .is_none_or(|id| id.role() == DoctorCheckRole::ReadinessPrerequisite)
        })
    }
}
