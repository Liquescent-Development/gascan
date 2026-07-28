use crate::daemon::DaemonPaths;
use gascan_proto::v1::gas_can_client::GasCanClient;
use gascan_proto::{API_MAJOR, API_MINOR, validate_transport_security};
use hyper_util::rt::TokioIo;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tonic::transport::{Channel, Endpoint};
use tower::service_fn;

const DAEMON_READY_TIMEOUT: Duration = Duration::from_secs(15);

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

pub struct Client {
    pub api: GasCanClient<Channel>,
}

impl Client {
    pub async fn daemon_attestation() -> Result<gascan_proto::v1::HandshakeResponse, ClientError> {
        let paths = DaemonPaths::for_user()?;
        let mut api = connect(paths.socket()).await?;
        Ok(api
            .handshake(gascan_proto::v1::HandshakeRequest {
                api_major: API_MAJOR,
                api_minor: API_MINOR,
                requested_capabilities: Vec::new(),
            })
            .await?
            .into_inner())
    }

    pub async fn connect_or_start() -> Result<Self, ClientError> {
        let paths = DaemonPaths::for_user()?;
        let socket = paths.socket().to_owned();
        let initial = tokio::time::timeout(Duration::from_millis(250), async {
            negotiate(connect(&socket).await?).await
        })
        .await;
        match initial {
            Ok(Ok(client)) => return Ok(client),
            Ok(Err(error @ ClientError::Api(_))) => return Err(error),
            Ok(Err(_)) | Err(_) => {}
        }
        let daemon = daemon_path()?;
        let (instance_path, owner_token) = daemon_launch_environment(
            &paths,
            std::env::var_os("GASCAN_DAEMON_INSTANCE_PATH").as_deref(),
            std::env::var_os("GASCAN_DAEMON_OWNER_TOKEN").as_deref(),
        )?;
        let mut command = tokio::process::Command::new(daemon);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .env("GASCAN_DAEMON_INSTANCE_PATH", instance_path)
            .env("GASCAN_DAEMON_OWNER_TOKEN", owner_token);
        if let Some(path) = std::env::var_os("GASCAN_DAEMON_STDERR_PATH") {
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
        let started_at = tokio::time::Instant::now();
        let deadline = tokio::time::Instant::now() + DAEMON_READY_TIMEOUT;
        let mut probes = 0_u64;
        loop {
            probes = probes.saturating_add(1);
            let result = tokio::time::timeout(Duration::from_millis(250), async {
                negotiate(connect(&socket).await?).await
            })
            .await
            .unwrap_or_else(|_| {
                Err(ClientError::Io(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "daemon readiness probe timed out",
                )))
            });
            match result {
                Ok(client) => return Ok(client),
                Err(error @ ClientError::Api(_)) => return Err(error),
                Err(error) if !startup_transient(&error) => return Err(error),
                Err(error) if tokio::time::Instant::now() >= deadline => {
                    return Err(ClientError::Io(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        format!(
                            "daemon readiness exhausted after {probes} probes in {:?}; last error: {error}",
                            started_at.elapsed()
                        ),
                    )));
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(25)).await,
            }
        }
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

fn daemon_path() -> Result<PathBuf, ClientError> {
    if let Some(path) = std::env::var_os("GASCAN_DAEMON") {
        return Ok(path.into());
    }
    let mut path = std::env::current_exe()?;
    path.set_file_name("gascand");
    Ok(path)
}

async fn connect(path: &Path) -> Result<GasCanClient<Channel>, ClientError> {
    let path = path.to_owned();
    let channel = Endpoint::from_static("http://[::]:50051")
        .connect_with_connector(service_fn(move |_| {
            let path = path.clone();
            async move {
                tokio::net::UnixStream::connect(path)
                    .await
                    .map(TokioIo::new)
            }
        }))
        .await?;
    Ok(GasCanClient::new(channel))
}

async fn negotiate(mut api: GasCanClient<Channel>) -> Result<Client, ClientError> {
    let requested_major = std::env::var("GASCAN_API_MAJOR")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(API_MAJOR);
    let response = api
        .handshake(gascan_proto::v1::HandshakeRequest {
            api_major: requested_major,
            api_minor: API_MINOR,
            requested_capabilities: Vec::new(),
        })
        .await?
        .into_inner();
    if let Some(rejection) = response.rejection {
        return Err(ClientError::Api(rejection.code));
    }
    if response.api_major != API_MAJOR {
        return Err(ClientError::Api("incompatible_api_major".to_owned()));
    }
    let security = response
        .transport_security
        .ok_or_else(|| ClientError::Api("missing_transport_security".to_owned()))?;
    validate_transport_security(&security)
        .map_err(|_| ClientError::Api("unsafe_transport_security".to_owned()))?;
    Ok(Client { api })
}

#[cfg(test)]
mod tests {
    use crate::daemon::DaemonPaths;

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
