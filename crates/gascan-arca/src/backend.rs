use async_trait::async_trait;
use gascan_core::runtime::{
    CreateFailure, CreateOutcome, CreateRequest, ExecRequest, ExecSession, RecreateRequest,
    RemoveRequest, RuntimeBackend, RuntimeCapabilities, RuntimeError, RuntimeResource,
    RuntimeSandbox,
};
use gascan_core::sandbox::SandboxId;
use gascan_engine_proto::v1;

use crate::{EngineTransport, TransportError, error, translate};

/// `RuntimeBackend` over Arca's engine contract.
///
/// Generic over its transport for the same reason `AppleBackend` is generic over
/// its command runner: the mapping is the part worth testing, and it is testable
/// without an engine only if something can stand in for one.
pub struct ArcaBackend<T> {
    transport: T,
}

impl<T> ArcaBackend<T> {
    pub const fn new(transport: T) -> Self {
        Self { transport }
    }

    /// Recovers the transport, so a test can assert on what was sent.
    pub fn into_transport(self) -> T {
        self.transport
    }
}

/// Unwraps an `Ack` response: success with nothing to say, or the engine's error.
fn ack(operation: &str, response: v1::AckResponse) -> Result<(), RuntimeError> {
    match response.outcome {
        Some(v1::ack_response::Outcome::Ok(_)) => Ok(()),
        Some(v1::ack_response::Outcome::Error(error)) => {
            Err(error::engine_error(operation, &error))
        }
        None => Err(translate::missing_outcome(operation)),
    }
}

/// Which create path answered, so the `Created` arm is validated by the
/// constructor that belongs to it.
///
/// `create` and `create_container` share a response type but not a contract: a
/// create must answer with the whole requested topology, while a recreate must
/// answer with the container and nothing else. `gascan-core` states that
/// difference as two constructors, and this enum is how one response handler
/// keeps both. The other two arms — a partial failure and an unset oneof — are
/// identical for both paths and stay shared.
enum CreatePath<'a> {
    Create(&'a CreateRequest),
    Recreate(&'a RecreateRequest),
}

impl CreatePath<'_> {
    /// The compiled request underneath, which the failure arms are stated over.
    fn request(&self) -> &CreateRequest {
        match self {
            Self::Create(request) => request,
            Self::Recreate(request) => request.create(),
        }
    }

    fn outcome(&self, created: Vec<RuntimeResource>) -> Result<CreateOutcome, RuntimeError> {
        match self {
            Self::Create(request) => CreateOutcome::new(request, created),
            Self::Recreate(request) => CreateOutcome::for_recreate(request, created),
        }
    }
}

/// Both create paths answer with the same response type, so they share this.
///
/// A resource that fails to map is a hard failure rather than a filtered
/// list: a malformed `Resource` has no identity this client can act on, so
/// it could not be removed even if it were reported, and the fact an
/// operator needs is that the engine sent something malformed.
fn create_outcome(
    path: CreatePath<'_>,
    operation: &str,
    response: v1::CreateResponse,
) -> Result<CreateOutcome, CreateFailure> {
    match response.outcome {
        Some(v1::create_response::Outcome::Created(created)) => {
            let resources = translate::runtime_resources(&created.created)
                .map_err(CreateFailure::from_source)?;
            path.outcome(resources).map_err(CreateFailure::from_source)
        }
        Some(v1::create_response::Outcome::Failed(failed)) => {
            let source = failed.error.as_ref().map_or_else(
                || translate::missing_outcome(operation),
                |error| error::engine_error(operation, error),
            );
            let resources = translate::runtime_resources(&failed.created)
                .map_err(CreateFailure::from_source)?;
            Err(CreateFailure::from_created_evidence(
                path.request(),
                resources,
                source,
            ))
        }
        None => Err(CreateFailure::from_source(translate::missing_outcome(
            operation,
        ))),
    }
}

#[async_trait]
impl<T: EngineTransport> RuntimeBackend for ArcaBackend<T> {
    async fn capabilities(&self) -> Result<RuntimeCapabilities, RuntimeError> {
        let response = self
            .transport
            .capabilities(v1::CapabilitiesRequest {})
            .await
            .map_err(TransportError::into_runtime_error)?;
        match response.outcome {
            Some(v1::capabilities_response::Outcome::Capabilities(capabilities)) => {
                translate::runtime_capabilities(&capabilities)
            }
            Some(v1::capabilities_response::Outcome::Error(error)) => {
                Err(error::engine_error("capabilities", &error))
            }
            None => Err(translate::missing_outcome("capabilities")),
        }
    }

    async fn inspect(&self, id: &SandboxId) -> Result<Option<RuntimeSandbox>, RuntimeError> {
        let response = self
            .transport
            .inspect(v1::InspectRequest {
                sandbox_id: id.to_string(),
            })
            .await
            .map_err(TransportError::into_runtime_error)?;
        match response.outcome {
            Some(v1::inspect_response::Outcome::Sandbox(sandbox)) => {
                translate::runtime_sandbox(&sandbox).map(Some)
            }
            Some(v1::inspect_response::Outcome::Absent(_)) => Ok(None),
            Some(v1::inspect_response::Outcome::Error(error)) => {
                Err(error::engine_error("inspect", &error))
            }
            None => Err(translate::missing_outcome("inspect")),
        }
    }

    async fn create(&self, request: CreateRequest) -> Result<CreateOutcome, CreateFailure> {
        let wire = translate::create_request(&request).map_err(CreateFailure::from_source)?;
        let response = self
            .transport
            .create(wire)
            .await
            .map_err(|error| CreateFailure::from_source(error.into_runtime_error()))?;
        create_outcome(CreatePath::Create(&request), "create", response)
    }

    async fn prepare_image(&self, image: &str) -> Result<(), RuntimeError> {
        let response = self
            .transport
            .prepare_image(v1::PrepareImageRequest {
                image: Some(translate::image_digest(image)?),
            })
            .await
            .map_err(TransportError::into_runtime_error)?;
        match response.outcome {
            Some(v1::prepare_image_response::Outcome::Ok(_)) => Ok(()),
            Some(v1::prepare_image_response::Outcome::Error(error)) => {
                Err(error::engine_error("prepare_image", &error))
            }
            None => Err(translate::missing_outcome("prepare_image")),
        }
    }

    async fn create_container(
        &self,
        request: RecreateRequest,
    ) -> Result<CreateOutcome, CreateFailure> {
        let wire =
            translate::create_container_request(&request).map_err(CreateFailure::from_source)?;
        let response = self
            .transport
            .create_container(wire)
            .await
            .map_err(|error| CreateFailure::from_source(error.into_runtime_error()))?;
        create_outcome(CreatePath::Recreate(&request), "create_container", response)
    }

    async fn start(&self, id: &SandboxId) -> Result<(), RuntimeError> {
        let response = self
            .transport
            .start(v1::StartRequest {
                sandbox_id: id.to_string(),
            })
            .await
            .map_err(TransportError::into_runtime_error)?;
        ack("start", response)
    }

    async fn stop(&self, id: &SandboxId) -> Result<(), RuntimeError> {
        let response = self
            .transport
            .stop(v1::StopRequest {
                sandbox_id: id.to_string(),
            })
            .await
            .map_err(TransportError::into_runtime_error)?;
        ack("stop", response)
    }

    async fn remove(&self, request: RemoveRequest) -> Result<(), RuntimeError> {
        let response = self
            .transport
            .remove(translate::remove_request(&request)?)
            .await
            .map_err(TransportError::into_runtime_error)?;
        ack("remove", response)
    }

    async fn exec(&self, _request: ExecRequest) -> Result<ExecSession, RuntimeError> {
        Err(RuntimeError::UnsupportedCapability {
            capability: "exec lands in the next task".to_owned(),
        })
    }

    async fn logs(
        &self,
        _id: &SandboxId,
        _since_millis: Option<i64>,
    ) -> Result<Vec<u8>, RuntimeError> {
        Err(RuntimeError::UnsupportedCapability {
            capability: "logs lands in the next task".to_owned(),
        })
    }

    async fn list_resources(&self) -> Result<Vec<RuntimeResource>, RuntimeError> {
        let response = self
            .transport
            .list_resources(v1::ListResourcesRequest {})
            .await
            .map_err(TransportError::into_runtime_error)?;
        match response.outcome {
            Some(v1::list_resources_response::Outcome::Resources(list)) => {
                translate::runtime_resources(&list.resources)
            }
            Some(v1::list_resources_response::Outcome::Error(error)) => {
                Err(error::engine_error("list_resources", &error))
            }
            None => Err(translate::missing_outcome("list_resources")),
        }
    }
}
