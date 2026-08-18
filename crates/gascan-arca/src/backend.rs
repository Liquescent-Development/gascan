use async_trait::async_trait;
use gascan_core::runtime::{
    CreateFailure, CreateOutcome, CreateRequest, ExecCancellation, ExecInput, ExecOutput,
    ExecRequest, ExecSession, RecreateRequest, RemoveRequest, RuntimeBackend, RuntimeCapabilities,
    RuntimeError, RuntimeResource, RuntimeSandbox,
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
            let resources = translate::runtime_resources(operation, &created.created)
                .map_err(CreateFailure::from_source)?;
            path.outcome(resources).map_err(CreateFailure::from_source)
        }
        Some(v1::create_response::Outcome::Failed(failed)) => {
            let source = failed.error.as_ref().map_or_else(
                || translate::missing_outcome(operation),
                |error| error::engine_error(operation, error),
            );
            let resources = translate::runtime_resources(operation, &failed.created)
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

/// What the engine says about itself, alongside what it can do.
///
/// `RuntimeCapabilities` deliberately carries no revision: the certification
/// gate resolves inside this crate and the revision never needs to leave it for
/// normal operation. `gascan doctor` is the exception -- it has to EXPLAIN why
/// offline sandboxes are refused, and "this engine build is not the certified
/// one" is unsayable without the value. So it is exposed here, once, for that
/// purpose, rather than widening the type every consumer uses.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EngineReport {
    pub capabilities: RuntimeCapabilities,
    pub build_revision: String,
}

/// The engine build whose network isolation has been observed.
///
/// `None` until the offline evidence exists. Exposed so `gascan doctor` can
/// name the revision it is comparing against instead of reporting an
/// unexplained refusal.
#[must_use]
pub const fn certified_engine_revision() -> Option<&'static str> {
    translate::CERTIFIED_ENGINE_REVISION
}

impl<T: EngineTransport> ArcaBackend<T> {
    /// Capabilities and the engine's own revision, from ONE call.
    ///
    /// One call and not two, because the two answers must describe the same
    /// engine: separate calls could straddle an engine restart and report a
    /// revision that has nothing to do with the flags beside it.
    pub async fn engine_report(&self) -> Result<EngineReport, RuntimeError> {
        let response = self
            .transport
            .capabilities(v1::CapabilitiesRequest {})
            .await
            .map_err(TransportError::into_runtime_error)?;
        match response.outcome {
            Some(v1::capabilities_response::Outcome::Capabilities(capabilities)) => {
                Ok(EngineReport {
                    capabilities: translate::runtime_capabilities(&capabilities)?,
                    build_revision: capabilities.build_revision,
                })
            }
            Some(v1::capabilities_response::Outcome::Error(error)) => {
                Err(error::engine_error("capabilities", &error))
            }
            None => Err(translate::missing_outcome("capabilities")),
        }
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

    /// Opens a session and pumps it until it ends.
    ///
    /// The initial `stdin` buffer is sent only when non-empty, and no `Close` is
    /// forged: the consumer sends `ExecInput::Close` when it means to. Both
    /// match the Apple backend, so a caller cannot tell the two apart by their
    /// framing.
    async fn exec(&self, request: ExecRequest) -> Result<ExecSession, RuntimeError> {
        let initial_stdin = request.stdin.clone();
        let stream = self
            .transport
            .exec(translate::exec_start(&request))
            .await
            .map_err(TransportError::into_runtime_error)?;
        let (to_engine, mut from_engine) = stream.split();

        let (input, mut inputs) = tokio::sync::mpsc::channel(16);
        let (outputs, output) = tokio::sync::mpsc::channel(32);
        let (cancellation, mut cancelled) = ExecCancellation::channel();

        tokio::spawn(async move {
            if !initial_stdin.is_empty() {
                let frame = v1::ExecClientFrame {
                    frame: Some(v1::exec_client_frame::Frame::Stdin(initial_stdin)),
                };
                tokio::select! {
                    result = to_engine.send(frame) => {
                        if result.is_err() {
                            let _ = outputs
                                .send(Err(RuntimeError::CommandIo {
                                    operation: "exec_input".to_owned(),
                                    message: "the engine closed the stream".to_owned(),
                                }))
                                .await;
                            return;
                        }
                    }
                    result = cancelled.changed() => {
                        if result.is_ok() && *cancelled.borrow() { return; }
                    }
                }
            }

            loop {
                tokio::select! {
                    result = cancelled.changed() => {
                        if result.is_ok() && *cancelled.borrow() { break; }
                    }
                    next = inputs.recv() => {
                        let Some(next) = next else { break };
                        let frame = v1::ExecClientFrame {
                            frame: Some(match next {
                                ExecInput::Stdin(bytes) => {
                                    v1::exec_client_frame::Frame::Stdin(bytes)
                                }
                                ExecInput::Resize { columns, rows } => {
                                    v1::exec_client_frame::Frame::Resize(v1::Resize {
                                        columns,
                                        rows,
                                    })
                                }
                                ExecInput::Signal(signal) => {
                                    v1::exec_client_frame::Frame::Signal(signal)
                                }
                                ExecInput::Close => {
                                    v1::exec_client_frame::Frame::Close(v1::Close {})
                                }
                            }),
                        };
                        let delivered = tokio::select! {
                            result = to_engine.send(frame) => result.is_ok(),
                            result = cancelled.changed() => {
                                if result.is_ok() && *cancelled.borrow() { break; }
                                continue;
                            }
                        };
                        if !delivered {
                            let _ = outputs
                                .send(Err(RuntimeError::CommandIo {
                                    operation: "exec_input".to_owned(),
                                    message: "the engine closed the stream".to_owned(),
                                }))
                                .await;
                            break;
                        }
                    }
                    next = from_engine.recv() => {
                        let (mapped, terminal) = match next {
                            None => break,
                            Some(Err(error)) => (Err(error.into_runtime_error()), true),
                            Some(Ok(frame)) => match frame.frame {
                                Some(v1::exec_server_frame::Frame::Stdout(bytes)) => {
                                    (Ok(ExecOutput::Stdout(bytes)), false)
                                }
                                Some(v1::exec_server_frame::Frame::Stderr(bytes)) => {
                                    (Ok(ExecOutput::Stderr(bytes)), false)
                                }
                                Some(v1::exec_server_frame::Frame::Exit(exit)) => (
                                    Ok(ExecOutput::Exit {
                                        code: exit.code,
                                        signal: exit.signal,
                                    }),
                                    true,
                                ),
                                Some(v1::exec_server_frame::Frame::Error(error)) => {
                                    (Err(error::engine_error("exec", &error)), true)
                                }
                                None => (Err(translate::missing_outcome("exec")), true),
                            },
                        };
                        let delivered = tokio::select! {
                            result = outputs.send(mapped) => result.is_ok(),
                            result = cancelled.changed() => {
                                !(result.is_ok() && *cancelled.borrow())
                            }
                        };
                        if !delivered || terminal {
                            break;
                        }
                    }
                }
            }
        });

        Ok(ExecSession::live_cancellable(input, output, cancellation))
    }

    /// Concatenates the chunk stream into the one buffer the trait returns.
    ///
    /// A mid-stream error discards what arrived. The signature has no way to say
    /// "here is some of it, and also it broke", and returning a short log beside
    /// a swallowed error would make a truncated log indistinguishable from a
    /// complete one.
    async fn logs(
        &self,
        id: &SandboxId,
        since_millis: Option<i64>,
    ) -> Result<Vec<u8>, RuntimeError> {
        let mut stream = self
            .transport
            .logs(v1::LogsRequest {
                sandbox_id: id.to_string(),
                since_unix_millis: since_millis,
            })
            .await
            .map_err(TransportError::into_runtime_error)?;
        let mut buffer = Vec::new();
        while let Some(chunk) = stream.recv().await {
            match chunk.map_err(TransportError::into_runtime_error)?.outcome {
                Some(v1::logs_chunk::Outcome::Data(data)) => buffer.extend_from_slice(&data),
                Some(v1::logs_chunk::Outcome::Error(error)) => {
                    return Err(error::engine_error("logs", &error));
                }
                None => return Err(translate::missing_outcome("logs")),
            }
        }
        Ok(buffer)
    }

    async fn list_resources(&self) -> Result<Vec<RuntimeResource>, RuntimeError> {
        let response = self
            .transport
            .list_resources(v1::ListResourcesRequest {})
            .await
            .map_err(TransportError::into_runtime_error)?;
        match response.outcome {
            Some(v1::list_resources_response::Outcome::Resources(list)) => {
                translate::runtime_resources("list_resources", &list.resources)
            }
            Some(v1::list_resources_response::Outcome::Error(error)) => {
                Err(error::engine_error("list_resources", &error))
            }
            None => Err(translate::missing_outcome("list_resources")),
        }
    }
}
