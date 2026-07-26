use crate::SshPaths;
use crate::ssh::{validate_host_identity_if_present, validate_managed_config_if_present};
use gascan_core::doctor::DoctorFact;
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

pub async fn ssh_doctor_facts(native_publish: bool) -> SshDoctorFacts {
    const SSH_CLIENT: &str = "/usr/bin/ssh";
    match SshPaths::for_user() {
        Ok(paths) => {
            ssh_doctor_facts_for_paths(&paths, Path::new(SSH_CLIENT), native_publish).await
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
    native_publish: bool,
) -> SshDoctorFacts {
    let client_fact = ssh_client_fact(client);
    let identity = match validate_host_identity_if_present(paths).await {
        Ok(Some(identity)) => DoctorFact::pass(format!(
            "managed Ed25519 identity is valid ({})",
            identity.fingerprint()
        )),
        Ok(None) => DoctorFact::pass("managed SSH identity has not been created yet"),
        Err(error) => DoctorFact::fail(format!(
            "managed SSH identity at {} is unsafe: {error}",
            paths.private_key()
        )),
    };
    let config = match validate_managed_config_if_present(paths) {
        Ok(false) => DoctorFact::pass("generated SSH config has not been published yet"),
        Ok(true) if client_fact.status == gascan_core::doctor::DoctorStatus::Pass => {
            validate_openssh_config(client, paths.config()).await
        }
        Ok(true) => DoctorFact::fail(format!(
            "generated SSH config at {} cannot be validated without the system OpenSSH client",
            paths.config()
        )),
        Err(error) => DoctorFact::fail(format!(
            "generated SSH config at {} is unsafe: {error}",
            paths.config()
        )),
    };
    SshDoctorFacts {
        client: client_fact,
        identity,
        config,
        native_publish: native_publish_fact(native_publish),
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
            detail.truncate(MAX_DETAIL);
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
