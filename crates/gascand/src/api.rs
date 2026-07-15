use crate::service::PreBeginFailure;
use crate::{
    ActualState, DesiredState, OperationEvent as StoredEvent, OperationStatus as StoredStatus,
    SandboxService, ServiceError, SocketPaths, UpRequest as ServiceUpRequest,
};
use camino::Utf8PathBuf;
use gascan_core::doctor::DoctorStatus;
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
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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
    shutting_down: AtomicBool,
    shutdown: Notify,
    accepting: AtomicBool,
}
#[derive(Clone, Copy)]
struct AdmissionClosed;
fn admission_status(_: AdmissionClosed) -> tonic::Status {
    tonic::Status::unavailable(error_code::BACKEND_UNAVAILABLE)
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
                shutting_down: AtomicBool::new(false),
                shutdown: Notify::new(),
                accepting: AtomicBool::new(true),
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
    fn admit_operation(&self) -> Result<OperationLease, AdmissionClosed> {
        self.ensure_accepting()?;
        self.operation_started();
        if self.inner.accepting.load(Ordering::Acquire) {
            Ok(OperationLease {
                tracker: self.clone(),
            })
        } else {
            self.operation_finished();
            Err(AdmissionClosed)
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
    async fn wait_for_operations(&self) {
        loop {
            let changed = self.inner.changed.notified();
            if self.inner.operations.load(Ordering::Acquire) == 0 {
                return;
            }
            changed.await;
        }
    }
    fn cancel_streams(&self) {
        self.inner.shutting_down.store(true, Ordering::Release);
        self.inner.shutdown.notify_waiters();
    }
    fn begin_shutdown(&self) {
        self.inner.accepting.store(false, Ordering::Release);
    }
    fn ensure_accepting(&self) -> Result<(), AdmissionClosed> {
        if self.inner.accepting.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err(AdmissionClosed)
        }
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
    shutdown_timeout: Duration,
}
impl DaemonConfig {
    #[must_use]
    pub fn new(paths: SocketPaths, idle_timeout: Duration) -> Self {
        Self {
            paths,
            idle_timeout,
            activity: ActivityTracker::new(),
            shutdown_timeout: Duration::from_secs(2),
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
        #[cfg(unix)]
        let mut signal = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        #[cfg(unix)]
        let terminated = async move {
            let _ = signal.recv().await;
            Ok::<(), io::Error>(())
        };
        #[cfg(not(unix))]
        let terminated = std::future::pending::<io::Result<()>>();
        let owned = config.paths.bind()?;
        Self::serve_bound(config, service, owned, terminated).await
    }

    async fn serve_bound<T, F>(
        config: DaemonConfig,
        service: T,
        owned: crate::socket::OwnedSocket,
        terminated: F,
    ) -> io::Result<()>
    where
        T: GasCan,
        F: std::future::Future<Output = io::Result<()>>,
    {
        if let Some(pid_path) = std::env::var_os("GASCAN_PID_PATH") {
            std::fs::write(pid_path, std::process::id().to_string())?;
        }
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
                    Ok(_) | Err(_) => None,
                },
                Err(error) => Some(Err(error)),
            });
        let tracker = config.activity();
        let idle = tracker.wait_for_idle(config.idle_timeout);
        let (began_tx, began_rx) = tokio::sync::oneshot::channel();
        let shutdown_tracker = tracker.clone();
        let shutdown = async move {
            let reason = tokio::select! {
                result = idle => { let _ = result; "idle" }
                result = terminated => { let _ = result; "terminated" }
            };
            if std::env::var_os("GASCAN_DAEMON_STDERR_PATH").is_some() {
                eprintln!("daemon shutdown began: {reason}");
            }
            shutdown_tracker.begin_shutdown();
            let _ = began_tx.send(());
        };
        let server = tonic::transport::Server::builder()
            .add_service(GasCanServer::new(service))
            .serve_with_incoming_shutdown(incoming, shutdown);
        tokio::pin!(server);
        tokio::select! {
            result = &mut server => {
                if std::env::var_os("GASCAN_DAEMON_STDERR_PATH").is_some() {
                    eprintln!("daemon server ended: {result:?}");
                }
                result.map_err(io::Error::other)?;
            }
            _ = async { let _ = began_rx.await; } => {
                tracker.wait_for_operations().await;
                tracker.cancel_streams();
                tokio::time::timeout(config.shutdown_timeout, &mut server).await.map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "daemon connections did not close after stream cancellation"))?.map_err(io::Error::other)?;
            }
        }
        drop(owned);
        Ok(())
    }
}

/// Tonic adapter for the durable sandbox lifecycle service.
pub struct SandboxApi<B: RuntimeBackend> {
    service: Arc<SandboxService<B>>,
    activity: ActivityTracker,
    sessions: Arc<tokio::sync::Mutex<SessionRegistry>>,
}

#[derive(Debug)]
struct PendingSession {
    id: SandboxId,
    argv: Vec<String>,
    environment: std::collections::BTreeMap<String, String>,
    tty: bool,
    expires: tokio::time::Instant,
}

#[derive(Default)]
struct SessionRegistry {
    pending: std::collections::HashMap<Vec<u8>, PendingSession>,
    expired: std::collections::VecDeque<Vec<u8>>,
}

impl SessionRegistry {
    fn insert(&mut self, token: Vec<u8>, session: PendingSession) {
        self.prune();
        while self.pending.len() >= 1024 {
            if let Some(oldest) = self.pending.keys().next().cloned() {
                let _ = self.pending.remove(&oldest);
                self.remember_expired(oldest);
            } else {
                break;
            }
        }
        self.pending.insert(token, session);
    }
    fn prune(&mut self) {
        let now = tokio::time::Instant::now();
        let expired = self
            .pending
            .iter()
            .filter(|(_, session)| session.expires <= now)
            .map(|(token, _)| token.clone())
            .collect::<Vec<_>>();
        for token in expired {
            let _ = self.pending.remove(&token);
            self.remember_expired(token);
        }
    }
    fn claim(&mut self, token: &[u8]) -> Result<PendingSession, &'static str> {
        if let Some(session) = self.pending.remove(token) {
            if session.expires > tokio::time::Instant::now() {
                return Ok(session);
            }
            self.remember_expired(token.to_vec());
            return Err(error_code::EXPIRED_SESSION_TOKEN);
        }
        if self.expired.iter().any(|expired| expired == token) {
            Err(error_code::EXPIRED_SESSION_TOKEN)
        } else {
            Err(error_code::UNKNOWN_SESSION_TOKEN)
        }
    }
    fn remember_expired(&mut self, token: Vec<u8>) {
        if self.expired.len() == 1024 {
            let _ = self.expired.pop_front();
        }
        self.expired.push_back(token);
    }
}
impl<B: RuntimeBackend> SandboxApi<B> {
    #[must_use]
    pub fn new(service: Arc<SandboxService<B>>, activity: ActivityTracker) -> Self {
        let sessions = Arc::new(tokio::sync::Mutex::new(SessionRegistry::default()));
        let weak = Arc::downgrade(&sessions);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let Some(sessions) = weak.upgrade() else {
                    break;
                };
                sessions.lock().await.prune();
            }
        });
        Self {
            service,
            activity,
            sessions,
        }
    }
}

pub struct ApiEventStream {
    inner: tokio_stream::wrappers::ReceiverStream<Result<v1::OperationEvent, tonic::Status>>,
}
impl ApiEventStream {
    fn new(
        mut source: tokio::sync::mpsc::Receiver<StoredEvent>,
        activity: ActivityTracker,
    ) -> Self {
        let (sender, receiver) = tokio::sync::mpsc::channel(16);
        tokio::spawn(async move {
            let _lease = activity.lease();
            loop {
                let cancelled = activity.inner.shutdown.notified();
                if activity.inner.shutting_down.load(Ordering::Acquire) {
                    break;
                }
                tokio::select! {
                    event = source.recv() => match event {
                        Some(event) => {
                            let cancelled = activity.inner.shutdown.notified();
                            if activity.inner.shutting_down.load(Ordering::Acquire) { break; }
                            let permit = tokio::select! {
                                permit = sender.reserve() => match permit { Ok(permit) => permit, Err(_) => break },
                                () = cancelled => break,
                            };
                            permit.send(Ok(wire_event(event)));
                        },
                        None => break,
                    },
                    () = cancelled => break,
                }
            }
        });
        Self {
            inner: tokio_stream::wrappers::ReceiverStream::new(receiver),
        }
    }
}
impl tokio_stream::Stream for ApiEventStream {
    type Item = Result<v1::OperationEvent, tonic::Status>;
    #[allow(
        clippy::result_large_err,
        reason = "the generated Tonic stream contract requires tonic::Status"
    )]
    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        Pin::new(&mut this.inner).poll_next(context)
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
        code: event
            .error_code
            .unwrap_or_else(|| error_code::INTERNAL.to_owned()),
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
        timestamp: Some(timestamp_from_millis(event.timestamp_millis)),
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
        last_operation_id: record.last_operation_id.map(|id| v1::OperationId {
            value: id.get() as u64,
        }),
        updated_at: Some(timestamp_from_millis(record.updated_at_millis)),
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
        ServiceError::Store(crate::StoreError::PendingOperationExists { .. }) => {
            tonic::Status::already_exists(error_code::OPERATION_CONFLICT)
        }
        ServiceError::Missing(_) => tonic::Status::not_found(error_code::SANDBOX_NOT_FOUND),
        ServiceError::Runtime(_) => tonic::Status::unavailable(error_code::BACKEND_UNAVAILABLE),
        ServiceError::Policy(_) | ServiceError::Sandbox(_) | ServiceError::Manifest(_) => {
            tonic::Status::invalid_argument(error_code::INVALID_REQUEST)
        }
        _ => tonic::Status::internal(error_code::INTERNAL),
    }
}

fn pre_begin_status(error: PreBeginFailure) -> tonic::Status {
    match error {
        PreBeginFailure::Conflict => tonic::Status::already_exists(error_code::OPERATION_CONFLICT),
        PreBeginFailure::Missing => tonic::Status::not_found(error_code::SANDBOX_NOT_FOUND),
        PreBeginFailure::Runtime => tonic::Status::unavailable(error_code::BACKEND_UNAVAILABLE),
        PreBeginFailure::Invalid => tonic::Status::invalid_argument(error_code::INVALID_REQUEST),
        PreBeginFailure::Internal => tonic::Status::internal(error_code::INTERNAL),
    }
}

fn timestamp_from_millis(millis: i64) -> prost_types::Timestamp {
    prost_types::Timestamp {
        seconds: millis.div_euclid(1_000),
        nanos: (millis.rem_euclid(1_000) * 1_000_000) as i32,
    }
}

fn timestamp_millis(timestamp: prost_types::Timestamp) -> Result<i64, ApiInputError> {
    if !(0..1_000_000_000).contains(&timestamp.nanos) {
        return Err(ApiInputError::Invalid);
    }
    timestamp
        .seconds
        .checked_mul(1_000)
        .and_then(|seconds| seconds.checked_add(i64::from(timestamp.nanos / 1_000_000)))
        .ok_or(ApiInputError::Invalid)
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

type EventStream =
    tokio_stream::wrappers::ReceiverStream<Result<v1::OperationEvent, tonic::Status>>;
pub struct AttachStream {
    data: tokio::sync::mpsc::Receiver<Result<v1::ServerFrame, tonic::Status>>,
    terminal: tokio::sync::oneshot::Receiver<AttachTerminal>,
    pending_terminal: Option<AttachTerminal>,
    ended: bool,
}

struct AttachTerminal {
    frame: Result<v1::ServerFrame, tonic::Status>,
}

impl tokio_stream::Stream for AttachStream {
    type Item = Result<v1::ServerFrame, tonic::Status>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.ended {
            return Poll::Ready(None);
        }
        if self.pending_terminal.is_none() {
            match Pin::new(&mut self.terminal).poll(context) {
                Poll::Ready(Ok(terminal)) => self.pending_terminal = Some(terminal),
                Poll::Ready(Err(_)) => {
                    self.ended = true;
                    return Poll::Ready(None);
                }
                Poll::Pending => {}
            }
        }
        match Pin::new(&mut self.data).poll_recv(context) {
            Poll::Ready(None) => {
                self.ended = true;
                let terminal = self.pending_terminal.take();
                Poll::Ready(match terminal {
                    Some(terminal) => Some(terminal.frame),
                    None => None,
                })
            }
            other => other,
        }
    }
}
const ATTACH_CANCEL_GRACE: Duration = Duration::from_millis(250);

fn session_event(token: Vec<u8>) -> v1::OperationEvent {
    v1::OperationEvent {
        operation_id: None,
        timestamp: None,
        phase: "session_ready".to_owned(),
        payload: Vec::new(),
        error: None,
        sequence: 1,
        status: v1::OperationStatus::Completed as i32,
        content_type: String::new(),
        session_token: token,
    }
}

fn event_stream(event: v1::OperationEvent) -> EventStream {
    let (sender, receiver) = tokio::sync::mpsc::channel(1);
    let _ = sender.try_send(Ok(event));
    tokio_stream::wrappers::ReceiverStream::new(receiver)
}

fn argv_from_wire(argv: Vec<Vec<u8>>) -> Result<Vec<String>, ApiInputError> {
    if argv.is_empty() {
        return Err(ApiInputError::Invalid);
    }
    argv.into_iter()
        .map(|argument| String::from_utf8(argument).map_err(|_| ApiInputError::Invalid))
        .collect()
}

fn validated_environment(
    environment: Vec<v1::EnvironmentVariable>,
) -> Result<std::collections::BTreeMap<String, String>, ApiInputError> {
    let mut seen = std::collections::BTreeSet::new();
    for variable in &environment {
        if variable.name.is_empty()
            || variable.name.chars().any(char::is_control)
            || variable.value.contains('\0')
            || !seen.insert(variable.name.clone())
        {
            return Err(ApiInputError::Invalid);
        }
    }
    let filtered = gascan_core::policy::filtered_host_environment(
        environment
            .into_iter()
            .map(|variable| (variable.name, variable.value)),
    );
    if filtered.len() != seen.len() {
        return Err(ApiInputError::Invalid);
    }
    Ok(filtered)
}

fn exec_input(frame: v1::ClientFrame) -> Result<gascan_core::runtime::ExecInput, ApiInputError> {
    match frame.frame {
        Some(v1::client_frame::Frame::Stdin(bytes)) => {
            Ok(gascan_core::runtime::ExecInput::Stdin(bytes))
        }
        Some(v1::client_frame::Frame::Resize(size)) => {
            Ok(gascan_core::runtime::ExecInput::Resize {
                columns: size.columns,
                rows: size.rows,
            })
        }
        Some(v1::client_frame::Frame::Signal(signal)) => {
            Ok(gascan_core::runtime::ExecInput::Signal(signal.number))
        }
        Some(v1::client_frame::Frame::Close(_)) => Ok(gascan_core::runtime::ExecInput::Close),
        None => Err(ApiInputError::Invalid),
    }
}

fn server_output(output: gascan_core::runtime::ExecOutput) -> v1::ServerFrame {
    let frame = match output {
        gascan_core::runtime::ExecOutput::Stdout(bytes) => v1::server_frame::Frame::Stdout(bytes),
        gascan_core::runtime::ExecOutput::Stderr(bytes) => v1::server_frame::Frame::Stderr(bytes),
        gascan_core::runtime::ExecOutput::Exit { code, signal } => {
            v1::server_frame::Frame::Exit(v1::Exit { code, signal })
        }
    };
    v1::ServerFrame { frame: Some(frame) }
}

fn server_error(code: impl Into<String>, message: impl Into<String>) -> v1::ServerFrame {
    v1::ServerFrame {
        frame: Some(v1::server_frame::Frame::Error(v1::Error {
            code: code.into(),
            message: message.into(),
            details: Vec::new(),
        })),
    }
}

fn attach_runtime_error(message: impl Into<String>) -> gascan_core::runtime::RuntimeError {
    gascan_core::runtime::RuntimeError::CommandIo {
        operation: "attach_bridge".to_owned(),
        message: message.into(),
    }
}

async fn attach_shutdown_requested_with(activity: &ActivityTracker, registered: impl FnOnce()) {
    let notified = activity.inner.shutdown.notified();
    tokio::pin!(notified);
    registered();
    if !activity.inner.shutting_down.load(Ordering::Acquire) {
        notified.await;
    }
}

async fn attach_shutdown_requested(activity: &ActivityTracker) {
    attach_shutdown_requested_with(activity, || {}).await;
}

enum AttachDataSend {
    Sent,
    Disconnected,
    Shutdown,
}

async fn send_attach_frame(
    sender: &tokio::sync::mpsc::Sender<Result<v1::ServerFrame, tonic::Status>>,
    frame: v1::ServerFrame,
    activity: &ActivityTracker,
) -> AttachDataSend {
    tokio::select! {
        result = sender.send(Ok(frame)) => if result.is_ok() { AttachDataSend::Sent } else { AttachDataSend::Disconnected },
        () = sender.closed() => AttachDataSend::Disconnected,
        () = attach_shutdown_requested(activity) => AttachDataSend::Shutdown,
    }
}

async fn finish_attach_session(
    session: &mut gascan_core::runtime::ExecSession,
    data: &tokio::sync::mpsc::Sender<Result<v1::ServerFrame, tonic::Status>>,
    terminal: &mut Option<tokio::sync::oneshot::Sender<AttachTerminal>>,
    forward_terminal: bool,
) {
    let graceful = async {
        session.send(gascan_core::runtime::ExecInput::Close).await?;
        loop {
            match session.next().await {
                Some(Ok(output @ gascan_core::runtime::ExecOutput::Exit { .. })) => {
                    return Ok(server_output(output));
                }
                Some(Err(error)) => return Ok(server_error(error.code(), error.to_string())),
                Some(Ok(output)) => data
                    .send(Ok(server_output(output)))
                    .await
                    .map_err(|_| attach_runtime_error("attach response data stream closed"))?,
                None => {
                    return Err(attach_runtime_error(
                        "runtime session closed without a terminal output",
                    ));
                }
            }
        }
    };
    let terminal_frame = match tokio::time::timeout(ATTACH_CANCEL_GRACE, graceful).await {
        Ok(Ok(frame)) => frame,
        Ok(Err(error)) => server_error(error.code(), error.to_string()),
        Err(_) => {
            session.cancel();
            let error = attach_runtime_error(
                "runtime session required forced termination after attach cancellation grace expired",
            );
            server_error(error.code(), error.to_string())
        }
    };
    if forward_terminal {
        if let Some(sender) = terminal.take() {
            let _ = sender.send(AttachTerminal {
                frame: Ok(terminal_frame),
            });
        }
    }
}

fn send_terminal(
    terminal: &mut Option<tokio::sync::oneshot::Sender<AttachTerminal>>,
    frame: Result<v1::ServerFrame, tonic::Status>,
) {
    if let Some(sender) = terminal.take() {
        let _ = sender.send(AttachTerminal { frame });
    }
}

async fn run_attach_bridge<S>(
    mut session: gascan_core::runtime::ExecSession,
    mut input: S,
    first_input: gascan_core::runtime::ExecInput,
    mut binder: gascan_proto::AttachSessionBinder,
    sender: tokio::sync::mpsc::Sender<Result<v1::ServerFrame, tonic::Status>>,
    mut terminal: Option<tokio::sync::oneshot::Sender<AttachTerminal>>,
    activity: ActivityTracker,
) where
    S: tokio_stream::Stream<Item = Result<v1::ClientFrame, tonic::Status>> + Unpin,
{
    let _lease = activity.lease();
    let mut input_closed = matches!(first_input, gascan_core::runtime::ExecInput::Close);
    tokio::select! {
        result = session.send(first_input) => {
            if let Err(error) = result {
                send_terminal(&mut terminal, Ok(server_error(error.code(), error.to_string())));
                return;
            }
        }
        () = sender.closed() => {
            finish_attach_session(&mut session, &sender, &mut terminal, false).await;
            return;
        }
        () = attach_shutdown_requested(&activity) => {
            finish_attach_session(&mut session, &sender, &mut terminal, !sender.is_closed()).await;
            return;
        }
    }
    loop {
        tokio::select! {
            output = session.next() => match output {
                Some(Ok(output)) => {
                    if matches!(output, gascan_core::runtime::ExecOutput::Exit { .. }) {
                        send_terminal(&mut terminal, Ok(server_output(output)));
                        break;
                    }
                    match send_attach_frame(&sender, server_output(output), &activity).await {
                        AttachDataSend::Sent => {}
                        AttachDataSend::Disconnected => {
                            finish_attach_session(&mut session, &sender, &mut terminal, false).await;
                            break;
                        }
                        AttachDataSend::Shutdown => {
                            finish_attach_session(&mut session, &sender, &mut terminal, !sender.is_closed()).await;
                            break;
                        }
                    }
                }
                Some(Err(error)) => {
                    send_terminal(&mut terminal, Ok(server_error(error.code(), error.to_string())));
                    break;
                }
                None => {
                    let error = attach_runtime_error("runtime session closed without a terminal output");
                    send_terminal(&mut terminal, Ok(server_error(error.code(), error.to_string())));
                    break;
                }
            },
            frame = input.next(), if !input_closed => match frame {
                Some(Ok(frame)) => {
                    let input_frame = if let v1::ClientFrame { frame: Some(_), .. } = &frame {
                        match binder.validate_frame(&frame.session_token) {
                            Ok(()) => exec_input(frame),
                            Err(error) => {
                                let code = error.code();
                                finish_attach_session(&mut session, &sender, &mut terminal, false).await;
                                send_terminal(&mut terminal, Ok(server_error(code, "attach frame rejected")));
                                break;
                            },
                        }
                    } else { Err(ApiInputError::Invalid) };
                    match input_frame {
                        Ok(frame) => {
                            input_closed = matches!(frame, gascan_core::runtime::ExecInput::Close);
                            if let Err(error) = session.send(frame).await {
                                send_terminal(&mut terminal, Ok(server_error(error.code(), error.to_string())));
                                break;
                            }
                        }
                        Err(_) => {
                            finish_attach_session(&mut session, &sender, &mut terminal, false).await;
                            send_terminal(&mut terminal, Ok(server_error(error_code::INVALID_REQUEST, "attach frame rejected")));
                            break;
                        }
                    }
                }
                Some(Err(error)) => { send_terminal(&mut terminal, Err(error)); break; }
                None => {
                    finish_attach_session(&mut session, &sender, &mut terminal, !sender.is_closed()).await;
                    break;
                }
            },
            () = sender.closed() => {
                finish_attach_session(&mut session, &sender, &mut terminal, false).await;
                break;
            },
            () = attach_shutdown_requested(&activity) => {
                finish_attach_session(&mut session, &sender, &mut terminal, !sender.is_closed()).await;
                break;
            },
        }
    }
}

#[tonic::async_trait]
impl<B: RuntimeBackend + 'static> GasCan for SandboxApi<B> {
    async fn handshake(
        &self,
        request: tonic::Request<v1::HandshakeRequest>,
    ) -> Result<tonic::Response<v1::HandshakeResponse>, tonic::Status> {
        self.activity.ensure_accepting().map_err(admission_status)?;
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
        self.activity.ensure_accepting().map_err(admission_status)?;
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
        self.activity.ensure_accepting().map_err(admission_status)?;
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
        self.activity.ensure_accepting().map_err(admission_status)?;
        let _lease = self.activity.lease();
        let report = self.service.doctor_report();
        let capabilities = report
            .checks
            .iter()
            .map(|check| v1::Capability {
                name: check.id.clone(),
                available: check.status == DoctorStatus::Pass,
                detail: serde_json::json!({
                    "detail": check.detail,
                    "remedy": check.remedy,
                    "status": check.status,
                })
                .to_string(),
            })
            .collect();
        let findings = report
            .checks
            .iter()
            .filter(|check| check.status != DoctorStatus::Pass)
            .map(|check| v1::Error {
                code: check.id.clone(),
                message: check.detail.clone(),
                details: check.remedy.as_bytes().to_vec(),
            })
            .collect();
        Ok(tonic::Response::new(v1::DoctorResponse {
            capabilities,
            findings,
        }))
    }
    type UpStream = ApiEventStream;
    async fn up(
        &self,
        request: tonic::Request<v1::UpRequest>,
    ) -> Result<tonic::Response<Self::UpStream>, tonic::Status> {
        let operation_lease = self.activity.admit_operation().map_err(admission_status)?;
        self.service
            .require_runtime_ready()
            .await
            .map_err(service_status)?;
        let spec = spec_for_root(request.into_inner().project_root)
            .await
            .map_err(ApiInputError::status)?;
        let service = self.service.clone();
        let (started, mut operation) = tokio::sync::mpsc::channel(1);
        tokio::spawn(async move {
            let _operation_lease = operation_lease;
            if let Err(error) = service
                .up_started(ServiceUpRequest::new(spec), started.clone())
                .await
            {
                let _ = started.send(Err((&error).into())).await;
            }
        });
        let operation = operation
            .recv()
            .await
            .ok_or_else(|| tonic::Status::internal(error_code::INTERNAL))?
            .map_err(pre_begin_status)?;
        Ok(tonic::Response::new(ApiEventStream::new(
            operation.events,
            self.activity.clone(),
        )))
    }
    type ApplyStream = ApiEventStream;
    async fn apply(
        &self,
        request: tonic::Request<v1::ApplyRequest>,
    ) -> Result<tonic::Response<Self::ApplyStream>, tonic::Status> {
        let operation_lease = self.activity.admit_operation().map_err(admission_status)?;
        self.service
            .require_runtime_ready()
            .await
            .map_err(service_status)?;
        let spec = spec_for_root(request.into_inner().project_root)
            .await
            .map_err(ApiInputError::status)?;
        let service = self.service.clone();
        let (started, mut operation) = tokio::sync::mpsc::channel(1);
        tokio::spawn(async move {
            let _operation_lease = operation_lease;
            if let Err(error) = service
                .apply_started(ServiceUpRequest::new(spec), started.clone())
                .await
            {
                let _ = started.send(Err((&error).into())).await;
            }
        });
        let operation = operation
            .recv()
            .await
            .ok_or_else(|| tonic::Status::internal(error_code::INTERNAL))?
            .map_err(pre_begin_status)?;
        Ok(tonic::Response::new(ApiEventStream::new(
            operation.events,
            self.activity.clone(),
        )))
    }
    type RunStream = EventStream;
    async fn run(
        &self,
        request: tonic::Request<v1::RunRequest>,
    ) -> Result<tonic::Response<Self::RunStream>, tonic::Status> {
        self.activity.ensure_accepting().map_err(admission_status)?;
        let _lease = self.activity.lease();
        let request = request.into_inner();
        let id = selector_id(request.sandbox).map_err(ApiInputError::status)?;
        let command = request
            .command
            .ok_or_else(|| tonic::Status::invalid_argument(error_code::INVALID_REQUEST))?;
        self.service
            .validate_exec(&id)
            .await
            .map_err(service_status)?;
        let mut token = vec![0_u8; 24];
        getrandom::fill(&mut token).map_err(|_| tonic::Status::internal(error_code::INTERNAL))?;
        self.sessions.lock().await.insert(
            token.clone(),
            PendingSession {
                id,
                argv: argv_from_wire(command.argv).map_err(ApiInputError::status)?,
                environment: validated_environment(command.environment)
                    .map_err(ApiInputError::status)?,
                tty: command.tty,
                expires: tokio::time::Instant::now() + Duration::from_secs(30),
            },
        );
        Ok(tonic::Response::new(event_stream(session_event(token))))
    }
    type ShellStream = EventStream;
    async fn shell(
        &self,
        request: tonic::Request<v1::ShellRequest>,
    ) -> Result<tonic::Response<Self::ShellStream>, tonic::Status> {
        self.activity.ensure_accepting().map_err(admission_status)?;
        let _lease = self.activity.lease();
        let request = request.into_inner();
        let id = selector_id(request.sandbox).map_err(ApiInputError::status)?;
        let command = request.command.unwrap_or(v1::CommandPayload {
            argv: Vec::new(),
            environment: Default::default(),
            tty: true,
        });
        let argv = if command.argv.is_empty() {
            vec!["sh".to_owned()]
        } else {
            argv_from_wire(command.argv).map_err(ApiInputError::status)?
        };
        self.service
            .validate_exec(&id)
            .await
            .map_err(service_status)?;
        let mut token = vec![0_u8; 24];
        getrandom::fill(&mut token).map_err(|_| tonic::Status::internal(error_code::INTERNAL))?;
        self.sessions.lock().await.insert(
            token.clone(),
            PendingSession {
                id,
                argv,
                environment: validated_environment(command.environment)
                    .map_err(ApiInputError::status)?,
                tty: true,
                expires: tokio::time::Instant::now() + Duration::from_secs(30),
            },
        );
        Ok(tonic::Response::new(event_stream(session_event(token))))
    }
    type DownStream = ApiEventStream;
    async fn down(
        &self,
        request: tonic::Request<v1::DownRequest>,
    ) -> Result<tonic::Response<Self::DownStream>, tonic::Status> {
        let operation_lease = self.activity.admit_operation().map_err(admission_status)?;
        let id = selector_id(request.into_inner().sandbox).map_err(ApiInputError::status)?;
        let service = self.service.clone();
        let (started, mut operation) = tokio::sync::mpsc::channel(1);
        tokio::spawn(async move {
            let _operation_lease = operation_lease;
            if let Err(error) = service.stop_started(&id, started.clone()).await {
                let _ = started.send(Err((&error).into())).await;
            }
        });
        let operation = operation
            .recv()
            .await
            .ok_or_else(|| tonic::Status::internal(error_code::INTERNAL))?
            .map_err(pre_begin_status)?;
        Ok(tonic::Response::new(ApiEventStream::new(
            operation.events,
            self.activity.clone(),
        )))
    }
    type DestroyStream = ApiEventStream;
    async fn destroy(
        &self,
        request: tonic::Request<v1::DestroyRequest>,
    ) -> Result<tonic::Response<Self::DestroyStream>, tonic::Status> {
        let operation_lease = self.activity.admit_operation().map_err(admission_status)?;
        let id = selector_id(request.into_inner().sandbox).map_err(ApiInputError::status)?;
        let service = self.service.clone();
        let (started, mut operation) = tokio::sync::mpsc::channel(1);
        tokio::spawn(async move {
            let _operation_lease = operation_lease;
            if let Err(error) = service.destroy_started(&id, started.clone()).await {
                let _ = started.send(Err((&error).into())).await;
            }
        });
        let operation = operation
            .recv()
            .await
            .ok_or_else(|| tonic::Status::internal(error_code::INTERNAL))?
            .map_err(pre_begin_status)?;
        Ok(tonic::Response::new(ApiEventStream::new(
            operation.events,
            self.activity.clone(),
        )))
    }
    type LogsStream = EventStream;
    async fn logs(
        &self,
        request: tonic::Request<v1::LogsRequest>,
    ) -> Result<tonic::Response<Self::LogsStream>, tonic::Status> {
        self.activity.ensure_accepting().map_err(admission_status)?;
        let _lease = self.activity.lease();
        let request = request.into_inner();
        let since_millis = request
            .since
            .map(timestamp_millis)
            .transpose()
            .map_err(ApiInputError::status)?;
        let id = selector_id(request.sandbox).map_err(ApiInputError::status)?;
        let bytes = self
            .service
            .logs(&id, since_millis)
            .await
            .map_err(service_status)?;
        let baseline = bytes.clone();
        let mut event = v1::OperationEvent {
            operation_id: None,
            timestamp: None,
            phase: "logs".to_owned(),
            payload: bytes,
            error: None,
            sequence: 1,
            status: v1::OperationStatus::Completed as i32,
            content_type: "application/octet-stream".to_owned(),
            session_token: Vec::new(),
        };
        if !request.follow {
            return Ok(tonic::Response::new(event_stream(event)));
        }
        event.status = v1::OperationStatus::Pending as i32;
        let (sender, receiver) = tokio::sync::mpsc::channel(2);
        let activity = self.activity.clone();
        let service = self.service.clone();
        tokio::spawn(async move {
            let _lease = activity.lease();
            if sender.send(Ok(event)).await.is_err() {
                return;
            }
            let mut previous = baseline;
            let mut sequence = 1_u64;
            loop {
                tokio::select! {
                    () = activity.inner.shutdown.notified() => {
                        sequence = sequence.saturating_add(1);
                        let terminal = v1::OperationEvent { operation_id: None, timestamp: None, phase: "logs".to_owned(), payload: Vec::new(), error: None, sequence, status: v1::OperationStatus::Completed as i32, content_type: String::new(), session_token: Vec::new() };
                        let _ = sender.send(Ok(terminal)).await;
                        break;
                    },
                    () = sender.closed() => break,
                    () = tokio::time::sleep(Duration::from_millis(50)) => {
                        let current = match service.logs(&id, since_millis).await { Ok(bytes) => bytes, Err(error) => {
                            sequence = sequence.saturating_add(1);
                            let failed = v1::OperationEvent { operation_id: None, timestamp: None, phase: "logs".to_owned(), payload: Vec::new(), error: Some(v1::Error { code: error.code().to_owned(), message: error.to_string(), details: Vec::new() }), sequence, status: v1::OperationStatus::Failed as i32, content_type: String::new(), session_token: Vec::new() };
                            let _ = sender.send(Ok(failed)).await;
                            break;
                        } };
                        if let Some(appended) = current.strip_prefix(previous.as_slice()) {
                            if !appended.is_empty() {
                                sequence = sequence.saturating_add(1);
                                let event = v1::OperationEvent { operation_id: None, timestamp: None, phase: "logs".to_owned(), payload: appended.to_vec(), error: None, sequence, status: v1::OperationStatus::Pending as i32, content_type: "application/octet-stream".to_owned(), session_token: Vec::new() };
                                if sender.send(Ok(event)).await.is_err() { break; }
                            }
                        }
                        previous = current;
                    }
                }
            }
        });
        Ok(tonic::Response::new(
            tokio_stream::wrappers::ReceiverStream::new(receiver),
        ))
    }
    type AttachStream = AttachStream;
    async fn attach(
        &self,
        request: tonic::Request<tonic::Streaming<v1::ClientFrame>>,
    ) -> Result<tonic::Response<Self::AttachStream>, tonic::Status> {
        self.activity.ensure_accepting().map_err(admission_status)?;
        let _lease = self.activity.lease();
        let mut input = request.into_inner();
        let first = input
            .message()
            .await?
            .ok_or_else(|| tonic::Status::invalid_argument(error_code::EMPTY_SESSION_TOKEN))?;
        let mut binder = gascan_proto::AttachSessionBinder::new();
        binder
            .validate_frame(&first.session_token)
            .map_err(|error| tonic::Status::invalid_argument(error.code()))?;
        let token = first.session_token.clone();
        let first_input = exec_input(first).map_err(ApiInputError::status)?;
        let pending = self.sessions.lock().await.claim(&token).map_err(|code| {
            if code == error_code::EXPIRED_SESSION_TOKEN {
                tonic::Status::failed_precondition(code)
            } else {
                tonic::Status::not_found(code)
            }
        })?;
        let session = self
            .service
            .exec(
                &pending.id,
                pending.argv,
                Vec::new(),
                pending.environment,
                pending.tty,
            )
            .await
            .map_err(service_status)?;
        let (sender, receiver) = tokio::sync::mpsc::channel(16);
        let (terminal_sender, terminal) = tokio::sync::oneshot::channel();
        let activity = self.activity.clone();
        tokio::spawn(async move {
            run_attach_bridge(
                session,
                input,
                first_input,
                binder,
                sender,
                Some(terminal_sender),
                activity,
            )
            .await;
        });
        Ok(tonic::Response::new(AttachStream {
            data: receiver,
            terminal,
            pending_terminal: None,
            ended: false,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ActivityTracker, ApiEventStream, ApiInputError, AttachStream, AttachTerminal,
        PendingSession, SessionRegistry, attach_shutdown_requested_with,
        run_attach_bridge as run_attach_bridge_impl, service_status, spec_for_root, wire_event,
        wire_status,
    };
    use crate::{
        ActualState, DesiredState, OperationEvent, OperationId, OperationStatus, SandboxRecord,
        ServiceError, StoreError,
    };
    use camino::Utf8PathBuf;
    use gascan_core::{
        runtime::{ExecInput, ExecOutput, ExecSession},
        sandbox::SandboxId,
    };
    use gascan_proto::{error_code, v1};
    use serde_json::json;
    use tokio_stream::StreamExt;

    async fn run_attach_bridge<S>(
        session: ExecSession,
        input: S,
        first_input: ExecInput,
        binder: gascan_proto::AttachSessionBinder,
        sender: tokio::sync::mpsc::Sender<Result<v1::ServerFrame, tonic::Status>>,
        activity: ActivityTracker,
    ) where
        S: tokio_stream::Stream<Item = Result<v1::ClientFrame, tonic::Status>> + Unpin,
    {
        let (terminal_sender, terminal) = tokio::sync::oneshot::channel::<AttachTerminal>();
        let terminal_output = sender.clone();
        let forward = tokio::spawn(async move {
            if let Ok(terminal) = terminal.await {
                let _ = terminal_output.send(terminal.frame).await;
            }
        });
        run_attach_bridge_impl(
            session,
            input,
            first_input,
            binder,
            sender,
            Some(terminal_sender),
            activity,
        )
        .await;
        let _ = forward.await;
    }

    fn bound_binder(token: &[u8]) -> gascan_proto::AttachSessionBinder {
        let mut binder = gascan_proto::AttachSessionBinder::new();
        assert!(binder.validate_frame(token).is_ok());
        binder
    }

    #[tokio::test]
    async fn attach_eof_emits_one_stable_terminal_error() {
        let (input, _inputs) = tokio::sync::mpsc::channel(1);
        let (outputs, output) = tokio::sync::mpsc::channel(1);
        drop(outputs);
        let session = ExecSession::live(input, output);
        let (sender, mut receiver) = tokio::sync::mpsc::channel(4);
        run_attach_bridge(
            session,
            tokio_stream::empty(),
            ExecInput::Close,
            bound_binder(b"eof"),
            sender,
            ActivityTracker::new(),
        )
        .await;
        let frame = receiver
            .recv()
            .await
            .and_then(Result::ok)
            .and_then(|frame| frame.frame);
        assert!(
            matches!(frame, Some(v1::server_frame::Frame::Error(error)) if error.code == "command_io")
        );
        assert!(receiver.recv().await.is_none());
    }

    #[tokio::test]
    async fn attach_first_send_failure_emits_one_terminal_error() {
        let (input, inputs) = tokio::sync::mpsc::channel(1);
        drop(inputs);
        let (_outputs, output) = tokio::sync::mpsc::channel(1);
        let session = ExecSession::live(input, output);
        let (sender, mut receiver) = tokio::sync::mpsc::channel(4);
        run_attach_bridge(
            session,
            tokio_stream::empty(),
            ExecInput::Close,
            bound_binder(b"first-send"),
            sender,
            ActivityTracker::new(),
        )
        .await;
        assert!(matches!(
            receiver
                .recv()
                .await
                .and_then(Result::ok)
                .and_then(|frame| frame.frame),
            Some(v1::server_frame::Frame::Error(_))
        ));
        assert!(receiver.recv().await.is_none());
    }

    #[tokio::test]
    async fn attach_shutdown_forced_timeout_is_bounded_and_typed()
    -> Result<(), Box<dyn std::error::Error>> {
        let activity = ActivityTracker::new();
        let (input, _inputs) = tokio::sync::mpsc::channel(1);
        let (_outputs, output) = tokio::sync::mpsc::channel(1);
        let session = ExecSession::live(input, output);
        let (sender, mut receiver) = tokio::sync::mpsc::channel(4);
        let bridge = tokio::spawn(run_attach_bridge(
            session,
            tokio_stream::pending(),
            ExecInput::Stdin(Vec::new()),
            bound_binder(b"shutdown"),
            sender,
            activity.clone(),
        ));
        tokio::task::yield_now().await;
        activity.cancel_streams();
        tokio::time::timeout(std::time::Duration::from_secs(1), bridge).await??;
        let frame = receiver
            .recv()
            .await
            .and_then(Result::ok)
            .and_then(|frame| frame.frame);
        assert!(
            matches!(frame, Some(v1::server_frame::Frame::Error(error)) if error.code == "command_io")
        );
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            activity.wait_for_idle(std::time::Duration::from_millis(1)),
        )
        .await??;
        Ok(())
    }

    #[tokio::test]
    async fn forced_terminal_error_follows_full_data_queue()
    -> Result<(), Box<dyn std::error::Error>> {
        let activity = ActivityTracker::new();
        let (input, _inputs) = tokio::sync::mpsc::channel(1);
        let (_outputs, output) = tokio::sync::mpsc::channel(1);
        let (sender, receiver) = tokio::sync::mpsc::channel(2);
        sender
            .send(Ok(v1::ServerFrame {
                frame: Some(v1::server_frame::Frame::Stdout(vec![1])),
            }))
            .await?;
        sender
            .send(Ok(v1::ServerFrame {
                frame: Some(v1::server_frame::Frame::Stderr(vec![2])),
            }))
            .await?;
        let (terminal_sender, terminal) = tokio::sync::oneshot::channel();
        let bridge = tokio::spawn(run_attach_bridge_impl(
            ExecSession::live(input, output),
            tokio_stream::pending(),
            ExecInput::Stdin(Vec::new()),
            bound_binder(b"full-terminal"),
            sender,
            Some(terminal_sender),
            activity.clone(),
        ));
        activity.cancel_streams();
        tokio::time::timeout(std::time::Duration::from_secs(1), bridge).await??;
        let mut stream = AttachStream {
            data: receiver,
            terminal,
            pending_terminal: None,
            ended: false,
        };
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(1), stream.next())
                .await?
                .and_then(Result::ok)
                .and_then(|frame| frame.frame),
            Some(v1::server_frame::Frame::Stdout(vec![1]))
        );
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(1), stream.next())
                .await?
                .and_then(Result::ok)
                .and_then(|frame| frame.frame),
            Some(v1::server_frame::Frame::Stderr(vec![2]))
        );
        assert!(matches!(
            tokio::time::timeout(std::time::Duration::from_secs(1), stream.next())
                .await?
                .and_then(Result::ok)
                .and_then(|frame| frame.frame),
            Some(v1::server_frame::Frame::Error(error)) if error.code == "command_io"
        ));
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(1), stream.next())
                .await?
                .is_none()
        );
        Ok(())
    }

    #[tokio::test]
    async fn shutdown_notification_between_registration_and_atomic_check_is_observed() {
        let activity = ActivityTracker::new();
        attach_shutdown_requested_with(&activity, || activity.cancel_streams()).await;
    }

    #[tokio::test]
    async fn attach_shutdown_gracefully_closes_before_timeout()
    -> Result<(), Box<dyn std::error::Error>> {
        let activity = ActivityTracker::new();
        let (input, mut inputs) = tokio::sync::mpsc::channel(2);
        let (outputs, output) = tokio::sync::mpsc::channel(1);
        let session = ExecSession::live(input, output);
        let runtime = tokio::spawn(async move {
            assert!(matches!(inputs.recv().await, Some(ExecInput::Stdin(_))));
            assert_eq!(inputs.recv().await, Some(ExecInput::Close));
            let _ = outputs.send(Ok(ExecOutput::Stdout(vec![0, 255]))).await;
            let _ = outputs.send(Ok(ExecOutput::Stderr(vec![254, 1]))).await;
            let _ = outputs
                .send(Ok(ExecOutput::Exit { code: 0, signal: 0 }))
                .await;
        });
        let (sender, mut receiver) = tokio::sync::mpsc::channel(4);
        let bridge = tokio::spawn(run_attach_bridge(
            session,
            tokio_stream::pending(),
            ExecInput::Stdin(Vec::new()),
            bound_binder(b"graceful-shutdown"),
            sender,
            activity.clone(),
        ));
        tokio::task::yield_now().await;
        activity.cancel_streams();
        tokio::time::timeout(std::time::Duration::from_secs(1), bridge).await??;
        runtime.await?;
        assert_eq!(
            receiver
                .recv()
                .await
                .and_then(Result::ok)
                .and_then(|frame| frame.frame),
            Some(v1::server_frame::Frame::Stdout(vec![0, 255]))
        );
        assert_eq!(
            receiver
                .recv()
                .await
                .and_then(Result::ok)
                .and_then(|frame| frame.frame),
            Some(v1::server_frame::Frame::Stderr(vec![254, 1]))
        );
        assert!(matches!(
            receiver
                .recv()
                .await
                .and_then(Result::ok)
                .and_then(|frame| frame.frame),
            Some(v1::server_frame::Frame::Exit(v1::Exit {
                code: 0,
                signal: 0
            }))
        ));
        assert!(receiver.recv().await.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn attach_client_half_close_forwards_actual_exit()
    -> Result<(), Box<dyn std::error::Error>> {
        let (input, mut inputs) = tokio::sync::mpsc::channel(2);
        let (outputs, output) = tokio::sync::mpsc::channel(1);
        let runtime = tokio::spawn(async move {
            assert!(matches!(inputs.recv().await, Some(ExecInput::Stdin(_))));
            assert_eq!(inputs.recv().await, Some(ExecInput::Close));
            let _ = outputs.send(Ok(ExecOutput::Stdout(vec![0, 255]))).await;
            let _ = outputs.send(Ok(ExecOutput::Stderr(vec![254, 1]))).await;
            let _ = outputs
                .send(Ok(ExecOutput::Exit {
                    code: 42,
                    signal: 0,
                }))
                .await;
        });
        let (sender, mut receiver) = tokio::sync::mpsc::channel(4);
        run_attach_bridge(
            ExecSession::live(input, output),
            tokio_stream::empty(),
            ExecInput::Stdin(Vec::new()),
            bound_binder(b"half-close"),
            sender,
            ActivityTracker::new(),
        )
        .await;
        runtime.await?;
        assert_eq!(
            receiver
                .recv()
                .await
                .and_then(Result::ok)
                .and_then(|frame| frame.frame),
            Some(v1::server_frame::Frame::Stdout(vec![0, 255]))
        );
        assert_eq!(
            receiver
                .recv()
                .await
                .and_then(Result::ok)
                .and_then(|frame| frame.frame),
            Some(v1::server_frame::Frame::Stderr(vec![254, 1]))
        );
        assert!(matches!(
            receiver
                .recv()
                .await
                .and_then(Result::ok)
                .and_then(|frame| frame.frame),
            Some(v1::server_frame::Frame::Exit(v1::Exit {
                code: 42,
                signal: 0
            }))
        ));
        assert!(receiver.recv().await.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn attach_client_disconnect_cancellation_releases_activity_lease()
    -> Result<(), Box<dyn std::error::Error>> {
        let activity = ActivityTracker::new();
        let (input, _inputs) = tokio::sync::mpsc::channel(1);
        let (_outputs, output) = tokio::sync::mpsc::channel(1);
        let session = ExecSession::live(input, output);
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        let bridge = tokio::spawn(run_attach_bridge(
            session,
            tokio_stream::pending(),
            ExecInput::Stdin(Vec::new()),
            bound_binder(b"disconnect"),
            sender,
            activity.clone(),
        ));
        drop(receiver);
        tokio::time::timeout(std::time::Duration::from_secs(1), bridge).await??;
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            activity.wait_for_idle(std::time::Duration::from_millis(1)),
        )
        .await??;
        Ok(())
    }

    #[tokio::test]
    async fn continuous_output_does_not_starve_resize_input()
    -> Result<(), Box<dyn std::error::Error>> {
        let token = b"fair";
        let (input, mut inputs) = tokio::sync::mpsc::channel(4);
        let (outputs, output) = tokio::sync::mpsc::channel(4);
        let session = ExecSession::live(input, output);
        let (sender, mut receiver) = tokio::sync::mpsc::channel(4);
        let output_task = tokio::spawn(async move {
            while outputs.send(Ok(ExecOutput::Stdout(vec![1]))).await.is_ok() {}
        });
        let resize = v1::ClientFrame {
            frame: Some(v1::client_frame::Frame::Resize(v1::Resize {
                columns: 80,
                rows: 24,
            })),
            session_token: token.to_vec(),
        };
        let bridge = tokio::spawn(run_attach_bridge(
            session,
            tokio_stream::iter([Ok(resize)]).chain(tokio_stream::pending()),
            ExecInput::Stdin(Vec::new()),
            bound_binder(token),
            sender,
            ActivityTracker::new(),
        ));
        let drain = tokio::spawn(async move { while receiver.recv().await.is_some() {} });
        assert_eq!(inputs.recv().await, Some(ExecInput::Stdin(Vec::new())));
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(1), inputs.recv()).await?,
            Some(ExecInput::Resize {
                columns: 80,
                rows: 24
            })
        );
        bridge.abort();
        output_task.abort();
        drain.abort();
        Ok(())
    }

    #[tokio::test]
    async fn attach_output_backpressure_is_bounded_and_disconnect_cancels_it()
    -> Result<(), Box<dyn std::error::Error>> {
        let activity = ActivityTracker::new();
        let (input, _inputs) = tokio::sync::mpsc::channel(1);
        let (outputs, output) = tokio::sync::mpsc::channel(2);
        let session = ExecSession::live(input, output);
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        let bridge = tokio::spawn(run_attach_bridge(
            session,
            tokio_stream::pending(),
            ExecInput::Stdin(Vec::new()),
            bound_binder(b"backpressure"),
            sender,
            activity.clone(),
        ));
        let producer = tokio::spawn(async move {
            for _ in 0..64 {
                if outputs
                    .send(Ok(ExecOutput::Stdout(vec![7; 1024])))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(!producer.is_finished());
        drop(receiver);
        tokio::time::timeout(std::time::Duration::from_secs(1), bridge).await??;
        tokio::time::timeout(std::time::Duration::from_secs(1), producer).await??;
        Ok(())
    }

    #[test]
    fn session_claim_is_atomic_and_distinguishes_expired_from_unknown() {
        let id = SandboxId::test("session-registry");
        let pending = |expires| PendingSession {
            id: id.clone(),
            argv: vec!["true".to_owned()],
            environment: Default::default(),
            tty: false,
            expires,
        };
        let mut registry = SessionRegistry::default();
        registry.insert(
            b"live".to_vec(),
            pending(tokio::time::Instant::now() + std::time::Duration::from_secs(1)),
        );
        assert!(registry.claim(b"live").is_ok());
        assert_eq!(
            registry.claim(b"live").err(),
            Some(gascan_proto::error_code::UNKNOWN_SESSION_TOKEN)
        );
        registry.insert(
            b"old".to_vec(),
            pending(tokio::time::Instant::now() - std::time::Duration::from_secs(1)),
        );
        assert_eq!(
            registry.claim(b"old").err(),
            Some(gascan_proto::error_code::EXPIRED_SESSION_TOKEN)
        );
        for index in 0..1_100_u32 {
            registry.insert(
                index.to_be_bytes().to_vec(),
                pending(tokio::time::Instant::now() + std::time::Duration::from_secs(1)),
            );
        }
        assert!(registry.pending.len() <= 1_024);
        assert!(registry.expired.len() <= 1_024);
        assert_eq!(
            registry.claim(b"old").err(),
            Some(gascan_proto::error_code::EXPIRED_SESSION_TOKEN)
        );
    }

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

    #[tokio::test]
    async fn shutdown_cancels_a_client_held_open_event_stream()
    -> Result<(), Box<dyn std::error::Error>> {
        let activity = ActivityTracker::new();
        let (held_sender, receiver) = tokio::sync::mpsc::channel(32);
        let mut stream = ApiEventStream::new(receiver, activity.clone());
        let operation_id = OperationId::new(1)?;
        for sequence in 1..=24 {
            held_sender.try_send(OperationEvent {
                sequence,
                operation_id,
                status: OperationStatus::Pending,
                details: None,
                error_code: None,
                timestamp_millis: sequence,
            })?;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        activity.cancel_streams();
        tokio::time::timeout(
            std::time::Duration::from_millis(100),
            activity.wait_for_idle(std::time::Duration::from_millis(1)),
        )
        .await??;
        while tokio::time::timeout(std::time::Duration::from_millis(100), stream.next())
            .await?
            .is_some()
        {}
        Ok(())
    }

    #[test]
    fn wire_metadata_uses_durable_store_values() -> Result<(), Box<dyn std::error::Error>> {
        let operation_id = OperationId::new(17)?;
        let event = wire_event(OperationEvent {
            sequence: 2,
            operation_id,
            status: OperationStatus::Failed,
            details: Some(json!({"message":"broken"})),
            error_code: Some("backend_unavailable".to_owned()),
            timestamp_millis: 1_725_000_000_123,
        });
        assert_eq!(
            event.error.map(|error| error.code),
            Some("backend_unavailable".to_owned())
        );
        assert_eq!(
            event
                .timestamp
                .map(|timestamp| (timestamp.seconds, timestamp.nanos)),
            Some((1_725_000_000, 123_000_000))
        );

        let root = Utf8PathBuf::from("/workspace/api-metadata");
        let status = wire_status(SandboxRecord {
            id: SandboxId::from_root("metadata", &root),
            canonical_root: root,
            desired_state: DesiredState::Running,
            actual_state: ActualState::Running,
            setup_resolution: None,
            tool_resolution: None,
            image_resolution: None,
            last_operation_id: Some(operation_id),
            updated_at_millis: 1_725_000_001_456,
        });
        assert_eq!(status.last_operation_id.map(|id| id.value), Some(17));
        assert_eq!(
            status
                .updated_at
                .map(|timestamp| (timestamp.seconds, timestamp.nanos)),
            Some((1_725_000_001, 456_000_000))
        );
        Ok(())
    }

    #[test]
    fn pending_operations_map_to_operation_conflict() {
        let root = Utf8PathBuf::from("/workspace/conflict");
        let sandbox_id = SandboxId::from_root("conflict", &root);
        let status = service_status(ServiceError::Store(StoreError::PendingOperationExists {
            sandbox_id,
        }));
        assert_eq!(status.code(), tonic::Code::AlreadyExists);
        assert_eq!(status.message(), error_code::OPERATION_CONFLICT);
    }
}
