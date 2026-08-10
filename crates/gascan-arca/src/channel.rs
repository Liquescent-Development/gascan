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
        let dialed = socket.clone();
        let channel = Endpoint::from_static("http://[::]:50051")
            .connect_with_connector(service_fn(move |_| {
                let socket = socket.clone();
                async move {
                    let stream = tokio::net::UnixStream::connect(socket).await?;
                    Ok::<_, std::io::Error>(TokioIo::new(stream))
                }
            }))
            .await
            .map_err(|error| {
                TransportError::rpc(
                    "connect",
                    format!("{}: {}", dialed.display(), source_chain(&error)),
                )
            })?;
        Ok(Self {
            client: SandboxEngineClient::new(channel),
        })
    }

    fn client(&self) -> SandboxEngineClient<Channel> {
        self.client.clone()
    }
}

/// Renders an error together with everything it was caused by.
///
/// `tonic::transport::Error` renders `Kind::Transport` as the fixed string
/// `transport error` (`tonic-0.12.3/src/transport/error.rs:52`), and that is
/// the kind every dial failure carries, so the `io::Error` that tells a missing
/// socket apart from a path that is not a socket is reachable only through
/// `source()`. `crates/gascan/src/client.rs` walks the same chain for the same
/// reason, in `definitely_inert_connect_error`.
fn source_chain(error: &(dyn std::error::Error + 'static)) -> String {
    let mut rendered = error.to_string();
    let mut last = rendered.clone();
    let mut cause = error.source();
    while let Some(error) = cause {
        let text = error.to_string();
        // A boxed source re-renders the error it wraps verbatim, so a chain can
        // repeat itself. Say each distinct cause once.
        if text != last {
            rendered.push_str(": ");
            rendered.push_str(&text);
            last = text;
        }
        cause = error.source();
    }
    rendered
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
                tokio::select! {
                    // Dropping the in-flight `message()` future is safe here
                    // for one reason and only that reason: this branch is
                    // reached when the last receiver is gone, so there is
                    // nobody left to deliver the message to and the RPC is
                    // being abandoned on purpose. Nothing is resumed after it.
                    () = from_engine.closed() => break,
                    message = streaming.message() => match message {
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
                    },
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
                tokio::select! {
                    // As in `exec`: dropping the in-flight `message()` future
                    // is safe because this branch means the receiver is gone
                    // and the task is terminating. It matters more here —
                    // `logs` has no half-close to prompt the engine, so
                    // without this an abandoned call holds the stream open for
                    // as long as the container stays quiet.
                    () = sender.closed() => break,
                    message = streaming.message() => match message {
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
                    },
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
