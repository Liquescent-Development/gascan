use gascan_arca::{EngineTransport, ExecStream, LogsStream, TransportError};
use gascan_engine_proto::v1;

/// A transport that fails every call, which is enough to prove the trait is
/// object-safe in the shape the backend needs and that the error mapping holds.
struct Unreachable;

#[async_trait::async_trait]
impl EngineTransport for Unreachable {
    async fn capabilities(
        &self,
        _request: v1::CapabilitiesRequest,
    ) -> Result<v1::CapabilitiesResponse, TransportError> {
        Err(TransportError::rpc("capabilities", "connection refused"))
    }
    async fn inspect(
        &self,
        _request: v1::InspectRequest,
    ) -> Result<v1::InspectResponse, TransportError> {
        Err(TransportError::rpc("inspect", "connection refused"))
    }
    async fn create(
        &self,
        _request: v1::CreateRequest,
    ) -> Result<v1::CreateResponse, TransportError> {
        Err(TransportError::rpc("create", "connection refused"))
    }
    async fn prepare_image(
        &self,
        _request: v1::PrepareImageRequest,
    ) -> Result<v1::PrepareImageResponse, TransportError> {
        Err(TransportError::rpc("prepare_image", "connection refused"))
    }
    async fn create_container(
        &self,
        _request: v1::CreateContainerRequest,
    ) -> Result<v1::CreateResponse, TransportError> {
        Err(TransportError::rpc(
            "create_container",
            "connection refused",
        ))
    }
    async fn start(&self, _request: v1::StartRequest) -> Result<v1::AckResponse, TransportError> {
        Err(TransportError::rpc("start", "connection refused"))
    }
    async fn stop(&self, _request: v1::StopRequest) -> Result<v1::AckResponse, TransportError> {
        Err(TransportError::rpc("stop", "connection refused"))
    }
    async fn remove(&self, _request: v1::RemoveRequest) -> Result<v1::AckResponse, TransportError> {
        Err(TransportError::rpc("remove", "connection refused"))
    }
    async fn exec(&self, _start: v1::ExecStart) -> Result<ExecStream, TransportError> {
        Err(TransportError::rpc("exec", "connection refused"))
    }
    async fn logs(&self, _request: v1::LogsRequest) -> Result<LogsStream, TransportError> {
        Err(TransportError::rpc("logs", "connection refused"))
    }
    async fn list_resources(
        &self,
        _request: v1::ListResourcesRequest,
    ) -> Result<v1::ListResourcesResponse, TransportError> {
        Err(TransportError::rpc("list_resources", "connection refused"))
    }
}

#[tokio::test]
async fn a_transport_fault_becomes_command_io_naming_the_rpc() {
    let error = Unreachable
        .capabilities(v1::CapabilitiesRequest {})
        .await
        .expect_err("this transport always fails")
        .into_runtime_error();

    assert_eq!(
        error.code(),
        "command_io",
        "a transport fault is I/O, not engine semantics",
    );
    let rendered = error.to_string();
    assert!(
        rendered.contains("capabilities"),
        "must name the rpc: {rendered}"
    );
    assert!(
        rendered.contains("connection refused"),
        "must carry the cause: {rendered}"
    );
}

#[test]
fn the_trait_is_usable_behind_a_reference() {
    fn accepts<T: EngineTransport>(_transport: &T) {}
    accepts(&Unreachable);
}
