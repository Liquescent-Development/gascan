use crate::reconcile::{ReconcileFinding, ReconcileReport};
use crate::{
    ActualState, DesiredState, ImageResolution, OperationEvent, OperationId, OperationKind,
    OperationRecord, SandboxRecord, SetupResolution, Store, StoreError, ToolResolution,
};
use async_trait::async_trait;
use gascan_core::manifest::ManifestError;
use gascan_core::policy::{PolicyCompiler, PolicyError};
use gascan_core::runtime::{
    ContainerState, CreateFailure, CreateRequest, RemoveRequest, ResourceKind, ResourceOwnership,
    RuntimeBackend, RuntimeError,
};
use gascan_core::sandbox::{SandboxError, SandboxId, SandboxSpec};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, Weak};
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
    pub id: OperationId,
    pub events: mpsc::Receiver<OperationEvent>,
}

pub struct SandboxService<B: RuntimeBackend> {
    runtime: B,
    store: Store,
    provisioner: Arc<dyn Provisioner>,
    locks: Mutex<HashMap<SandboxId, Weak<AsyncMutex<()>>>>,
}

#[derive(Debug, Error)]
pub enum ServiceError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
    #[error(transparent)]
    Create(#[from] CreateFailure),
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
    #[error("database worker task failed: {0}")]
    DatabaseWorker(String),
    #[error("failed to fingerprint desired setup: {0}")]
    Fingerprint(String),
    #[error("destroy left expected owned resources for sandbox {0}")]
    IncompleteDestroy(SandboxId),
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

    async fn database<T, F>(&self, action: F) -> Result<T, ServiceError>
    where
        T: Send + 'static,
        F: FnOnce(Store) -> Result<T, StoreError> + Send + 'static,
    {
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || action(store))
            .await
            .map_err(|error| ServiceError::DatabaseWorker(error.to_string()))?
            .map_err(ServiceError::Store)
    }

    fn keyed_lock(&self, id: &SandboxId) -> Result<Arc<AsyncMutex<()>>, ServiceError> {
        let mut locks = self.locks.lock().map_err(|_| ServiceError::LockPoisoned)?;
        locks.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = locks.get(id).and_then(Weak::upgrade) {
            return Ok(lock);
        }
        let lock = Arc::new(AsyncMutex::new(()));
        locks.insert(id.clone(), Arc::downgrade(&lock));
        Ok(lock)
    }

    #[doc(hidden)]
    pub fn keyed_lock_count(&self) -> Result<usize, ServiceError> {
        let mut locks = self.locks.lock().map_err(|_| ServiceError::LockPoisoned)?;
        locks.retain(|_, lock| lock.strong_count() > 0);
        Ok(locks.len())
    }

    pub async fn up(&self, request: UpRequest) -> Result<Operation, ServiceError> {
        let id = request.spec.id().clone();
        let lock = self.keyed_lock(&id)?;
        let _guard = lock.lock().await;
        let capabilities = self.runtime.capabilities().await?;
        let create = PolicyCompiler::compile(request.spec.clone(), &capabilities)?;
        let desired_fingerprint = desired_fingerprint(&request.spec).await?;
        let existing = self
            .database({
                let id = id.clone();
                move |store| store.sandbox(&id)
            })
            .await?;
        let prior = existing.clone();
        let reuse_resolution = prior
            .as_ref()
            .is_some_and(|record| resolution_matches(record, &desired_fingerprint));
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
        if record.actual_state == ActualState::Absent {
            record.actual_state = ActualState::Creating;
        }
        let operation = self
            .database({
                let record = record.clone();
                move |store| store.begin_operation(&record, OperationKind::Create)
            })
            .await?;
        let (sender, receiver) = mpsc::channel(16);
        self.initialize_operation(operation.id, &id, record.actual_state, &sender)
            .await?;
        if let Err(error) = self
            .emit(operation.id, json!({"phase":"validated"}), &sender)
            .await
        {
            let actual = self.runtime_actual(&id, record.actual_state).await;
            let code = error.code();
            let details = json!({"message":error.to_string(),"phase":"validated"});
            let _ = self
                .database(move |store| store.fail_operation(operation.id, actual, code, details))
                .await;
            return Err(error);
        }
        let result = self
            .up_runtime(
                &request.spec,
                &create,
                prior.as_ref(),
                operation.id,
                &sender,
                &desired_fingerprint,
            )
            .await;
        match result {
            Ok((actual, resolution)) => {
                if !reuse_resolution {
                    record.setup_resolution = Some(SetupResolution::new(
                        1,
                        json!({"desired_fingerprint":desired_fingerprint,"resolution":resolution.setup}),
                    ));
                    record.tool_resolution = Some(ToolResolution::new(
                        1,
                        json!({"desired_fingerprint":desired_fingerprint,"resolution":resolution.tools}),
                    ));
                }
                record.actual_state = actual;
                self.database({
                    let record = record.clone();
                    move |store| store.put_sandbox(&record)
                })
                .await?;
                let terminal = self
                    .database(move |store| store.complete_operation(operation.id, actual))
                    .await?;
                self.send_terminal(terminal.id, &sender).await?;
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
                let code = error.code();
                let details = json!({"message":error.to_string()});
                self.database(move |store| {
                    store.fail_operation(operation.id, actual, code, details)
                })
                .await?;
                self.send_terminal(operation.id, &sender).await?;
                Err(error)
            }
        }
    }

    async fn up_runtime(
        &self,
        spec: &SandboxSpec,
        create: &CreateRequest,
        prior: Option<&SandboxRecord>,
        operation_id: OperationId,
        sender: &mpsc::Sender<OperationEvent>,
        desired_fingerprint: &str,
    ) -> Result<(ActualState, ProvisionResolution), ServiceError> {
        let id = spec.id();
        let inspected = self.runtime.inspect(id).await?;
        let mut created = None;
        if let Some(runtime) = &inspected {
            if runtime.ownership.managed_by != "gascan" || runtime.ownership.sandbox_id != *id {
                return Err(ServiceError::Ownership(id.clone()));
            }
        } else {
            match self.runtime.create(create.clone()).await {
                Ok(outcome) => created = Some(outcome),
                Err(failure) => {
                    if !failure.created().is_empty() {
                        self.runtime
                            .remove(RemoveRequest::from_resources(failure.created().to_vec())?)
                            .await?;
                    }
                    return Err(ServiceError::Create(failure));
                }
            }
            self.emit(operation_id, json!({"phase":"created"}), sender)
                .await?;
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
            self.emit(operation_id, json!({"phase":"started"}), sender)
                .await?;
            let durable_match = inspected.is_some()
                && prior.is_some_and(|record| resolution_matches(record, desired_fingerprint));
            let resolution = if !durable_match {
                self.emit(operation_id, json!({"phase":"before_provision","desired_fingerprint":desired_fingerprint}), sender).await?;
                self.provisioner
                    .provision(ProvisionRequest { spec, create })
                    .await?
            } else {
                ProvisionResolution::default()
            };
            if !durable_match {
                self.emit(operation_id, json!({"phase":"after_provision","resolution_version":1,"desired_fingerprint":desired_fingerprint,"setup":resolution.setup,"tools":resolution.tools}), sender).await?;
            }
            self.emit(operation_id, json!({"phase":"before_health"}), sender).await?;
            self.provisioner.health_check(id).await?;
            self.emit(operation_id, json!({"phase":"after_health","desired_fingerprint":desired_fingerprint}), sender).await?;
            Ok::<_, ServiceError>(resolution)
        }
        .await;
        match result {
            Ok(resolution) => Ok((ActualState::Running, resolution)),
            Err(error) if created.is_some() => {
                if let Some(outcome) = created {
                    if !outcome.created().is_empty() {
                        self.runtime
                            .remove(RemoveRequest::from_resources(outcome.created().to_vec())?)
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
            .database({
                let id = id.clone();
                move |store| store.sandbox(&id)
            })
            .await?
            .ok_or_else(|| ServiceError::Missing(id.clone()))?;
        record.desired_state = desired;
        let operation = self
            .database({
                let record = record.clone();
                move |store| store.begin_operation(&record, kind)
            })
            .await?;
        let (sender, receiver) = mpsc::channel(16);
        self.initialize_operation(operation.id, id, record.actual_state, &sender)
            .await?;
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
            let code = error.code();
            let details = json!({"message":error.to_string()});
            self.database(move |store| store.fail_operation(operation.id, actual, code, details))
                .await?;
            self.send_terminal(operation.id, &sender).await?;
            return Err(error);
        }
        self.database(move |store| store.complete_operation(operation.id, target))
            .await?;
        self.send_terminal(operation.id, &sender).await?;
        Ok(Operation {
            id: operation.id,
            events: receiver,
        })
    }

    pub async fn destroy(&self, id: &SandboxId) -> Result<Operation, ServiceError> {
        let lock = self.keyed_lock(id)?;
        let _guard = lock.lock().await;
        let mut record = self
            .database({
                let id = id.clone();
                move |store| store.sandbox(&id)
            })
            .await?
            .ok_or_else(|| ServiceError::Missing(id.clone()))?;
        let prior_actual = record.actual_state;
        record.desired_state = DesiredState::Absent;
        if record.actual_state != ActualState::Absent {
            record.actual_state = ActualState::Destroying;
        }
        let operation = self
            .database({
                let record = record.clone();
                move |store| store.begin_operation(&record, OperationKind::Destroy)
            })
            .await?;
        let (sender, receiver) = mpsc::channel(16);
        self.initialize_operation(operation.id, id, record.actual_state, &sender)
            .await?;
        let result = async {
            if let Some(runtime) = self.runtime.inspect(id).await? {
                if runtime.ownership.managed_by != "gascan" || runtime.ownership.sandbox_id != *id {
                    return Err(ServiceError::Ownership(id.clone()));
                }
                if runtime.state == ContainerState::Running {
                    self.runtime.stop(id).await?;
                }
            }
            let expected = PolicyCompiler::expected_resource_identities(id)?
                .into_iter()
                .collect::<HashSet<_>>();
            let resources = self
                .runtime
                .list_resources()
                .await?
                .into_iter()
                .filter(|resource| {
                    expected.contains(resource.identity())
                        && resource.sandbox_id() == Some(id)
                        && resource.ownership() == ResourceOwnership::GasCanOwned
                })
                .collect::<Vec<_>>();
            if !resources.is_empty() {
                self.runtime
                    .remove(RemoveRequest::from_resources(resources)?)
                    .await?;
            }
            let remaining = self
                .runtime
                .list_resources()
                .await?
                .into_iter()
                .any(|resource| {
                    expected.contains(resource.identity())
                        && resource.sandbox_id() == Some(id)
                        && resource.ownership() == ResourceOwnership::GasCanOwned
                });
            if remaining {
                return Err(ServiceError::IncompleteDestroy(id.clone()));
            }
            Ok::<_, ServiceError>(())
        }
        .await;
        if let Err(error) = result {
            let actual = self.runtime_actual(id, prior_actual).await;
            let code = error.code();
            let details = json!({"message":error.to_string()});
            self.database(move |store| store.fail_operation(operation.id, actual, code, details))
                .await?;
            self.send_terminal(operation.id, &sender).await?;
            return Err(error);
        }
        self.database(move |store| store.complete_operation(operation.id, ActualState::Absent))
            .await?;
        self.send_terminal(operation.id, &sender).await?;
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
        let desired_fingerprint = desired_fingerprint(&request.spec).await?;
        let mut record = self
            .database({
                let id = id.clone();
                move |store| store.sandbox(&id)
            })
            .await?
            .ok_or_else(|| ServiceError::Missing(id.clone()))?;
        let unchanged = resolution_matches(&record, &desired_fingerprint);
        let operation = self
            .database({
                let record = record.clone();
                move |store| store.begin_operation(&record, OperationKind::Apply)
            })
            .await?;
        let (sender, receiver) = mpsc::channel(16);
        self.initialize_operation(operation.id, &id, record.actual_state, &sender)
            .await?;
        let prior_actual = record.actual_state;
        let preflight = async {
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
            Ok::<_, ServiceError>(())
        }
        .await;
        if let Err(error) = preflight {
            let actual = self.runtime_actual(&id, prior_actual).await;
            let code = error.code();
            let details = json!({"message":error.to_string()});
            self.database(move |store| store.fail_operation(operation.id, actual, code, details))
                .await?;
            self.send_terminal(operation.id, &sender).await?;
            return Err(error);
        }
        if unchanged {
            self.database(move |store| {
                store.complete_operation(operation.id, ActualState::Running)
            })
            .await?;
            self.send_terminal(operation.id, &sender).await?;
            return Ok(Operation {
                id: operation.id,
                events: receiver,
            });
        }
        let result = async {
            self.emit(operation.id, json!({"phase":"before_provision","desired_fingerprint":desired_fingerprint}), &sender).await?;
            let resolution = self.provisioner
                .provision(ProvisionRequest {
                    spec: &request.spec,
                    create: &create,
                })
                .await?;
            self.emit(operation.id, json!({"phase":"after_provision","resolution_version":1,"desired_fingerprint":desired_fingerprint,"setup":resolution.setup,"tools":resolution.tools}), &sender).await?;
            self.emit(operation.id, json!({"phase":"before_health"}), &sender).await?;
            self.provisioner.health_check(&id).await?;
            self.emit(operation.id, json!({"phase":"after_health","desired_fingerprint":desired_fingerprint}), &sender).await?;
            Ok::<_, ServiceError>(resolution)
        }
        .await;
        let resolution = match result {
            Ok(resolution) => resolution,
            Err(error) => {
                let actual = self.runtime_actual(&id, prior_actual).await;
                let code = error.code();
                let details = json!({"message":error.to_string()});
                self.database(move |store| {
                    store.fail_operation(operation.id, actual, code, details)
                })
                .await?;
                self.send_terminal(operation.id, &sender).await?;
                return Err(error);
            }
        };
        record.setup_resolution = Some(SetupResolution::new(
            1,
            json!({"desired_fingerprint":desired_fingerprint,"resolution":resolution.setup}),
        ));
        record.tool_resolution = Some(ToolResolution::new(
            1,
            json!({"desired_fingerprint":desired_fingerprint,"resolution":resolution.tools}),
        ));
        record.actual_state = ActualState::Running;
        self.database({
            let record = record.clone();
            move |store| store.put_sandbox(&record)
        })
        .await?;
        self.database(move |store| store.complete_operation(operation.id, ActualState::Running))
            .await?;
        self.send_terminal(operation.id, &sender).await?;
        Ok(Operation {
            id: operation.id,
            events: receiver,
        })
    }

    pub async fn reconcile(&self) -> Result<ReconcileReport, ServiceError> {
        self.recover_pending().await?;
        let records = self.database(|store| store.list_sandboxes()).await?;
        let known = records
            .iter()
            .map(|record| record.id.clone())
            .collect::<HashSet<_>>();
        let expected = known
            .iter()
            .map(PolicyCompiler::expected_resource_identities)
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .flatten()
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
                    if resource.sandbox_id().is_none_or(|id| !known.contains(id))
                        || !expected.contains(resource.identity()) =>
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
        for operation in self.database(|store| store.pending_operations()).await? {
            let lock = self.keyed_lock(&operation.sandbox_id)?;
            let _guard = lock.lock().await;
            let still_pending = self
                .database({
                    let id = operation.id;
                    move |store| {
                        Ok(store
                            .pending_operations()?
                            .into_iter()
                            .any(|item| item.id == id))
                    }
                })
                .await?;
            if !still_pending {
                continue;
            }
            let events = self
                .database({
                    let id = operation.id;
                    move |store| store.operation_events(id)
                })
                .await?;
            let mut record = self
                .database({
                    let id = operation.sandbox_id.clone();
                    move |store| store.sandbox(&id)
                })
                .await?
                .ok_or_else(|| ServiceError::Missing(operation.sandbox_id.clone()))?;
            let hook_evidence = ordered_hook_evidence(&events, &record);
            let inspected = self.runtime.inspect(&operation.sandbox_id).await?;
            if inspected.as_ref().is_some_and(|runtime| {
                runtime.ownership.managed_by != "gascan"
                    || runtime.ownership.sandbox_id != operation.sandbox_id
            }) {
                let actual = record.actual_state;
                self.database(move |store| {
                    store.fail_operation(
                        operation.id,
                        actual,
                        "ownership_mismatch",
                        json!({"phase":"reconcile"}),
                    )
                })
                .await?;
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
            let expected_absent = if operation.kind == OperationKind::Destroy {
                let expected = PolicyCompiler::expected_resource_identities(&operation.sandbox_id)?
                    .into_iter()
                    .collect::<HashSet<_>>();
                !self.runtime.list_resources().await?.iter().any(|resource| {
                    expected.contains(resource.identity())
                        && resource.sandbox_id() == Some(&operation.sandbox_id)
                        && resource.ownership() == ResourceOwnership::GasCanOwned
                })
            } else {
                false
            };
            let converged = match operation.kind {
                OperationKind::Create => actual == ActualState::Running && hook_evidence,
                OperationKind::Start => actual == ActualState::Running,
                OperationKind::Stop => actual == ActualState::Stopped,
                OperationKind::Destroy => actual == ActualState::Absent && expected_absent,
                OperationKind::Apply => {
                    hook_evidence && matches!(actual, ActualState::Running | ActualState::Stopped)
                }
                OperationKind::Reconcile => true,
            };
            if converged {
                if matches!(operation.kind, OperationKind::Create | OperationKind::Apply) {
                    if let Some(details) = events
                        .iter()
                        .filter_map(|event| event.details.as_ref())
                        .find(|details| {
                            details.get("phase").and_then(Value::as_str) == Some("after_provision")
                        })
                    {
                        let fingerprint = details
                            .get("desired_fingerprint")
                            .cloned()
                            .unwrap_or(Value::Null);
                        record.setup_resolution = Some(SetupResolution::new(
                            1,
                            json!({"desired_fingerprint":fingerprint,"resolution":details.get("setup").cloned().unwrap_or(Value::Null)}),
                        ));
                        record.tool_resolution = Some(ToolResolution::new(
                            1,
                            json!({"desired_fingerprint":fingerprint,"resolution":details.get("tools").cloned().unwrap_or(Value::Null)}),
                        ));
                        record.actual_state = actual;
                        self.database({
                            let record = record.clone();
                            move |store| store.put_sandbox(&record)
                        })
                        .await?;
                    }
                }
                self.database(move |store| store.complete_operation(operation.id, actual))
                    .await?;
            } else {
                self.database(move |store| {
                    store.fail_operation(
                        operation.id,
                        actual,
                        "interrupted_operation",
                        json!({"phase":"reconcile","actual":format!("{actual:?}")}),
                    )
                })
                .await?;
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

    async fn emit(
        &self,
        id: OperationId,
        details: Value,
        sender: &mpsc::Sender<OperationEvent>,
    ) -> Result<(), ServiceError> {
        let event = self
            .database(move |store| store.append_operation_event(id, details))
            .await?;
        sender
            .try_send(event)
            .map_err(|_| ServiceError::EventStreamUnavailable)
    }
    async fn send_initial(
        &self,
        id: OperationId,
        sender: &mpsc::Sender<OperationEvent>,
    ) -> Result<(), ServiceError> {
        if let Some(event) = self
            .database(move |store| store.operation_events(id))
            .await?
            .first()
            .cloned()
        {
            sender
                .try_send(event)
                .map_err(|_| ServiceError::EventStreamUnavailable)?;
        }
        Ok(())
    }
    async fn send_terminal(
        &self,
        id: OperationId,
        sender: &mpsc::Sender<OperationEvent>,
    ) -> Result<(), ServiceError> {
        if let Some(event) = self
            .database(move |store| store.operation_events(id))
            .await?
            .last()
            .cloned()
        {
            sender
                .try_send(event)
                .map_err(|_| ServiceError::EventStreamUnavailable)?;
        }
        Ok(())
    }

    async fn initialize_operation(
        &self,
        operation_id: OperationId,
        sandbox_id: &SandboxId,
        fallback: ActualState,
        sender: &mpsc::Sender<OperationEvent>,
    ) -> Result<(), ServiceError> {
        if let Err(error) = self.send_initial(operation_id, sender).await {
            let actual = self.runtime_actual(sandbox_id, fallback).await;
            let code = error.code();
            let details = json!({"message":error.to_string(),"phase":"initial_event"});
            let _ = self
                .database(move |store| store.fail_operation(operation_id, actual, code, details))
                .await;
            return Err(error);
        }
        Ok(())
    }
}

impl ServiceError {
    fn code(&self) -> &'static str {
        match self {
            Self::Runtime(error) => error.code(),
            Self::Create(error) => error.code(),
            Self::Policy(error) => error.code(),
            Self::Missing(_) => "not_found",
            Self::Ownership(_) => "ownership_mismatch",
            Self::Provision(_) => "provision_failed",
            Self::Store(_) => "store_error",
            Self::Sandbox(_) => "sandbox_error",
            Self::Manifest(_) => "manifest_error",
            Self::LockPoisoned => "lock_poisoned",
            Self::EventStreamUnavailable => "event_stream_unavailable",
            Self::DatabaseWorker(_) => "database_worker_failed",
            Self::Fingerprint(_) => "fingerprint_failed",
            Self::IncompleteDestroy(_) => "incomplete_destroy",
        }
    }
}

fn resolution_matches(record: &SandboxRecord, fingerprint: &str) -> bool {
    let matches = |details: &Value| {
        details.get("desired_fingerprint").and_then(Value::as_str) == Some(fingerprint)
    };
    record
        .setup_resolution
        .as_ref()
        .is_some_and(|value| matches(&value.details))
        && record
            .tool_resolution
            .as_ref()
            .is_some_and(|value| matches(&value.details))
}

fn ordered_hook_evidence(events: &[OperationEvent], record: &SandboxRecord) -> bool {
    let phases = events
        .iter()
        .filter_map(|event| event.details.as_ref())
        .filter_map(|details| {
            Some((
                details.get("phase")?.as_str()?,
                details.get("desired_fingerprint").and_then(Value::as_str),
                details.get("resolution_version").and_then(Value::as_u64),
            ))
        })
        .collect::<Vec<_>>();
    let before_health = phases
        .iter()
        .position(|(phase, _, _)| *phase == "before_health");
    let after_health = phases
        .iter()
        .rposition(|(phase, _, _)| *phase == "after_health");
    let (Some(before_health), Some(after_health)) = (before_health, after_health) else {
        return false;
    };
    if before_health >= after_health {
        return false;
    }
    let Some(health_fingerprint) = phases[after_health].1 else {
        return false;
    };
    let before_provision = phases
        .iter()
        .position(|(phase, _, _)| *phase == "before_provision");
    let after_provision = phases
        .iter()
        .position(|(phase, _, version)| *phase == "after_provision" && *version == Some(1));
    match (before_provision, after_provision) {
        (Some(before), Some(after)) if before < after && after < before_health => {
            phases[after].1 == Some(health_fingerprint)
        }
        (None, None) => resolution_matches(record, health_fingerprint),
        _ => false,
    }
}

async fn desired_fingerprint(spec: &SandboxSpec) -> Result<String, ServiceError> {
    let root = spec.canonical_root().to_owned();
    let setup = spec.manifest().setup().map(ToOwned::to_owned);
    let tools = spec.manifest().tools().clone();
    tokio::task::spawn_blocking(move || {
        let setup_bytes = setup
            .map(|path| std::fs::read(root.join(path)))
            .transpose()
            .map_err(|error| ServiceError::Fingerprint(error.to_string()))?;
        let mut hash = Sha256::new();
        hash.update(serde_json::to_vec(&tools).map_err(StoreError::Json)?);
        if let Some(bytes) = setup_bytes {
            hash.update(bytes);
        }
        Ok(format!("sha256:{:x}", hash.finalize()))
    })
    .await
    .map_err(|error| ServiceError::DatabaseWorker(error.to_string()))?
}
