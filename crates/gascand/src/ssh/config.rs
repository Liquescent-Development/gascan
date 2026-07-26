use super::identity::{
    HostIdentity, open_revalidated_identity, open_revalidated_identity_async, parse_public_key,
};
use super::{
    ManagedSshHost, PUBLIC_MODE, SshError, SshPaths, StateDirectory, maximum_managed_file_bytes,
    random_staging_name,
};
use camino::{Utf8Path, Utf8PathBuf};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::ffi::OsString;
use std::fmt::Write as _;
use std::io::Write as _;
use std::net::{IpAddr, Ipv4Addr};

const CONFIG_NAME: &str = "config";
const KNOWN_HOSTS_PREFIX: &str = "known_hosts.";

pub struct PreparedSshFiles {
    generation: String,
    known_hosts: Utf8PathBuf,
    config: Vec<u8>,
}

impl PreparedSshFiles {
    #[must_use]
    pub fn generation(&self) -> &str {
        &self.generation
    }

    #[must_use]
    pub fn known_hosts(&self) -> &Utf8Path {
        &self.known_hosts
    }

    pub(crate) fn config_bytes(&self) -> &[u8] {
        &self.config
    }
}

pub fn publish_openssh_files(
    paths: &SshPaths,
    identity: &HostIdentity,
    hosts: &[ManagedSshHost],
) -> Result<(), SshError> {
    let prepared = prepare_openssh_files(paths, identity, hosts)?;
    commit_openssh_files(paths, prepared)
}

pub fn prepare_openssh_files(
    paths: &SshPaths,
    identity: &HostIdentity,
    hosts: &[ManagedSshHost],
) -> Result<PreparedSshFiles, SshError> {
    validate_identity_metadata(paths, identity)?;
    let validated = validate_hosts(identity, hosts)?;
    let known_hosts = render_known_hosts(&validated)?;
    let generation = generation_name(known_hosts.as_bytes());
    let known_hosts_path = paths.directory().join(&generation);
    let config = render_config(identity, &validated, &known_hosts_path)?.into_bytes();
    let directory = open_revalidated_identity(paths, identity)?;
    ensure_generation(&directory, &generation, known_hosts.as_bytes())?;
    Ok(PreparedSshFiles {
        generation,
        known_hosts: known_hosts_path,
        config,
    })
}

pub fn commit_openssh_files(paths: &SshPaths, prepared: PreparedSshFiles) -> Result<(), SshError> {
    commit_openssh_files_with(paths, prepared, || Ok(()))
}

fn commit_openssh_files_with<F>(
    paths: &SshPaths,
    prepared: PreparedSshFiles,
    before_config_commit: F,
) -> Result<(), SshError>
where
    F: FnOnce() -> Result<(), SshError>,
{
    validate_generation_reference(paths, &prepared)?;
    let directory = StateDirectory::open(paths)?;
    verify_generation(&directory, &prepared.generation)?;
    atomic_replace_with(
        &directory,
        CONFIG_NAME,
        &prepared.config,
        before_config_commit,
    )
}

pub async fn readiness_ssh_args(
    paths: &SshPaths,
    identity: &HostIdentity,
    host: &ManagedSshHost,
    generation_known_hosts: &Utf8Path,
) -> Result<Vec<OsString>, SshError> {
    validate_identity_metadata(paths, identity)?;
    let normalized_host_key = validate_host(identity, host)?;
    validate_generation_path(paths, generation_known_hosts)?;
    let directory = open_revalidated_identity_async(paths, identity).await?;
    let generation = generation_known_hosts
        .file_name()
        .ok_or(SshError::InvalidState(
            "managed known-hosts generation is invalid",
        ))?;
    let contents = read_verified_generation(&directory, generation)?;
    verify_host_generation_record(&contents, host, &normalized_host_key)?;
    let identity_path = openssh_path(identity.private_key())?;
    let known_hosts_path = openssh_path(generation_known_hosts)?;
    Ok(vec![
        OsString::from("-F"),
        OsString::from("/dev/null"),
        OsString::from("-o"),
        OsString::from("HostName=127.0.0.1"),
        OsString::from("-o"),
        OsString::from(format!("Port={}", host.active.port)),
        OsString::from("-o"),
        OsString::from("User=workspace"),
        OsString::from("-o"),
        OsString::from(format!("IdentityFile={identity_path}")),
        OsString::from("-o"),
        OsString::from(format!("HostKeyAlias={}", host.active.alias)),
        OsString::from("-o"),
        OsString::from(format!("UserKnownHostsFile={known_hosts_path}")),
        OsString::from("-o"),
        OsString::from("StrictHostKeyChecking=yes"),
        OsString::from("-o"),
        OsString::from("IdentitiesOnly=yes"),
        OsString::from("-o"),
        OsString::from("BatchMode=yes"),
        OsString::from("-o"),
        OsString::from("ForwardAgent=no"),
        OsString::from("-o"),
        OsString::from("ClearAllForwardings=yes"),
        OsString::from("127.0.0.1"),
        OsString::from("/usr/bin/true"),
    ])
}

fn validate_identity_metadata(paths: &SshPaths, identity: &HostIdentity) -> Result<(), SshError> {
    if identity.private_key() != paths.private_key {
        return Err(SshError::InvalidState(
            "SSH config identity is outside managed state",
        ));
    }
    let parsed = parse_public_key(identity.public_key().as_bytes())?;
    if parsed.normalized != identity.public_key() || parsed.fingerprint != identity.fingerprint() {
        return Err(SshError::InvalidState(
            "SSH config identity metadata is inconsistent",
        ));
    }
    Ok(())
}

fn validate_hosts<'a>(
    identity: &HostIdentity,
    hosts: &'a [ManagedSshHost],
) -> Result<Vec<(&'a ManagedSshHost, String)>, SshError> {
    let mut validated = Vec::with_capacity(hosts.len());
    for host in hosts {
        validated.push((host, validate_host(identity, host)?));
    }
    validated.sort_by(|left, right| left.0.active.alias.cmp(&right.0.active.alias));
    if validated
        .windows(2)
        .any(|pair| pair[0].0.active.alias == pair[1].0.active.alias)
    {
        return Err(SshError::InvalidState("managed SSH aliases are not unique"));
    }
    Ok(validated)
}

fn validate_host(identity: &HostIdentity, host: &ManagedSshHost) -> Result<String, SshError> {
    validate_alias(&host.active.alias)?;
    if host.active.host != IpAddr::V4(Ipv4Addr::LOCALHOST) || host.active.port == 0 {
        return Err(SshError::InvalidState(
            "managed SSH endpoint must be IPv4 loopback with a nonzero port",
        ));
    }
    if host.active.client_key_fingerprint != identity.fingerprint() {
        return Err(SshError::InvalidState(
            "managed SSH client fingerprint is inconsistent",
        ));
    }
    let parsed_host_key = parse_public_key(host.host_public_key.as_bytes())?;
    if parsed_host_key.fingerprint != host.active.host_key_fingerprint {
        return Err(SshError::InvalidState(
            "managed SSH host fingerprint is inconsistent",
        ));
    }
    Ok(parsed_host_key.normalized)
}

fn render_config(
    identity: &HostIdentity,
    hosts: &[(&ManagedSshHost, String)],
    known_hosts_path: &Utf8Path,
) -> Result<String, SshError> {
    let identity_path = openssh_path(identity.private_key())?;
    let known_hosts_path = openssh_path(known_hosts_path)?;
    let mut config = String::from("# Generated by Gas Can. Do not edit.\n");
    for (host, _) in hosts {
        writeln!(
            config,
            "\nHost {}\n    HostName 127.0.0.1\n    Port {}\n    User workspace\n    IdentityFile {}\n    IdentitiesOnly yes\n    HostKeyAlias {}\n    UserKnownHostsFile {}\n    StrictHostKeyChecking yes\n    ForwardAgent no",
            host.active.alias,
            host.active.port,
            identity_path,
            host.active.alias,
            known_hosts_path,
        )
        .map_err(|_| SshError::InvalidState("managed SSH config could not be rendered"))?;
    }
    Ok(config)
}

fn render_known_hosts(hosts: &[(&ManagedSshHost, String)]) -> Result<String, SshError> {
    let mut known_hosts = String::new();
    for (host, public_key) in hosts {
        writeln!(
            known_hosts,
            "{},[127.0.0.1]:{} {}",
            host.active.alias, host.active.port, public_key
        )
        .map_err(|_| SshError::InvalidState("managed known-hosts could not be rendered"))?;
    }
    Ok(known_hosts)
}

fn generation_name(contents: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(contents);
    let mut name = String::with_capacity(KNOWN_HOSTS_PREFIX.len() + digest.len() * 2);
    name.push_str(KNOWN_HOSTS_PREFIX);
    for byte in digest {
        name.push(HEX[usize::from(byte >> 4)] as char);
        name.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    name
}

fn validate_generation_reference(
    paths: &SshPaths,
    prepared: &PreparedSshFiles,
) -> Result<(), SshError> {
    if prepared.known_hosts != paths.directory().join(&prepared.generation) {
        return Err(SshError::InvalidState(
            "prepared known-hosts generation is outside managed state",
        ));
    }
    validate_generation_path(paths, &prepared.known_hosts)
}

fn validate_generation_path(paths: &SshPaths, path: &Utf8Path) -> Result<(), SshError> {
    let Some(name) = path.file_name() else {
        return Err(SshError::InvalidState(
            "managed known-hosts generation is invalid",
        ));
    };
    let valid_name = name.strip_prefix(KNOWN_HOSTS_PREFIX).is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    });
    if path.parent() != Some(paths.directory()) || !valid_name {
        return Err(SshError::InvalidState(
            "managed known-hosts generation is invalid",
        ));
    }
    Ok(())
}

fn validate_alias(alias: &str) -> Result<(), SshError> {
    let Some(suffix) = alias.strip_prefix("gascan-") else {
        return Err(SshError::InvalidState("managed SSH alias is invalid"));
    };
    if suffix.is_empty()
        || alias.len() > 128
        || !suffix
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        || suffix.starts_with('-')
        || suffix.ends_with('-')
        || suffix.contains("--")
    {
        return Err(SshError::InvalidState("managed SSH alias is invalid"));
    }
    Ok(())
}

fn openssh_path(path: &camino::Utf8Path) -> Result<String, SshError> {
    if path
        .as_str()
        .bytes()
        .any(|byte| byte.is_ascii_control() || byte == b'$')
    {
        return Err(SshError::InvalidState(
            "managed SSH path cannot be represented in OpenSSH config",
        ));
    }
    let safe = path.as_str().bytes().all(|byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'_' | b'-' | b':')
    });
    if safe {
        return Ok(path.as_str().to_owned());
    }
    let escaped = path
        .as_str()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('%', "%%");
    Ok(format!("\"{escaped}\""))
}

fn ensure_generation(
    directory: &StateDirectory,
    generation: &str,
    contents: &[u8],
) -> Result<(), SshError> {
    if contents.len() as u64 > maximum_managed_file_bytes() {
        return Err(SshError::InvalidState(
            "generated managed SSH file is too large",
        ));
    }
    if directory.metadata(generation, PUBLIC_MODE)?.is_some() {
        let (existing, _) =
            directory.read_file(generation, PUBLIC_MODE, maximum_managed_file_bytes())?;
        if existing != contents {
            return Err(SshError::InvalidState(
                "managed known-hosts generation contents are inconsistent",
            ));
        }
        return Ok(());
    }

    let staging = random_staging_name()?;
    let mut guard = StagingGuard::new(directory, &staging);
    {
        let mut file = directory.create_staging(&staging, PUBLIC_MODE)?;
        file.write_all(contents)
            .map_err(|error| SshError::io("write known-hosts generation", error))?;
        file.flush()
            .map_err(|error| SshError::io("flush known-hosts generation", error))?;
        file.sync_all()
            .map_err(|error| SshError::io("sync known-hosts generation", error))?;
    }
    directory.rename_new(&staging, generation)?;
    directory.sync()?;
    guard.disarm();
    let (published, _) =
        directory.read_file(generation, PUBLIC_MODE, maximum_managed_file_bytes())?;
    if published != contents {
        return Err(SshError::InvalidState(
            "managed known-hosts generation changed during publication",
        ));
    }
    Ok(())
}

fn verify_generation(directory: &StateDirectory, generation: &str) -> Result<(), SshError> {
    read_verified_generation(directory, generation).map(|_| ())
}

fn read_verified_generation(
    directory: &StateDirectory,
    generation: &str,
) -> Result<Vec<u8>, SshError> {
    let (contents, _) =
        directory.read_file(generation, PUBLIC_MODE, maximum_managed_file_bytes())?;
    if generation_name(&contents) != generation {
        return Err(SshError::InvalidState(
            "managed known-hosts generation failed verification",
        ));
    }
    Ok(contents)
}

fn verify_host_generation_record(
    contents: &[u8],
    host: &ManagedSshHost,
    normalized_host_key: &str,
) -> Result<(), SshError> {
    const INVALID_CONTENTS: SshError =
        SshError::InvalidState("managed known-hosts generation contents are invalid");
    let text = std::str::from_utf8(contents).map_err(|_| {
        SshError::InvalidState("managed known-hosts generation contents are invalid")
    })?;
    let mut aliases = HashSet::new();
    let mut endpoints = HashSet::new();
    let mut target_records = 0_u8;
    for line in text.lines().filter(|line| !line.is_empty()) {
        let mut fields = line.split_ascii_whitespace();
        let (Some(host_pattern), Some(algorithm), Some(encoded), None) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            return Err(INVALID_CONTENTS);
        };
        let Some((alias, endpoint)) = host_pattern.split_once(',') else {
            return Err(INVALID_CONTENTS);
        };
        validate_alias(alias)?;
        let Some(port_text) = endpoint.strip_prefix("[127.0.0.1]:") else {
            return Err(INVALID_CONTENTS);
        };
        let port = port_text.parse::<u16>().map_err(|_| INVALID_CONTENTS)?;
        if port == 0 || port.to_string() != port_text {
            return Err(INVALID_CONTENTS);
        }
        let parsed_key = parse_public_key(format!("{algorithm} {encoded}").as_bytes())?;
        let canonical = format!("{alias},[127.0.0.1]:{port} {}", parsed_key.normalized);
        if line != canonical || !aliases.insert(alias) || !endpoints.insert(port) {
            return Err(INVALID_CONTENTS);
        }
        if alias == host.active.alias
            && port == host.active.port
            && parsed_key.normalized == normalized_host_key
        {
            target_records += 1;
        }
    }
    if target_records != 1 {
        return Err(SshError::InvalidState(
            "managed known-hosts generation does not match endpoint",
        ));
    }
    Ok(())
}

#[cfg(test)]
fn atomic_replace(
    directory: &StateDirectory,
    target: &str,
    contents: &[u8],
) -> Result<(), SshError> {
    atomic_replace_with(directory, target, contents, || Ok(()))
}

fn atomic_replace_with<F>(
    directory: &StateDirectory,
    target: &str,
    contents: &[u8],
    before_publish: F,
) -> Result<(), SshError>
where
    F: FnOnce() -> Result<(), SshError>,
{
    if contents.len() as u64 > maximum_managed_file_bytes() {
        return Err(SshError::InvalidState(
            "generated managed SSH file is too large",
        ));
    }
    let previous = directory.metadata(target, PUBLIC_MODE)?;
    let staging = random_staging_name()?;
    let mut guard = StagingGuard::new(directory, &staging);
    {
        let mut file = directory.create_staging(&staging, PUBLIC_MODE)?;
        file.write_all(contents)
            .map_err(|error| SshError::io("write managed SSH staging file", error))?;
        file.flush()
            .map_err(|error| SshError::io("flush managed SSH staging file", error))?;
        file.sync_all()
            .map_err(|error| SshError::io("sync managed SSH staging file", error))?;
    }
    before_publish()?;
    if directory.metadata(target, PUBLIC_MODE)? != previous {
        return Err(SshError::InvalidState(
            "managed SSH target changed during replacement",
        ));
    }
    match previous {
        None => directory.rename_new(&staging, target)?,
        Some(_) => directory.rename_replace(&staging, target)?,
    }
    directory.sync()?;
    directory
        .metadata(target, PUBLIC_MODE)?
        .ok_or(SshError::InvalidState(
            "managed SSH file disappeared after replacement",
        ))?;
    guard.disarm();
    Ok(())
}

struct StagingGuard<'a> {
    directory: &'a StateDirectory,
    name: &'a str,
    armed: bool,
}

impl<'a> StagingGuard<'a> {
    const fn new(directory: &'a StateDirectory, name: &'a str) -> Self {
        Self {
            directory,
            name,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for StagingGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.directory.remove(self.name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PUBLIC_MODE, SshError, SshPaths, StateDirectory, atomic_replace, atomic_replace_with,
        commit_openssh_files_with, prepare_openssh_files, publish_openssh_files,
    };
    use crate::ssh::{ActiveSsh, HostIdentity, ManagedSshHost, ensure_host_identity};
    use std::fs;
    use std::net::{IpAddr, Ipv4Addr};

    fn host(alias: &str, port: u16, identity: &HostIdentity) -> ManagedSshHost {
        ManagedSshHost {
            active: ActiveSsh {
                host: IpAddr::V4(Ipv4Addr::LOCALHOST),
                port,
                alias: alias.to_owned(),
                host_key_fingerprint: identity.fingerprint().to_owned(),
                client_key_fingerprint: identity.fingerprint().to_owned(),
            },
            host_public_key: identity.public_key().to_owned(),
        }
    }

    fn configured_known_hosts(config: &str) -> Result<&str, Box<dyn std::error::Error>> {
        config
            .lines()
            .find_map(|line| line.trim().strip_prefix("UserKnownHostsFile "))
            .ok_or_else(|| "generated config does not name known-hosts".into())
    }

    #[tokio::test]
    async fn failure_before_config_commit_preserves_the_active_generation()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let home = temp.path().canonicalize()?;
        let paths = SshPaths::for_environment(None, Some(home.as_os_str()))?;
        let identity = ensure_host_identity(&paths).await?;
        publish_openssh_files(&paths, &identity, &[host("gascan-before", 2222, &identity)])?;
        let config_before = fs::read_to_string(paths.config().as_std_path())?;
        let known_hosts_before = configured_known_hosts(&config_before)?.to_owned();
        let trusted_bytes_before = fs::read(&known_hosts_before)?;
        let prepared =
            prepare_openssh_files(&paths, &identity, &[host("gascan-after", 2223, &identity)])?;

        let result = commit_openssh_files_with(&paths, prepared, || {
            Err(SshError::InvalidState("injected interruption"))
        });

        assert!(result.is_err());
        assert_eq!(
            fs::read_to_string(paths.config().as_std_path())?,
            config_before
        );
        assert_eq!(fs::read(known_hosts_before)?, trusted_bytes_before);
        Ok(())
    }

    #[test]
    fn interrupted_atomic_replacement_preserves_previous_valid_file()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let home = temp.path().canonicalize()?;
        let paths = SshPaths::for_environment(None, Some(home.as_os_str()))?;
        let directory = StateDirectory::open(&paths)?;
        atomic_replace(&directory, "config", b"previous\n")?;
        let result = atomic_replace_with(&directory, "config", b"replacement\n", || {
            Err(SshError::InvalidState("injected interruption"))
        });
        assert!(result.is_err());
        assert_eq!(fs::read(paths.config().as_std_path())?, b"previous\n");
        assert!(directory.metadata("config", PUBLIC_MODE)?.is_some());
        assert!(fs::read_dir(paths.directory().as_std_path())?.all(|entry| {
            entry.is_ok_and(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
        }));
        Ok(())
    }
}
