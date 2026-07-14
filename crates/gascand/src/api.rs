use crate::{
    ActualState, DesiredState, OperationEvent as StoredEvent, OperationStatus as StoredStatus,
    SandboxService, ServiceError, SocketPaths, UpRequest as ServiceUpRequest,
};
use camino::Utf8PathBuf;
use gascan_core::manifest::Manifest;
use gascan_core::runtime::RuntimeBackend;
use gascan_core::sandbox::{SandboxId, SandboxSpec};
use gascan_proto::v1;
use gascan_proto::v1::gas_can_server::{GasCan, GasCanServer};
use gascan_proto::{
    API_MAJOR, API_MINOR, error_code, local_transport_security, validate_api_major,
};
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::sync::Notify;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::UnixListenerStream;

#[derive(Clone, Debug)]
pub struct ActivityTracker {
    inner: Arc<ActivityInner>,
}
#[derive(Debug)]
struct ActivityInner {
    leases: AtomicUsize,
    operations: AtomicUsize,
    generation: AtomicUsize,
    changed: Notify,
}

impl Default for ActivityTracker {
    fn default() -> Self {
        Self::new()
    }
}
impl ActivityTracker {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(ActivityInner {
                leases: AtomicUsize::new(0),
                operations: AtomicUsize::new(0),
                generation: AtomicUsize::new(0),
                changed: Notify::new(),
            }),
        }
    }
    #[must_use]
    pub fn lease(&self) -> ActivityLease {
        self.inner.leases.fetch_add(1, Ordering::AcqRel);
        self.touch();
        ActivityLease {
            tracker: self.clone(),
        }
    }
    pub fn operation_started(&self) {
        self.inner.operations.fetch_add(1, Ordering::AcqRel);
        self.touch();
    }
    pub fn operation_finished(&self) {
        decrement(&self.inner.operations);
        self.touch();
    }
    #[must_use]
    pub fn operation(&self) -> OperationLease {
        self.operation_started();
        OperationLease {
            tracker: self.clone(),
        }
    }
    fn touch(&self) {
        self.inner.generation.fetch_add(1, Ordering::AcqRel);
        self.inner.changed.notify_waiters();
    }
    fn idle(&self) -> bool {
        self.inner.leases.load(Ordering::Acquire) == 0
            && self.inner.operations.load(Ordering::Acquire) == 0
    }
    pub async fn wait_for_idle(&self, timeout: Duration) -> io::Result<()> {
        loop {
            loop {
                let changed = self.inner.changed.notified();
                if self.idle() {
                    break;
                }
                changed.await;
            }
            let generation = self.inner.generation.load(Ordering::Acquire);
            tokio::select! {
                () = tokio::time::sleep(timeout) => {
                    if self.idle() && self.inner.generation.load(Ordering::Acquire) == generation { return Ok(()); }
                }
                () = self.inner.changed.notified() => {}
            }
        }
    }
}

fn decrement(value: &AtomicUsize) {
    let _ = value.fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
        current.checked_sub(1)
    });
}

#[derive(Debug)]
pub struct ActivityLease {
    tracker: ActivityTracker,
}
impl Drop for ActivityLease {
    fn drop(&mut self) {
        decrement(&self.tracker.inner.leases);
        self.tracker.touch();
    }
}

#[derive(Debug)]
pub struct OperationLease {
    tracker: ActivityTracker,
}
impl Drop for OperationLease {
    fn drop(&mut self) {
        self.tracker.operation_finished();
    }
}

#[derive(Clone, Debug)]
pub struct DaemonConfig {
    pub paths: SocketPaths,
    pub idle_timeout: Duration,
    activity: ActivityTracker,
}
impl DaemonConfig {
    #[must_use]
    pub fn new(paths: SocketPaths, idle_timeout: Duration) -> Self {
        Self {
            paths,
            idle_timeout,
            activity: ActivityTracker::new(),
        }
    }
    #[must_use]
    pub fn activity(&self) -> ActivityTracker {
        self.activity.clone()
    }
}

pub struct Daemon;
impl Daemon {
    pub async fn serve_idle(config: DaemonConfig) -> io::Result<()> {
        let socket = config.paths.bind()?;
        let tracker = ActivityTracker::new();
        tracker.wait_for_idle(config.idle_timeout).await?;
        drop(socket);
        Ok(())
    }

    pub async fn serve<T>(config: DaemonConfig, service: T) -> io::Result<()>
    where
        T: GasCan,
    {
        let owned = config.paths.bind()?;
        owned.set_nonblocking(true)?;
        let listener = tokio::net::UnixListener::from_std(owned.try_clone()?)?;
        let expected_uid = crate::PeerUid::current();
        let incoming =
            UnixListenerStream::new(listener).filter_map(move |accepted| match accepted {
                Ok(stream) => match stream.peer_cred() {
                    Ok(credentials)
                        if crate::validate_peer_uid(
                            crate::PeerUid::new(credentials.uid()),
                            expected_uid,
                        )
                        .is_ok() =>
                    {
                        Some(Ok(stream))
                    }
                    Ok(_) => None,
                    Err(error) => Some(Err(error)),
                },
                Err(error) => Some(Err(error)),
            });
        let tracker = config.activity();
        let idle = tracker.wait_for_idle(config.idle_timeout);
        #[cfg(unix)]
        let terminated = async {
            let mut signal =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
            let _ = signal.recv().await;
            Ok::<(), io::Error>(())
        };
        #[cfg(not(unix))]
        let terminated = std::future::pending::<io::Result<()>>();
        let shutdown = async move {
            tokio::select! {
                result = idle => { let _ = result; }
                result = terminated => { let _ = result; }
            }
        };
        tonic::transport::Server::builder()
            .add_service(GasCanServer::new(service))
            .serve_with_incoming_shutdown(incoming, shutdown)
            .await
            .map_err(io::Error::other)?;
        drop(owned);
        Ok(())
    }
}

/// Minimal v1 endpoint preserving handshake and local-transport contracts.
#[derive(Clone, Debug)]
pub struct LocalApi {
    activity: ActivityTracker,
}
impl LocalApi {
    #[must_use]
    pub fn new(activity: ActivityTracker) -> Self {
        Self { activity }
    }
    fn unavailable() -> tonic::Status {
        tonic::Status::unimplemented(error_code::BACKEND_UNAVAILABLE)
    }
}

/// Tonic adapter for the durable sandbox lifecycle service.
pub struct SandboxApi<B: RuntimeBackend> {
    service: Arc<SandboxService<B>>,
    activity: ActivityTracker,
}
impl<B: RuntimeBackend> SandboxApi<B> {
    #[must_use]
    pub fn new(service: Arc<SandboxService<B>>, activity: ActivityTracker) -> Self {
        Self { service, activity }
    }
}

pub struct ApiEventStream {
    inner: tokio_stream::wrappers::ReceiverStream<StoredEvent>,
    _lease: ActivityLease,
}
impl tokio_stream::Stream for ApiEventStream {
    type Item = Result<v1::OperationEvent, tonic::Status>;
    #[allow(
        clippy::result_large_err,
        reason = "the generated Tonic stream contract requires tonic::Status"
    )]
    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        Pin::new(&mut this.inner)
            .poll_next(context)
            .map(|item| item.map(|event| Ok(wire_event(event))))
    }
}

fn wire_event(event: StoredEvent) -> v1::OperationEvent {
    let details = event.details.unwrap_or_default();
    let phase = details
        .get("phase")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("operation")
        .to_owned();
    let payload = serde_json::to_vec(&details).unwrap_or_default();
    let status = match event.status {
        StoredStatus::Pending => v1::OperationStatus::Pending,
        StoredStatus::Completed => v1::OperationStatus::Completed,
        StoredStatus::Failed => v1::OperationStatus::Failed,
    } as i32;
    let error = (event.status == StoredStatus::Failed).then(|| v1::Error {
        code: error_code::INTERNAL.to_owned(),
        message: details
            .get("message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("operation failed")
            .to_owned(),
        details: payload.clone(),
    });
    v1::OperationEvent {
        operation_id: Some(v1::OperationId {
            value: event.operation_id.get() as u64,
        }),
        timestamp: None,
        phase,
        payload,
        error,
        sequence: event.sequence as u64,
        status,
        content_type: "application/json".to_owned(),
        session_token: Vec::new(),
    }
}

fn wire_status(record: crate::SandboxRecord) -> v1::SandboxStatus {
    let desired_state = match record.desired_state {
        DesiredState::Running => v1::DesiredState::Running,
        DesiredState::Stopped => v1::DesiredState::Stopped,
        DesiredState::Absent => v1::DesiredState::Absent,
    } as i32;
    let actual_state = match record.actual_state {
        ActualState::Creating | ActualState::Destroying => v1::ActualState::Pending,
        ActualState::Running => v1::ActualState::Running,
        ActualState::Stopped => v1::ActualState::Stopped,
        ActualState::Absent => v1::ActualState::Absent,
    } as i32;
    v1::SandboxStatus {
        sandbox_id: record.id.to_string(),
        desired_state,
        actual_state,
        last_operation_id: None,
        updated_at: None,
        capabilities: Vec::new(),
    }
}

#[derive(Clone, Copy)]
enum ApiInputError {
    Invalid,
    Internal,
}
impl ApiInputError {
    fn status(self) -> tonic::Status {
        match self {
            Self::Invalid => tonic::Status::invalid_argument(error_code::INVALID_REQUEST),
            Self::Internal => tonic::Status::internal(error_code::INTERNAL),
        }
    }
}

fn selector_id(selector: Option<v1::SandboxSelector>) -> Result<SandboxId, ApiInputError> {
    let value = selector.ok_or(ApiInputError::Invalid)?.sandbox_id;
    SandboxId::try_from(value).map_err(|_| ApiInputError::Invalid)
}

fn service_status(error: ServiceError) -> tonic::Status {
    match error {
        ServiceError::Missing(_) => tonic::Status::not_found(error_code::SANDBOX_NOT_FOUND),
        ServiceError::Runtime(_) => tonic::Status::unavailable(error_code::BACKEND_UNAVAILABLE),
        ServiceError::Policy(_) | ServiceError::Sandbox(_) | ServiceError::Manifest(_) => {
            tonic::Status::invalid_argument(error_code::INVALID_REQUEST)
        }
        _ => tonic::Status::internal(error_code::INTERNAL),
    }
}

async fn spec_for_root(project_root: String) -> Result<SandboxSpec, ApiInputError> {
    if project_root.is_empty() {
        return Err(ApiInputError::Invalid);
    }
    let root = Utf8PathBuf::from(project_root);
    if !root.is_absolute() {
        return Err(ApiInputError::Invalid);
    }
    tokio::task::spawn_blocking(move || {
        let manifest = Manifest::load(&root).map_err(|_| ApiInputError::Invalid)?;
        let name = manifest
            .name()
            .map(ToOwned::to_owned)
            .or_else(|| root.file_name().map(ToOwned::to_owned))
            .ok_or(ApiInputError::Invalid)?;
        SandboxSpec::from_root(&name, &root, manifest).map_err(|_| ApiInputError::Invalid)
    })
    .await
    .map_err(|_| ApiInputError::Internal)?
}

type EventStream = tokio_stream::Empty<Result<v1::OperationEvent, tonic::Status>>;
type AttachStream = tokio_stream::Empty<Result<v1::ServerFrame, tonic::Status>>;

#[tonic::async_trait]
impl GasCan for LocalApi {
    async fn handshake(
        &self,
        request: tonic::Request<v1::HandshakeRequest>,
    ) -> Result<tonic::Response<v1::HandshakeResponse>, tonic::Status> {
        let _lease = self.activity.lease();
        let request = request.into_inner();
        let rejection = validate_api_major(request.api_major)
            .err()
            .map(|error| v1::Error {
                code: error.code().to_owned(),
                message: format!(
                    "API major {} is unsupported; expected {API_MAJOR}",
                    request.api_major
                ),
                details: Vec::new(),
            });
        Ok(tonic::Response::new(v1::HandshakeResponse {
            api_major: API_MAJOR,
            api_minor: API_MINOR,
            capabilities: Vec::new(),
            transport_security: Some(local_transport_security()),
            rejection,
        }))
    }
    async fn status(
        &self,
        _: tonic::Request<v1::StatusRequest>,
    ) -> Result<tonic::Response<v1::StatusResponse>, tonic::Status> {
        let _lease = self.activity.lease();
        Err(Self::unavailable())
    }
    async fn list(
        &self,
        _: tonic::Request<v1::ListRequest>,
    ) -> Result<tonic::Response<v1::ListResponse>, tonic::Status> {
        let _lease = self.activity.lease();
        Err(Self::unavailable())
    }
    async fn doctor(
        &self,
        _: tonic::Request<v1::DoctorRequest>,
    ) -> Result<tonic::Response<v1::DoctorResponse>, tonic::Status> {
        let _lease = self.activity.lease();
        Err(Self::unavailable())
    }
    type UpStream = EventStream;
    async fn up(
        &self,
        _: tonic::Request<v1::UpRequest>,
    ) -> Result<tonic::Response<Self::UpStream>, tonic::Status> {
        let _lease = self.activity.lease();
        Err(Self::unavailable())
    }
    type ApplyStream = EventStream;
    async fn apply(
        &self,
        _: tonic::Request<v1::ApplyRequest>,
    ) -> Result<tonic::Response<Self::ApplyStream>, tonic::Status> {
        let _lease = self.activity.lease();
        Err(Self::unavailable())
    }
    type RunStream = EventStream;
    async fn run(
        &self,
        _: tonic::Request<v1::RunRequest>,
    ) -> Result<tonic::Response<Self::RunStream>, tonic::Status> {
        let _lease = self.activity.lease();
        Err(Self::unavailable())
    }
    type ShellStream = EventStream;
    async fn shell(
        &self,
        _: tonic::Request<v1::ShellRequest>,
    ) -> Result<tonic::Response<Self::ShellStream>, tonic::Status> {
        let _lease = self.activity.lease();
        Err(Self::unavailable())
    }
    type DownStream = EventStream;
    async fn down(
        &self,
        _: tonic::Request<v1::DownRequest>,
    ) -> Result<tonic::Response<Self::DownStream>, tonic::Status> {
        let _lease = self.activity.lease();
        Err(Self::unavailable())
    }
    type DestroyStream = EventStream;
    async fn destroy(
        &self,
        _: tonic::Request<v1::DestroyRequest>,
    ) -> Result<tonic::Response<Self::DestroyStream>, tonic::Status> {
        let _lease = self.activity.lease();
        Err(Self::unavailable())
    }
    type LogsStream = EventStream;
    async fn logs(
        &self,
        _: tonic::Request<v1::LogsRequest>,
    ) -> Result<tonic::Response<Self::LogsStream>, tonic::Status> {
        let _lease = self.activity.lease();
        Err(Self::unavailable())
    }
    type AttachStream = AttachStream;
    async fn attach(
        &self,
        _: tonic::Request<tonic::Streaming<v1::ClientFrame>>,
    ) -> Result<tonic::Response<Self::AttachStream>, tonic::Status> {
        let _lease = self.activity.lease();
        Err(Self::unavailable())
    }
}

#[tonic::async_trait]
impl<B: RuntimeBackend + 'static> GasCan for SandboxApi<B> {
    async fn handshake(
        &self,
        request: tonic::Request<v1::HandshakeRequest>,
    ) -> Result<tonic::Response<v1::HandshakeResponse>, tonic::Status> {
        let _lease = self.activity.lease();
        let request = request.into_inner();
        let rejection = validate_api_major(request.api_major)
            .err()
            .map(|error| v1::Error {
                code: error.code().to_owned(),
                message: format!(
                    "API major {} is unsupported; expected {API_MAJOR}",
                    request.api_major
                ),
                details: Vec::new(),
            });
        Ok(tonic::Response::new(v1::HandshakeResponse {
            api_major: API_MAJOR,
            api_minor: API_MINOR,
            capabilities: Vec::new(),
            transport_security: Some(local_transport_security()),
            rejection,
        }))
    }
    async fn status(
        &self,
        request: tonic::Request<v1::StatusRequest>,
    ) -> Result<tonic::Response<v1::StatusResponse>, tonic::Status> {
        let _lease = self.activity.lease();
        let id = selector_id(request.into_inner().sandbox).map_err(ApiInputError::status)?;
        let store = self.service.store().clone();
        let record = tokio::task::spawn_blocking(move || store.sandbox(&id))
            .await
            .map_err(|_| tonic::Status::internal(error_code::INTERNAL))?
            .map_err(|_| tonic::Status::internal(error_code::INTERNAL))?
            .ok_or_else(|| tonic::Status::not_found(error_code::SANDBOX_NOT_FOUND))?;
        Ok(tonic::Response::new(v1::StatusResponse {
            sandbox: Some(wire_status(record)),
        }))
    }
    async fn list(
        &self,
        _: tonic::Request<v1::ListRequest>,
    ) -> Result<tonic::Response<v1::ListResponse>, tonic::Status> {
        let _lease = self.activity.lease();
        let store = self.service.store().clone();
        let records = tokio::task::spawn_blocking(move || store.list_sandboxes())
            .await
            .map_err(|_| tonic::Status::internal(error_code::INTERNAL))?
            .map_err(|_| tonic::Status::internal(error_code::INTERNAL))?;
        Ok(tonic::Response::new(v1::ListResponse {
            sandboxes: records.into_iter().map(wire_status).collect(),
        }))
    }
    async fn doctor(
        &self,
        _: tonic::Request<v1::DoctorRequest>,
    ) -> Result<tonic::Response<v1::DoctorResponse>, tonic::Status> {
        let _lease = self.activity.lease();
        Ok(tonic::Response::new(v1::DoctorResponse {
            capabilities: Vec::new(),
            findings: Vec::new(),
        }))
    }
    type UpStream = ApiEventStream;
    async fn up(
        &self,
        request: tonic::Request<v1::UpRequest>,
    ) -> Result<tonic::Response<Self::UpStream>, tonic::Status> {
        let _operation = self.activity.operation();
        let spec = spec_for_root(request.into_inner().project_root)
            .await
            .map_err(ApiInputError::status)?;
        let operation = self
            .service
            .up(ServiceUpRequest::new(spec))
            .await
            .map_err(service_status)?;
        Ok(tonic::Response::new(ApiEventStream {
            inner: tokio_stream::wrappers::ReceiverStream::new(operation.events),
            _lease: self.activity.lease(),
        }))
    }
    type ApplyStream = ApiEventStream;
    async fn apply(
        &self,
        request: tonic::Request<v1::ApplyRequest>,
    ) -> Result<tonic::Response<Self::ApplyStream>, tonic::Status> {
        let _operation = self.activity.operation();
        let spec = spec_for_root(request.into_inner().project_root)
            .await
            .map_err(ApiInputError::status)?;
        let operation = self
            .service
            .apply(ServiceUpRequest::new(spec))
            .await
            .map_err(service_status)?;
        Ok(tonic::Response::new(ApiEventStream {
            inner: tokio_stream::wrappers::ReceiverStream::new(operation.events),
            _lease: self.activity.lease(),
        }))
    }
    type RunStream = EventStream;
    async fn run(
        &self,
        _: tonic::Request<v1::RunRequest>,
    ) -> Result<tonic::Response<Self::RunStream>, tonic::Status> {
        let _lease = self.activity.lease();
        Err(LocalApi::unavailable())
    }
    type ShellStream = EventStream;
    async fn shell(
        &self,
        _: tonic::Request<v1::ShellRequest>,
    ) -> Result<tonic::Response<Self::ShellStream>, tonic::Status> {
        let _lease = self.activity.lease();
        Err(LocalApi::unavailable())
    }
    type DownStream = ApiEventStream;
    async fn down(
        &self,
        request: tonic::Request<v1::DownRequest>,
    ) -> Result<tonic::Response<Self::DownStream>, tonic::Status> {
        let _operation = self.activity.operation();
        let id = selector_id(request.into_inner().sandbox).map_err(ApiInputError::status)?;
        let operation = self.service.stop(&id).await.map_err(service_status)?;
        Ok(tonic::Response::new(ApiEventStream {
            inner: tokio_stream::wrappers::ReceiverStream::new(operation.events),
            _lease: self.activity.lease(),
        }))
    }
    type DestroyStream = ApiEventStream;
    async fn destroy(
        &self,
        request: tonic::Request<v1::DestroyRequest>,
    ) -> Result<tonic::Response<Self::DestroyStream>, tonic::Status> {
        let _operation = self.activity.operation();
        let id = selector_id(request.into_inner().sandbox).map_err(ApiInputError::status)?;
        let operation = self.service.destroy(&id).await.map_err(service_status)?;
        Ok(tonic::Response::new(ApiEventStream {
            inner: tokio_stream::wrappers::ReceiverStream::new(operation.events),
            _lease: self.activity.lease(),
        }))
    }
    type LogsStream = EventStream;
    async fn logs(
        &self,
        _: tonic::Request<v1::LogsRequest>,
    ) -> Result<tonic::Response<Self::LogsStream>, tonic::Status> {
        let _lease = self.activity.lease();
        Err(LocalApi::unavailable())
    }
    type AttachStream = AttachStream;
    async fn attach(
        &self,
        _: tonic::Request<tonic::Streaming<v1::ClientFrame>>,
    ) -> Result<tonic::Response<Self::AttachStream>, tonic::Status> {
        let _lease = self.activity.lease();
        Err(LocalApi::unavailable())
    }
}

#[cfg(test)]
mod tests {
    use super::{ApiInputError, spec_for_root};

    #[tokio::test]
    async fn project_roots_are_absolute_and_manifest_bound()
    -> Result<(), Box<dyn std::error::Error>> {
        assert!(matches!(
            spec_for_root(String::new()).await,
            Err(ApiInputError::Invalid)
        ));
        assert!(matches!(
            spec_for_root("relative".to_owned()).await,
            Err(ApiInputError::Invalid)
        ));
        let directory = tempfile::tempdir()?;
        let root = directory
            .path()
            .to_str()
            .ok_or("non-UTF-8 fixture")?
            .to_owned();
        let spec = spec_for_root(root)
            .await
            .map_err(|_| std::io::Error::other("default manifest rejected"))?;
        assert_eq!(
            spec.canonical_root().as_std_path(),
            directory.path().canonicalize()?
        );
        Ok(())
    }
}
