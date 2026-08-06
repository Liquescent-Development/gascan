use crate::ssh::manager::resolution_enabled;
use crate::ssh::{
    ManagedSshDiagnostic, ManagedSshDiagnosticKind, PublishedSshSnapshot, SshManager,
    inspect_host_identity_if_present, inspect_managed_ssh_artifacts,
};
use crate::store::{ActualState, SshResolution, Store};
use crate::{ServiceError, SshPaths};
use camino::Utf8Path;
use gascan_core::doctor::{DoctorFact, DoctorStatus};
use gascan_core::sandbox::SandboxId;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SshDoctorFacts {
    pub client: DoctorFact,
    pub identity: DoctorFact,
    pub config: DoctorFact,
    pub native_publish: DoctorFact,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SshDoctorCondition {
    Ready,
    NotCreated,
    Missing,
    Inconsistent,
    Unsafe,
    TransitionPending,
    Internal,
}

impl SshDoctorCondition {
    const fn status(self) -> DoctorStatus {
        match self {
            Self::Ready | Self::NotCreated => DoctorStatus::Pass,
            Self::Missing
            | Self::Inconsistent
            | Self::Unsafe
            | Self::TransitionPending
            | Self::Internal => DoctorStatus::Fail,
        }
    }

    fn detail(self, paths: &SshPaths) -> String {
        match self {
            Self::Ready => "generated SSH config is accepted by OpenSSH".to_owned(),
            Self::NotCreated => "generated SSH config has not been published yet".to_owned(),
            Self::Missing => format!(
                "generated SSH config at {} is missing while durable or generated SSH state exists",
                paths.config()
            ),
            Self::Inconsistent => format!(
                "generated SSH config at {} differs from durable SSH state",
                paths.config()
            ),
            Self::Unsafe => {
                format!("generated SSH config at {} is unsafe", paths.config())
            }
            Self::TransitionPending => {
                "generated SSH config validation is waiting for an active lifecycle transition"
                    .to_owned()
            }
            Self::Internal => format!(
                "generated SSH config at {} could not be inspected",
                paths.config()
            ),
        }
    }

    fn remedy(self, paths: &SshPaths) -> String {
        match self {
            Self::Ready | Self::NotCreated => String::new(),
            Self::Missing | Self::Inconsistent | Self::TransitionPending => {
                "run `gascan up`".to_owned()
            }
            Self::Unsafe => format!(
                "repair or remove the unsafe managed SSH path {}",
                paths.config()
            ),
            Self::Internal => {
                "retry `gascan doctor`; if the problem persists, run `gascan up`".to_owned()
            }
        }
    }

    fn fact(self, paths: &SshPaths) -> DoctorFact {
        DoctorFact {
            status: self.status(),
            detail: self.detail(paths),
            remedy: Some(self.remedy(paths)),
        }
    }

    fn fact_with_detail(self, paths: &SshPaths, detail: impl Into<String>) -> DoctorFact {
        DoctorFact {
            status: self.status(),
            detail: detail.into(),
            remedy: Some(self.remedy(paths)),
        }
    }
}

fn identity_fact(
    condition: SshDoctorCondition,
    paths: &SshPaths,
    detail: impl Into<String>,
) -> DoctorFact {
    DoctorFact {
        status: condition.status(),
        detail: detail.into(),
        remedy: Some(condition.remedy(paths)),
    }
}

fn condition_from_diagnostic(kind: ManagedSshDiagnosticKind) -> SshDoctorCondition {
    match kind {
        ManagedSshDiagnosticKind::Missing => SshDoctorCondition::Missing,
        ManagedSshDiagnosticKind::Inconsistent => SshDoctorCondition::Inconsistent,
        ManagedSshDiagnosticKind::Unsafe => SshDoctorCondition::Unsafe,
        ManagedSshDiagnosticKind::Internal => SshDoctorCondition::Internal,
    }
}

fn diagnostic_fact<E: std::fmt::Display>(
    diagnostic: &ManagedSshDiagnostic<E>,
    subject: &str,
) -> DoctorFact {
    let condition = condition_from_diagnostic(diagnostic.kind());
    let detail = match diagnostic.kind() {
        ManagedSshDiagnosticKind::Missing => format!(
            "{subject} at {} is missing: {}",
            diagnostic.path(),
            diagnostic.source()
        ),
        ManagedSshDiagnosticKind::Inconsistent => format!(
            "{subject} at {} is inconsistent: {}",
            diagnostic.path(),
            diagnostic.source()
        ),
        ManagedSshDiagnosticKind::Unsafe => format!(
            "{subject} at {} is unsafe: {}",
            diagnostic.path(),
            diagnostic.source()
        ),
        ManagedSshDiagnosticKind::Internal => format!(
            "{subject} at {} could not be inspected: {}",
            diagnostic.path(),
            diagnostic.source()
        ),
    };
    let remedy = if diagnostic.kind() == ManagedSshDiagnosticKind::Unsafe {
        format!(
            "repair or remove the unsafe managed SSH path {}",
            diagnostic.path()
        )
    } else {
        match condition {
            SshDoctorCondition::Missing
            | SshDoctorCondition::Inconsistent
            | SshDoctorCondition::TransitionPending => "run `gascan up`".to_owned(),
            SshDoctorCondition::Internal => {
                "retry `gascan doctor`; if the problem persists, run `gascan up`".to_owned()
            }
            SshDoctorCondition::Ready
            | SshDoctorCondition::NotCreated
            | SshDoctorCondition::Unsafe => String::new(),
        }
    };
    DoctorFact {
        status: condition.status(),
        detail,
        remedy: Some(remedy),
    }
}

fn snapshot_diagnostic_fact<E: std::fmt::Display>(
    paths: &SshPaths,
    diagnostic: &ManagedSshDiagnostic<E>,
) -> DoctorFact {
    let mut fact = diagnostic_fact(diagnostic, "generated SSH config state");
    let state = match diagnostic.kind() {
        ManagedSshDiagnosticKind::Missing => "missing",
        ManagedSshDiagnosticKind::Inconsistent => "inconsistent",
        ManagedSshDiagnosticKind::Unsafe => "unsafe",
        ManagedSshDiagnosticKind::Internal => "not inspectable",
    };
    fact.detail = format!(
        "generated SSH config at {} is {state}: {}",
        paths.config(),
        diagnostic.source()
    );
    fact
}

fn generation_cleanup_fact(
    paths: &SshPaths,
    publication: &crate::ssh::manager::InspectedSshPublication,
    active: DoctorFact,
) -> DoctorFact {
    if active.status != DoctorStatus::Pass {
        return active;
    }
    if let Some(diagnostic) = publication.unsafe_generation() {
        return diagnostic_fact(diagnostic, "managed known-hosts generation");
    }
    let cleanup = publication.generation_cleanup();
    if cleanup.unsafe_entries > 0 {
        return DoctorFact::fail(format!(
            "managed known-hosts generations in {} contain unsafe entries",
            paths.directory()
        ))
        .with_remedy(format!(
            "repair or remove the unsafe managed SSH path {}",
            paths.directory()
        ));
    }
    if cleanup.stale == 0 {
        return active;
    }
    let suffix = if cleanup.stale == 1 { "" } else { "s" };
    let verb = if cleanup.stale == 1 {
        "remains"
    } else {
        "remain"
    };
    DoctorFact::warning(format!(
        "{} obsolete managed known-hosts generation{suffix} {verb} in {}",
        cleanup.stale,
        paths.directory()
    ))
}

fn ready_or_diagnostic_identity_fact<E: std::fmt::Display>(
    paths: &SshPaths,
    inspection: &Result<Option<crate::HostIdentity>, ManagedSshDiagnostic<E>>,
) -> DoctorFact {
    match inspection {
        Ok(Some(identity)) => identity_fact(
            SshDoctorCondition::Ready,
            paths,
            format!(
                "managed Ed25519 identity is valid ({})",
                identity.fingerprint()
            ),
        ),
        Ok(None) => identity_fact(
            SshDoctorCondition::Missing,
            paths,
            format!(
                "managed SSH identity at {} is missing while durable or generated SSH state exists",
                paths.private_key()
            ),
        ),
        Err(diagnostic) => diagnostic_fact(diagnostic, "managed SSH identity"),
    }
}

/// Inspect the caller-provided workspace for a single Doctor request.
pub(crate) fn workspace_fact(path: &Utf8Path) -> DoctorFact {
    let metadata = path
        .canonicalize()
        .map_err(|error| error.to_string())
        .and_then(|path| std::fs::metadata(path).map_err(|error| error.to_string()));
    match metadata {
        Ok(metadata) if metadata.is_dir() => DoctorFact::pass("workspace directory is accessible"),
        Ok(_) => DoctorFact::fail("workspace is not a directory"),
        Err(error) => DoctorFact::fail(format!("workspace is inaccessible: {error}")),
    }
}

pub async fn ssh_doctor_facts(store: &Store, native_publish: bool) -> SshDoctorFacts {
    const SSH_CLIENT: &str = "/usr/bin/ssh";
    match SshPaths::for_user() {
        Ok(paths) => {
            ssh_doctor_facts_for_paths(&paths, Path::new(SSH_CLIENT), store, native_publish).await
        }
        Err(error) => {
            let unavailable = || {
                DoctorFact::fail(format!(
                    "managed SSH state path is unavailable or unsafe: {error}"
                ))
                .with_remedy("repair or remove the unsafe managed SSH state path")
            };
            SshDoctorFacts {
                client: ssh_client_fact(Path::new(SSH_CLIENT)),
                identity: unavailable(),
                config: unavailable(),
                native_publish: native_publish_fact(native_publish),
            }
        }
    }
}

#[doc(hidden)]
pub async fn ssh_doctor_facts_for_paths(
    paths: &SshPaths,
    client: &Path,
    store: &Store,
    native_publish: bool,
) -> SshDoctorFacts {
    let client_fact = ssh_client_fact(client);
    let _publication = match SshManager.inspection_guard_for_paths(paths).await {
        Ok(publication) => publication,
        Err(error) => {
            let unavailable = || publication_inspection_failure_fact(&error);
            return SshDoctorFacts {
                client: client_fact,
                identity: unavailable(),
                config: unavailable(),
                native_publish: native_publish_fact(native_publish),
            };
        }
    };
    let durable = durable_ssh_state(store);
    let artifacts = inspect_managed_ssh_artifacts(paths);
    let identity_inspection = inspect_host_identity_if_present(paths).await;
    if let Err(diagnostic) = &identity_inspection
        && matches!(
            diagnostic.kind(),
            ManagedSshDiagnosticKind::Unsafe | ManagedSshDiagnosticKind::Internal
        )
    {
        let identity = diagnostic_fact(diagnostic, "managed SSH identity");
        let reason = if diagnostic.kind() == ManagedSshDiagnosticKind::Unsafe {
            "unsafe"
        } else {
            "not inspectable"
        };
        let config = DoctorFact {
            status: identity.status,
            detail: format!(
                "generated SSH config at {} cannot be validated because the managed identity is {reason}",
                paths.config(),
            ),
            remedy: identity.remedy.clone(),
        };
        return SshDoctorFacts {
            client: client_fact,
            identity,
            config,
            native_publish: native_publish_fact(native_publish),
        };
    }
    if let Err(diagnostic) = &artifacts
        && matches!(
            diagnostic.kind(),
            ManagedSshDiagnosticKind::Unsafe | ManagedSshDiagnosticKind::Internal
        )
    {
        let identity = diagnostic_fact(diagnostic, "managed SSH state");
        return SshDoctorFacts {
            client: client_fact,
            config: identity.clone(),
            identity,
            native_publish: native_publish_fact(native_publish),
        };
    }
    let publication_inspection = SshManager.inspect_publication_for_paths(paths);
    if let Err(diagnostic) = &publication_inspection
        && matches!(
            diagnostic.kind(),
            ManagedSshDiagnosticKind::Unsafe | ManagedSshDiagnosticKind::Internal
        )
    {
        let config = snapshot_diagnostic_fact(paths, diagnostic);
        let identity = ready_or_diagnostic_identity_fact(paths, &identity_inspection);
        return SshDoctorFacts {
            client: client_fact,
            identity,
            config,
            native_publish: native_publish_fact(native_publish),
        };
    }
    let snapshot_inspection = match (&identity_inspection, &publication_inspection) {
        (Ok(Some(identity)), Ok(publication)) if publication.config_present() => {
            Some(SshManager.snapshot_from_inspected_publication(paths, identity, publication))
        }
        _ => None,
    };
    if snapshot_inspection
        .as_ref()
        .and_then(|result| result.as_ref().err())
        .is_some_and(|diagnostic| {
            matches!(
                diagnostic.kind(),
                ManagedSshDiagnosticKind::Unsafe | ManagedSshDiagnosticKind::Internal
            )
        })
        && let Some(Err(diagnostic)) = &snapshot_inspection
    {
        let config = snapshot_diagnostic_fact(paths, diagnostic);
        let identity = ready_or_diagnostic_identity_fact(paths, &identity_inspection);
        return SshDoctorFacts {
            client: client_fact,
            identity,
            config,
            native_publish: native_publish_fact(native_publish),
        };
    }
    let identity_partial = matches!(identity_inspection, Ok(None))
        || matches!(
            identity_inspection,
            Err(ref diagnostic) if diagnostic.kind() == ManagedSshDiagnosticKind::Missing
        );
    let config_partial = matches!(
        publication_inspection,
        Ok(ref publication) if !publication.config_present()
    ) || matches!(
        publication_inspection,
        Err(ref diagnostic) if diagnostic.kind() == ManagedSshDiagnosticKind::Missing
    );
    let semantic_failure = matches!(
        identity_inspection,
        Err(ref diagnostic)
            if matches!(
                diagnostic.kind(),
                ManagedSshDiagnosticKind::Inconsistent
                    | ManagedSshDiagnosticKind::Unsafe
                    | ManagedSshDiagnosticKind::Internal
            )
    ) || matches!(
        publication_inspection,
        Err(ref diagnostic)
            if matches!(
                diagnostic.kind(),
                ManagedSshDiagnosticKind::Inconsistent
                    | ManagedSshDiagnosticKind::Unsafe
                    | ManagedSshDiagnosticKind::Internal
            )
    ) || matches!(
        snapshot_inspection,
        Some(Err(ref diagnostic))
            if matches!(
                diagnostic.kind(),
                ManagedSshDiagnosticKind::Inconsistent
                    | ManagedSshDiagnosticKind::Unsafe
                    | ManagedSshDiagnosticKind::Internal
            )
    );
    let snapshot_partial = matches!(
        snapshot_inspection,
        Some(Err(ref diagnostic))
            if diagnostic.kind() == ManagedSshDiagnosticKind::Missing
    );
    let safe_partial =
        (identity_partial || config_partial || snapshot_partial) && !semantic_failure;
    let transition_pending = durable.as_ref().is_ok_and(|durable| {
        durable.operation_pending
            && durable.consistent
            && safe_partial
            && (durable.expects_managed_state || !matches!(&artifacts, Ok(false)))
    });
    if transition_pending {
        return SshDoctorFacts {
            client: client_fact,
            identity: identity_fact(
                SshDoctorCondition::TransitionPending,
                paths,
                "managed SSH identity validation is waiting for an active lifecycle transition",
            ),
            config: SshDoctorCondition::TransitionPending.fact(paths),
            native_publish: native_publish_fact(native_publish),
        };
    }
    let requires_complete_state = match (&durable, &artifacts) {
        (Ok(durable), Ok(artifacts)) => durable.expects_managed_state || *artifacts,
        _ => true,
    };
    let (identity, config) = if !requires_complete_state {
        (
            identity_fact(
                SshDoctorCondition::NotCreated,
                paths,
                "managed SSH identity has not been created yet",
            ),
            SshDoctorCondition::NotCreated.fact(paths),
        )
    } else {
        match identity_inspection {
            Ok(Some(identity)) => {
                let identity_fact = identity_fact(
                    SshDoctorCondition::Ready,
                    paths,
                    format!(
                        "managed Ed25519 identity is valid ({})",
                        identity.fingerprint()
                    ),
                );
                let config_fact = match publication_inspection {
                    Ok(publication) if !publication.config_present() => {
                        SshDoctorCondition::Missing.fact(paths)
                    }
                    Err(diagnostic) => snapshot_diagnostic_fact(paths, &diagnostic),
                    Ok(publication) => {
                        let active = match snapshot_inspection {
                            Some(Ok(snapshot)) => {
                                validate_complete_publication(
                                    paths,
                                    client,
                                    &client_fact,
                                    durable,
                                    snapshot,
                                )
                                .await
                            }
                            Some(Err(diagnostic)) => snapshot_diagnostic_fact(paths, &diagnostic),
                            None => SshDoctorCondition::Missing.fact(paths),
                        };
                        generation_cleanup_fact(paths, &publication, active)
                    }
                };
                (identity_fact, config_fact)
            }
            Ok(None) => {
                let identity = identity_fact(
                    SshDoctorCondition::Missing,
                    paths,
                    format!(
                        "managed SSH identity at {} is missing while durable or generated SSH state exists",
                        paths.private_key()
                    ),
                );
                let config = match publication_inspection {
                    Err(diagnostic) => snapshot_diagnostic_fact(paths, &diagnostic),
                    Ok(publication) if !publication.config_present() => {
                        SshDoctorCondition::Missing.fact(paths)
                    }
                    Ok(_) => SshDoctorCondition::Inconsistent.fact_with_detail(
                        paths,
                        format!(
                            "generated SSH config at {} cannot be validated without the complete managed identity",
                            paths.config()
                        ),
                    ),
                };
                (identity, config)
            }
            Err(diagnostic) => {
                let identity = diagnostic_fact(&diagnostic, "managed SSH identity");
                let config = match publication_inspection {
                    Err(diagnostic) => snapshot_diagnostic_fact(paths, &diagnostic),
                    Ok(publication) if !publication.config_present() => {
                        SshDoctorCondition::Missing.fact(paths)
                    }
                    Ok(_) => SshDoctorCondition::Inconsistent.fact_with_detail(
                        paths,
                        format!(
                            "generated SSH config at {} cannot be validated without the complete managed identity",
                            paths.config()
                        ),
                    ),
                };
                (identity, config)
            }
        }
    };
    SshDoctorFacts {
        client: client_fact,
        identity,
        config,
        native_publish: native_publish_fact(native_publish),
    }
}

fn publication_inspection_failure_fact(error: &ServiceError) -> DoctorFact {
    DoctorFact::fail(format!(
        "managed SSH publication could not be inspected safely: {error}"
    ))
    .with_remedy("run `gascan daemon restart`; if the error persists, report an internal error")
}

struct DurableSshState {
    expected: Vec<(SandboxId, SshResolution, Option<u16>)>,
    expects_managed_state: bool,
    consistent: bool,
    operation_pending: bool,
}

fn durable_ssh_state(store: &Store) -> Result<DurableSshState, crate::StoreError> {
    let mut state = DurableSshState {
        expected: Vec::new(),
        expects_managed_state: false,
        consistent: true,
        operation_pending: false,
    };
    for snapshot in store.ssh_doctor_snapshot()? {
        let record = snapshot.record;
        let transport = snapshot.transport;
        let transport_enabled = transport.is_some_and(|policy| policy.is_enabled());
        let resolution_enabled = record
            .ssh_resolution
            .as_ref()
            .is_some_and(resolution_enabled);
        let managed = transport_enabled || resolution_enabled;
        state.expects_managed_state |= managed;
        state.operation_pending |= snapshot.operation_pending;
        if transport_enabled != resolution_enabled {
            state.consistent = false;
        }
        let publication_expected = transport_enabled
            && resolution_enabled
            && (record.actual_state == ActualState::Running
                || (record.actual_state == ActualState::Creating && snapshot.operation_pending));
        match record.actual_state {
            ActualState::Running | ActualState::Creating if publication_expected => {
                if let Some(resolution) = record.ssh_resolution {
                    state.expected.push((
                        record.id,
                        resolution,
                        transport.and_then(|policy| policy.host_port()),
                    ));
                }
            }
            ActualState::Stopped => {}
            ActualState::Absent | ActualState::Creating | ActualState::Destroying if managed => {
                state.consistent = false;
            }
            _ => {}
        }
    }
    Ok(state)
}

async fn validate_complete_publication(
    paths: &SshPaths,
    client: &Path,
    client_fact: &DoctorFact,
    durable: Result<DurableSshState, crate::StoreError>,
    snapshot: PublishedSshSnapshot,
) -> DoctorFact {
    let durable = match durable {
        Ok(durable) if durable.consistent => durable,
        Ok(_) => {
            return SshDoctorCondition::Inconsistent.fact(paths);
        }
        Err(error) => {
            return SshDoctorCondition::Inconsistent.fact_with_detail(
                paths,
                format!(
                    "generated SSH config at {} could not be compared with durable SSH state: {error}",
                    paths.config()
                ),
            );
        }
    };
    let expected = durable
        .expected
        .iter()
        .map(|(id, resolution, port)| (id, resolution, *port))
        .collect::<Vec<_>>();
    if let Err(error) = snapshot.validate_exact(&expected) {
        return SshDoctorCondition::Inconsistent.fact_with_detail(
            paths,
            format!(
                "generated SSH config at {} differs from durable SSH state: {error}",
                paths.config()
            ),
        );
    }
    if !client_fact.status.is_available() {
        return SshDoctorCondition::Inconsistent.fact_with_detail(
            paths,
            format!(
                "generated SSH config at {} cannot be validated without the system OpenSSH client",
                paths.config()
            ),
        );
    }
    validate_openssh_config(client, paths).await
}

fn native_publish_fact(native_publish: bool) -> DoctorFact {
    if native_publish {
        DoctorFact::pass("Apple runtime supports native IPv4 loopback publication")
    } else {
        DoctorFact::fail("Apple runtime does not support native IPv4 loopback publication")
            .with_remedy(
                "install a supported Apple container release with loopback publication support",
            )
    }
}

fn ssh_client_fact(client: &Path) -> DoctorFact {
    match std::fs::symlink_metadata(client) {
        Ok(metadata)
            if metadata.file_type().is_file() && metadata.permissions().mode() & 0o111 != 0 =>
        {
            DoctorFact::pass("system OpenSSH client is executable")
        }
        Ok(_) => DoctorFact::fail(format!(
            "system OpenSSH client at {} is not a regular executable",
            client.display()
        )),
        Err(error) => DoctorFact::fail(format!(
            "system OpenSSH client at {} is unavailable: {error}",
            client.display()
        )),
    }
}

async fn validate_openssh_config(client: &Path, paths: &SshPaths) -> DoctorFact {
    let config = paths.config();
    let mut command = tokio::process::Command::new(client);
    command
        .arg("-G")
        .arg("-F")
        .arg(config)
        .arg("gascan-doctor")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let output = match tokio::time::timeout(Duration::from_secs(10), command.output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => {
            return SshDoctorCondition::Inconsistent.fact_with_detail(
                paths,
                format!("generated SSH config at {config} could not be checked: {error}"),
            );
        }
        Err(_) => {
            return SshDoctorCondition::Inconsistent.fact_with_detail(
                paths,
                format!("generated SSH config at {config} exceeded its 10 second validation bound"),
            );
        }
    };
    if output.status.success() {
        SshDoctorCondition::Ready.fact(paths)
    } else {
        let mut detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        const MAX_DETAIL: usize = 4096;
        if detail.len() > MAX_DETAIL {
            let mut boundary = MAX_DETAIL;
            while !detail.is_char_boundary(boundary) {
                boundary -= 1;
            }
            detail.truncate(boundary);
            detail.push('…');
        }
        SshDoctorCondition::Inconsistent.fact_with_detail(
            paths,
            format!(
                "generated SSH config at {config} was rejected by OpenSSH{}",
                if detail.is_empty() {
                    String::new()
                } else {
                    format!(": {detail}")
                }
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ServiceError, SshError, ensure_host_identity, publish_openssh_files};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn poisoned_publication_registry_recommends_daemon_restart_not_filesystem_repair() -> TestResult
    {
        let temp = tempfile::tempdir()?;
        let home = temp.path().canonicalize()?;
        let paths = SshPaths::for_environment(None, Some(home.as_os_str()))?;
        let error = ServiceError::SshConfigUnsafe(SshError::InvalidState(
            "managed SSH publication lock registry was poisoned",
        ));

        let fact = publication_inspection_failure_fact(&error);

        assert_eq!(
            fact.remedy.as_deref(),
            Some("run `gascan daemon restart`; if the error persists, report an internal error")
        );
        assert!(
            !fact
                .remedy
                .as_deref()
                .is_some_and(|remedy| remedy.contains(paths.directory().as_str()))
        );
        Ok(())
    }

    #[tokio::test]
    async fn warning_ssh_client_remains_available_for_config_validation() -> TestResult {
        let temp = tempfile::tempdir()?;
        let home = temp.path().join("home");
        std::fs::create_dir(&home)?;
        std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700))?;
        let home = home.canonicalize()?;
        let paths = SshPaths::for_environment(None, Some(home.as_os_str()))?;
        let identity = ensure_host_identity(&paths).await?;
        publish_openssh_files(&paths, &identity, &[])?;
        let client = temp.path().join("ssh");
        std::fs::write(&client, "#!/bin/sh\nexit 0\n")?;
        std::fs::set_permissions(&client, std::fs::Permissions::from_mode(0o755))?;
        let client_fact = DoctorFact::warning("OpenSSH client is usable but untested");
        let durable = DurableSshState {
            expected: Vec::new(),
            expects_managed_state: false,
            consistent: true,
            operation_pending: false,
        };
        let snapshot = SshManager.published_snapshot_for_paths(&paths).await?;

        let fact =
            validate_complete_publication(&paths, &client, &client_fact, Ok(durable), snapshot)
                .await;

        assert_eq!(fact.status, DoctorStatus::Pass, "{}", fact.detail);
        Ok(())
    }
}
