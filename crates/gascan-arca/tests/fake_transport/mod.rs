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

    pub fn ok_ack() -> v1::AckResponse {
        v1::AckResponse {
            outcome: Some(v1::ack_response::Outcome::Ok(v1::Ack {})),
        }
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

    async fn exec(&self, _start: v1::ExecStart) -> Result<ExecStream, TransportError> {
        Err(TransportError::rpc("exec", "this fake scripts no exec"))
    }

    async fn logs(&self, _request: v1::LogsRequest) -> Result<LogsStream, TransportError> {
        Err(TransportError::rpc("logs", "this fake scripts no logs"))
    }

    async fn list_resources(
        &self,
        _request: v1::ListResourcesRequest,
    ) -> Result<v1::ListResourcesResponse, TransportError> {
        self.record(Call::ListResources);
        Self::take(&self.list_resources, "list_resources")
    }
}

/// A policy-validated `CreateRequest`, which is the only kind that exists.
///
/// `CreateRequest`'s fields are `pub(crate)` to `gascan-core` and it derives no
/// `Deserialize`, so `PolicyCompiler` is the only construction path — there is
/// deliberately no fixture constructor. This mirrors `request_with_manifest` in
/// `gascan-apple/tests/backend_fake_runner.rs`, which solves the same problem the
/// same way. The `TempDir` must outlive the request: the compiled request names
/// its canonical root.
pub fn policy_request(name: &str) -> (tempfile::TempDir, gascan_core::runtime::CreateRequest) {
    use camino::Utf8Path;
    use gascan_core::manifest::Manifest;
    use gascan_core::policy::PolicyCompiler;
    use gascan_core::runtime::{NetworkIsolation, RuntimeCapabilities, RuntimeVersion};
    use gascan_core::sandbox::SandboxSpec;

    let root = tempfile::tempdir().expect("a temporary project root");
    let path = Utf8Path::from_path(root.path()).expect("a utf-8 temporary path");
    std::fs::write(
        path.join("gascan.toml"),
        "version = 1\nnetwork = 'networked'\n",
    )
    .expect("a manifest");
    let spec = SandboxSpec::from_root(name, path, Manifest::load(path).expect("a manifest"))
        .expect("a spec");
    let capabilities = RuntimeCapabilities {
        version: RuntimeVersion::new(1, 1, 0),
        bind_mounts: true,
        named_volumes: true,
        tty: true,
        signals: true,
        loopback_publish: true,
        resource_limits: true,
        offline: NetworkIsolation::Proven,
    };
    let request = PolicyCompiler::compile(spec, &capabilities).expect("a validated request");
    (root, request)
}
