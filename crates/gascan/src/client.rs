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
    pub async fn connect_or_start() -> Result<Self, ClientError> {
        let socket = socket_path();
        if let Ok(client) = connect(&socket).await {
            return negotiate(client).await;
        }
        let daemon = daemon_path()?;
        let _child = tokio::process::Command::new(daemon)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            match connect(&socket).await {
                Ok(client) => return negotiate(client).await,
                Err(error) if tokio::time::Instant::now() >= deadline => return Err(error),
                Err(_) => tokio::time::sleep(Duration::from_millis(25)).await,
            }
        }
    }
}

fn socket_path() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(format!(
                "/tmp/gascan-{}",
                rustix::process::geteuid().as_raw()
            ))
        })
        .join("gascan/gascand.sock")
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
    let response = api
        .handshake(gascan_proto::v1::HandshakeRequest {
            api_major: API_MAJOR,
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
