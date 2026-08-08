use async_trait::async_trait;
use gascan_core::runtime::RuntimeError;
use gascan_engine_proto::v1;
use thiserror::Error;
use tokio::sync::mpsc;

/// A transport fault: an unreachable engine, or a stream that broke.
///
/// The contract reserves gRPC status codes for exactly this and carries every
/// engine meaning in the response body, so engine semantics never arrive here.
#[derive(Debug, Error)]
#[error("{operation}: engine transport failure: {message}")]
pub struct TransportError {
    operation: String,
    message: String,
}

impl TransportError {
    pub fn rpc(operation: &str, message: impl Into<String>) -> Self {
        Self {
            operation: operation.to_owned(),
            message: message.into(),
        }
    }

    /// A transport fault is I/O against the engine, so it reports as
    /// `command_io` — the code the daemon's exec path already expects when a
    /// stream breaks.
    pub fn into_runtime_error(self) -> RuntimeError {
        RuntimeError::CommandIo {
            operation: self.operation,
            message: self.message,
        }
    }
}

/// A live bidirectional exec stream, already opened.
pub struct ExecStream {
    input: mpsc::Sender<v1::ExecClientFrame>,
    output: mpsc::Receiver<Result<v1::ExecServerFrame, TransportError>>,
}

impl ExecStream {
    pub const fn new(
        input: mpsc::Sender<v1::ExecClientFrame>,
        output: mpsc::Receiver<Result<v1::ExecServerFrame, TransportError>>,
    ) -> Self {
        Self { input, output }
    }

    /// Hands both halves to the pump task that owns the session.
    pub fn split(
        self,
    ) -> (
        mpsc::Sender<v1::ExecClientFrame>,
        mpsc::Receiver<Result<v1::ExecServerFrame, TransportError>>,
    ) {
        (self.input, self.output)
    }
}

/// A server-streaming log response.
pub struct LogsStream {
    chunks: mpsc::Receiver<Result<v1::LogsChunk, TransportError>>,
}

impl LogsStream {
    pub const fn new(chunks: mpsc::Receiver<Result<v1::LogsChunk, TransportError>>) -> Self {
        Self { chunks }
    }

    pub async fn recv(&mut self) -> Option<Result<v1::LogsChunk, TransportError>> {
        self.chunks.recv().await
    }
}

/// The engine, in wire types.
///
/// The seam is deliberately stated in the generated types rather than in Gas
/// Can's: a seam in core types would put the mapping below the fake, and the
/// mapping is the part with the bugs.
#[async_trait]
pub trait EngineTransport: Send + Sync {
    async fn capabilities(
        &self,
        request: v1::CapabilitiesRequest,
    ) -> Result<v1::CapabilitiesResponse, TransportError>;

    async fn inspect(
        &self,
        request: v1::InspectRequest,
    ) -> Result<v1::InspectResponse, TransportError>;

    async fn create(
        &self,
        request: v1::CreateRequest,
    ) -> Result<v1::CreateResponse, TransportError>;

    async fn prepare_image(
        &self,
        request: v1::PrepareImageRequest,
    ) -> Result<v1::PrepareImageResponse, TransportError>;

    async fn create_container(
        &self,
        request: v1::CreateContainerRequest,
    ) -> Result<v1::CreateResponse, TransportError>;

    async fn start(&self, request: v1::StartRequest) -> Result<v1::AckResponse, TransportError>;

    async fn stop(&self, request: v1::StopRequest) -> Result<v1::AckResponse, TransportError>;

    async fn remove(&self, request: v1::RemoveRequest) -> Result<v1::AckResponse, TransportError>;

    /// Opens an exec session.
    ///
    /// Takes the `ExecStart` payload, not a first frame: the contract requires
    /// exactly one `ExecStart` and requires it first, so building that frame
    /// here means no implementation of this trait can get it wrong.
    async fn exec(&self, start: v1::ExecStart) -> Result<ExecStream, TransportError>;

    async fn logs(&self, request: v1::LogsRequest) -> Result<LogsStream, TransportError>;

    async fn list_resources(
        &self,
        request: v1::ListResourcesRequest,
    ) -> Result<v1::ListResourcesResponse, TransportError>;
}
