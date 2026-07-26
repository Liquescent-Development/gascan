use crate::SshPaths;
use crate::ssh::manager::resolution_enabled;
use crate::ssh::{
    SshManager, managed_ssh_artifacts_present, validate_host_identity_if_present,
    validate_managed_config_if_present,
};
use crate::store::{ActualState, SshResolution, Store};
use gascan_core::doctor::DoctorFact;
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
            identity: DoctorFact::unknown(
                "managed SSH identity validation is waiting for an active lifecycle transition",
            ),
            config: DoctorFact::unknown(
                "generated SSH config validation is waiting for an active lifecycle transition",
            ),
            native_publish: native_publish_fact(native_publish),
        };
    }
    let requires_complete_state = match (&durable, &artifacts) {
        (Ok(durable), Ok(artifacts)) => durable.expects_managed_state || *artifacts,
        _ => true,
    };
    let (identity, config) = if !requires_complete_state {
        (
            DoctorFact::pass("managed SSH identity has not been created yet"),
            DoctorFact::pass("generated SSH config has not been published yet"),
        )
    } else {
        match validate_host_identity_if_present(paths).await {
            Ok(Some(identity)) => {
                let identity_fact = DoctorFact::pass(format!(
                    "managed Ed25519 identity is valid ({})",
                    identity.fingerprint()
                ));
                let config_fact =
                    validate_complete_publication(paths, client, &client_fact, durable, artifacts)
                        .await;
                (identity_fact, config_fact)
            }
            Ok(None) => (
                DoctorFact::fail(format!(
                    "managed SSH identity at {} is missing while durable or generated SSH state exists",
                    paths.private_key()
                )),
                DoctorFact::fail(format!(
                    "generated SSH config at {} cannot be validated without the complete managed identity",
                    paths.config()
                )),
            ),
            Err(error) => (
                DoctorFact::fail(format!(
                    "managed SSH identity at {} is unsafe: {error}",
                    paths.private_key()
                )),
                DoctorFact::fail(format!(
                    "generated SSH config at {} cannot be validated because the managed identity is unsafe",
                    paths.config()
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
            return DoctorFact::fail(format!(
                "generated SSH config at {} differs from durable SSH state",
                paths.config()
            ));
        }
        Err(error) => {
            return DoctorFact::fail(format!(
                "generated SSH config at {} could not be compared with durable SSH state: {error}",
                paths.config()
            ));
        }
    };
    if let Err(error) = artifacts {
        return DoctorFact::fail(format!(
            "generated SSH config at {} is unsafe: {error}",
            paths.config()
        ));
    }
    match validate_managed_config_if_present(paths) {
        Ok(false) => {
            return DoctorFact::fail(format!(
                "generated SSH config at {} is missing while durable or generated SSH state exists",
                paths.config()
            ));
        }
        Err(error) => {
            return DoctorFact::fail(format!(
                "generated SSH config at {} is unsafe: {error}",
                paths.config()
            ));
        }
        Ok(true) => {}
    }
    let snapshot = match SshManager.published_snapshot_for_paths(paths).await {
        Ok(snapshot) => snapshot,
        Err(error) => {
            return DoctorFact::fail(format!(
                "generated SSH config at {} is unsafe or inconsistent: {error}",
                paths.config()
            ));
        }
    };
    let expected = durable
        .expected
        .iter()
        .map(|(id, resolution, port)| (id, resolution, *port))
        .collect::<Vec<_>>();
    if let Err(error) = snapshot.validate_exact(&expected) {
        return DoctorFact::fail(format!(
            "generated SSH config at {} differs from durable SSH state: {error}",
            paths.config()
        ));
    }
    if client_fact.status != gascan_core::doctor::DoctorStatus::Pass {
        return DoctorFact::fail(format!(
            "generated SSH config at {} cannot be validated without the system OpenSSH client",
            paths.config()
        ));
    }
    validate_openssh_config(client, paths.config()).await
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

async fn validate_openssh_config(client: &Path, config: &camino::Utf8Path) -> DoctorFact {
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
            return DoctorFact::fail(format!(
                "generated SSH config at {config} could not be checked: {error}"
            ));
        }
        Err(_) => {
            return DoctorFact::fail(format!(
                "generated SSH config at {config} exceeded its 10 second validation bound"
            ));
        }
    };
    if output.status.success() {
        DoctorFact::pass("generated SSH config is accepted by OpenSSH")
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
        DoctorFact::fail(format!(
            "generated SSH config at {config} was rejected by OpenSSH{}",
            if detail.is_empty() {
                String::new()
            } else {
                format!(": {detail}")
            }
        ))
    }
}
