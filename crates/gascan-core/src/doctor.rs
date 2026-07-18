use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DoctorStatus {
    Pass,
    Fail,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DoctorFact {
    pub status: DoctorStatus,
    pub detail: String,
}

impl DoctorFact {
    pub fn pass(detail: impl Into<String>) -> Self {
        Self {
            status: DoctorStatus::Pass,
            detail: detail.into(),
        }
    }
    pub fn fail(detail: impl Into<String>) -> Self {
        Self {
            status: DoctorStatus::Fail,
            detail: detail.into(),
        }
    }
    pub fn unknown(detail: impl Into<String>) -> Self {
        Self {
            status: DoctorStatus::Unknown,
            detail: detail.into(),
        }
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
        }
    }

    pub fn into_report(self) -> DoctorReport {
        let entries = [
            (
                "host.architecture",
                self.architecture,
                "run gascan on Apple silicon",
            ),
            (
                "host.macos",
                self.macos,
                "upgrade this host to macOS 26 or newer",
            ),
            (
                "runtime.cli",
                self.cli,
                "install Apple container 1.1.0 in PATH",
            ),
            (
                "runtime.version",
                self.version,
                "install the supported Apple container 1.1.0 release",
            ),
            (
                "runtime.service",
                self.service,
                "run `container system start` and retry",
            ),
            (
                "runtime.kernel",
                self.kernel,
                "run `container system start`, install its recommended kernel, and retry",
            ),
            (
                "runtime.schema",
                self.schema,
                "install matching Apple container 1.1.0 CLI and service components",
            ),
            (
                "storage.state",
                self.state_storage,
                "free disk space in the Apple container application root",
            ),
            (
                "storage.images",
                self.image_storage,
                "free disk space on the Apple application/state/image filesystem",
            ),
            (
                "workspace.access",
                self.workspace,
                "grant gascan read/write access to the canonical workspace",
            ),
            (
                "runtime.bind_mounts",
                self.bind_mounts,
                "install a supported Apple container release with bind-mount support",
            ),
            (
                "runtime.named_volumes",
                self.named_volumes,
                "install a supported Apple container release with named-volume support",
            ),
            (
                "runtime.tty",
                self.tty,
                "install a supported Apple container release with TTY support",
            ),
            (
                "runtime.signals",
                self.signals,
                "install a supported Apple container release with signal support",
            ),
            (
                "runtime.loopback_publish",
                self.loopback_publish,
                "install a supported Apple container release with loopback publication support",
            ),
            (
                "runtime.resource_limits",
                self.resource_limits,
                "install a supported Apple container release with resource-limit support",
            ),
            (
                "runtime.offline",
                self.offline,
                "install a supported Apple container release with proven offline isolation",
            ),
        ];
        DoctorReport {
            checks: entries
                .into_iter()
                .map(|(id, fact, remedy)| DoctorCheck {
                    id: id.to_owned(),
                    status: fact.status,
                    detail: fact.detail,
                    remedy: remedy.to_owned(),
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
        self.checks
            .iter()
            .all(|check| check.status == DoctorStatus::Pass)
    }
}
