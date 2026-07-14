use crate::runtime::{
    ContainerState, CreateOutcome, CreateRequest, ExecRequest, ExecSession, RemoveRequest,
    ResourceIdentity, ResourceKind, ResourceOwnership, RuntimeBackend, RuntimeCall,
    RuntimeCapabilities, RuntimeError, RuntimeOutcome, RuntimeResource, RuntimeSandbox,
};
use crate::sandbox::SandboxId;
use async_trait::async_trait;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{Mutex, Semaphore};

#[derive(Clone)]
pub struct FakeRuntime {
    inner: Arc<Mutex<FakeState>>,
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
    exec_result: ExecSession,
    logs: Vec<u8>,
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
                exec_result: ExecSession::from_output(Vec::new(), Vec::new(), 0),
                logs: Vec::new(),
            })),
        }
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
        self.inner.lock().await.exec_result = ExecSession::from_output(stdout, stderr, exit_code);
    }

    pub async fn set_logs(&self, logs: Vec<u8>) {
        self.inner.lock().await.logs = logs;
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

    pub async fn volume_exists(&self, name: &str) -> bool {
        self.inner
            .lock()
            .await
            .resources
            .values()
            .any(|resource| resource.kind() == ResourceKind::Volume && resource.name() == name)
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

    async fn create(&self, request: CreateRequest) -> Result<CreateOutcome, RuntimeError> {
        let mut state = self.inner.lock().await;
        state.calls.push(RuntimeCall::Create(request.clone()));
        fail_once(&mut state, FailureBoundary::Create)?;
        if request.id != request.ownership.sandbox_id {
            return Err(RuntimeError::OwnershipMismatch {
                resource: request.id.to_string(),
            });
        }
        if state.sandboxes.contains_key(&request.id) {
            return Err(RuntimeError::Conflict {
                resource: request.id.to_string(),
                message: "sandbox already exists".to_owned(),
            });
        }
        let mut created = Vec::new();
        for volume in request.volumes() {
            let identity = ResourceIdentity::new(ResourceKind::Volume, volume.name.clone())?;
            if let Some(existing) = state.resources.get(&identity) {
                if existing.ownership() != ResourceOwnership::GasCanOwned
                    || existing.sandbox_id() != Some(&request.id)
                {
                    return Err(RuntimeError::Conflict {
                        resource: volume.name.clone(),
                        message: "volume exists with different ownership".to_owned(),
                    });
                }
            } else {
                let resource = RuntimeResource::discovered(
                    identity.clone(),
                    Some(request.id.clone()),
                    ResourceOwnership::GasCanOwned,
                );
                state.resources.insert(identity, resource.clone());
                created.push(resource);
            }
        }
        state.sandboxes.insert(
            request.id.clone(),
            RuntimeSandbox {
                id: request.id.clone(),
                state: ContainerState::Stopped,
                ownership: request.ownership,
            },
        );
        let identity = ResourceIdentity::new(ResourceKind::Container, request.id.to_string())?;
        let resource = RuntimeResource::discovered(
            identity.clone(),
            Some(request.id),
            ResourceOwnership::GasCanOwned,
        );
        state.resources.insert(identity, resource.clone());
        created.push(resource);
        let outcome = CreateOutcome::new(created)?;
        state
            .outcomes
            .push(RuntimeOutcome::Created(outcome.clone()));
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
            if actual.ownership() != ResourceOwnership::GasCanOwned
                || actual.sandbox_id() != expected.sandbox_id()
                || expected.ownership() != ResourceOwnership::GasCanOwned
            {
                return Err(RuntimeError::OwnershipMismatch {
                    resource: actual.name().to_owned(),
                });
            }
        }
        for expected in request.resources() {
            state.resources.remove(expected.identity());
            if expected.kind() == ResourceKind::Container {
                if let Some(id) = expected.sandbox_id() {
                    state.sandboxes.remove(id);
                }
            }
        }
        state.outcomes.push(RuntimeOutcome::Removed(request));
        Ok(())
    }

    async fn exec(&self, request: ExecRequest) -> Result<ExecSession, RuntimeError> {
        let mut state = self.inner.lock().await;
        state.calls.push(RuntimeCall::Exec(request.clone()));
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
        Ok(state.exec_result.clone())
    }

    async fn logs(&self, id: &SandboxId) -> Result<Vec<u8>, RuntimeError> {
        let mut state = self.inner.lock().await;
        state.calls.push(RuntimeCall::Logs(id.clone()));
        fail_once(&mut state, FailureBoundary::Logs)?;
        if !state.sandboxes.contains_key(id) {
            return Err(missing(id));
        }
        Ok(state.logs.clone())
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

fn fixture_capabilities() -> RuntimeCapabilities {
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
