use crate::runtime::{
    ContainerState, CreateRequest, ExecRequest, ExecSession, OwnedResource, RuntimeBackend,
    RuntimeCall, RuntimeCapabilities, RuntimeError, RuntimeSandbox,
};
use crate::sandbox::SandboxId;
use async_trait::async_trait;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::Mutex;

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
    ListOwned,
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
            Self::ListOwned => "list_owned",
        }
    }
}

struct FakeState {
    capabilities: RuntimeCapabilities,
    sandboxes: HashMap<SandboxId, RuntimeSandbox>,
    calls: Vec<RuntimeCall>,
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
                calls: Vec::new(),
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

    pub async fn seed_unowned(&self, id: SandboxId) {
        let ownership = crate::runtime::OwnershipMetadata {
            managed_by: "foreign-runtime-client".to_owned(),
            sandbox_id: id.clone(),
        };
        self.inner.lock().await.sandboxes.insert(
            id.clone(),
            RuntimeSandbox {
                id,
                state: ContainerState::Stopped,
                ownership,
            },
        );
    }

    pub async fn set_exec_result(&self, stdout: Vec<u8>, stderr: Vec<u8>, exit_code: i32) {
        self.inner.lock().await.exec_result = ExecSession::from_output(stdout, stderr, exit_code);
    }

    pub async fn set_logs(&self, logs: Vec<u8>) {
        self.inner.lock().await.logs = logs;
    }
}

fn fail_once(state: &mut FakeState, boundary: FailureBoundary) -> Result<(), RuntimeError> {
    if state.failures.remove(&boundary) {
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

    async fn create(&self, request: CreateRequest) -> Result<(), RuntimeError> {
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
        state.sandboxes.insert(
            request.id.clone(),
            RuntimeSandbox {
                id: request.id,
                state: ContainerState::Stopped,
                ownership: request.ownership,
            },
        );
        Ok(())
    }

    async fn start(&self, id: &SandboxId) -> Result<(), RuntimeError> {
        let mut state = self.inner.lock().await;
        state.calls.push(RuntimeCall::Start(id.clone()));
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

    async fn remove(&self, id: &SandboxId) -> Result<(), RuntimeError> {
        let mut state = self.inner.lock().await;
        state.calls.push(RuntimeCall::Remove(id.clone()));
        fail_once(&mut state, FailureBoundary::Remove)?;
        state.sandboxes.remove(id).ok_or_else(|| missing(id))?;
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

    async fn list_owned(&self) -> Result<Vec<OwnedResource>, RuntimeError> {
        let mut state = self.inner.lock().await;
        state.calls.push(RuntimeCall::ListOwned);
        fail_once(&mut state, FailureBoundary::ListOwned)?;
        let mut resources = state
            .sandboxes
            .values()
            .filter(|sandbox| {
                sandbox.ownership.managed_by == "gascan"
                    && sandbox.ownership.sandbox_id == sandbox.id
            })
            .map(|sandbox| OwnedResource {
                id: sandbox.id.clone(),
                ownership: sandbox.ownership.clone(),
            })
            .collect::<Vec<_>>();
        resources.sort_by(|left, right| left.id.as_str().cmp(right.id.as_str()));
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
