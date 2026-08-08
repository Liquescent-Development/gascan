use async_trait::async_trait;
use gascan_engine_proto::v1;
use gascan_engine_proto::v1::sandbox_engine_client::SandboxEngineClient;
use hyper_util::rt::TokioIo;
use std::path::PathBuf;
use tokio::sync::mpsc;
use tonic::transport::{Channel, Endpoint};
use tower::service_fn;

use crate::{EngineTransport, ExecStream, LogsStream, TransportError};

/// `EngineTransport` over a real gRPC channel.
///
/// Thin on purpose: each unary method is one call and one error conversion, so
/// the part that no test can reach until an engine exists is almost entirely
/// `tonic`'s.
#[derive(Clone)]
pub struct ChannelTransport {
    client: SandboxEngineClient<Channel>,
}

impl ChannelTransport {
    /// Dials the engine over a Unix socket.
    ///
    /// The authority is a placeholder that the connector ignores, which is the
    /// same shape the daemon client already uses for its own socket.
    pub async fn connect(socket: PathBuf) -> Result<Self, TransportError> {
        let channel = Endpoint::from_static("http://[::]:50051")
            .connect_with_connector(service_fn(move |_| {
                let socket = socket.clone();
                async move {
                    let stream = tokio::net::UnixStream::connect(socket).await?;
                    Ok::<_, std::io::Error>(TokioIo::new(stream))
                }
            }))
            .await
            .map_err(|error| TransportError::rpc("connect", error.to_string()))?;
        Ok(Self {
            client: SandboxEngineClient::new(channel),
        })
    }

    fn client(&self) -> SandboxEngineClient<Channel> {
        self.client.clone()
    }
}

fn status(operation: &str, status: tonic::Status) -> TransportError {
    TransportError::rpc(
        operation,
        format!("{}: {}", status.code(), status.message()),
    )
}

#[async_trait]
impl EngineTransport for ChannelTransport {
    async fn capabilities(
        &self,
        request: v1::CapabilitiesRequest,
    ) -> Result<v1::CapabilitiesResponse, TransportError> {
        self.client()
            .capabilities(request)
            .await
            .map(tonic::Response::into_inner)
            .map_err(|error| status("capabilities", error))
    }

    async fn inspect(
        &self,
        request: v1::InspectRequest,
    ) -> Result<v1::InspectResponse, TransportError> {
        self.client()
            .inspect(request)
            .await
            .map(tonic::Response::into_inner)
            .map_err(|error| status("inspect", error))
    }

    async fn create(
        &self,
        request: v1::CreateRequest,
    ) -> Result<v1::CreateResponse, TransportError> {
        self.client()
            .create(request)
            .await
            .map(tonic::Response::into_inner)
            .map_err(|error| status("create", error))
    }

    async fn prepare_image(
        &self,
        request: v1::PrepareImageRequest,
    ) -> Result<v1::PrepareImageResponse, TransportError> {
        self.client()
            .prepare_image(request)
            .await
            .map(tonic::Response::into_inner)
            .map_err(|error| status("prepare_image", error))
    }

    async fn create_container(
        &self,
        request: v1::CreateContainerRequest,
    ) -> Result<v1::CreateResponse, TransportError> {
        self.client()
            .create_container(request)
            .await
            .map(tonic::Response::into_inner)
            .map_err(|error| status("create_container", error))
    }

    async fn start(&self, request: v1::StartRequest) -> Result<v1::AckResponse, TransportError> {
        self.client()
            .start(request)
            .await
            .map(tonic::Response::into_inner)
            .map_err(|error| status("start", error))
    }

    async fn stop(&self, request: v1::StopRequest) -> Result<v1::AckResponse, TransportError> {
        self.client()
            .stop(request)
            .await
            .map(tonic::Response::into_inner)
            .map_err(|error| status("stop", error))
    }

    async fn remove(&self, request: v1::RemoveRequest) -> Result<v1::AckResponse, TransportError> {
        self.client()
            .remove(request)
            .await
            .map(tonic::Response::into_inner)
            .map_err(|error| status("remove", error))
    }

    async fn exec(&self, start: v1::ExecStart) -> Result<ExecStream, TransportError> {
        let (to_engine, outbound) = mpsc::channel::<v1::ExecClientFrame>(16);
        to_engine
            .send(v1::ExecClientFrame {
                frame: Some(v1::exec_client_frame::Frame::Start(start)),
            })
            .await
            .map_err(|_| TransportError::rpc("exec", "the outbound stream closed immediately"))?;

        let mut streaming = self
            .client()
            .exec(tokio_stream::wrappers::ReceiverStream::new(outbound))
            .await
            .map_err(|error| status("exec", error))?
            .into_inner();

        let (from_engine, inbound) = mpsc::channel(32);
        tokio::spawn(async move {
            loop {
                match streaming.message().await {
                    Ok(Some(frame)) => {
                        if from_engine.send(Ok(frame)).await.is_err() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        let _ = from_engine.send(Err(status("exec", error))).await;
                        break;
                    }
                }
            }
        });

        Ok(ExecStream::new(to_engine, inbound))
    }

    async fn logs(&self, request: v1::LogsRequest) -> Result<LogsStream, TransportError> {
        let mut streaming = self
            .client()
            .logs(request)
            .await
            .map_err(|error| status("logs", error))?
            .into_inner();

        let (sender, receiver) = mpsc::channel(32);
        tokio::spawn(async move {
            loop {
                match streaming.message().await {
                    Ok(Some(chunk)) => {
                        if sender.send(Ok(chunk)).await.is_err() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        let _ = sender.send(Err(status("logs", error))).await;
                        break;
                    }
                }
            }
        });

        Ok(LogsStream::new(receiver))
    }

    async fn list_resources(
        &self,
        request: v1::ListResourcesRequest,
    ) -> Result<v1::ListResourcesResponse, TransportError> {
        self.client()
            .list_resources(request)
            .await
            .map(tonic::Response::into_inner)
            .map_err(|error| status("list_resources", error))
    }
}
