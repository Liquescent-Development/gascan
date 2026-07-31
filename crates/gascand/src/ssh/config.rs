use super::identity::{
    HostIdentity, open_revalidated_identity, open_revalidated_identity_async, parse_public_key,
};
use super::{
    ManagedSshDiagnostic, ManagedSshDiagnosticKind, ManagedSshHost, PUBLIC_MODE, SshError,
    SshPaths, StateDirectory, maximum_managed_file_bytes, random_staging_name,
};
use camino::{Utf8Path, Utf8PathBuf};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashSet};
use std::ffi::OsString;
use std::fmt::Write as _;
use std::io::Write as _;
use std::net::{IpAddr, Ipv4Addr};

const CONFIG_NAME: &str = "config";
const KNOWN_HOSTS_PREFIX: &str = "known_hosts.";

#[derive(Debug, thiserror::Error)]
pub enum SshConfigCommitError {
    #[error("{0}")]
    Unpublished(#[source] SshError),
    #[error(
        "managed SSH publication durability is uncertain after {original}; restoration failed: {restoration}"
    )]
    PublishedButUncertain {
        original: SshError,
        restoration: SshError,
    },
}

#[doc(hidden)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SshConfigCommitFault {
    AfterRename,
    AfterRenameAndRestore,
}

impl From<SshError> for SshConfigCommitError {
    fn from(error: SshError) -> Self {
        Self::Unpublished(error)
    }
}

pub struct PreparedSshFiles {
    generation: String,
    known_hosts: Utf8PathBuf,
    config: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct GenerationCleanup {
    pub removed: usize,
    pub stale: usize,
    pub unsafe_entries: usize,
}

pub(crate) struct GenerationInspection {
    pub(crate) cleanup: GenerationCleanup,
    pub(crate) unsafe_entry: Option<ManagedSshDiagnostic<SshError>>,
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
) -> Result<(), SshConfigCommitError> {
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

pub fn commit_openssh_files(
    paths: &SshPaths,
    prepared: PreparedSshFiles,
) -> Result<(), SshConfigCommitError> {
    commit_openssh_files_with_fault(paths, prepared, None)
}

#[doc(hidden)]
pub fn commit_openssh_files_with_fault(
    paths: &SshPaths,
    prepared: PreparedSshFiles,
    fault: Option<SshConfigCommitFault>,
) -> Result<(), SshConfigCommitError> {
    commit_openssh_files_with_points(
        paths,
        prepared,
        || Ok(()),
        move |point| match point {
            AtomicReplacePoint::AfterRenameBeforeDirectorySync
                if matches!(
                    fault,
                    Some(
                        SshConfigCommitFault::AfterRename
                            | SshConfigCommitFault::AfterRenameAndRestore
                    )
                ) =>
            {
                Err(SshError::InvalidState(
                    "injected post-rename config publication failure",
                ))
            }
            AtomicReplacePoint::BeforeRestore
                if fault == Some(SshConfigCommitFault::AfterRenameAndRestore) =>
            {
                Err(SshError::InvalidState(
                    "injected config publication restoration failure",
                ))
            }
            _ => Ok(()),
        },
    )
}

#[doc(hidden)]
pub fn commit_openssh_files_with_cleanup_fault(
    paths: &SshPaths,
    prepared: PreparedSshFiles,
) -> Result<(), SshConfigCommitError> {
    commit_openssh_files_with_points(
        paths,
        prepared,
        || Ok(()),
        |point| {
            if point == AtomicReplacePoint::BeforeGenerationCleanup {
                Err(SshError::InvalidState(
                    "injected known-hosts generation cleanup failure",
                ))
            } else {
                Ok(())
            }
        },
    )
}

#[cfg(test)]
fn commit_openssh_files_with<F>(
    paths: &SshPaths,
    prepared: PreparedSshFiles,
    before_config_commit: F,
) -> Result<(), SshConfigCommitError>
where
    F: FnOnce() -> Result<(), SshError>,
{
    commit_openssh_files_with_points(paths, prepared, before_config_commit, |_| Ok(()))
}

fn commit_openssh_files_with_points<F, G>(
    paths: &SshPaths,
    prepared: PreparedSshFiles,
    before_config_commit: F,
    mut at_point: G,
) -> Result<(), SshConfigCommitError>
where
    F: FnOnce() -> Result<(), SshError>,
    G: FnMut(AtomicReplacePoint) -> Result<(), SshError>,
{
    validate_generation_reference(paths, &prepared)?;
    let generation = prepared.generation.clone();
    let directory = StateDirectory::open(paths)?;
    verify_generation(&directory, &prepared.generation)?;
    atomic_replace_with_faults(
        &directory,
        CONFIG_NAME,
        &prepared.config,
        before_config_commit,
        &mut at_point,
    )?;

    let retained = BTreeSet::from([generation]);
    let _ = at_point(AtomicReplacePoint::BeforeGenerationCleanup).and_then(|()| {
        inspect_known_hosts_generations_in(&directory, &retained, true)
            .map(|_| ())
            .map_err(ManagedSshDiagnostic::into_source)
    });
    Ok(())
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

pub(crate) fn generation_name(contents: &[u8]) -> String {
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

pub fn prune_known_hosts_generations(
    paths: &SshPaths,
    retained: &BTreeSet<String>,
) -> Result<GenerationCleanup, SshError> {
    let directory = StateDirectory::open(paths)?;
    inspect_known_hosts_generations_in(&directory, retained, true)
        .map(|inspection| inspection.cleanup)
        .map_err(ManagedSshDiagnostic::into_source)
}

pub(crate) fn inspect_known_hosts_generations_in(
    directory: &StateDirectory,
    retained: &BTreeSet<String>,
    remove: bool,
) -> Result<GenerationInspection, ManagedSshDiagnostic<SshError>> {
    inspect_known_hosts_generations_with_points(directory, retained, remove, |_| Ok(()))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GenerationCleanupPoint {
    BeforeMetadata,
    BeforeDirectorySync,
}

fn inspect_known_hosts_generations_with_points<F>(
    directory: &StateDirectory,
    retained: &BTreeSet<String>,
    remove: bool,
    mut at_point: F,
) -> Result<GenerationInspection, ManagedSshDiagnostic<SshError>>
where
    F: FnMut(GenerationCleanupPoint) -> Result<(), SshError>,
{
    let mut cleanup = GenerationCleanup::default();
    let mut unsafe_entry = None;
    let mut cleanup_error = None;

    for bytes in directory.entry_names()? {
        if !bytes.starts_with(KNOWN_HOSTS_PREFIX.as_bytes()) {
            continue;
        }
        let Some(name) = std::str::from_utf8(&bytes)
            .ok()
            .filter(|name| valid_generation_name(name))
        else {
            cleanup.unsafe_entries += 1;
            if unsafe_entry.is_none() {
                let path = std::str::from_utf8(&bytes).ok().map_or_else(
                    || directory.path().to_owned(),
                    |name| directory.path().join(name),
                );
                unsafe_entry = Some(ManagedSshDiagnostic::new(
                    ManagedSshDiagnosticKind::Unsafe,
                    path,
                    SshError::InvalidState("managed known-hosts generation name is unsafe"),
                ));
            }
            continue;
        };

        if let Err(error) = at_point(GenerationCleanupPoint::BeforeMetadata) {
            cleanup_error = Some(ManagedSshDiagnostic::new(
                ManagedSshDiagnosticKind::Internal,
                directory.path().join(name),
                error,
            ));
            break;
        }
        let identity = match directory.metadata_inspected(name, PUBLIC_MODE) {
            Ok(Some(identity)) => identity,
            Ok(None) => continue,
            Err(diagnostic) if diagnostic.kind() == ManagedSshDiagnosticKind::Unsafe => {
                cleanup.unsafe_entries += 1;
                if unsafe_entry.is_none() {
                    unsafe_entry = Some(diagnostic);
                }
                continue;
            }
            Err(diagnostic) => {
                cleanup_error = Some(diagnostic);
                break;
            }
        };
        if retained.contains(name) {
            continue;
        }
        if !remove {
            cleanup.stale += 1;
            continue;
        }
        match directory.remove_identity_checked(name, identity, PUBLIC_MODE) {
            Ok(()) => cleanup.removed += 1,
            Err(diagnostic) if diagnostic.kind() == ManagedSshDiagnosticKind::Unsafe => {
                cleanup.unsafe_entries += 1;
                if unsafe_entry.is_none() {
                    unsafe_entry = Some(diagnostic);
                }
            }
            Err(diagnostic) => {
                cleanup_error = Some(diagnostic);
                break;
            }
        }
    }

    if cleanup.removed > 0 {
        at_point(GenerationCleanupPoint::BeforeDirectorySync).map_err(|error| {
            ManagedSshDiagnostic::new(
                ManagedSshDiagnosticKind::Internal,
                directory.path().to_owned(),
                error,
            )
        })?;
        directory.sync().map_err(|error| {
            ManagedSshDiagnostic::new(
                ManagedSshDiagnosticKind::Internal,
                directory.path().to_owned(),
                error,
            )
        })?;
    }
    if let Some(error) = cleanup_error {
        return Err(error);
    }
    Ok(GenerationInspection {
        cleanup,
        unsafe_entry,
    })
}

fn valid_generation_name(name: &str) -> bool {
    name.strip_prefix(KNOWN_HOSTS_PREFIX).is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
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
    if path.parent() != Some(paths.directory()) || !valid_generation_name(name) {
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
) -> Result<(), SshConfigCommitError> {
    atomic_replace_with(directory, target, contents, || Ok(()))
}

#[cfg(test)]
fn atomic_replace_with<F>(
    directory: &StateDirectory,
    target: &str,
    contents: &[u8],
    before_publish: F,
) -> Result<(), SshConfigCommitError>
where
    F: FnOnce() -> Result<(), SshError>,
{
    atomic_replace_with_faults(directory, target, contents, before_publish, |_| Ok(()))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AtomicReplacePoint {
    AfterRenameBeforeDirectorySync,
    AfterDirectorySyncBeforeMetadata,
    BeforeRestore,
    BeforeGenerationCleanup,
}

fn atomic_replace_with_faults<F, G>(
    directory: &StateDirectory,
    target: &str,
    contents: &[u8],
    before_publish: F,
    mut at_point: G,
) -> Result<(), SshConfigCommitError>
where
    F: FnOnce() -> Result<(), SshError>,
    G: FnMut(AtomicReplacePoint) -> Result<(), SshError>,
{
    if contents.len() as u64 > maximum_managed_file_bytes() {
        return Err(SshError::InvalidState("generated managed SSH file is too large").into());
    }
    let previous_identity = directory.metadata(target, PUBLIC_MODE)?;
    let previous = if previous_identity.is_some() {
        let (contents, identity) =
            directory.read_file(target, PUBLIC_MODE, maximum_managed_file_bytes())?;
        if Some(identity) != previous_identity {
            return Err(
                SshError::InvalidState("managed SSH target changed during replacement").into(),
            );
        }
        Some(contents)
    } else {
        None
    };
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
    let replacement_identity =
        directory
            .metadata(&staging, PUBLIC_MODE)?
            .ok_or(SshError::InvalidState(
                "managed SSH staging file disappeared before publication",
            ))?;
    before_publish()?;
    if directory.metadata(target, PUBLIC_MODE)? != previous_identity {
        return Err(SshError::InvalidState("managed SSH target changed during replacement").into());
    }
    match previous_identity {
        None => directory.rename_new(&staging, target)?,
        Some(_) => directory.rename_replace(&staging, target)?,
    }
    guard.disarm();
    let published = (|| {
        at_point(AtomicReplacePoint::AfterRenameBeforeDirectorySync)?;
        directory.sync()?;
        at_point(AtomicReplacePoint::AfterDirectorySyncBeforeMetadata)?;
        directory
            .metadata(target, PUBLIC_MODE)?
            .ok_or(SshError::InvalidState(
                "managed SSH file disappeared after replacement",
            ))?;
        Ok::<_, SshError>(())
    })();
    if let Err(error) = published {
        return match restore_previous(
            directory,
            target,
            replacement_identity,
            previous.as_deref(),
            &mut at_point,
        ) {
            Ok(()) => Err(SshConfigCommitError::Unpublished(error)),
            Err(restoration) => Err(SshConfigCommitError::PublishedButUncertain {
                original: error,
                restoration,
            }),
        };
    }
    Ok(())
}

fn restore_previous<G>(
    directory: &StateDirectory,
    target: &str,
    replacement_identity: super::FileIdentity,
    previous: Option<&[u8]>,
    at_point: &mut G,
) -> Result<(), SshError>
where
    G: FnMut(AtomicReplacePoint) -> Result<(), SshError>,
{
    at_point(AtomicReplacePoint::BeforeRestore)?;
    if let Some(previous) = previous {
        let staging = random_staging_name()?;
        let mut guard = StagingGuard::new(directory, &staging);
        {
            let mut file = directory.create_staging(&staging, PUBLIC_MODE)?;
            file.write_all(previous)
                .map_err(|error| SshError::io("write SSH restoration staging file", error))?;
            file.flush()
                .map_err(|error| SshError::io("flush SSH restoration staging file", error))?;
            file.sync_all()
                .map_err(|error| SshError::io("sync SSH restoration staging file", error))?;
        }
        let restored_identity =
            directory
                .metadata(&staging, PUBLIC_MODE)?
                .ok_or(SshError::InvalidState(
                    "SSH restoration staging file disappeared",
                ))?;
        if directory.metadata(target, PUBLIC_MODE)? != Some(replacement_identity) {
            return Err(SshError::InvalidState(
                "managed SSH target changed before restoration",
            ));
        }
        directory.rename_replace(&staging, target)?;
        guard.disarm();
        directory.sync()?;
        let (restored, identity) =
            directory.read_file(target, PUBLIC_MODE, maximum_managed_file_bytes())?;
        if identity != restored_identity || restored != previous {
            return Err(SshError::InvalidState(
                "managed SSH config restoration could not be verified",
            ));
        }
    } else {
        if directory.metadata(target, PUBLIC_MODE)? != Some(replacement_identity) {
            return Err(SshError::InvalidState(
                "managed SSH target changed before restoration",
            ));
        }
        directory.remove_checked(target)?;
        directory.sync()?;
        if directory.metadata(target, PUBLIC_MODE)?.is_some() {
            return Err(SshError::InvalidState(
                "managed SSH config removal could not be verified",
            ));
        }
    }
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
        AtomicReplacePoint, GenerationCleanupPoint, PUBLIC_MODE, SshConfigCommitError, SshError,
        SshPaths, StateDirectory, atomic_replace, atomic_replace_with, atomic_replace_with_faults,
        commit_openssh_files, commit_openssh_files_with, commit_openssh_files_with_points,
        generation_name, inspect_known_hosts_generations_with_points, prepare_openssh_files,
        publish_openssh_files,
    };
    use crate::ssh::{ActiveSsh, HostIdentity, ManagedSshHost, ensure_host_identity};
    use std::fs;
    use std::net::{IpAddr, Ipv4Addr};
    use std::os::unix::fs::PermissionsExt as _;

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

    #[tokio::test]
    async fn generation_cleanup_keeps_the_publication_lock_until_pruning_finishes()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let home = temp.path().canonicalize()?;
        let paths = SshPaths::for_environment(None, Some(home.as_os_str()))?;
        let identity = ensure_host_identity(&paths).await?;
        let first =
            prepare_openssh_files(&paths, &identity, &[host("gascan-first", 2222, &identity)])?;
        let second_host = host("gascan-second", 2223, &identity);
        let second_identity = identity.clone();
        let second_paths = paths.clone();
        let (cleanup_reached_tx, cleanup_reached_rx) = std::sync::mpsc::sync_channel(0);
        let (continue_cleanup_tx, continue_cleanup_rx) = std::sync::mpsc::sync_channel(0);
        let first_paths = paths.clone();
        let first_publication = std::thread::spawn(move || {
            commit_openssh_files_with_points(
                &first_paths,
                first,
                || Ok(()),
                |point| {
                    if point == AtomicReplacePoint::BeforeGenerationCleanup {
                        cleanup_reached_tx.send(()).map_err(|_| {
                            SshError::InvalidState("cleanup test coordination failed")
                        })?;
                        continue_cleanup_rx.recv().map_err(|_| {
                            SshError::InvalidState("cleanup test coordination failed")
                        })?;
                    }
                    Ok(())
                },
            )
        });
        cleanup_reached_rx.recv()?;

        let (second_started_tx, second_started_rx) = std::sync::mpsc::sync_channel(0);
        let (second_prepared_tx, second_prepared_rx) = std::sync::mpsc::sync_channel(1);
        let second_publication = std::thread::spawn(move || {
            second_started_tx
                .send(())
                .map_err(|_| SshError::InvalidState("cleanup test coordination failed"))?;
            let second = prepare_openssh_files(&second_paths, &second_identity, &[second_host])?;
            second_prepared_tx
                .send(())
                .map_err(|_| SshError::InvalidState("cleanup test coordination failed"))?;
            commit_openssh_files(&second_paths, second)
        });
        second_started_rx.recv()?;
        let prepared_while_cleanup_paused = second_prepared_rx
            .recv_timeout(std::time::Duration::from_millis(500))
            .is_ok();
        continue_cleanup_tx.send(())?;

        first_publication
            .join()
            .map_err(|_| std::io::Error::other("first publication thread panicked"))??;
        second_publication
            .join()
            .map_err(|_| std::io::Error::other("second publication thread panicked"))??;

        assert!(
            !prepared_while_cleanup_paused,
            "a second publisher acquired the directory lock before cleanup finished"
        );
        let config = fs::read_to_string(paths.config())?;
        assert!(config.contains("Host gascan-second"));
        let active = configured_known_hosts(&config)?;
        assert!(std::path::Path::new(active).exists());
        Ok(())
    }

    #[tokio::test]
    async fn partial_generation_cleanup_syncs_before_returning_a_later_inspection_error()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let home = temp.path().canonicalize()?;
        let paths = SshPaths::for_environment(None, Some(home.as_os_str()))?;
        ensure_host_identity(&paths).await?;
        let first = paths.directory().join(generation_name(b"first stale\n"));
        let second = paths.directory().join(generation_name(b"second stale\n"));
        for (path, contents) in [
            (&first, b"first stale\n".as_slice()),
            (&second, b"second stale\n".as_slice()),
        ] {
            fs::write(path, contents)?;
            fs::set_permissions(path, fs::Permissions::from_mode(u32::from(PUBLIC_MODE)))?;
        }
        let directory = StateDirectory::open(&paths)?;
        let mut metadata_inspections = 0_usize;

        let result = inspect_known_hosts_generations_with_points(
            &directory,
            &std::collections::BTreeSet::new(),
            true,
            |point| match point {
                GenerationCleanupPoint::BeforeMetadata => {
                    metadata_inspections += 1;
                    if metadata_inspections == 2 {
                        Err(SshError::InvalidState(
                            "injected later generation inspection failure",
                        ))
                    } else {
                        Ok(())
                    }
                }
                GenerationCleanupPoint::BeforeDirectorySync => Err(SshError::InvalidState(
                    "injected generation cleanup directory sync failure",
                )),
            },
        );

        let error = result
            .err()
            .ok_or("generation cleanup unexpectedly passed")?;
        assert!(matches!(
            error.source(),
            SshError::InvalidState("injected generation cleanup directory sync failure")
        ));
        assert_eq!(
            usize::from(first.exists()) + usize::from(second.exists()),
            1
        );
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

    #[test]
    fn directory_sync_failure_after_rename_restores_previous_config()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let home = temp.path().canonicalize()?;
        let paths = SshPaths::for_environment(None, Some(home.as_os_str()))?;
        let directory = StateDirectory::open(&paths)?;
        atomic_replace(&directory, "config", b"previous\n")?;

        let result = atomic_replace_with_faults(
            &directory,
            "config",
            b"replacement\n",
            || Ok(()),
            |point| {
                if point == AtomicReplacePoint::AfterRenameBeforeDirectorySync {
                    Err(SshError::InvalidState(
                        "injected directory synchronization failure",
                    ))
                } else {
                    Ok(())
                }
            },
        );

        assert!(result.is_err());
        assert_eq!(fs::read(paths.config().as_std_path())?, b"previous\n");
        Ok(())
    }

    #[test]
    fn post_rename_metadata_failure_restores_previous_config()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let home = temp.path().canonicalize()?;
        let paths = SshPaths::for_environment(None, Some(home.as_os_str()))?;
        let directory = StateDirectory::open(&paths)?;
        atomic_replace(&directory, "config", b"previous\n")?;

        let result = atomic_replace_with_faults(
            &directory,
            "config",
            b"replacement\n",
            || Ok(()),
            |point| {
                if point == AtomicReplacePoint::AfterDirectorySyncBeforeMetadata {
                    Err(SshError::InvalidState(
                        "injected post-rename metadata failure",
                    ))
                } else {
                    Ok(())
                }
            },
        );

        assert!(result.is_err());
        assert_eq!(fs::read(paths.config().as_std_path())?, b"previous\n");
        Ok(())
    }

    #[test]
    fn post_rename_failure_removes_fresh_config_when_no_prior_config_existed()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let home = temp.path().canonicalize()?;
        let paths = SshPaths::for_environment(None, Some(home.as_os_str()))?;
        let directory = StateDirectory::open(&paths)?;

        let result = atomic_replace_with_faults(
            &directory,
            "config",
            b"replacement\n",
            || Ok(()),
            |point| {
                if point == AtomicReplacePoint::AfterRenameBeforeDirectorySync {
                    Err(SshError::InvalidState(
                        "injected directory synchronization failure",
                    ))
                } else {
                    Ok(())
                }
            },
        );

        assert!(result.is_err());
        assert!(!paths.config().exists());
        Ok(())
    }

    #[test]
    fn restoration_failure_reports_published_but_uncertain()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let home = temp.path().canonicalize()?;
        let paths = SshPaths::for_environment(None, Some(home.as_os_str()))?;
        let directory = StateDirectory::open(&paths)?;
        atomic_replace(&directory, "config", b"previous\n")?;

        let result = atomic_replace_with_faults(
            &directory,
            "config",
            b"replacement\n",
            || Ok(()),
            |point| match point {
                AtomicReplacePoint::AfterRenameBeforeDirectorySync => Err(SshError::InvalidState(
                    "injected directory synchronization failure",
                )),
                AtomicReplacePoint::BeforeRestore => {
                    Err(SshError::InvalidState("injected restoration failure"))
                }
                AtomicReplacePoint::AfterDirectorySyncBeforeMetadata
                | AtomicReplacePoint::BeforeGenerationCleanup => Ok(()),
            },
        );

        assert!(matches!(
            result,
            Err(SshConfigCommitError::PublishedButUncertain { .. })
        ));
        assert_eq!(fs::read(paths.config().as_std_path())?, b"replacement\n");
        Ok(())
    }
}
