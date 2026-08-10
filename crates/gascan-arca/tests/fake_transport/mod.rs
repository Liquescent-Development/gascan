use gascan_arca::{EngineTransport, ExecStream, LogsStream, TransportError};
use gascan_engine_proto::v1;
use std::sync::Mutex;

/// A scripted engine. Each field holds the response the matching RPC returns;
/// `calls` records what was asked, so a test can assert on the request the
/// mapping produced as well as on the answer it made of the response.
#[derive(Default)]
pub struct FakeEngine {
    pub capabilities: Mutex<Option<v1::CapabilitiesResponse>>,
    pub inspect: Mutex<Option<v1::InspectResponse>>,
    pub create: Mutex<Option<v1::CreateResponse>>,
    pub prepare_image: Mutex<Option<v1::PrepareImageResponse>>,
    pub ack: Mutex<Option<v1::AckResponse>>,
    pub list_resources: Mutex<Option<v1::ListResourcesResponse>>,
    /// Chunks the next `logs` call streams, in order.
    pub logs_chunks: Mutex<Vec<Result<v1::LogsChunk, TransportError>>>,
    /// Frames the next `exec` call streams back, in order.
    pub exec_frames: Mutex<Vec<Result<v1::ExecServerFrame, TransportError>>>,
    /// Frames the client sent, captured by the fake's pump.
    pub exec_sent: std::sync::Arc<Mutex<Vec<v1::ExecClientFrame>>>,
    /// Set when the client→engine stream closes, which is how cancellation is
    /// observable from the engine's side.
    pub exec_client_stream_closed: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub calls: Mutex<Vec<Call>>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Call {
    Capabilities,
    Inspect(v1::InspectRequest),
    Create(v1::CreateRequest),
    PrepareImage(v1::PrepareImageRequest),
    CreateContainer(v1::CreateContainerRequest),
    Start(v1::StartRequest),
    Stop(v1::StopRequest),
    Remove(v1::RemoveRequest),
    Logs(v1::LogsRequest),
    ListResources,
}

impl FakeEngine {
    pub fn record(&self, call: Call) {
        self.calls.lock().expect("test lock").push(call);
    }

    pub fn calls(&self) -> Vec<Call> {
        self.calls.lock().expect("test lock").clone()
    }

    fn take<T>(slot: &Mutex<Option<T>>, operation: &str) -> Result<T, TransportError> {
        slot.lock()
            .expect("test lock")
            .take()
            .ok_or_else(|| TransportError::rpc(operation, "the test scripted no response"))
    }

    pub fn engine_error(code: &str) -> v1::EngineError {
        v1::EngineError {
            code: code.to_owned(),
            resource: "code-a1b2c3d4e5f6".to_owned(),
            message: "the engine refused".to_owned(),
        }
    }
}

#[async_trait::async_trait]
impl EngineTransport for FakeEngine {
    async fn capabilities(
        &self,
        _request: v1::CapabilitiesRequest,
    ) -> Result<v1::CapabilitiesResponse, TransportError> {
        self.record(Call::Capabilities);
        Self::take(&self.capabilities, "capabilities")
    }

    async fn inspect(
        &self,
        request: v1::InspectRequest,
    ) -> Result<v1::InspectResponse, TransportError> {
        self.record(Call::Inspect(request));
        Self::take(&self.inspect, "inspect")
    }

    async fn create(
        &self,
        request: v1::CreateRequest,
    ) -> Result<v1::CreateResponse, TransportError> {
        self.record(Call::Create(request));
        Self::take(&self.create, "create")
    }

    async fn prepare_image(
        &self,
        request: v1::PrepareImageRequest,
    ) -> Result<v1::PrepareImageResponse, TransportError> {
        self.record(Call::PrepareImage(request));
        Self::take(&self.prepare_image, "prepare_image")
    }

    async fn create_container(
        &self,
        request: v1::CreateContainerRequest,
    ) -> Result<v1::CreateResponse, TransportError> {
        self.record(Call::CreateContainer(request));
        Self::take(&self.create, "create_container")
    }

    async fn start(&self, request: v1::StartRequest) -> Result<v1::AckResponse, TransportError> {
        self.record(Call::Start(request));
        Self::take(&self.ack, "start")
    }

    async fn stop(&self, request: v1::StopRequest) -> Result<v1::AckResponse, TransportError> {
        self.record(Call::Stop(request));
        Self::take(&self.ack, "stop")
    }

    async fn remove(&self, request: v1::RemoveRequest) -> Result<v1::AckResponse, TransportError> {
        self.record(Call::Remove(request));
        Self::take(&self.ack, "remove")
    }

    async fn exec(&self, start: v1::ExecStart) -> Result<ExecStream, TransportError> {
        self.exec_sent
            .lock()
            .expect("test lock")
            .push(v1::ExecClientFrame {
                frame: Some(v1::exec_client_frame::Frame::Start(start)),
            });

        let frames = std::mem::take(&mut *self.exec_frames.lock().expect("test lock"));
        let (server, from_server) = tokio::sync::mpsc::channel(frames.len().max(1));
        for frame in frames {
            server.send(frame).await.expect("the receiver is alive");
        }

        let (to_server, mut client_frames) = tokio::sync::mpsc::channel(16);
        let sent = std::sync::Arc::clone(&self.exec_sent);
        let closed = std::sync::Arc::clone(&self.exec_client_stream_closed);
        tokio::spawn(async move {
            // Load-bearing, despite looking like a formality. The engine holds
            // its half open for as long as the session lives, because a real
            // engine does not half-close merely because it has nothing to say
            // yet. Letting `server` drop with the fake's `exec` would end the
            // server stream immediately, which the pump reads as the session
            // ending — a cancellation-independent way out of its select.
            //
            // Two things break if this line goes. Every scripted-frame test
            // becomes a race between the server's frames and the consumer's
            // input. And `cancelling_a_held_session_closes_the_stream_to_the_engine`
            // stops isolating anything: it would still pass, but through that
            // other exit, so it would no longer notice a backend that ignored
            // cancellation entirely. Its leading `!closed` assertion does not
            // guard against this — that one races.
            let _server = server;
            while let Some(frame) = client_frames.recv().await {
                sent.lock().expect("test lock").push(frame);
            }
            // The pump dropped its sender. From the engine's side that is what
            // cancellation looks like, so it is the only observable this fake needs.
            closed.store(true, std::sync::atomic::Ordering::SeqCst);
        });

        Ok(ExecStream::new(to_server, from_server))
    }

    async fn logs(&self, request: v1::LogsRequest) -> Result<LogsStream, TransportError> {
        self.record(Call::Logs(request));
        let chunks = std::mem::take(&mut *self.logs_chunks.lock().expect("test lock"));
        let (sender, receiver) = tokio::sync::mpsc::channel(chunks.len().max(1));
        for chunk in chunks {
            sender.send(chunk).await.expect("the receiver is alive");
        }
        Ok(LogsStream::new(receiver))
    }

    async fn list_resources(
        &self,
        _request: v1::ListResourcesRequest,
    ) -> Result<v1::ListResourcesResponse, TransportError> {
        self.record(Call::ListResources);
        Self::take(&self.list_resources, "list_resources")
    }
}
