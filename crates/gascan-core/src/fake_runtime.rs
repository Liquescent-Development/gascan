use crate::runtime::{
    ContainerState, CreateFailure, CreateOutcome, CreateRequest, ExecCancellation, ExecInput,
    ExecOutput, ExecRequest, ExecSession, RemoveRequest, ResourceIdentity, ResourceKind,
    ResourceOwnership, RuntimeBackend, RuntimeCall, RuntimeCapabilities, RuntimeError,
    RuntimeOutcome, RuntimeResource, RuntimeSandbox,
};
use crate::sandbox::SandboxId;
use async_trait::async_trait;
use std::collections::VecDeque;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{Mutex, Semaphore};

#[derive(Clone)]
pub struct FakeRuntime {
    inner: Arc<Mutex<FakeState>>,
    persistence: Arc<Option<PathBuf>>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct FakeSnapshot {
    sandboxes: Vec<RuntimeSandbox>,
    resources: Vec<PersistedResource>,
    logs: Vec<FakeLogRecord>,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct FakeLogRecord {
    sandbox_id: SandboxId,
    timestamp_millis: i64,
    bytes: Vec<u8>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct PersistedResource {
    kind: ResourceKind,
    name: String,
    sandbox_id: Option<SandboxId>,
    ownership: ResourceOwnership,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FailureBoundary {
    Capabilities,
    Inspect,
    Create,
    Start,
    Stop,
    Remove,
    Exec,
    Logs,
    ListResources,
}

impl FailureBoundary {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Capabilities => "capabilities",
            Self::Inspect => "inspect",
            Self::Create => "create",
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Remove => "remove",
            Self::Exec => "exec",
            Self::Logs => "logs",
            Self::ListResources => "list_resources",
        }
    }
}

struct FakeState {
    capabilities: RuntimeCapabilities,
    sandboxes: HashMap<SandboxId, RuntimeSandbox>,
    resources: HashMap<ResourceIdentity, RuntimeResource>,
    gates: HashMap<FailureBoundary, Arc<Semaphore>>,
    calls: Vec<RuntimeCall>,
    outcomes: Vec<RuntimeOutcome>,
    failures: HashSet<FailureBoundary>,
    create_failure_after_mutations: Option<usize>,
    exec_result: (Vec<u8>, Vec<u8>, i32),
    exec_results: VecDeque<(Vec<u8>, Vec<u8>, i32)>,
    exec_errors: VecDeque<RuntimeError>,
    exec_input_failures: usize,
    exec_stream_errors: VecDeque<RuntimeError>,
    exec_cancellations: usize,
    logs: Vec<FakeLogRecord>,
}

impl FakeRuntime {
    pub fn new(capabilities: RuntimeCapabilities) -> Self {
        Self {
            inner: Arc::new(Mutex::new(FakeState {
                capabilities,
                sandboxes: HashMap::new(),
                resources: HashMap::new(),
                gates: HashMap::new(),
                calls: Vec::new(),
                outcomes: Vec::new(),
                failures: HashSet::new(),
                create_failure_after_mutations: None,
                exec_result: (Vec::new(), Vec::new(), 0),
                exec_results: VecDeque::new(),
                exec_errors: VecDeque::new(),
                exec_input_failures: 0,
                exec_stream_errors: VecDeque::new(),
                exec_cancellations: 0,
                logs: Vec::new(),
            })),
            persistence: Arc::new(None),
        }
    }

    pub async fn persistent(
        capabilities: RuntimeCapabilities,
        path: impl AsRef<Path>,
    ) -> Result<Self, RuntimeError> {
        let path = path.as_ref().to_owned();
        let runtime = Self {
            inner: Arc::new(Mutex::new(load_state(capabilities, &path)?)),
            persistence: Arc::new(Some(path)),
        };
        Ok(runtime)
    }

    pub fn failing_once(boundary: FailureBoundary) -> Self {
        let runtime = Self::new(fixture_capabilities());
        if let Ok(mut state) = runtime.inner.try_lock() {
            state.failures.insert(boundary);
        }
        runtime
    }

    pub async fn calls(&self) -> Vec<RuntimeCall> {
        self.inner.lock().await.calls.clone()
    }

    pub async fn outcomes(&self) -> Vec<RuntimeOutcome> {
        self.inner.lock().await.outcomes.clone()
    }

    pub async fn gate(&self, boundary: FailureBoundary) {
        self.inner
            .lock()
            .await
            .gates
            .insert(boundary, Arc::new(Semaphore::new(0)));
    }

    pub async fn release(&self, boundary: FailureBoundary, permits: usize) {
        if let Some(gate) = self.inner.lock().await.gates.get(&boundary).cloned() {
            gate.add_permits(permits);
        }
    }

    pub async fn inject_failure(&self, boundary: FailureBoundary) {
        self.inner.lock().await.failures.insert(boundary);
    }

    pub async fn fail_create_after_mutations(&self, mutations: usize) {
        self.inner.lock().await.create_failure_after_mutations = Some(mutations);
    }

    pub async fn seed_unowned(&self, id: SandboxId) {
        let ownership = crate::runtime::OwnershipMetadata {
            managed_by: "foreign-runtime-client".to_owned(),
            sandbox_id: id.clone(),
        };
        let mut state = self.inner.lock().await;
        state.sandboxes.insert(
            id.clone(),
            RuntimeSandbox {
                id: id.clone(),
                state: ContainerState::Stopped,
                ownership,
            },
        );
        insert_container_resource(&mut state, id, ResourceOwnership::Foreign);
    }

    pub async fn set_exec_result(&self, stdout: Vec<u8>, stderr: Vec<u8>, exit_code: i32) {
        self.inner.lock().await.exec_result = (stdout, stderr, exit_code);
    }

    pub async fn queue_exec_results<I>(&self, results: I)
    where
        I: IntoIterator<Item = (Vec<u8>, Vec<u8>, i32)>,
    {
        self.inner.lock().await.exec_results.extend(results);
    }

    pub async fn queue_exec_error(&self, error: RuntimeError) {
        self.inner.lock().await.exec_errors.push_back(error);
    }

    pub async fn queue_exec_input_failure(&self) {
        self.inner.lock().await.exec_input_failures += 1;
    }

    pub async fn queue_exec_stream_error(&self, error: RuntimeError) {
        self.inner.lock().await.exec_stream_errors.push_back(error);
    }

    pub async fn exec_cancellations(&self) -> usize {
        self.inner.lock().await.exec_cancellations
    }

    pub async fn set_logs(&self, logs: Vec<u8>) {
        let mut state = self.inner.lock().await;
        if let Some(id) = state.sandboxes.keys().next().cloned() {
            state.logs = vec![FakeLogRecord {
                sandbox_id: id,
                timestamp_millis: 0,
                bytes: logs,
            }];
        }
        let _ = persist_state(&state, self.persistence.as_deref());
    }

    pub async fn seed_owned(&self, id: SandboxId) {
        let ownership = crate::runtime::OwnershipMetadata {
            managed_by: "gascan".to_owned(),
            sandbox_id: id.clone(),
        };
        let mut state = self.inner.lock().await;
        state.sandboxes.insert(
            id.clone(),
            RuntimeSandbox {
                id: id.clone(),
                state: ContainerState::Stopped,
                ownership,
            },
        );
        insert_container_resource(&mut state, id, ResourceOwnership::GasCanOwned);
    }

    pub async fn seed_mismatched(&self, id: SandboxId) {
        let ownership = crate::runtime::OwnershipMetadata {
            managed_by: "gascan".to_owned(),
            sandbox_id: SandboxId::test("different-owner"),
        };
        let mut state = self.inner.lock().await;
        state.sandboxes.insert(
            id.clone(),
            RuntimeSandbox {
                id: id.clone(),
                state: ContainerState::Stopped,
                ownership,
            },
        );
        insert_container_resource(&mut state, id, ResourceOwnership::Mismatched);
    }

    pub async fn seed_volume(
        &self,
        name: &str,
        sandbox_id: Option<SandboxId>,
        ownership: ResourceOwnership,
    ) -> Result<(), RuntimeError> {
        let identity = ResourceIdentity::new(ResourceKind::Volume, name)?;
        self.inner.lock().await.resources.insert(
            identity.clone(),
            RuntimeResource::discovered(identity, sandbox_id, ownership),
        );
        Ok(())
    }

    pub async fn seed_network(
        &self,
        name: &str,
        sandbox_id: Option<SandboxId>,
        ownership: ResourceOwnership,
    ) -> Result<(), RuntimeError> {
        let identity = ResourceIdentity::new(ResourceKind::Network, name)?;
        self.inner.lock().await.resources.insert(
            identity.clone(),
            RuntimeResource::discovered(identity, sandbox_id, ownership),
        );
        Ok(())
    }

    pub async fn volume_exists(&self, name: &str) -> bool {
        self.inner
            .lock()
            .await
            .resources
            .values()
            .any(|resource| resource.kind() == ResourceKind::Volume && resource.name() == name)
    }

    pub async fn network_exists(&self, name: &str) -> bool {
        self.inner
            .lock()
            .await
            .resources
            .values()
            .any(|resource| resource.kind() == ResourceKind::Network && resource.name() == name)
    }

    pub async fn created_count(&self) -> usize {
        self.inner
            .lock()
            .await
            .calls
            .iter()
            .filter(|call| matches!(call, RuntimeCall::Create(_)))
            .count()
    }
}

async fn wait_gate(runtime: &FakeRuntime, boundary: FailureBoundary) {
    let gate = runtime.inner.lock().await.gates.get(&boundary).cloned();
    if let Some(gate) = gate {
        if let Ok(permit) = gate.acquire().await {
            permit.forget();
        }
    }
}

fn insert_container_resource(state: &mut FakeState, id: SandboxId, ownership: ResourceOwnership) {
    if let Ok(identity) = ResourceIdentity::new(ResourceKind::Container, id.to_string()) {
        state.resources.insert(
            identity.clone(),
            RuntimeResource::discovered(identity, Some(id), ownership),
        );
    }
}

impl Default for FakeRuntime {
    fn default() -> Self {
        Self::new(fixture_capabilities())
    }
}

fn fail_once(state: &mut FakeState, boundary: FailureBoundary) -> Result<(), RuntimeError> {
    if state.failures.remove(&boundary) {
        state.outcomes.push(RuntimeOutcome::Failure {
            boundary: boundary.as_str().to_owned(),
            code: "injected_failure".to_owned(),
        });
        return Err(RuntimeError::InjectedFailure {
            boundary: boundary.as_str().to_owned(),
        });
    }
    Ok(())
}

fn missing(id: &SandboxId) -> RuntimeError {
    RuntimeError::NotFound {
        resource: id.to_string(),
    }
}

fn load_state(capabilities: RuntimeCapabilities, path: &Path) -> Result<FakeState, RuntimeError> {
    let snapshot = match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice::<FakeSnapshot>(&bytes).map_err(|error| {
            RuntimeError::InvalidOutput {
                operation: "fake_runtime_load".to_owned(),
                message: error.to_string(),
            }
        })?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => FakeSnapshot {
            sandboxes: Vec::new(),
            resources: Vec::new(),
            logs: Vec::new(),
        },
        Err(error) => {
            return Err(RuntimeError::CommandIo {
                operation: "fake_runtime_load".to_owned(),
                message: error.to_string(),
            });
        }
    };
    let resources = snapshot
        .resources
        .into_iter()
        .map(|resource| {
            let identity = ResourceIdentity::new(resource.kind, resource.name)?;
            Ok((
                identity.clone(),
                RuntimeResource::discovered(identity, resource.sandbox_id, resource.ownership),
            ))
        })
        .collect::<Result<HashMap<_, _>, RuntimeError>>()?;
    Ok(FakeState {
        capabilities,
        sandboxes: snapshot
            .sandboxes
            .into_iter()
            .map(|sandbox| (sandbox.id.clone(), sandbox))
            .collect(),
        resources,
        gates: HashMap::new(),
        calls: Vec::new(),
        outcomes: Vec::new(),
        failures: HashSet::new(),
        create_failure_after_mutations: None,
        exec_result: (Vec::new(), Vec::new(), 0),
        exec_results: VecDeque::new(),
        exec_errors: VecDeque::new(),
        exec_input_failures: 0,
        exec_stream_errors: VecDeque::new(),
        exec_cancellations: 0,
        logs: snapshot.logs,
    })
}

fn persist_state(state: &FakeState, path: Option<&Path>) -> Result<(), RuntimeError> {
    let Some(path) = path else {
        return Ok(());
    };
    let snapshot = FakeSnapshot {
        sandboxes: state.sandboxes.values().cloned().collect(),
        resources: state
            .resources
            .values()
            .map(|resource| PersistedResource {
                kind: resource.kind(),
                name: resource.name().to_owned(),
                sandbox_id: resource.sandbox_id().cloned(),
                ownership: resource.ownership(),
            })
            .collect(),
        logs: state.logs.clone(),
    };
    let bytes = serde_json::to_vec(&snapshot).map_err(|error| RuntimeError::InvalidOutput {
        operation: "fake_runtime_save".to_owned(),
        message: error.to_string(),
    })?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| RuntimeError::CommandIo {
            operation: "fake_runtime_save".to_owned(),
            message: error.to_string(),
        })?;
    }
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, bytes)
        .and_then(|()| std::fs::rename(&temporary, path))
        .map_err(|error| RuntimeError::CommandIo {
            operation: "fake_runtime_save".to_owned(),
            message: error.to_string(),
        })
}

fn interpret_fake_command(
    argv: &[String],
    environment: &std::collections::BTreeMap<String, String>,
    stdin: Vec<u8>,
    configured: (Vec<u8>, Vec<u8>, i32),
    resize: Option<(u32, u32)>,
) -> (Vec<u8>, Vec<u8>, i32) {
    match argv.first().map(String::as_str) {
        Some("fake-echo-stdin") => (stdin, Vec::new(), 0),
        Some("fake-exit") => (
            Vec::new(),
            Vec::new(),
            argv.get(1)
                .and_then(|value| value.parse().ok())
                .unwrap_or(64),
        ),
        Some("fake-stdout") => (
            argv.get(1)
                .map_or_else(Vec::new, |value| value.as_bytes().to_vec()),
            Vec::new(),
            0,
        ),
        Some("fake-stderr") => (
            Vec::new(),
            argv.get(1)
                .map_or_else(Vec::new, |value| value.as_bytes().to_vec()),
            0,
        ),
        Some("fake-env") => (
            argv.get(1)
                .and_then(|name| environment.get(name))
                .map_or_else(Vec::new, |value| value.as_bytes().to_vec()),
            Vec::new(),
            0,
        ),
        Some("fake-last-resize") => (
            resize.map_or_else(Vec::new, |(columns, rows)| {
                format!("{columns}x{rows}").into_bytes()
            }),
            Vec::new(),
            0,
        ),
        Some("select-gascamp") | Some("/usr/local/bin/select-gascamp") => {
            (br#"{"source":"bundled"}"#.to_vec(), Vec::new(), 0)
        }
        Some("true") | Some("sh") => (Vec::new(), Vec::new(), 0),
        _ => configured,
    }
}

#[async_trait]
impl RuntimeBackend for FakeRuntime {
    async fn capabilities(&self) -> Result<RuntimeCapabilities, RuntimeError> {
        let mut state = self.inner.lock().await;
        state.calls.push(RuntimeCall::Capabilities);
        fail_once(&mut state, FailureBoundary::Capabilities)?;
        Ok(state.capabilities.clone())
    }

    async fn inspect(&self, id: &SandboxId) -> Result<Option<RuntimeSandbox>, RuntimeError> {
        let mut state = self.inner.lock().await;
        state.calls.push(RuntimeCall::Inspect(id.clone()));
        fail_once(&mut state, FailureBoundary::Inspect)?;
        Ok(state.sandboxes.get(id).cloned())
    }

    async fn create(&self, request: CreateRequest) -> Result<CreateOutcome, CreateFailure> {
        let mut state = self.inner.lock().await;
        state.calls.push(RuntimeCall::Create(request.clone()));
        fail_once(&mut state, FailureBoundary::Create).map_err(CreateFailure::from_source)?;
        if request.id != request.ownership.sandbox_id {
            return Err(CreateFailure::from_source(
                RuntimeError::OwnershipMismatch {
                    resource: request.id.to_string(),
                },
            ));
        }
        if state.sandboxes.contains_key(&request.id) {
            return Err(CreateFailure::from_source(RuntimeError::Conflict {
                resource: request.id.to_string(),
                message: "sandbox already exists".to_owned(),
            }));
        }
        let mut created = Vec::new();
        if let Some(name) = request.network().managed_name() {
            let identity = match ResourceIdentity::new(ResourceKind::Network, name) {
                Ok(identity) => identity,
                Err(error) => return Err(create_failure(&request, created, error)),
            };
            if state.resources.contains_key(&identity) {
                return Err(create_failure(
                    &request,
                    created,
                    RuntimeError::Conflict {
                        resource: name.to_owned(),
                        message: "network already exists".to_owned(),
                    },
                ));
            }
            let resource = RuntimeResource::discovered(
                identity.clone(),
                Some(request.id().clone()),
                ResourceOwnership::GasCanOwned,
            );
            state.resources.insert(identity, resource.clone());
            created.push(resource);
            fail_after_create_mutation(&mut state, &request, &created)?;
        }
        for volume in request.volumes() {
            let identity = ResourceIdentity::new(ResourceKind::Volume, volume.name.clone())
                .map_err(CreateFailure::from_source)?;
            if let Some(existing) = state.resources.get(&identity) {
                if existing.ownership() != ResourceOwnership::GasCanOwned
                    || existing.sandbox_id() != Some(&request.id)
                {
                    return Err(create_failure(
                        &request,
                        created,
                        RuntimeError::Conflict {
                            resource: volume.name.clone(),
                            message: "volume exists with different ownership".to_owned(),
                        },
                    ));
                }
            } else {
                let resource = RuntimeResource::discovered(
                    identity.clone(),
                    Some(request.id.clone()),
                    ResourceOwnership::GasCanOwned,
                );
                state.resources.insert(identity, resource.clone());
                created.push(resource);
                fail_after_create_mutation(&mut state, &request, &created)?;
            }
        }
        state.sandboxes.insert(
            request.id.clone(),
            RuntimeSandbox {
                id: request.id.clone(),
                state: ContainerState::Stopped,
                ownership: request.ownership.clone(),
            },
        );
        let identity = ResourceIdentity::new(ResourceKind::Container, request.id.to_string())
            .map_err(CreateFailure::from_source)?;
        let resource = RuntimeResource::discovered(
            identity.clone(),
            Some(request.id.clone()),
            ResourceOwnership::GasCanOwned,
        );
        state.resources.insert(identity, resource.clone());
        created.push(resource);
        fail_after_create_mutation(&mut state, &request, &created)?;
        let outcome = CreateOutcome::new(&request, created).map_err(CreateFailure::from_source)?;
        state
            .outcomes
            .push(RuntimeOutcome::Created(outcome.clone()));
        persist_state(&state, self.persistence.as_deref()).map_err(CreateFailure::from_source)?;
        Ok(outcome)
    }

    async fn start(&self, id: &SandboxId) -> Result<(), RuntimeError> {
        self.inner
            .lock()
            .await
            .calls
            .push(RuntimeCall::Start(id.clone()));
        wait_gate(self, FailureBoundary::Start).await;
        let mut state = self.inner.lock().await;
        fail_once(&mut state, FailureBoundary::Start)?;
        state
            .sandboxes
            .get_mut(id)
            .ok_or_else(|| missing(id))?
            .state = ContainerState::Running;
        persist_state(&state, self.persistence.as_deref())?;
        Ok(())
    }

    async fn stop(&self, id: &SandboxId) -> Result<(), RuntimeError> {
        let mut state = self.inner.lock().await;
        state.calls.push(RuntimeCall::Stop(id.clone()));
        fail_once(&mut state, FailureBoundary::Stop)?;
        state
            .sandboxes
            .get_mut(id)
            .ok_or_else(|| missing(id))?
            .state = ContainerState::Stopped;
        persist_state(&state, self.persistence.as_deref())?;
        Ok(())
    }

    async fn remove(&self, request: RemoveRequest) -> Result<(), RuntimeError> {
        let mut state = self.inner.lock().await;
        state.calls.push(RuntimeCall::Remove(request.clone()));
        fail_once(&mut state, FailureBoundary::Remove)?;
        for expected in request.resources() {
            let actual =
                state
                    .resources
                    .get(expected.identity())
                    .ok_or_else(|| RuntimeError::NotFound {
                        resource: expected.name().to_owned(),
                    })?;
            if actual.ownership() == ResourceOwnership::Foreign {
                return Err(RuntimeError::ForeignResourceRefused {
                    resource: actual.name().to_owned(),
                });
            }
            if actual != expected {
                return Err(RuntimeError::OwnershipMismatch {
                    resource: actual.name().to_owned(),
                });
            }
        }
        let mut removal = request.resources().to_vec();
        removal.sort_by_key(|resource| match resource.kind() {
            ResourceKind::Container => 0,
            ResourceKind::Volume => 1,
            ResourceKind::Network => 2,
        });
        for expected in &removal {
            state.resources.remove(expected.identity());
            if expected.kind() == ResourceKind::Container {
                if let Some(id) = expected.sandbox_id() {
                    state.sandboxes.remove(id);
                }
            }
        }
        state
            .outcomes
            .push(RuntimeOutcome::Removed(RemoveRequest::from_resources(
                removal,
            )?));
        persist_state(&state, self.persistence.as_deref())?;
        Ok(())
    }

    async fn exec(&self, request: ExecRequest) -> Result<ExecSession, RuntimeError> {
        let mut state = self.inner.lock().await;
        state.calls.push(RuntimeCall::Exec(request.clone()));
        if let Some(error) = state.exec_errors.pop_front() {
            return Err(error);
        }
        fail_once(&mut state, FailureBoundary::Exec)?;
        let sandbox = state
            .sandboxes
            .get(&request.id)
            .ok_or_else(|| missing(&request.id))?;
        if sandbox.state != ContainerState::Running {
            return Err(RuntimeError::InvalidState {
                resource: request.id.to_string(),
                message: "exec requires a running sandbox".to_owned(),
            });
        }
        if state.exec_input_failures > 0 {
            state.exec_input_failures -= 1;
            let (input, inputs) = tokio::sync::mpsc::channel(1);
            drop(inputs);
            let (_outputs, output) = tokio::sync::mpsc::channel(1);
            return Ok(ExecSession::live(input, output));
        }
        if let Some(error) = state.exec_stream_errors.pop_front() {
            let (input, mut inputs) = tokio::sync::mpsc::channel(1);
            tokio::spawn(async move {
                let _ = inputs.recv().await;
            });
            let (outputs, output) = tokio::sync::mpsc::channel(1);
            let _ = outputs.try_send(Err(error));
            return Ok(ExecSession::live(input, output));
        }
        let configured = state
            .exec_results
            .pop_front()
            .unwrap_or_else(|| state.exec_result.clone());
        let runtime = self.clone();
        let (input, mut inputs) = tokio::sync::mpsc::channel(16);
        let (outputs, output) = tokio::sync::mpsc::channel(1);
        let (cancellation, mut cancelled) = ExecCancellation::channel();
        tokio::spawn(async move {
            let ready_then_drain = request
                .argv
                .first()
                .is_some_and(|arg| arg == "fake-ready-then-drain");
            if ready_then_drain
                && outputs
                    .send(Ok(ExecOutput::Stdout(b"ready".to_vec())))
                    .await
                    .is_err()
            {
                return;
            }
            let mut stdin = request.stdin;
            let mut signal = 0;
            let mut resize = None;
            while let Some(frame) = inputs.recv().await {
                match frame {
                    ExecInput::Stdin(bytes) => stdin.extend(bytes),
                    ExecInput::Resize { columns, rows } => resize = Some((columns, rows)),
                    ExecInput::Signal(number) => signal = number,
                    ExecInput::Close => break,
                }
            }
            let (stdout, stderr, code) = if ready_then_drain {
                (Vec::new(), Vec::new(), 0)
            } else {
                interpret_fake_command(
                    &request.argv,
                    &request.environment,
                    stdin,
                    configured,
                    resize,
                )
            };
            {
                let mut state = runtime.inner.lock().await;
                let timestamp_millis = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .and_then(|duration| i64::try_from(duration.as_millis()).ok())
                    .unwrap_or(i64::MAX);
                state.logs.push(FakeLogRecord {
                    sandbox_id: request.id.clone(),
                    timestamp_millis,
                    bytes: [stdout.as_slice(), stderr.as_slice()].concat(),
                });
                let _ = persist_state(&state, runtime.persistence.as_deref());
            }
            if !stdout.is_empty()
                && !send_fake_exec_output(&outputs, &mut cancelled, ExecOutput::Stdout(stdout))
                    .await
            {
                record_fake_exec_cancellation(&runtime, &cancelled).await;
                return;
            }
            if !stderr.is_empty()
                && !send_fake_exec_output(&outputs, &mut cancelled, ExecOutput::Stderr(stderr))
                    .await
            {
                record_fake_exec_cancellation(&runtime, &cancelled).await;
                return;
            }
            let code = if signal == 0 {
                code
            } else {
                128_i32.saturating_add(signal)
            };
            if !send_fake_exec_output(&outputs, &mut cancelled, ExecOutput::Exit { code, signal })
                .await
            {
                record_fake_exec_cancellation(&runtime, &cancelled).await;
            }
        });
        Ok(ExecSession::live_cancellable(input, output, cancellation))
    }

    async fn logs(
        &self,
        id: &SandboxId,
        since_millis: Option<i64>,
    ) -> Result<Vec<u8>, RuntimeError> {
        let mut state = self.inner.lock().await;
        state.calls.push(RuntimeCall::Logs(id.clone()));
        fail_once(&mut state, FailureBoundary::Logs)?;
        if !state.sandboxes.contains_key(id) {
            return Err(missing(id));
        }
        Ok(state
            .logs
            .iter()
            .filter(|record| {
                record.sandbox_id == *id
                    && since_millis.is_none_or(|since| record.timestamp_millis >= since)
            })
            .flat_map(|record| record.bytes.iter().copied())
            .collect())
    }

    async fn list_resources(&self) -> Result<Vec<RuntimeResource>, RuntimeError> {
        let mut state = self.inner.lock().await;
        state.calls.push(RuntimeCall::ListResources);
        fail_once(&mut state, FailureBoundary::ListResources)?;
        let mut resources = state.resources.values().cloned().collect::<Vec<_>>();
        resources.sort_by(|left, right| left.identity().cmp(right.identity()));
        Ok(resources)
    }
}

async fn send_fake_exec_output(
    outputs: &tokio::sync::mpsc::Sender<Result<ExecOutput, RuntimeError>>,
    cancelled: &mut tokio::sync::watch::Receiver<bool>,
    output: ExecOutput,
) -> bool {
    tokio::select! {
        biased;
        _ = cancelled.changed() => false,
        result = outputs.send(Ok(output)) => result.is_ok(),
    }
}

async fn record_fake_exec_cancellation(
    runtime: &FakeRuntime,
    cancelled: &tokio::sync::watch::Receiver<bool>,
) {
    if *cancelled.borrow() {
        runtime.inner.lock().await.exec_cancellations += 1;
    }
}

fn create_failure(
    request: &CreateRequest,
    created: Vec<RuntimeResource>,
    source: RuntimeError,
) -> CreateFailure {
    CreateFailure::new(request, created, source).unwrap_or_else(CreateFailure::from_source)
}

fn fail_after_create_mutation(
    state: &mut FakeState,
    request: &CreateRequest,
    created: &[RuntimeResource],
) -> Result<(), CreateFailure> {
    if state.create_failure_after_mutations == Some(created.len()) {
        state.create_failure_after_mutations = None;
        let source = RuntimeError::InjectedFailure {
            boundary: "create_after_mutation".to_owned(),
        };
        state.outcomes.push(RuntimeOutcome::Failure {
            boundary: "create_after_mutation".to_owned(),
            code: source.code().to_owned(),
        });
        return Err(create_failure(request, created.to_vec(), source));
    }
    Ok(())
}

pub fn fixture_capabilities() -> RuntimeCapabilities {
    RuntimeCapabilities {
        version: crate::runtime::RuntimeVersion::new(1, 0, 0),
        bind_mounts: true,
        named_volumes: true,
        tty: true,
        signals: true,
        loopback_publish: true,
        resource_limits: true,
        offline: crate::runtime::NetworkIsolation::Proven,
    }
}
