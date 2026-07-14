use crate::reconcile::{ReconcileFinding, ReconcileReport};
use crate::{
    ActualState, DesiredState, ImageResolution, OperationEvent, OperationKind, OperationRecord,
    SandboxRecord, SetupResolution, Store, StoreError, ToolResolution,
};
use async_trait::async_trait;
use gascan_core::manifest::ManifestError;
use gascan_core::policy::{PolicyCompiler, PolicyError};
use gascan_core::runtime::{
    ContainerState, CreateRequest, RemoveRequest, ResourceKind, ResourceOwnership, RuntimeBackend,
    RuntimeError,
};
use gascan_core::sandbox::{SandboxError, SandboxId, SandboxSpec};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use thiserror::Error;
use tokio::sync::{Mutex as AsyncMutex, mpsc};

pub struct UpRequest {
    spec: SandboxSpec,
}
impl UpRequest {
    pub const fn new(spec: SandboxSpec) -> Self {
        Self { spec }
    }
}

#[derive(Clone, Debug)]
pub struct ProvisionRequest<'a> {
    pub spec: &'a SandboxSpec,
    pub create: &'a CreateRequest,
}

#[derive(Clone, Debug, Default)]
pub struct ProvisionResolution {
    pub setup: Option<Value>,
    pub tools: Option<Value>,
}

#[async_trait]
pub trait Provisioner: Send + Sync {
    async fn provision(
        &self,
        request: ProvisionRequest<'_>,
    ) -> Result<ProvisionResolution, ServiceError>;
    async fn health_check(&self, id: &SandboxId) -> Result<(), ServiceError>;
}

pub struct NoopProvisioner;
#[async_trait]
impl Provisioner for NoopProvisioner {
    async fn provision(
        &self,
        _request: ProvisionRequest<'_>,
    ) -> Result<ProvisionResolution, ServiceError> {
        Ok(ProvisionResolution::default())
    }
    async fn health_check(&self, _id: &SandboxId) -> Result<(), ServiceError> {
        Ok(())
    }
}

pub struct Operation {
    pub id: i64,
    pub events: mpsc::Receiver<OperationEvent>,
}

pub struct SandboxService<B: RuntimeBackend> {
    runtime: B,
    store: Store,
    provisioner: Arc<dyn Provisioner>,
    locks: Mutex<HashMap<SandboxId, Arc<AsyncMutex<()>>>>,
}

#[derive(Debug, Error)]
pub enum ServiceError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
    #[error(transparent)]
    Policy(#[from] PolicyError),
    #[error(transparent)]
    Sandbox(#[from] SandboxError),
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    #[error("sandbox {0} does not exist")]
    Missing(SandboxId),
    #[error("sandbox {0} is not owned by gascan")]
    Ownership(SandboxId),
    #[error("provisioning failed: {0}")]
    Provision(String),
    #[error("keyed lock registry was poisoned")]
    LockPoisoned,
    #[error("bounded operation event stream could not accept its durable event")]
    EventStreamUnavailable,
}

impl<B: RuntimeBackend> SandboxService<B> {
    pub fn new(runtime: B, store: Store, provisioner: Arc<dyn Provisioner>) -> Self {
        Self {
            runtime,
            store,
            provisioner,
            locks: Mutex::new(HashMap::new()),
        }
    }

    pub const fn store(&self) -> &Store {
        &self.store
    }
    pub fn list(&self) -> Result<Vec<SandboxRecord>, ServiceError> {
        Ok(self.store.list_sandboxes()?)
    }
    pub fn status(&self, id: &SandboxId) -> Result<Option<SandboxRecord>, ServiceError> {
        Ok(self.store.sandbox(id)?)
    }
    pub fn latest_operation(&self) -> Result<Option<OperationRecord>, ServiceError> {
        Ok(self.store.latest_operation()?)
    }

    fn keyed_lock(&self, id: &SandboxId) -> Result<Arc<AsyncMutex<()>>, ServiceError> {
        let mut locks = self.locks.lock().map_err(|_| ServiceError::LockPoisoned)?;
        Ok(locks
            .entry(id.clone())
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone())
    }

    pub async fn up(&self, request: UpRequest) -> Result<Operation, ServiceError> {
        let id = request.spec.id().clone();
        let lock = self.keyed_lock(&id)?;
        let _guard = lock.lock().await;
        let capabilities = self.runtime.capabilities().await?;
        let create = PolicyCompiler::compile(request.spec.clone(), &capabilities)?;
        let existing = self.store.sandbox(&id)?;
        let prior = existing.clone();
        let mut record = existing.unwrap_or_else(|| SandboxRecord {
            id: id.clone(),
            canonical_root: request.spec.canonical_root().to_owned(),
            desired_state: DesiredState::Running,
            actual_state: ActualState::Creating,
            setup_resolution: None,
            tool_resolution: None,
            image_resolution: Some(ImageResolution::new(1, json!({"digest": create.image()}))),
        });
        record.desired_state = DesiredState::Running;
        let operation = self.store.begin_operation(&record, OperationKind::Create)?;
        let (sender, receiver) = mpsc::channel(16);
        self.send_initial(operation.id, &sender)?;
        self.emit(operation.id, json!({"phase":"validated"}), &sender)?;
        let result = self
            .up_runtime(
                &request.spec,
                &create,
                prior.as_ref(),
                operation.id,
                &sender,
            )
            .await;
        match result {
            Ok((actual, resolution)) => {
                if let Some(details) = resolution.setup {
                    record.setup_resolution = Some(SetupResolution::new(1, details));
                }
                if let Some(details) = resolution.tools {
                    record.tool_resolution = Some(ToolResolution::new(1, details));
                }
                record.actual_state = actual;
                self.store.put_sandbox(&record)?;
                let terminal = self.store.complete_operation(operation.id, actual)?;
                self.send_terminal(terminal.id, &sender)?;
                Ok(Operation {
                    id: operation.id,
                    events: receiver,
                })
            }
            Err(error) => {
                let actual = self.runtime.inspect(&id).await.ok().flatten().map_or(
                    ActualState::Absent,
                    |sandbox| match sandbox.state {
                        ContainerState::Running => ActualState::Running,
                        ContainerState::Stopped => ActualState::Stopped,
                        ContainerState::Creating => ActualState::Creating,
                    },
                );
                self.store.fail_operation(
                    operation.id,
                    actual,
                    error.code(),
                    json!({"message":error.to_string()}),
                )?;
                self.send_terminal(operation.id, &sender)?;
                Err(error)
            }
        }
    }

    async fn up_runtime(
        &self,
        spec: &SandboxSpec,
        create: &CreateRequest,
        prior: Option<&SandboxRecord>,
        operation_id: i64,
        sender: &mpsc::Sender<OperationEvent>,
    ) -> Result<(ActualState, ProvisionResolution), ServiceError> {
        let id = spec.id();
        let inspected = self.runtime.inspect(id).await?;
        let mut created = None;
        if let Some(runtime) = &inspected {
            if runtime.ownership.managed_by != "gascan" || runtime.ownership.sandbox_id != *id {
                return Err(ServiceError::Ownership(id.clone()));
            }
        } else {
            created = Some(self.runtime.create(create.clone()).await?);
            self.emit(operation_id, json!({"phase":"created"}), sender)?;
        }
        let result = async {
            let current = self
                .runtime
                .inspect(id)
                .await?
                .ok_or_else(|| ServiceError::Missing(id.clone()))?;
            if current.state != ContainerState::Running {
                self.runtime.start(id).await?;
            }
            self.emit(operation_id, json!({"phase":"started"}), sender)?;
            let resolution = if prior.is_none() {
                self.provisioner
                    .provision(ProvisionRequest { spec, create })
                    .await?
            } else {
                ProvisionResolution::default()
            };
            self.provisioner.health_check(id).await?;
            Ok::<_, ServiceError>(resolution)
        }
        .await;
        match result {
            Ok(resolution) => Ok((ActualState::Running, resolution)),
            Err(error) if created.is_some() => {
                if let Some(outcome) = created {
                    if !outcome.created.is_empty() {
                        self.runtime
                            .remove(RemoveRequest::from_resources(outcome.created)?)
                            .await?;
                    }
                }
                Err(error)
            }
            Err(error) => {
                if self.runtime.inspect(id).await?.is_some() {
                    let _ = self.runtime.stop(id).await;
                }
                Err(error)
            }
        }
    }

    pub async fn start(&self, id: &SandboxId) -> Result<Operation, ServiceError> {
        self.simple_state(
            id,
            OperationKind::Start,
            DesiredState::Running,
            ActualState::Running,
        )
        .await
    }
    pub async fn stop(&self, id: &SandboxId) -> Result<Operation, ServiceError> {
        self.simple_state(
            id,
            OperationKind::Stop,
            DesiredState::Stopped,
            ActualState::Stopped,
        )
        .await
    }

    async fn simple_state(
        &self,
        id: &SandboxId,
        kind: OperationKind,
        desired: DesiredState,
        target: ActualState,
    ) -> Result<Operation, ServiceError> {
        let lock = self.keyed_lock(id)?;
        let _guard = lock.lock().await;
        let mut record = self
            .store
            .sandbox(id)?
            .ok_or_else(|| ServiceError::Missing(id.clone()))?;
        record.desired_state = desired;
        let operation = self.store.begin_operation(&record, kind)?;
        let (sender, receiver) = mpsc::channel(16);
        self.send_initial(operation.id, &sender)?;
        let result = async {
            let runtime = self
                .runtime
                .inspect(id)
                .await?
                .ok_or_else(|| ServiceError::Missing(id.clone()))?;
            if runtime.ownership.managed_by != "gascan" || runtime.ownership.sandbox_id != *id {
                return Err(ServiceError::Ownership(id.clone()));
            }
            match (target, runtime.state) {
                (ActualState::Running, ContainerState::Running)
                | (ActualState::Stopped, ContainerState::Stopped) => Ok(()),
                (ActualState::Running, _) => self.runtime.start(id).await.map_err(Into::into),
                _ => self.runtime.stop(id).await.map_err(Into::into),
            }
        }
        .await;
        if let Err(error) = result {
            let actual = self.runtime_actual(id, record.actual_state).await;
            self.store.fail_operation(
                operation.id,
                actual,
                error.code(),
                json!({"message":error.to_string()}),
            )?;
            self.send_terminal(operation.id, &sender)?;
            return Err(error);
        }
        self.store.complete_operation(operation.id, target)?;
        self.send_terminal(operation.id, &sender)?;
        Ok(Operation {
            id: operation.id,
            events: receiver,
        })
    }

    pub async fn destroy(&self, id: &SandboxId) -> Result<Operation, ServiceError> {
        let lock = self.keyed_lock(id)?;
        let _guard = lock.lock().await;
        let mut record = self
            .store
            .sandbox(id)?
            .ok_or_else(|| ServiceError::Missing(id.clone()))?;
        let prior_actual = record.actual_state;
        record.desired_state = DesiredState::Absent;
        if record.actual_state != ActualState::Absent {
            record.actual_state = ActualState::Destroying;
        }
        let operation = self
            .store
            .begin_operation(&record, OperationKind::Destroy)?;
        let (sender, receiver) = mpsc::channel(16);
        self.send_initial(operation.id, &sender)?;
        let result = async {
            if let Some(runtime) = self.runtime.inspect(id).await? {
                if runtime.ownership.managed_by != "gascan" || runtime.ownership.sandbox_id != *id {
                    return Err(ServiceError::Ownership(id.clone()));
                }
                if runtime.state == ContainerState::Running {
                    self.runtime.stop(id).await?;
                }
                let resources = self
                    .runtime
                    .list_resources()
                    .await?
                    .into_iter()
                    .filter(|resource| {
                        resource.sandbox_id() == Some(id)
                            && resource.ownership() == ResourceOwnership::GasCanOwned
                    })
                    .collect::<Vec<_>>();
                if !resources.is_empty() {
                    self.runtime
                        .remove(RemoveRequest::from_resources(resources)?)
                        .await?;
                }
            }
            Ok::<_, ServiceError>(())
        }
        .await;
        if let Err(error) = result {
            let actual = self.runtime_actual(id, prior_actual).await;
            self.store.fail_operation(
                operation.id,
                actual,
                error.code(),
                json!({"message":error.to_string()}),
            )?;
            self.send_terminal(operation.id, &sender)?;
            return Err(error);
        }
        self.store
            .complete_operation(operation.id, ActualState::Absent)?;
        self.send_terminal(operation.id, &sender)?;
        Ok(Operation {
            id: operation.id,
            events: receiver,
        })
    }

    pub async fn apply(&self, request: UpRequest) -> Result<Operation, ServiceError> {
        let id = request.spec.id().clone();
        let lock = self.keyed_lock(&id)?;
        let _guard = lock.lock().await;
        let capabilities = self.runtime.capabilities().await?;
        let create = PolicyCompiler::compile(request.spec.clone(), &capabilities)?;
        let mut record = self
            .store
            .sandbox(&id)?
            .ok_or_else(|| ServiceError::Missing(id.clone()))?;
        let desired_setup = request
            .spec
            .manifest()
            .setup()
            .map(|path| json!({"path":path.as_str()}));
        let desired_tools = json!(request.spec.manifest().tools());
        let unchanged = record.setup_resolution.as_ref().map(|r| &r.details)
            == desired_setup.as_ref()
            && record.tool_resolution.as_ref().map(|r| &r.details) == Some(&desired_tools);
        let operation = self.store.begin_operation(&record, OperationKind::Apply)?;
        let (sender, receiver) = mpsc::channel(16);
        self.send_initial(operation.id, &sender)?;
        if unchanged {
            self.store
                .complete_operation(operation.id, record.actual_state)?;
            self.send_terminal(operation.id, &sender)?;
            return Ok(Operation {
                id: operation.id,
                events: receiver,
            });
        }
        let prior_actual = record.actual_state;
        let result = async {
            let runtime = self
                .runtime
                .inspect(&id)
                .await?
                .ok_or_else(|| ServiceError::Missing(id.clone()))?;
            if runtime.ownership.managed_by != "gascan" || runtime.ownership.sandbox_id != id {
                return Err(ServiceError::Ownership(id.clone()));
            }
            if runtime.state != ContainerState::Running {
                self.runtime.start(&id).await?;
            }
            self.provisioner
                .provision(ProvisionRequest {
                    spec: &request.spec,
                    create: &create,
                })
                .await?;
            self.provisioner.health_check(&id).await
        }
        .await;
        if let Err(error) = result {
            self.store.fail_operation(
                operation.id,
                prior_actual,
                error.code(),
                json!({"message":error.to_string()}),
            )?;
            self.send_terminal(operation.id, &sender)?;
            return Err(error);
        }
        record.setup_resolution = desired_setup.map(|details| SetupResolution::new(1, details));
        record.tool_resolution = Some(ToolResolution::new(1, desired_tools));
        record.actual_state = ActualState::Running;
        self.store.put_sandbox(&record)?;
        self.store
            .complete_operation(operation.id, ActualState::Running)?;
        self.send_terminal(operation.id, &sender)?;
        Ok(Operation {
            id: operation.id,
            events: receiver,
        })
    }

    pub async fn reconcile(&self) -> Result<ReconcileReport, ServiceError> {
        self.recover_pending().await?;
        let records = self.store.list_sandboxes()?;
        let known = records
            .iter()
            .map(|record| record.id.clone())
            .collect::<HashSet<_>>();
        let inventory = self.runtime.list_resources().await?;
        let actual_owned = inventory
            .iter()
            .filter(|resource| {
                resource.kind() == ResourceKind::Container
                    && resource.ownership() == ResourceOwnership::GasCanOwned
            })
            .filter_map(|resource| resource.sandbox_id().cloned())
            .collect::<HashSet<_>>();
        let mut findings = inventory
            .into_iter()
            .filter_map(|resource| match resource.ownership() {
                ResourceOwnership::GasCanOwned
                    if resource.sandbox_id().is_none_or(|id| !known.contains(id)) =>
                {
                    Some(ReconcileFinding::UnknownOwned(resource))
                }
                ResourceOwnership::GasCanOwned => None,
                ResourceOwnership::Foreign => Some(ReconcileFinding::UnknownUnowned(resource)),
                ResourceOwnership::Mismatched => {
                    Some(ReconcileFinding::OwnershipMismatch(resource))
                }
            })
            .collect::<Vec<_>>();
        for record in records {
            let inspected = self.runtime.inspect(&record.id).await?;
            if inspected.as_ref().is_some_and(|runtime| {
                runtime.ownership.managed_by != "gascan"
                    || runtime.ownership.sandbox_id != record.id
            }) {
                if let Some(resource) =
                    self.runtime
                        .list_resources()
                        .await?
                        .into_iter()
                        .find(|resource| {
                            resource.kind() == ResourceKind::Container
                                && resource.sandbox_id() == Some(&record.id)
                        })
                {
                    findings.push(ReconcileFinding::OwnershipMismatch(resource));
                }
            } else if !actual_owned.contains(&record.id) {
                findings.push(ReconcileFinding::MissingOwned(record.id));
            }
        }
        findings.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
        findings.dedup();
        Ok(ReconcileReport { findings })
    }

    async fn recover_pending(&self) -> Result<(), ServiceError> {
        for operation in self.store.pending_operations()? {
            let record = self
                .store
                .sandbox(&operation.sandbox_id)?
                .ok_or_else(|| ServiceError::Missing(operation.sandbox_id.clone()))?;
            let inspected = self.runtime.inspect(&operation.sandbox_id).await?;
            if inspected.as_ref().is_some_and(|runtime| {
                runtime.ownership.managed_by != "gascan"
                    || runtime.ownership.sandbox_id != operation.sandbox_id
            }) {
                self.store.fail_operation(
                    operation.id,
                    record.actual_state,
                    "ownership_mismatch",
                    json!({"phase":"reconcile"}),
                )?;
                continue;
            }
            let actual =
                inspected
                    .as_ref()
                    .map_or(ActualState::Absent, |runtime| match runtime.state {
                        ContainerState::Creating => ActualState::Creating,
                        ContainerState::Running => ActualState::Running,
                        ContainerState::Stopped => ActualState::Stopped,
                    });
            let converged = match operation.kind {
                OperationKind::Create => actual == ActualState::Running,
                OperationKind::Start => actual == ActualState::Running,
                OperationKind::Stop => actual == ActualState::Stopped,
                OperationKind::Destroy => actual == ActualState::Absent,
                OperationKind::Apply => {
                    matches!(actual, ActualState::Running | ActualState::Stopped)
                }
                OperationKind::Reconcile => true,
            };
            if converged {
                self.store.complete_operation(operation.id, actual)?;
            } else {
                self.store.fail_operation(
                    operation.id,
                    actual,
                    "interrupted_operation",
                    json!({"phase":"reconcile","actual":format!("{actual:?}")}),
                )?;
            }
        }
        Ok(())
    }

    async fn runtime_actual(&self, id: &SandboxId, fallback: ActualState) -> ActualState {
        self.runtime
            .inspect(id)
            .await
            .ok()
            .flatten()
            .map_or(fallback, |runtime| match runtime.state {
                ContainerState::Creating => ActualState::Creating,
                ContainerState::Running => ActualState::Running,
                ContainerState::Stopped => ActualState::Stopped,
            })
    }

    fn emit(
        &self,
        id: i64,
        details: Value,
        sender: &mpsc::Sender<OperationEvent>,
    ) -> Result<(), ServiceError> {
        let event = self.store.append_operation_event(id, details)?;
        sender
            .try_send(event)
            .map_err(|_| ServiceError::EventStreamUnavailable)
    }
    fn send_initial(
        &self,
        id: i64,
        sender: &mpsc::Sender<OperationEvent>,
    ) -> Result<(), ServiceError> {
        if let Some(event) = self.store.operation_events(id)?.first().cloned() {
            sender
                .try_send(event)
                .map_err(|_| ServiceError::EventStreamUnavailable)?;
        }
        Ok(())
    }
    fn send_terminal(
        &self,
        id: i64,
        sender: &mpsc::Sender<OperationEvent>,
    ) -> Result<(), ServiceError> {
        if let Some(event) = self.store.operation_events(id)?.last().cloned() {
            sender
                .try_send(event)
                .map_err(|_| ServiceError::EventStreamUnavailable)?;
        }
        Ok(())
    }
}

impl ServiceError {
    fn code(&self) -> &'static str {
        match self {
            Self::Runtime(error) => error.code(),
            Self::Policy(error) => error.code(),
            Self::Missing(_) => "not_found",
            Self::Ownership(_) => "ownership_mismatch",
            Self::Provision(_) => "provision_failed",
            Self::Store(_) => "store_error",
            Self::Sandbox(_) => "sandbox_error",
            Self::Manifest(_) => "manifest_error",
            Self::LockPoisoned => "lock_poisoned",
            Self::EventStreamUnavailable => "event_stream_unavailable",
        }
    }
}
