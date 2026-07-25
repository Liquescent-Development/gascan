use super::config::{
    PreparedSshFiles, commit_openssh_files, prepare_openssh_files, readiness_ssh_args,
};
use super::identity::parse_public_key;
use super::port::PortReservation;
use super::{
    ActiveSsh, HostIdentity, ManagedSshHost, PUBLIC_MODE, SshPaths, StateDirectory,
    ensure_host_identity, maximum_managed_file_bytes,
};
use crate::service::ServiceError;
use crate::store::SshResolution;
use camino::{Utf8Path, Utf8PathBuf};
use gascan_core::policy::ControlPlanePolicy;
use gascan_core::runtime::{
    ContainerState, CreateFailure, ExecInput, ExecOutput, ExecRequest, RuntimeBackend, RuntimeError,
};
use gascan_core::sandbox::{SandboxId, SandboxSpec};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ffi::OsStr;
use std::net::{IpAddr, Ipv4Addr};
use std::process::Stdio;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::OwnedMutexGuard;

const CONFIG_NAME: &str = "config";
const SSH_CLIENT: &str = "/usr/bin/ssh";
const READINESS_TIMEOUT: Duration = Duration::from_secs(15);
pub(crate) const HOST_KEY_READ_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_HOST_KEY_OUTPUT: usize = 16 * 1024;
const HOST_KEY_PATH: &str = "/home/workspace/.config/gascan/ssh/host/ssh_host_ed25519_key.pub";
type PublicationLock = Arc<tokio::sync::Mutex<()>>;
type PublicationGuard = OwnedMutexGuard<()>;
static PUBLICATION_LOCKS: OnceLock<Mutex<HashMap<Utf8PathBuf, Weak<tokio::sync::Mutex<()>>>>> =
    OnceLock::new();

pub struct PreparedSshCreate {
    identity: HostIdentity,
    host_port: u16,
    reservation: Option<PortReservation>,
}

impl PreparedSshCreate {
    #[must_use]
    pub const fn identity(&self) -> &HostIdentity {
        &self.identity
    }

    #[must_use]
    pub const fn host_port(&self) -> u16 {
        self.host_port
    }

    pub fn release_reservation(&mut self) {
        if let Some(reservation) = self.reservation.take() {
            reservation.release();
        }
    }

    #[must_use]
    pub fn control_plane(&self) -> ControlPlanePolicy<'_> {
        ControlPlanePolicy {
            ssh_authorized_key: Some(self.identity.public_key()),
            ssh_host_port: Some(self.host_port),
        }
    }
}

pub(crate) struct PreparedSshActivation {
    managed: ManagedSshHost,
    paths: SshPaths,
    prepared: PreparedSshFiles,
    _publication: PublicationGuard,
}

impl PreparedSshActivation {
    pub(crate) fn resolution(&self) -> SshResolution {
        enabled_resolution(&self.managed.active)
    }

    pub(crate) fn commit(self) -> Result<ActiveSsh, ServiceError> {
        let active = self.managed.active;
        commit_openssh_files(&self.paths, self.prepared)
            .map_err(ServiceError::SshConfigUpdateFailed)?;
        Ok(active)
    }
}

pub struct SshManager;

impl SshManager {
    pub async fn prepare_create(
        &self,
        spec: &SandboxSpec,
    ) -> Result<Option<PreparedSshCreate>, ServiceError> {
        if !spec.manifest().ssh().enabled() {
            return Ok(None);
        }
        let paths = SshPaths::for_user().map_err(ServiceError::SshConfigUnsafe)?;
        self.prepare_create_for_paths(spec, &paths).await
    }

    #[doc(hidden)]
    pub async fn prepare_create_for_paths(
        &self,
        spec: &SandboxSpec,
        paths: &SshPaths,
    ) -> Result<Option<PreparedSshCreate>, ServiceError> {
        if !spec.manifest().ssh().enabled() {
            return Ok(None);
        }
        let identity = ensure_host_identity(paths)
            .await
            .map_err(ServiceError::SshConfigUnsafe)?;
        let (host_port, reservation) = if let Some(host_port) = spec.manifest().ssh().host_port() {
            (host_port, None)
        } else {
            let reservation = PortReservation::reserve()
                .map_err(|error| ServiceError::SshPortUnavailable(error.to_string()))?;
            (reservation.port(), Some(reservation))
        };
        Ok(Some(PreparedSshCreate {
            identity,
            host_port,
            reservation,
        }))
    }

    pub async fn activate(
        &self,
        id: &SandboxId,
        runtime: &impl RuntimeBackend,
        expected: Option<&SshResolution>,
    ) -> Result<Option<ActiveSsh>, ServiceError> {
        let paths = SshPaths::for_user().map_err(ServiceError::SshConfigUnsafe)?;
        let prepared = self
            .prepare_activation_for_paths(
                id,
                runtime,
                expected,
                &paths,
                Utf8Path::new(SSH_CLIENT),
                HOST_KEY_READ_TIMEOUT,
            )
            .await?;
        prepared.map(PreparedSshActivation::commit).transpose()
    }

    pub async fn deactivate(&self, id: &SandboxId) -> Result<(), ServiceError> {
        let paths = SshPaths::for_user().map_err(ServiceError::SshConfigUnsafe)?;
        self.deactivate_for_paths(id, &paths).await
    }

    pub(crate) async fn prepare_activation_for_paths(
        &self,
        id: &SandboxId,
        runtime: &impl RuntimeBackend,
        expected: Option<&SshResolution>,
        paths: &SshPaths,
        readiness_program: &Utf8Path,
        host_key_timeout: Duration,
    ) -> Result<Option<PreparedSshActivation>, ServiceError> {
        if expected.is_some_and(|resolution| !resolution_enabled(resolution)) {
            return Ok(None);
        }
        let publication = publication_guard(paths).await?;
        let identity = ensure_host_identity(paths)
            .await
            .map_err(ServiceError::SshConfigUnsafe)?;
        let managed =
            verified_managed_host(id, runtime, expected, &identity, host_key_timeout).await?;
        let mut hosts = load_active_hosts(paths, &identity)?;
        hosts.retain(|host| host.active.alias != managed.active.alias);
        hosts.push(managed.clone());
        let prepared = prepare_openssh_files(paths, &identity, &hosts)
            .map_err(ServiceError::SshConfigUnsafe)?;
        run_readiness(
            readiness_program.as_std_path().as_os_str(),
            readiness_ssh_args(paths, &identity, &managed, prepared.known_hosts())
                .await
                .map_err(ServiceError::SshConfigUnsafe)?,
        )
        .await?;
        Ok(Some(PreparedSshActivation {
            managed,
            paths: paths.clone(),
            prepared,
            _publication: publication,
        }))
    }

    pub(crate) async fn verify_for_reconcile(
        &self,
        id: &SandboxId,
        runtime: &impl RuntimeBackend,
        expected: &SshResolution,
        paths: &SshPaths,
        readiness_program: &Utf8Path,
        host_key_timeout: Duration,
    ) -> Result<Option<ManagedSshHost>, ServiceError> {
        if !resolution_enabled(expected) {
            return Ok(None);
        }
        let identity = ensure_host_identity(paths)
            .await
            .map_err(ServiceError::SshConfigUnsafe)?;
        let managed =
            verified_managed_host(id, runtime, Some(expected), &identity, host_key_timeout).await?;
        let prepared = prepare_openssh_files(paths, &identity, std::slice::from_ref(&managed))
            .map_err(ServiceError::SshConfigUnsafe)?;
        run_readiness(
            readiness_program.as_std_path().as_os_str(),
            readiness_ssh_args(paths, &identity, &managed, prepared.known_hosts())
                .await
                .map_err(ServiceError::SshConfigUnsafe)?,
        )
        .await?;
        Ok(Some(managed))
    }

    pub(crate) async fn publish_reconciled(
        &self,
        paths: &SshPaths,
        hosts: &[ManagedSshHost],
    ) -> Result<(), ServiceError> {
        let _publication = publication_guard(paths).await?;
        let identity = futures_identity(paths).map_err(ServiceError::SshConfigUnsafe)?;
        let prepared = prepare_openssh_files(paths, &identity, hosts)
            .map_err(ServiceError::SshConfigUnsafe)?;
        commit_openssh_files(paths, prepared).map_err(ServiceError::SshConfigUpdateFailed)
    }

    pub(crate) async fn deactivate_for_paths(
        &self,
        id: &SandboxId,
        paths: &SshPaths,
    ) -> Result<(), ServiceError> {
        let _publication = publication_guard(paths).await?;
        let identity = futures_identity(paths).map_err(ServiceError::SshConfigUnsafe)?;
        let mut hosts = load_active_hosts(paths, &identity)?;
        let alias = alias(id);
        let previous = hosts.len();
        hosts.retain(|host| host.active.alias != alias);
        if hosts.len() == previous {
            return Ok(());
        }
        let prepared = prepare_openssh_files(paths, &identity, &hosts)
            .map_err(ServiceError::SshConfigUnsafe)?;
        commit_openssh_files(paths, prepared).map_err(ServiceError::SshConfigUpdateFailed)
    }
}

async fn publication_guard(paths: &SshPaths) -> Result<PublicationGuard, ServiceError> {
    let key = paths.directory().to_owned();
    let publication = {
        let registry = PUBLICATION_LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
        let mut registry = registry.lock().map_err(|_| {
            ServiceError::SshConfigUnsafe(super::SshError::InvalidState(
                "managed SSH publication lock registry was poisoned",
            ))
        })?;
        registry.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = registry.get(&key).and_then(Weak::upgrade) {
            lock
        } else {
            let lock = PublicationLock::new(tokio::sync::Mutex::new(()));
            registry.insert(key, Arc::downgrade(&lock));
            lock
        }
    };
    Ok(publication.lock_owned().await)
}

pub(crate) fn disabled_resolution() -> SshResolution {
    SshResolution::new(
        1,
        serde_json::json!({
            "enabled": false,
            "host_key_fingerprint": "",
            "client_key_fingerprint": "",
        }),
    )
}

pub(crate) fn is_native_port_collision(failure: &CreateFailure, selected_port: u16) -> bool {
    matches!(
        failure.source(),
        RuntimeError::CommandFailed {
            operation,
            exit_code: Some(_),
            stderr,
        } if operation == "container"
            && stderr.lines().any(|line| {
                line.strip_prefix("Error: listen tcp 127.0.0.1:")
                    .and_then(|line| line.strip_suffix(": bind: address already in use"))
                    .and_then(|port| port.parse::<u16>().ok())
                    == Some(selected_port)
            })
    )
}

fn enabled_resolution(active: &ActiveSsh) -> SshResolution {
    SshResolution::new(
        1,
        serde_json::json!({
            "enabled": true,
            "host_key_fingerprint": active.host_key_fingerprint,
            "client_key_fingerprint": active.client_key_fingerprint,
        }),
    )
}

fn resolution_enabled(resolution: &SshResolution) -> bool {
    resolution.version == 1
        && resolution
            .details
            .get("enabled")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
}

fn expected_fingerprints(
    expected: Option<&SshResolution>,
) -> Result<Option<(&str, &str)>, ServiceError> {
    let Some(expected) = expected else {
        return Ok(None);
    };
    if !resolution_enabled(expected) {
        return Err(ServiceError::SshHostKeyMismatch(
            "durable SSH identity is not enabled and verified",
        ));
    }
    let host = expected
        .details
        .get("host_key_fingerprint")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or(ServiceError::SshHostKeyMismatch(
            "durable SSH host fingerprint is missing",
        ))?;
    let client = expected
        .details
        .get("client_key_fingerprint")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or(ServiceError::SshHostKeyMismatch(
            "durable SSH client fingerprint is missing",
        ))?;
    Ok(Some((host, client)))
}

async fn verified_managed_host(
    id: &SandboxId,
    runtime: &impl RuntimeBackend,
    expected: Option<&SshResolution>,
    identity: &HostIdentity,
    host_key_timeout: Duration,
) -> Result<ManagedSshHost, ServiceError> {
    let inspected = runtime
        .inspect(id)
        .await?
        .ok_or_else(|| ServiceError::Missing(id.clone()))?;
    if inspected.ownership.managed_by != "gascan" || inspected.ownership.sandbox_id != *id {
        return Err(ServiceError::Ownership(id.clone()));
    }
    if inspected.state != ContainerState::Running {
        return Err(ServiceError::SshNotReady(
            "SSH activation requires a running sandbox",
        ));
    }
    let mut mappings = inspected
        .ports()
        .iter()
        .filter(|mapping| mapping.guest_port == 22);
    let mapping = mappings.next().ok_or(ServiceError::SshNotReady(
        "runtime inspection is missing the native SSH mapping",
    ))?;
    if mappings.next().is_some()
        || mapping.host_address != IpAddr::V4(Ipv4Addr::LOCALHOST)
        || mapping.host_port < 1024
    {
        return Err(ServiceError::SshNotReady(
            "runtime inspection has an invalid native SSH mapping",
        ));
    }
    let public_key = read_host_public_key(id, runtime, host_key_timeout).await?;
    let parsed = parse_public_key(&public_key)
        .map_err(|_| ServiceError::SshHostKeyMismatch("guest SSH host key is invalid"))?;
    if let Some((expected_host, expected_client)) = expected_fingerprints(expected)?
        && (parsed.fingerprint != expected_host || identity.fingerprint() != expected_client)
    {
        return Err(ServiceError::SshHostKeyMismatch(
            "guest or client SSH identity changed",
        ));
    }
    Ok(ManagedSshHost {
        active: ActiveSsh {
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: mapping.host_port,
            alias: alias(id),
            host_key_fingerprint: parsed.fingerprint,
            client_key_fingerprint: identity.fingerprint().to_owned(),
        },
        host_public_key: parsed.normalized,
    })
}

async fn read_host_public_key(
    id: &SandboxId,
    runtime: &impl RuntimeBackend,
    timeout: Duration,
) -> Result<Vec<u8>, ServiceError> {
    tokio::time::timeout(timeout, read_host_public_key_inner(id, runtime))
        .await
        .map_err(|_| ServiceError::SshHostKeyMismatch("guest SSH host key read timed out"))?
}

async fn read_host_public_key_inner(
    id: &SandboxId,
    runtime: &impl RuntimeBackend,
) -> Result<Vec<u8>, ServiceError> {
    let mut session = runtime
        .exec(ExecRequest {
            id: id.clone(),
            argv: vec!["/usr/bin/cat".to_owned(), HOST_KEY_PATH.to_owned()],
            stdin: Vec::new(),
            environment: BTreeMap::new(),
            tty: false,
        })
        .await
        .map_err(|_| ServiceError::SshHostKeyMismatch("guest SSH host key could not be read"))?;
    session
        .send(ExecInput::Close)
        .await
        .map_err(|_| ServiceError::SshHostKeyMismatch("guest SSH host key could not be read"))?;
    let mut stdout = Vec::new();
    while let Some(output) = session.next().await {
        match output
            .map_err(|_| ServiceError::SshHostKeyMismatch("guest SSH host key could not be read"))?
        {
            ExecOutput::Stdout(bytes) => {
                if stdout.len().saturating_add(bytes.len()) > MAX_HOST_KEY_OUTPUT {
                    session.cancel();
                    while session.next().await.is_some() {}
                    return Err(ServiceError::SshHostKeyMismatch(
                        "guest SSH host key output is excessive",
                    ));
                }
                stdout.extend(bytes);
            }
            ExecOutput::Stderr(_) => {}
            ExecOutput::Exit { code: 0, signal: 0 } => return Ok(stdout),
            ExecOutput::Exit { .. } => {
                return Err(ServiceError::SshHostKeyMismatch(
                    "guest SSH host key command failed",
                ));
            }
        }
    }
    Err(ServiceError::SshHostKeyMismatch(
        "guest SSH host key command ended without status",
    ))
}

async fn run_readiness(program: &OsStr, args: Vec<std::ffi::OsString>) -> Result<(), ServiceError> {
    let mut command = Command::new(program);
    command
        .args(args)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let status = tokio::time::timeout(READINESS_TIMEOUT, command.status())
        .await
        .map_err(|_| ServiceError::SshNotReady("strict SSH readiness timed out"))?
        .map_err(|_| ServiceError::SshNotReady("strict SSH readiness could not start"))?;
    if status.success() {
        Ok(())
    } else {
        Err(ServiceError::SshNotReady(
            "strict SSH readiness command failed",
        ))
    }
}

fn load_active_hosts(
    paths: &SshPaths,
    identity: &HostIdentity,
) -> Result<Vec<ManagedSshHost>, ServiceError> {
    let directory = StateDirectory::open(paths).map_err(ServiceError::SshConfigUnsafe)?;
    if directory
        .metadata(CONFIG_NAME, PUBLIC_MODE)
        .map_err(ServiceError::SshConfigUnsafe)?
        .is_none()
    {
        return Ok(Vec::new());
    }
    let (config, _) = directory
        .read_file(CONFIG_NAME, PUBLIC_MODE, maximum_managed_file_bytes())
        .map_err(ServiceError::SshConfigUnsafe)?;
    let text = std::str::from_utf8(&config).map_err(|_| {
        ServiceError::SshConfigUnsafe(super::SshError::InvalidState(
            "managed SSH config is not UTF-8",
        ))
    })?;
    let references = text
        .lines()
        .filter_map(|line| line.strip_prefix("    UserKnownHostsFile "))
        .map(parse_rendered_path)
        .collect::<Result<BTreeSet<_>, _>>()?;
    if references.is_empty() {
        drop(directory);
        let prepared =
            prepare_openssh_files(paths, identity, &[]).map_err(ServiceError::SshConfigUnsafe)?;
        if prepared.config_bytes() != config {
            return Err(ServiceError::SshConfigUnsafe(
                super::SshError::InvalidState("managed SSH config is inconsistent"),
            ));
        }
        return Ok(Vec::new());
    }
    let mut references = references.into_iter();
    let Some(known_hosts_path) = references.next() else {
        return Err(ServiceError::SshConfigUnsafe(
            super::SshError::InvalidState("managed SSH config generation is missing"),
        ));
    };
    if references.next().is_some() {
        return Err(ServiceError::SshConfigUnsafe(
            super::SshError::InvalidState("managed SSH config generations are inconsistent"),
        ));
    }
    let known_hosts_path = Utf8PathBuf::from(known_hosts_path);
    if known_hosts_path.parent() != Some(paths.directory()) {
        return Err(ServiceError::SshConfigUnsafe(
            super::SshError::InvalidState("managed SSH config generation is outside state"),
        ));
    }
    let generation = known_hosts_path.file_name().ok_or_else(|| {
        ServiceError::SshConfigUnsafe(super::SshError::InvalidState(
            "managed SSH config generation is invalid",
        ))
    })?;
    let (known_hosts, _) = directory
        .read_file(generation, PUBLIC_MODE, maximum_managed_file_bytes())
        .map_err(ServiceError::SshConfigUnsafe)?;
    drop(directory);
    let hosts = parse_known_hosts(&known_hosts, identity)?;
    let prepared =
        prepare_openssh_files(paths, identity, &hosts).map_err(ServiceError::SshConfigUnsafe)?;
    if prepared.known_hosts() != known_hosts_path || prepared.config_bytes() != config {
        return Err(ServiceError::SshConfigUnsafe(
            super::SshError::InvalidState("managed SSH config is inconsistent"),
        ));
    }
    Ok(hosts)
}

fn parse_known_hosts(
    contents: &[u8],
    identity: &HostIdentity,
) -> Result<Vec<ManagedSshHost>, ServiceError> {
    let text = std::str::from_utf8(contents)
        .map_err(|_| config_state("managed known-hosts is not UTF-8"))?;
    let mut hosts = Vec::new();
    let mut aliases = BTreeSet::new();
    let mut ports = BTreeSet::new();
    for line in text.lines() {
        if line.is_empty() {
            return Err(config_state("managed known-hosts contains an empty record"));
        }
        let mut fields = line.split_ascii_whitespace();
        let (Some(pattern), Some(kind), Some(encoded), None) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            return Err(config_state("managed known-hosts record is malformed"));
        };
        let Some((alias, endpoint)) = pattern.split_once(",[127.0.0.1]:") else {
            return Err(config_state("managed known-hosts endpoint is malformed"));
        };
        let id = alias
            .strip_prefix("gascan-")
            .and_then(|value| SandboxId::try_from(value.to_owned()).ok())
            .filter(|id| alias == format!("gascan-{id}"))
            .ok_or_else(|| config_state("managed known-hosts alias is malformed"))?;
        let port = endpoint
            .parse::<u16>()
            .ok()
            .filter(|port| *port >= 1024 && port.to_string() == endpoint)
            .ok_or_else(|| config_state("managed known-hosts port is malformed"))?;
        let parsed = parse_public_key(format!("{kind} {encoded}").as_bytes())
            .map_err(ServiceError::SshConfigUnsafe)?;
        if line != format!("{alias},[127.0.0.1]:{port} {}", parsed.normalized)
            || !aliases.insert(alias.to_owned())
            || !ports.insert(port)
        {
            return Err(config_state("managed known-hosts record is inconsistent"));
        }
        hosts.push(ManagedSshHost {
            active: ActiveSsh {
                host: IpAddr::V4(Ipv4Addr::LOCALHOST),
                port,
                alias: format!("gascan-{id}"),
                host_key_fingerprint: parsed.fingerprint,
                client_key_fingerprint: identity.fingerprint().to_owned(),
            },
            host_public_key: parsed.normalized,
        });
    }
    Ok(hosts)
}

fn parse_rendered_path(value: &str) -> Result<String, ServiceError> {
    if let Some(quoted) = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
    {
        let mut parsed = String::with_capacity(quoted.len());
        let mut characters = quoted.chars();
        while let Some(character) = characters.next() {
            match character {
                '\\' => {
                    let escaped = characters
                        .next()
                        .filter(|escaped| matches!(escaped, '\\' | '"'))
                        .ok_or_else(|| config_state("managed SSH config path is malformed"))?;
                    parsed.push(escaped);
                }
                '%' => {
                    if characters.next() != Some('%') {
                        return Err(config_state("managed SSH config path is malformed"));
                    }
                    parsed.push('%');
                }
                _ => parsed.push(character),
            }
        }
        Ok(parsed)
    } else if value.is_empty() || value.chars().any(char::is_whitespace) {
        Err(config_state("managed SSH config path is malformed"))
    } else {
        Ok(value.to_owned())
    }
}

fn futures_identity(paths: &SshPaths) -> Result<HostIdentity, super::SshError> {
    std::thread::scope(|scope| {
        scope
            .spawn(|| {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|error| super::SshError::io("start SSH identity worker", error))?
                    .block_on(ensure_host_identity(paths))
            })
            .join()
    })
    .map_err(|_| super::SshError::InvalidState("managed SSH identity worker failed"))?
}

fn config_state(message: &'static str) -> ServiceError {
    ServiceError::SshConfigUnsafe(super::SshError::InvalidState(message))
}

fn alias(id: &SandboxId) -> String {
    format!("gascan-{id}")
}
