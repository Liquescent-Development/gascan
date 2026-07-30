use crate::SshPaths;
use crate::service::ServiceError;
use crate::ssh::manager::resolution_enabled;
use crate::ssh::{
    SshError, SshManager, managed_ssh_artifacts_present, validate_host_identity_if_present,
    validate_managed_config_if_present,
};
use crate::store::{ActualState, SshResolution, Store};
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
}

impl SshDoctorCondition {
    const fn status(self) -> DoctorStatus {
        match self {
            Self::Ready | Self::NotCreated => DoctorStatus::Pass,
            Self::Missing | Self::Inconsistent | Self::Unsafe | Self::TransitionPending => {
                DoctorStatus::Fail
            }
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
    let remedy = if condition == SshDoctorCondition::Unsafe {
        format!(
            "repair or remove the unsafe managed SSH path {}",
            paths.private_key()
        )
    } else {
        condition.remedy(paths)
    };
    DoctorFact {
        status: condition.status(),
        detail: detail.into(),
        remedy: Some(remedy),
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
            let unavailable = || {
                DoctorFact::fail(format!(
                    "managed SSH publication could not be inspected safely: {error}"
                ))
                .with_remedy(format!(
                    "repair or remove the unsafe managed SSH path {}",
                    paths.directory()
                ))
            };
            return SshDoctorFacts {
                client: client_fact,
                identity: unavailable(),
                config: unavailable(),
                native_publish: native_publish_fact(native_publish),
            };
        }
    };
    let durable = durable_ssh_state(store);
    let artifacts = managed_ssh_artifacts_present(paths);
    let transition_pending = durable.as_ref().is_ok_and(|durable| {
        durable.operation_pending
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
        match validate_host_identity_if_present(paths).await {
            Ok(Some(identity)) => {
                let identity_fact = identity_fact(
                    SshDoctorCondition::Ready,
                    paths,
                    format!(
                        "managed Ed25519 identity is valid ({})",
                        identity.fingerprint()
                    ),
                );
                let config_fact =
                    validate_complete_publication(paths, client, &client_fact, durable, artifacts)
                        .await;
                (identity_fact, config_fact)
            }
            Ok(None) => (
                identity_fact(
                    SshDoctorCondition::Missing,
                    paths,
                    format!(
                        "managed SSH identity at {} is missing while durable or generated SSH state exists",
                        paths.private_key()
                    ),
                ),
                SshDoctorCondition::Inconsistent.fact_with_detail(
                    paths,
                    format!(
                        "generated SSH config at {} cannot be validated without the complete managed identity",
                        paths.config()
                    ),
                ),
            ),
            Err(error) => (
                identity_fact(
                    SshDoctorCondition::Unsafe,
                    paths,
                    format!(
                        "managed SSH identity at {} is unsafe: {error}",
                        paths.private_key()
                    ),
                ),
                SshDoctorCondition::Unsafe
                    .fact_with_detail(
                        paths,
                        format!(
                            "generated SSH config at {} cannot be validated because the managed identity is unsafe",
                            paths.config()
                        ),
                    )
                    .with_remedy(format!(
                        "repair or remove the unsafe managed SSH path {}",
                        paths.private_key()
                    )),
            ),
        }
    };
    SshDoctorFacts {
        client: client_fact,
        identity,
        config,
        native_publish: native_publish_fact(native_publish),
    }
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
        match record.actual_state {
            ActualState::Running if transport_enabled && resolution_enabled => {
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
    artifacts: Result<bool, crate::SshError>,
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
    if let Err(error) = artifacts {
        return SshDoctorCondition::Unsafe.fact_with_detail(
            paths,
            format!(
                "generated SSH config at {} is unsafe: {error}",
                paths.config()
            ),
        );
    }
    match validate_managed_config_if_present(paths) {
        Ok(false) => {
            return SshDoctorCondition::Missing.fact(paths);
        }
        Err(error) => {
            return SshDoctorCondition::Unsafe.fact_with_detail(
                paths,
                format!(
                    "generated SSH config at {} is unsafe: {error}",
                    paths.config()
                ),
            );
        }
        Ok(true) => {}
    }
    let snapshot = match SshManager.published_snapshot_for_paths(paths).await {
        Ok(snapshot) => snapshot,
        Err(error) => {
            return publication_error_condition(&error).fact_with_detail(
                paths,
                format!(
                    "generated SSH config at {} is unsafe or inconsistent: {error}",
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

fn publication_error_condition(error: &ServiceError) -> SshDoctorCondition {
    match error {
        ServiceError::SshConfigUnsafe(SshError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            SshDoctorCondition::Missing
        }
        ServiceError::SshConfigUnsafe(SshError::InvalidState(_)) => {
            SshDoctorCondition::Inconsistent
        }
        _ => SshDoctorCondition::Unsafe,
    }
}

fn native_publish_fact(native_publish: bool) -> DoctorFact {
    if native_publish {
        DoctorFact::pass("Apple runtime supports native IPv4 loopback publication")
    } else {
        DoctorFact::fail("Apple runtime does not support native IPv4 loopback publication")
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
    use crate::{ensure_host_identity, publish_openssh_files};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

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

        let fact =
            validate_complete_publication(&paths, &client, &client_fact, Ok(durable), Ok(true))
                .await;

        assert_eq!(fact.status, DoctorStatus::Pass, "{}", fact.detail);
        Ok(())
    }
}
