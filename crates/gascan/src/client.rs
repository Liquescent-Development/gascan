use gascan_proto::v1::gas_can_client::GasCanClient;
use gascan_proto::{API_MAJOR, API_MINOR, validate_transport_security};
use hyper_util::rt::TokioIo;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tonic::transport::{Channel, Endpoint};
use tower::service_fn;

#[derive(Debug)]
pub enum ClientError {
    Io(std::io::Error),
    Transport(tonic::transport::Error),
    Rpc(Box<tonic::Status>),
    Api(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "daemon I/O error: {error}"),
            Self::Transport(error) => write!(formatter, "daemon transport error: {error}"),
            Self::Rpc(error) => write!(formatter, "daemon error: {}", error.message()),
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
        let mut api = connect(&socket_path()).await?;
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
        let socket = socket_path();
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
        let mut command = tokio::process::Command::new(daemon);
        command.stdin(Stdio::null()).stdout(Stdio::null());
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
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
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

fn socket_path() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| default_runtime_base(rustix::process::geteuid().as_raw()))
        .join("gascan/gascand.sock")
}

#[cfg(target_os = "macos")]
fn default_runtime_base(uid: u32) -> PathBuf {
    PathBuf::from(format!("/private/tmp/gascan-{uid}"))
}

#[cfg(not(target_os = "macos"))]
fn default_runtime_base(uid: u32) -> PathBuf {
    PathBuf::from(format!("/tmp/gascan-{uid}"))
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
    #[test]
    #[cfg(target_os = "macos")]
    fn default_runtime_base_avoids_the_tmp_symlink() {
        assert_eq!(
            super::default_runtime_base(501),
            std::path::PathBuf::from("/private/tmp/gascan-501")
        );
    }
}
