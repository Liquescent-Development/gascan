pub mod host;

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
    /// Every check a report carries, in the order [`DoctorFacts::into_report`]
    /// emits them.
    ///
    /// Written out rather than derived, and paid for by
    /// `every_check_id_round_trips_through_a_fact`: a new variant missing from
    /// here is a check `DoctorFacts::field_mut` was never asked about, which is
    /// how a daemon's answer to it would be dropped on the way through the CLI.
    pub const ALL: [Self; 21] = [
        Self::HostArchitecture,
        Self::HostMacos,
        Self::RuntimeCli,
        Self::RuntimeVersion,
        Self::RuntimeService,
        Self::RuntimeKernel,
        Self::RuntimeSchema,
        Self::StorageState,
        Self::StorageImages,
        Self::WorkspaceAccess,
        Self::RuntimeBindMounts,
        Self::RuntimeNamedVolumes,
        Self::RuntimeTty,
        Self::RuntimeSignals,
        Self::RuntimeLoopbackPublish,
        Self::RuntimeResourceLimits,
        Self::RuntimeOffline,
        Self::SshClient,
        Self::SshIdentity,
        Self::SshConfig,
        Self::SshNativePublish,
    ];

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
    /// Every fact a daemon would have supplied, as a named failure.
    ///
    /// **Not `unavailable`, and the difference is the point.** `Unknown` reads
    /// as "not measured yet"; a daemon that could not be reached is a measured
    /// state with a cause, and `gascan doctor` must exit non-zero for it. The
    /// host half is overwritten afterwards by
    /// [`host::HostFacts::apply`](crate::doctor::host::HostFacts::apply), so
    /// what remains failing is exactly what needed a daemon.
    pub fn runtime_unreachable(detail: impl Into<String>) -> Self {
        let detail = detail.into();
        let fact = || DoctorFact::fail(detail.clone());
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

    /// The field answering `id`, so a report can be taken apart and put back
    /// together.
    ///
    /// The inverse of [`Self::into_report`]'s pairing, kept beside it and
    /// pinned to it by `every_check_id_round_trips_through_a_fact`. The CLI
    /// needs it because it rebuilds a daemon's report in order to overwrite the
    /// host half with facts it measured itself.
    pub const fn field_mut(&mut self, id: DoctorCheckId) -> &mut DoctorFact {
        match id {
            DoctorCheckId::HostArchitecture => &mut self.architecture,
            DoctorCheckId::HostMacos => &mut self.macos,
            DoctorCheckId::RuntimeCli => &mut self.cli,
            DoctorCheckId::RuntimeVersion => &mut self.version,
            DoctorCheckId::RuntimeService => &mut self.service,
            DoctorCheckId::RuntimeKernel => &mut self.kernel,
            DoctorCheckId::RuntimeSchema => &mut self.schema,
            DoctorCheckId::StorageState => &mut self.state_storage,
            DoctorCheckId::StorageImages => &mut self.image_storage,
            DoctorCheckId::WorkspaceAccess => &mut self.workspace,
            DoctorCheckId::RuntimeBindMounts => &mut self.bind_mounts,
            DoctorCheckId::RuntimeNamedVolumes => &mut self.named_volumes,
            DoctorCheckId::RuntimeTty => &mut self.tty,
            DoctorCheckId::RuntimeSignals => &mut self.signals,
            DoctorCheckId::RuntimeLoopbackPublish => &mut self.loopback_publish,
            DoctorCheckId::RuntimeResourceLimits => &mut self.resource_limits,
            DoctorCheckId::RuntimeOffline => &mut self.offline,
            DoctorCheckId::SshClient => &mut self.ssh_client,
            DoctorCheckId::SshIdentity => &mut self.ssh_identity,
            DoctorCheckId::SshConfig => &mut self.ssh_config,
            DoctorCheckId::SshNativePublish => &mut self.ssh_native_publish,
        }
    }

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

    /// Builds the report, taking the remedy prose from the backend that
    /// produced these facts.
    ///
    /// The `(id, fact)` pairing stays here because it is structural -- which
    /// field answers which check -- while the prose is the backend's. Before
    /// this the two were one table, which is why every backend got Apple's
    /// advice.
    pub fn into_report(self, remedies: &dyn DoctorRemedies) -> DoctorReport {
        let entries = [
            (DoctorCheckId::HostArchitecture, self.architecture),
            (DoctorCheckId::HostMacos, self.macos),
            (DoctorCheckId::RuntimeCli, self.cli),
            (DoctorCheckId::RuntimeVersion, self.version),
            (DoctorCheckId::RuntimeService, self.service),
            (DoctorCheckId::RuntimeKernel, self.kernel),
            (DoctorCheckId::RuntimeSchema, self.schema),
            (DoctorCheckId::StorageState, self.state_storage),
            (DoctorCheckId::StorageImages, self.image_storage),
            (DoctorCheckId::WorkspaceAccess, self.workspace),
            (DoctorCheckId::RuntimeBindMounts, self.bind_mounts),
            (DoctorCheckId::RuntimeNamedVolumes, self.named_volumes),
            (DoctorCheckId::RuntimeTty, self.tty),
            (DoctorCheckId::RuntimeSignals, self.signals),
            (DoctorCheckId::RuntimeLoopbackPublish, self.loopback_publish),
            (DoctorCheckId::RuntimeResourceLimits, self.resource_limits),
            (DoctorCheckId::RuntimeOffline, self.offline),
            (DoctorCheckId::SshClient, self.ssh_client),
            (DoctorCheckId::SshIdentity, self.ssh_identity),
            (DoctorCheckId::SshConfig, self.ssh_config),
            (DoctorCheckId::SshNativePublish, self.ssh_native_publish),
        ];
        DoctorReport {
            checks: entries
                .into_iter()
                .map(|(id, fact)| DoctorCheck {
                    id: id.as_str().to_owned(),
                    status: fact.status,
                    detail: fact.detail,
                    remedy: fact
                        .remedy
                        .unwrap_or_else(|| remedies.remedy(id).to_owned()),
                })
                .collect(),
        }
    }
}

/// The remedy prose for each check, owned by the backend that produced the fact.
///
/// **`DoctorFacts::into_report` used to pair every fact with hardcoded Apple
/// prose.** An Arca-backed daemon whose engine socket was dead told the user to
/// "install Apple container 1.1.0 in PATH" -- advice that is not merely
/// unhelpful but actively misdirecting, since installing it would change
/// nothing.
///
/// A trait with one implementation per backend, and NOT a `match` on the
/// backend inside `into_report`: the report builder should not know how many
/// backends exist, and a scattered match is how the third backend gets Apple's
/// prose in the two arms someone forgets.
///
/// Each implementation matches EXHAUSTIVELY on `DoctorCheckId`, so a new check
/// is a compile error in every backend rather than a check that silently falls
/// back to someone else's advice. That is stronger than the test that asserts
/// coverage, and it is why this returns `&'static str` rather than an `Option`.
///
/// Both implementations live here rather than in the backend crates. The prose
/// is user-facing product copy, not runtime behaviour, and keeping the sets
/// side by side is what makes "does the Arca set still mention Apple?"
/// answerable by reading one file -- and testable from `gascan-core`, which
/// cannot depend on the backend crates.
pub trait DoctorRemedies: Send + Sync {
    fn remedy(&self, id: DoctorCheckId) -> &'static str;
}

/// The remedy prose a backend owns.
///
/// The one place a `BackendSelection` becomes a `DoctorRemedies`. Both `gascand`
/// and `gascan` need the mapping now that the CLI assembles the report when no
/// daemon answers, and a second copy of it is how one of them starts handing
/// out the other backend's advice -- the exact defect `DoctorRemedies` exists
/// to close.
#[must_use]
pub fn remedies_for(backend: crate::backend::BackendSelection) -> &'static dyn DoctorRemedies {
    match backend {
        // The fake backend is a fabricated Apple runtime and its remedies are
        // Apple's, which is what `gascand`'s fake arm already assumed.
        #[cfg(debug_assertions)]
        crate::backend::BackendSelection::Fake => &AppleRemedies,
        crate::backend::BackendSelection::Apple => &AppleRemedies,
        crate::backend::BackendSelection::Arca => &ArcaRemedies,
    }
}

/// Apple's remedies, unchanged from when they were `into_report`'s hardcoded
/// table. This is the reference set and its wording is deliberately untouched.
#[derive(Clone, Copy, Debug, Default)]
pub struct AppleRemedies;

impl DoctorRemedies for AppleRemedies {
    fn remedy(&self, id: DoctorCheckId) -> &'static str {
        match id {
            DoctorCheckId::HostArchitecture => "run gascan on Apple silicon",
            DoctorCheckId::HostMacos => "upgrade this host to macOS 26 or newer",
            DoctorCheckId::RuntimeCli => "install Apple container 1.1.0 in PATH",
            DoctorCheckId::RuntimeVersion => "install the supported Apple container 1.1.0 release",
            DoctorCheckId::RuntimeService => "run `container system start` and retry",
            DoctorCheckId::RuntimeKernel => {
                "run `container system start`, install its recommended kernel, and retry"
            }
            DoctorCheckId::RuntimeSchema => {
                "install matching Apple container 1.1.0 CLI and service components"
            }
            DoctorCheckId::StorageState => {
                "free disk space in the Apple container application root"
            }
            DoctorCheckId::StorageImages => {
                "free disk space on the Apple application/state/image filesystem"
            }
            DoctorCheckId::WorkspaceAccess => {
                "grant gascan read/write access to the canonical workspace"
            }
            DoctorCheckId::RuntimeBindMounts => {
                "install a supported Apple container release with bind-mount support"
            }
            DoctorCheckId::RuntimeNamedVolumes => {
                "install a supported Apple container release with named-volume support"
            }
            DoctorCheckId::RuntimeTty => {
                "install a supported Apple container release with TTY support"
            }
            DoctorCheckId::RuntimeSignals => {
                "install a supported Apple container release with signal support"
            }
            DoctorCheckId::RuntimeLoopbackPublish => {
                "install a supported Apple container release with loopback publication support"
            }
            DoctorCheckId::RuntimeResourceLimits => {
                "install a supported Apple container release with resource-limit support"
            }
            DoctorCheckId::RuntimeOffline => {
                "install a supported Apple container release with proven offline isolation"
            }
            DoctorCheckId::SshClient => "install the system OpenSSH client at /usr/bin/ssh",
            DoctorCheckId::SshIdentity => {
                "restore the matching managed SSH identity and safe permissions; otherwise destroy and recreate affected sandboxes"
            }
            DoctorCheckId::SshConfig => {
                "remove unsafe generated SSH config state, then run `gascan up`"
            }
            DoctorCheckId::SshNativePublish => {
                "install a supported Apple container release with loopback publication support"
            }
        }
    }
}

/// The Arca engine's remedies.
///
/// **No string here names Apple's runtime**, and a test asserts it. Under this
/// backend there is no `container` CLI to install and no `container system
/// start` to run: the five runtime checks describe the engine executable, the
/// version it reports, whether its socket answers `Capabilities`, its kernel
/// artifact, and the contract minor it speaks.
///
/// The host and SSH checks keep Apple's wording because they are not about
/// either runtime -- an arm64 host and `/usr/bin/ssh` are the same requirement
/// whichever engine is driving -- and rewording them would be churn that made
/// the two sets harder to compare, not easier.
///
/// Capability remedies say the engine build does not implement the capability
/// rather than naming a release to install, because there is no released engine
/// to name: the `.pkg` carries no engine payload, so "install a supported
/// release" would be advice the user cannot act on.
#[derive(Clone, Copy, Debug, Default)]
pub struct ArcaRemedies;

impl DoctorRemedies for ArcaRemedies {
    fn remedy(&self, id: DoctorCheckId) -> &'static str {
        match id {
            DoctorCheckId::HostArchitecture => "run gascan on Apple silicon",
            DoctorCheckId::HostMacos => "upgrade this host to macOS 26 or newer",
            DoctorCheckId::RuntimeCli => {
                "set GASCAN_ENGINE_BIN to a built arca-engine; scripts/build-arca-engine.sh prints its path"
            }
            DoctorCheckId::RuntimeVersion => {
                "rebuild the engine from the revision in engine/arca-pin.json with scripts/build-arca-engine.sh"
            }
            DoctorCheckId::RuntimeService => {
                "check that GASCAN_ENGINE_SOCKET names a socket this user owns, then run `gascan daemon restart`"
            }
            DoctorCheckId::RuntimeKernel => {
                "fetch the engine artifacts recorded in engine/arca-pin.json, then run `gascan daemon restart`"
            }
            DoctorCheckId::RuntimeSchema => {
                "the engine speaks a contract this build does not; rebuild it from engine/arca-pin.json"
            }
            DoctorCheckId::StorageState => "free disk space in the engine state root",
            DoctorCheckId::StorageImages => "free disk space on the engine image filesystem",
            DoctorCheckId::WorkspaceAccess => {
                "grant gascan read/write access to the canonical workspace"
            }
            DoctorCheckId::RuntimeBindMounts => {
                "this engine build does not implement bind mounts; rebuild it from engine/arca-pin.json"
            }
            DoctorCheckId::RuntimeNamedVolumes => {
                "this engine build does not implement named volumes; rebuild it from engine/arca-pin.json"
            }
            DoctorCheckId::RuntimeTty => {
                "this engine build does not implement TTY support; rebuild it from engine/arca-pin.json"
            }
            DoctorCheckId::RuntimeSignals => {
                "this engine build does not implement signal delivery; rebuild it from engine/arca-pin.json"
            }
            DoctorCheckId::RuntimeLoopbackPublish => {
                "this engine build does not implement loopback publication; rebuild it from engine/arca-pin.json"
            }
            DoctorCheckId::RuntimeResourceLimits => {
                "this engine build does not implement resource limits; rebuild it from engine/arca-pin.json"
            }
            // Deliberately does not promise that rebuilding helps. Isolation is
            // gated on the engine's revision matching a CERTIFIED one, and an
            // uncertified build stays uncertified however often it is rebuilt.
            DoctorCheckId::RuntimeOffline => {
                "this engine build has no recorded offline-isolation proof, so offline sandboxes are refused"
            }
            DoctorCheckId::SshClient => "install the system OpenSSH client at /usr/bin/ssh",
            DoctorCheckId::SshIdentity => {
                "restore the matching managed SSH identity and safe permissions; otherwise destroy and recreate affected sandboxes"
            }
            DoctorCheckId::SshConfig => {
                "remove unsafe generated SSH config state, then run `gascan up`"
            }
            DoctorCheckId::SshNativePublish => {
                "this engine build does not implement loopback publication; rebuild it from engine/arca-pin.json"
            }
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

#[cfg(test)]
mod report_shape_tests {
    use super::{DoctorCheckId, DoctorFact, DoctorFacts, DoctorStatus};

    /// **All four hand-written tables must agree about every check.**
    ///
    /// `as_str`, `from_name`, `ALL` and `field_mut` are four transcriptions of
    /// one list of twenty-one variants, and a variant any one of them forgets
    /// is a check whose answer is silently dropped -- by the CLI, on the way
    /// from a daemon that measured it correctly.
    ///
    /// `from_name` is the one that had no test at all, and it is the CLI's
    /// ingest path: rename a check in `as_str` and forget the `from_name` arm,
    /// and every unit test still passes because `as_str` is self-consistent,
    /// while at run time the daemon's answer is thrown away and the check
    /// renders as "the daemon did not report this check".
    #[test]
    fn every_check_id_round_trips_through_a_fact() {
        // A count `ALL` cannot drift from silently. `std::mem::variant_count`
        // is unstable, so a variant added to the enum and to `field_mut` -- and
        // mapped onto an existing `DoctorFacts` field, which forces no struct
        // change -- would otherwise leave `ALL` and `into_report` both at 21
        // and this test green while the check does not exist.
        assert_eq!(
            DoctorCheckId::ALL.len(),
            21,
            "a check was added or removed; update ALL and this count together"
        );
        for id in DoctorCheckId::ALL {
            assert_eq!(
                DoctorCheckId::from_name(id.as_str()),
                Some(id),
                "{} does not survive as_str -> from_name",
                id.as_str()
            );
            let mut facts = DoctorFacts::unavailable("not collected");
            *facts.field_mut(id) = DoctorFact::pass(format!("marked {}", id.as_str()));
            let report = facts.into_report(&super::AppleRemedies);
            let check = report.check(id.as_str());
            assert!(
                check.is_some(),
                "{} is missing from the report",
                id.as_str()
            );
            let Some(check) = check else { continue };
            assert_eq!(
                check.detail,
                format!("marked {}", id.as_str()),
                "{} answers a different field than field_mut writes",
                id.as_str()
            );
            assert_eq!(check.status, DoctorStatus::Pass);
            assert_eq!(
                report.checks.len(),
                DoctorCheckId::ALL.len(),
                "the report and DoctorCheckId::ALL disagree about how many checks exist"
            );
        }
    }

    /// A daemon that could not be reached fails every check, and says why.
    #[test]
    fn an_unreachable_runtime_fails_every_check_with_its_cause() {
        let report = DoctorFacts::runtime_unreachable("engine_exited: the engine exited")
            .into_report(&super::ArcaRemedies);
        assert!(!report.is_ready());
        for check in &report.checks {
            assert_eq!(check.status, DoctorStatus::Fail, "{} passed", check.id);
            assert!(check.detail.contains("engine_exited"), "{}", check.id);
        }
    }
}
