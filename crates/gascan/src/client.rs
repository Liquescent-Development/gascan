use crate::daemon::{
    DaemonEndpoint, DaemonIdentity, DaemonLaunch, DaemonPaths, DaemonSpawner, DaemonStartupMonitor,
    DaemonState, EndpointPathState, EndpointProbe, EndpointSession, FileIdentity,
    InstanceTimestamp, SupervisorError,
};
use gascan_proto::v1::gas_can_client::GasCanClient;
use gascan_proto::{API_MAJOR, API_MINOR, validate_transport_security};
use hyper_util::rt::TokioIo;
#[cfg(test)]
use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tonic::transport::{Channel, Endpoint};
use tower::service_fn;

const ENDPOINT_PROBE_TIMEOUT: Duration = Duration::from_millis(250);

#[derive(Debug)]
pub enum ClientError {
    Io(std::io::Error),
    Transport(tonic::transport::Error),
    Rpc(Box<tonic::Status>),
    Api(String),
}

impl ClientError {
    pub fn stable_code(&self) -> Option<&str> {
        match self {
            Self::Rpc(status) => Some(status.message()),
            Self::Api(message) => Some(message),
            Self::Io(_) | Self::Transport(_) => None,
        }
    }

    pub fn cause(&self) -> Option<String> {
        match self {
            Self::Rpc(status) => gascan_proto::error_detail::decode_message(status.details()),
            Self::Io(_) | Self::Transport(_) | Self::Api(_) => None,
        }
    }

    pub fn failure_details(&self) -> Option<serde_json::Value> {
        match self {
            Self::Rpc(status) => gascan_proto::error_detail::decode_details(status.details())
                .and_then(|details| serde_json::from_slice(&details).ok()),
            Self::Io(_) | Self::Transport(_) | Self::Api(_) => None,
        }
    }
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "daemon I/O error: {error}"),
            Self::Transport(error) => write!(formatter, "daemon transport error: {error}"),
            Self::Rpc(error) => match self.cause() {
                Some(cause) => write!(formatter, "error: {cause}"),
                None => write!(formatter, "daemon error: {}", error.message()),
            },
            Self::Api(message) => write!(formatter, "API mismatch: {message}"),
        }
    }
}
impl std::error::Error for ClientError {}
impl From<std::io::Error> for ClientError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}
impl From<tonic::transport::Error> for ClientError {
    fn from(value: tonic::transport::Error) -> Self {
        Self::Transport(value)
    }
}
impl From<tonic::Status> for ClientError {
    fn from(value: tonic::Status) -> Self {
        Self::Rpc(Box::new(value))
    }
}
impl From<SupervisorError> for ClientError {
    fn from(value: SupervisorError) -> Self {
        match value {
            SupervisorError::Client(error) => error,
            SupervisorError::Io(error) => Self::Io(error),
            error => Self::Io(std::io::Error::other(error.to_string())),
        }
    }
}

pub struct Client {
    pub api: GasCanClient<Channel>,
}

impl Client {
    pub async fn daemon_attestation() -> Result<gascan_proto::v1::HandshakeResponse, ClientError> {
        let status = crate::daemon::inspect().await?;
        if !matches!(status.state, DaemonState::Current | DaemonState::Outdated) {
            return Err(ClientError::Io(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                status
                    .detail
                    .unwrap_or_else(|| "daemon attestation is not trusted".to_owned()),
            )));
        }
        let identity = status.identity.ok_or_else(|| {
            ClientError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "daemon attestation did not identify a live endpoint",
            ))
        })?;
        let daemon_executable =
            identity
                .executable
                .into_os_string()
                .into_string()
                .map_err(|_| {
                    ClientError::Io(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "daemon executable path is not valid UTF-8",
                    ))
                })?;
        Ok(gascan_proto::v1::HandshakeResponse {
            api_major: API_MAJOR,
            api_minor: API_MINOR,
            transport_security: Some(gascan_proto::local_transport_security()),
            daemon_pid: identity.pid,
            daemon_executable,
            daemon_start_identity: identity.start_identity,
            daemon_instance_token: identity.instance_token,
            release_version: identity.release_version.unwrap_or_default(),
            daemon_started_at: identity.started_at.map(|timestamp| prost_types::Timestamp {
                seconds: timestamp.seconds,
                nanos: timestamp.nanos,
            }),
            ..Default::default()
        })
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct TonicEndpoint;

#[tonic::async_trait]
impl DaemonEndpoint for TonicEndpoint {
    type Connection = Client;

    async fn probe(
        &self,
        paths: &DaemonPaths,
        expected_path: EndpointPathState,
    ) -> Result<EndpointProbe<Self::Connection>, ClientError> {
        let EndpointPathState::SafeSocket(expected_path) = expected_path else {
            return Ok(EndpointProbe::AbsentOrInert);
        };
        let connected =
            match tokio::time::timeout(ENDPOINT_PROBE_TIMEOUT, connect(paths, expected_path)).await
            {
                Err(_) => {
                    return Ok(EndpointProbe::Unresponsive(
                        "daemon endpoint connection timed out".to_owned(),
                    ));
                }
                Ok(Err(error)) if definitely_inert_connect_error(&error) => {
                    return Ok(EndpointProbe::AbsentOrInert);
                }
                Ok(Err(error)) if startup_transient(&error) => {
                    return Ok(EndpointProbe::Unresponsive(error.to_string()));
                }
                Ok(Err(error)) => return Ok(EndpointProbe::Unsafe(error.to_string())),
                Ok(Ok(api)) => api,
            };
        let ConnectedApi { mut api, peer_pid } = connected;
        let handshake = match tokio::time::timeout(
            ENDPOINT_PROBE_TIMEOUT,
            api.handshake(gascan_proto::v1::HandshakeRequest {
                api_major: requested_api_major(),
                api_minor: API_MINOR,
                requested_capabilities: Vec::new(),
            }),
        )
        .await
        {
            Err(_) => {
                return Ok(EndpointProbe::Unresponsive(
                    "daemon endpoint handshake timed out".to_owned(),
                ));
            }
            Ok(Err(status)) => {
                let error = ClientError::from(status);
                if startup_transient(&error) {
                    return Ok(EndpointProbe::Unresponsive(error.to_string()));
                }
                return Ok(EndpointProbe::Unsafe(error.to_string()));
            }
            Ok(Ok(response)) => response.into_inner(),
        };
        let identity = match identity_from_handshake(&handshake) {
            Ok(identity) => identity,
            Err(error) => return Ok(EndpointProbe::Unsafe(error.to_string())),
        };
        if peer_pid.is_some_and(|peer_pid| peer_pid != identity.pid) {
            return Ok(EndpointProbe::Unsafe(
                "daemon endpoint peer PID contradicts the handshake identity".to_owned(),
            ));
        }
        if identity.release_version.as_deref() == Some(env!("CARGO_PKG_VERSION")) {
            if let Some(rejection) = &handshake.rejection {
                return Err(ClientError::Api(rejection.code.clone()));
            }
            if handshake.api_major != API_MAJOR {
                return Err(ClientError::Api("incompatible_api_major".to_owned()));
            }
        }
        let safe_transport = handshake
            .transport_security
            .as_ref()
            .is_some_and(|security| validate_transport_security(security).is_ok());
        let compatible_api = handshake.rejection.is_none() && handshake.api_major == API_MAJOR;
        let healthy = if identity.release_version.is_none() {
            true
        } else {
            match tokio::time::timeout(
                ENDPOINT_PROBE_TIMEOUT,
                api.daemon_status(gascan_proto::v1::DaemonStatusRequest {}),
            )
            .await
            {
                Ok(Ok(response)) => status_confirms_handshake(&handshake, &response.into_inner()),
                Err(_) | Ok(Err(_)) => false,
            }
        };
        Ok(EndpointProbe::Connected(EndpointSession {
            connection: Client { api },
            identity,
            compatible_api,
            safe_transport,
            healthy,
        }))
    }

    async fn graceful_shutdown(
        &self,
        connection: &mut Self::Connection,
        instance_token: &str,
    ) -> Result<(), ClientError> {
        let response = connection
            .api
            .shutdown_daemon(gascan_proto::v1::ShutdownDaemonRequest {
                daemon_instance_token: instance_token.to_owned(),
            })
            .await?
            .into_inner();
        if !response.accepted {
            return Err(ClientError::Api("daemon_shutdown_not_accepted".to_owned()));
        }
        Ok(())
    }
}

fn requested_api_major() -> u32 {
    std::env::var("GASCAN_API_MAJOR")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(API_MAJOR)
}

fn identity_from_handshake(
    handshake: &gascan_proto::v1::HandshakeResponse,
) -> Result<DaemonIdentity, ClientError> {
    let release_version =
        (!handshake.release_version.is_empty()).then(|| handshake.release_version.clone());
    let started_at = handshake
        .daemon_started_at
        .map(|timestamp| InstanceTimestamp {
            seconds: timestamp.seconds,
            nanos: timestamp.nanos,
        });
    Ok(DaemonIdentity {
        pid: handshake.daemon_pid,
        executable: PathBuf::from(&handshake.daemon_executable),
        start_identity: handshake.daemon_start_identity.clone(),
        instance_token: handshake.daemon_instance_token.clone(),
        release_version,
        started_at,
    })
}

fn status_confirms_handshake(
    handshake: &gascan_proto::v1::HandshakeResponse,
    status: &gascan_proto::v1::DaemonStatusResponse,
) -> bool {
    status.health == gascan_proto::v1::DaemonHealth::Healthy as i32
        && status.release_version == handshake.release_version
        && status.daemon_pid == handshake.daemon_pid
        && status.daemon_executable == handshake.daemon_executable
        && status.daemon_start_identity == handshake.daemon_start_identity
        && status.daemon_instance_token == handshake.daemon_instance_token
        && status.daemon_started_at == handshake.daemon_started_at
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct TokioDaemonSpawner;

impl DaemonSpawner for TokioDaemonSpawner {
    fn spawn(&self, launch: &DaemonLaunch) -> std::io::Result<DaemonStartupMonitor> {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

        let flags =
            rustix::fs::OFlags::RDWR | rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC;
        let startup_descriptor = match rustix::fs::open(
            &launch.startup_diagnostic_path,
            flags | rustix::fs::OFlags::CREATE | rustix::fs::OFlags::EXCL,
            rustix::fs::Mode::from_raw_mode(0o600),
        ) {
            Ok(descriptor) => descriptor,
            Err(rustix::io::Errno::EXIST) => rustix::fs::open(
                &launch.startup_diagnostic_path,
                flags,
                rustix::fs::Mode::empty(),
            )?,
            Err(error) => return Err(error.into()),
        };
        let startup_file = std::fs::File::from(startup_descriptor);
        let metadata = startup_file.metadata()?;
        if !metadata.file_type().is_file()
            || metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.permissions().mode() & 0o777 != 0o600
            || metadata.nlink() != 1
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "daemon startup diagnostic file ownership, type, links, or mode is unsafe",
            ));
        }
        startup_file.set_len(0)?;
        let mut command = tokio::process::Command::new(&launch.executable);
        command
            .current_dir(&launch.current_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .env("GASCAN_DAEMON_INSTANCE_PATH", &launch.instance_path)
            .env("GASCAN_DAEMON_OWNER_TOKEN", &launch.owner_token)
            .env(
                "GASCAN_CONTROLLER_STARTUP_PATH",
                &launch.startup_diagnostic_path,
            );
        if let Some(path) = &launch.stderr_path {
            command.stderr(
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)?,
            );
        } else {
            command.stderr(Stdio::null());
        }
        let _child = command.spawn()?;
        Ok(DaemonStartupMonitor::from_file(
            startup_file,
            launch.owner_token.clone(),
        ))
    }
}

fn startup_transient(error: &ClientError) -> bool {
    match error {
        ClientError::Io(_) | ClientError::Transport(_) => true,
        ClientError::Rpc(status) => {
            status.code() == tonic::Code::Unavailable
                || (status.code() == tonic::Code::Unknown
                    && status.message().contains("transport error"))
        }
        ClientError::Api(_) => false,
    }
}

fn definitely_inert_connect_error(error: &ClientError) -> bool {
    fn inert(kind: std::io::ErrorKind) -> bool {
        matches!(
            kind,
            std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
        )
    }

    match error {
        ClientError::Io(error) => inert(error.kind()),
        ClientError::Transport(error) => {
            let mut source: Option<&(dyn std::error::Error + 'static)> = Some(error);
            while let Some(error) = source {
                if let Some(error) = error.downcast_ref::<std::io::Error>() {
                    return inert(error.kind());
                }
                source = error.source();
            }
            false
        }
        ClientError::Rpc(_) | ClientError::Api(_) => false,
    }
}

#[cfg(test)]
fn daemon_launch_environment(
    paths: &DaemonPaths,
    instance_override: Option<&OsStr>,
    owner_override: Option<&OsStr>,
) -> Result<(PathBuf, String), ClientError> {
    let instance = instance_override.map_or_else(|| paths.instance().to_owned(), PathBuf::from);
    let owner = match owner_override {
        Some(value) => value
            .to_str()
            .ok_or_else(|| {
                ClientError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "daemon owner token must be valid UTF-8",
                ))
            })?
            .to_owned(),
        None => {
            let mut random = [0_u8; 32];
            getrandom::fill(&mut random).map_err(std::io::Error::other)?;
            random.iter().map(|byte| format!("{byte:02x}")).collect()
        }
    };
    if owner.is_empty() {
        return Err(ClientError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "daemon owner token must not be empty",
        )));
    }
    Ok((instance, owner))
}

pub(crate) fn daemon_path() -> Result<PathBuf, ClientError> {
    if let Some(path) = std::env::var_os("GASCAN_DAEMON") {
        return Ok(path.into());
    }
    let mut path = std::env::current_exe()?;
    path.set_file_name("gascand");
    Ok(path)
}

struct ConnectedApi {
    api: GasCanClient<Channel>,
    peer_pid: Option<u32>,
}

async fn connect(
    paths: &DaemonPaths,
    expected_path: FileIdentity,
) -> Result<ConnectedApi, ClientError> {
    let observed_peer_pid = std::sync::Arc::new(std::sync::Mutex::new(None));
    let connector_peer_pid = std::sync::Arc::clone(&observed_peer_pid);
    let paths = paths.clone();
    let channel = Endpoint::from_static("http://[::]:50051")
        .connect_with_connector(service_fn(move |_| {
            let paths = paths.clone();
            let connector_peer_pid = std::sync::Arc::clone(&connector_peer_pid);
            async move {
                let stream = tokio::net::UnixStream::connect(paths.socket()).await?;
                crate::daemon::validate_endpoint_path_identity(&paths, expected_path)?;
                let credentials = stream.peer_cred()?;
                if credentials.uid() != rustix::process::geteuid().as_raw() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "daemon endpoint peer UID does not match the effective user",
                    ));
                }
                let peer_pid = credentials
                    .pid()
                    .map(u32::try_from)
                    .transpose()
                    .map_err(|_| {
                        std::io::Error::new(
                            std::io::ErrorKind::PermissionDenied,
                            "daemon endpoint peer PID is invalid",
                        )
                    })?;
                *connector_peer_pid.lock().map_err(|_| {
                    std::io::Error::other("daemon endpoint peer credentials were poisoned")
                })? = peer_pid;
                Ok(TokioIo::new(stream))
            }
        }))
        .await?;
    let peer_pid = *observed_peer_pid
        .lock()
        .map_err(|_| std::io::Error::other("daemon endpoint peer credentials were poisoned"))?;
    Ok(ConnectedApi {
        api: GasCanClient::new(channel),
        peer_pid,
    })
}

#[cfg(test)]
mod tests {
    use super::{ClientError, TokioDaemonSpawner, definitely_inert_connect_error};
    use crate::daemon::{DaemonLaunch, DaemonPaths, DaemonSpawner};
    use std::os::unix::fs::PermissionsExt as _;

    #[test]
    fn endpoint_absence_requires_a_definite_connect_error() {
        for kind in [
            std::io::ErrorKind::NotFound,
            std::io::ErrorKind::ConnectionRefused,
        ] {
            assert!(definitely_inert_connect_error(&ClientError::Io(
                std::io::Error::from(kind)
            )));
        }
        for kind in [
            std::io::ErrorKind::TimedOut,
            std::io::ErrorKind::PermissionDenied,
            std::io::ErrorKind::Other,
        ] {
            assert!(!definitely_inert_connect_error(&ClientError::Io(
                std::io::Error::from(kind)
            )));
        }
    }

    #[test]
    fn daemon_launch_environment_sets_normal_paths_and_fresh_owner_token()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let paths =
            DaemonPaths::from_runtime_root(temp.path().canonicalize()?.join("gascan-runtime"));
        let (instance, first) = super::daemon_launch_environment(&paths, None, None)?;
        let (_, second) = super::daemon_launch_environment(&paths, None, None)?;
        assert_eq!(instance, paths.instance());
        assert_eq!(first.len(), 64);
        assert_eq!(second.len(), 64);
        assert_ne!(first, second);
        Ok(())
    }

    #[test]
    fn daemon_launch_environment_preserves_e2e_overrides() -> Result<(), Box<dyn std::error::Error>>
    {
        let temp = tempfile::tempdir()?;
        let root = temp.path().canonicalize()?;
        let paths = DaemonPaths::from_runtime_root(root.join("gascan-runtime"));
        let override_path = root.join("e2e-instance.json");
        let (instance, owner) = super::daemon_launch_environment(
            &paths,
            Some(override_path.as_os_str()),
            Some(std::ffi::OsStr::new("e2e-owner")),
        )?;
        assert_eq!(instance, override_path);
        assert_eq!(owner, "e2e-owner");
        assert!(
            super::daemon_launch_environment(&paths, None, Some(std::ffi::OsStr::new("")),)
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn endpoint_wire_identity_preserves_legacy_and_exact_release_versions()
    -> Result<(), Box<dyn std::error::Error>> {
        let base = gascan_proto::v1::HandshakeResponse {
            api_major: gascan_proto::API_MAJOR,
            api_minor: gascan_proto::API_MINOR,
            transport_security: Some(gascan_proto::local_transport_security()),
            daemon_instance_token: "11".repeat(32),
            daemon_pid: 42,
            daemon_executable: "/trusted/gascand".to_owned(),
            daemon_start_identity: "start:42".to_owned(),
            ..Default::default()
        };
        let legacy = super::identity_from_handshake(&base)?;
        assert_eq!(legacy.release_version, None);
        assert_eq!(legacy.started_at, None);

        let timestamp_without_release = gascan_proto::v1::HandshakeResponse {
            daemon_started_at: Some(prost_types::Timestamp {
                seconds: 1_785_264_099,
                nanos: 456_000_000,
            }),
            ..base.clone()
        };
        let contradictory = super::identity_from_handshake(&timestamp_without_release)?;
        assert_eq!(contradictory.release_version, None);
        assert_eq!(
            contradictory.started_at,
            Some(crate::daemon::InstanceTimestamp {
                seconds: 1_785_264_099,
                nanos: 456_000_000,
            }),
            "wire conversion erased a timestamp-without-release contradiction"
        );

        let current = gascan_proto::v1::HandshakeResponse {
            release_version: env!("CARGO_PKG_VERSION").to_owned(),
            daemon_started_at: Some(prost_types::Timestamp {
                seconds: 1_785_264_100,
                nanos: 123_000_000,
            }),
            ..base.clone()
        };
        assert_eq!(
            super::identity_from_handshake(&current)?
                .release_version
                .as_deref(),
            Some(env!("CARGO_PKG_VERSION"))
        );

        let outdated = gascan_proto::v1::HandshakeResponse {
            release_version: "0.1.10".to_owned(),
            ..current
        };
        assert_eq!(
            super::identity_from_handshake(&outdated)?
                .release_version
                .as_deref(),
            Some("0.1.10")
        );
        Ok(())
    }

    #[test]
    fn endpoint_status_must_repeat_the_handshake_identity_exactly()
    -> Result<(), Box<dyn std::error::Error>> {
        let handshake = gascan_proto::v1::HandshakeResponse {
            api_major: gascan_proto::API_MAJOR,
            api_minor: gascan_proto::API_MINOR,
            transport_security: Some(gascan_proto::local_transport_security()),
            daemon_instance_token: "11".repeat(32),
            daemon_pid: 42,
            daemon_executable: "/trusted/gascand".to_owned(),
            daemon_start_identity: "start:42".to_owned(),
            release_version: env!("CARGO_PKG_VERSION").to_owned(),
            daemon_started_at: Some(prost_types::Timestamp {
                seconds: 1_785_264_100,
                nanos: 123_000_000,
            }),
            ..Default::default()
        };
        let mut status = gascan_proto::v1::DaemonStatusResponse {
            release_version: handshake.release_version.clone(),
            daemon_pid: handshake.daemon_pid,
            daemon_executable: handshake.daemon_executable.clone(),
            daemon_start_identity: handshake.daemon_start_identity.clone(),
            daemon_instance_token: handshake.daemon_instance_token.clone(),
            daemon_started_at: handshake.daemon_started_at,
            health: gascan_proto::v1::DaemonHealth::Healthy as i32,
        };
        assert!(super::status_confirms_handshake(&handshake, &status));
        status.daemon_instance_token = "22".repeat(32);
        assert!(!super::status_confirms_handshake(&handshake, &status));
        Ok(())
    }

    #[tokio::test]
    async fn daemon_spawner_uses_protected_cwd_environment_and_detached_stdin()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().canonicalize()?;
        let runtime = root.join("runtime");
        std::fs::create_dir(&runtime)?;
        std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700))?;
        let script = root.join("fixture-gascand");
        std::fs::write(
            &script,
            "#!/bin/sh\nif IFS= read -r ignored; then stdin_state=data; else stdin_state=eof; fi\nprintf '%s\\n%s\\n%s\\n%s\\n%s\\n' \"$PWD\" \"$GASCAN_DAEMON_INSTANCE_PATH\" \"$GASCAN_DAEMON_OWNER_TOKEN\" \"$GASCAN_CONTROLLER_STARTUP_PATH\" \"$stdin_state\" >&2\nprintf 'stdout-must-be-null\\n'\n",
        )?;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700))?;
        let diagnostic = root.join("daemon.stderr");
        let launch = DaemonLaunch {
            executable: script,
            current_dir: runtime.clone(),
            instance_path: runtime.join("daemon-instance.json"),
            owner_token: "test-owner".to_owned(),
            stderr_path: Some(diagnostic.clone()),
            startup_diagnostic_path: runtime.join("daemon-startup-error.json"),
        };

        DaemonSpawner::spawn(&TokioDaemonSpawner, &launch)?;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(1);
        let output = loop {
            let output = std::fs::read_to_string(&diagnostic).unwrap_or_default();
            if output.lines().count() >= 5 {
                break output;
            }
            if tokio::time::Instant::now() >= deadline {
                return Err("fixture daemon did not write diagnostics".into());
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        };
        let lines = output.lines().collect::<Vec<_>>();
        assert_eq!(lines[0], runtime.to_string_lossy());
        assert_eq!(lines[1], launch.instance_path.to_string_lossy());
        assert_eq!(lines[2], "test-owner");
        assert_eq!(lines[3], launch.startup_diagnostic_path.to_string_lossy());
        assert_eq!(lines[4], "eof");

        std::fs::write(&launch.startup_diagnostic_path, b"do-not-truncate")?;
        std::fs::set_permissions(
            &launch.startup_diagnostic_path,
            std::fs::Permissions::from_mode(0o644),
        )?;
        let error = DaemonSpawner::spawn(&TokioDaemonSpawner, &launch)
            .err()
            .ok_or("unsafe startup diagnostic was accepted")?;
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        assert_eq!(
            std::fs::read(&launch.startup_diagnostic_path)?,
            b"do-not-truncate"
        );

        std::fs::remove_file(&launch.startup_diagnostic_path)?;
        let target = runtime.join("foreign-startup-target");
        std::fs::write(&target, b"do-not-follow")?;
        std::os::unix::fs::symlink(&target, &launch.startup_diagnostic_path)?;
        assert!(DaemonSpawner::spawn(&TokioDaemonSpawner, &launch).is_err());
        assert_eq!(std::fs::read(target)?, b"do-not-follow");
        Ok(())
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn default_runtime_base_avoids_the_tmp_symlink() {
        assert_eq!(
            crate::daemon::default_runtime_base(),
            std::path::PathBuf::from("/private/tmp")
        );
    }

    #[test]
    fn rpc_errors_show_the_cause_when_the_daemon_sends_one() {
        let details = gascan_proto::error_detail::encode(
            gascan_proto::error_code::INVALID_MANIFEST,
            "unknown variant `kiener`, expected `workspace` or `root`",
        );
        let status = tonic::Status::with_details(
            tonic::Code::InvalidArgument,
            gascan_proto::error_code::INVALID_MANIFEST,
            tonic::codegen::Bytes::from(details),
        );
        let rendered = format!("{}", super::ClientError::Rpc(Box::new(status)));
        assert!(
            rendered.contains("unknown variant `kiener`"),
            "the cause must reach the operator: {rendered}"
        );
    }

    #[test]
    fn rpc_errors_expose_the_stable_code_and_daemon_cause_separately() {
        let details = gascan_proto::error_detail::encode(
            "resource_conflict",
            "resource conflict for port 3000: already reserved",
        );
        let error = super::ClientError::Rpc(Box::new(tonic::Status::with_details(
            tonic::Code::AlreadyExists,
            "resource_conflict",
            tonic::codegen::Bytes::from(details),
        )));
        assert_eq!(error.stable_code(), Some("resource_conflict"));
        assert_eq!(
            error.cause().as_deref(),
            Some("resource conflict for port 3000: already reserved")
        );
    }

    #[test]
    fn rpc_errors_expose_structured_storage_change_details()
    -> Result<(), Box<dyn std::error::Error>> {
        let message = "storage settings changed for tools (10GiB → 20GiB); run `gascan destroy --yes` and `gascan up` to recreate the sandbox";
        let changes = serde_json::json!({"changes":[{
            "volume":"tools",
            "recorded_bytes":10 * 1024_u64.pow(3),
            "requested_bytes":20 * 1024_u64.pow(3),
        }]});
        let encoded = gascan_proto::error_detail::encode_with_details(
            gascan_proto::error_code::STORAGE_CHANGE_REQUIRES_RECREATE,
            message,
            serde_json::to_vec(&changes)?.as_slice(),
        );
        let error = super::ClientError::Rpc(Box::new(tonic::Status::with_details(
            tonic::Code::FailedPrecondition,
            gascan_proto::error_code::STORAGE_CHANGE_REQUIRES_RECREATE,
            tonic::codegen::Bytes::from(encoded),
        )));
        assert_eq!(error.failure_details(), Some(changes));
        assert_eq!(error.to_string(), format!("error: {message}"));
        Ok(())
    }

    #[test]
    fn rpc_errors_fall_back_to_the_code_without_details() {
        let status = tonic::Status::invalid_argument(gascan_proto::error_code::INVALID_REQUEST);
        let rendered = format!("{}", super::ClientError::Rpc(Box::new(status)));
        assert_eq!(rendered, "daemon error: invalid_request");
    }

    #[test]
    fn malformed_details_never_panic_and_fall_back() {
        let status = tonic::Status::with_details(
            tonic::Code::InvalidArgument,
            gascan_proto::error_code::INVALID_REQUEST,
            tonic::codegen::Bytes::from_static(&[0x0a, 0x05]),
        );
        let rendered = format!("{}", super::ClientError::Rpc(Box::new(status)));
        assert_eq!(rendered, "daemon error: invalid_request");
    }
}
