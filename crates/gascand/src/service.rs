use crate::reconcile::{ReconcileFinding, ReconcileReport};
use crate::{
    ActualState, DesiredState, ImageResolution, OperationEvent, OperationId, OperationKind,
    OperationRecord, SandboxRecord, SetupResolution, StorageResolution, Store, StoreError,
    ToolResolution,
};
use async_trait::async_trait;
use gascan_core::doctor::{DoctorFacts, DoctorReport};
use gascan_core::manifest::ManifestError;
use gascan_core::policy::{
    CONTAINER_PATH, MISE_CACHE_DIR, MISE_DATA_DIR, MISE_GLOBAL_CONFIG_FILE, MISE_SYSTEM_DATA_DIR,
    PolicyCompiler, PolicyError, WORKSPACE_HOME,
};
use gascan_core::provision::{
    AppliedState, ProvisionPlan, ProvisionStep, ProvisioningPlanner, SetupScript,
};
use gascan_core::runtime::{
    ContainerState, CreateFailure, CreateOutcome, CreateRequest, ExecInput, ExecOutput,
    ExecRequest, RecreateRequest, RemoveRequest, ResourceIdentity, ResourceKind, ResourceOwnership,
    RetainedResources, RuntimeBackend, RuntimeCapabilities, RuntimeError, RuntimeResource,
    immutable_image_reference, same_immutable_image,
};
use gascan_core::sandbox::{SandboxError, SandboxId, SandboxSpec};
use serde::de::{Error as _, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex, Weak};
use thiserror::Error;
use tokio::sync::{Mutex as AsyncMutex, mpsc};

const SAFE_MISE_WORKDIR: &str = "/home/workspace/.config/gascan/mise-workdir";
const MAX_PROVISION_STDOUT_BYTES: usize = 1024 * 1024;
const MAX_PROVISION_STDERR_TAIL_BYTES: usize = 64 * 1024;

struct GuestExecOutcome {
    stdout: Vec<u8>,
    stderr_tail: Vec<u8>,
    code: i32,
    signal: i32,
}

struct UpRuntimeContext<'a> {
    operation_id: OperationId,
    sender: &'a mpsc::Sender<OperationEvent>,
    desired_fingerprint: &'a str,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ImageState {
    recorded: Option<String>,
    running: String,
    approved: String,
}

impl ImageState {
    fn change_required(&self) -> bool {
        !self
            .recorded
            .as_deref()
            .is_some_and(|recorded| same_immutable_image(recorded, &self.approved))
            || !same_immutable_image(&self.running, &self.approved)
    }
}

struct BoundedTail {
    bytes: Vec<u8>,
    limit: usize,
}

impl BoundedTail {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
        }
    }

    fn extend(&mut self, bytes: &[u8]) {
        if self.limit == 0 {
            self.bytes.clear();
            return;
        }
        if bytes.len() >= self.limit {
            self.bytes.clear();
            self.bytes
                .extend_from_slice(&bytes[bytes.len() - self.limit..]);
            return;
        }
        let overflow = self
            .bytes
            .len()
            .saturating_add(bytes.len())
            .saturating_sub(self.limit);
        if overflow > 0 {
            self.bytes.drain(..overflow);
        }
        self.bytes.extend_from_slice(bytes);
    }

    fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StorageCapacityChange {
    pub volume: &'static str,
    pub recorded_bytes: Option<u64>,
    pub requested_bytes: u64,
}

struct ProvisionedResolution {
    resolution: ProvisionResolution,
    tool_hash: String,
}

#[derive(Deserialize)]
struct MiseToolRecord {
    version: String,
    installed: bool,
    active: bool,
}

struct MiseInventory(BTreeMap<String, Vec<MiseToolRecord>>);

impl<'de> Deserialize<'de> for MiseInventory {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct InventoryVisitor;

        impl<'de> Visitor<'de> for InventoryVisitor {
            type Value = MiseInventory;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a mise tool inventory object with unique tool keys")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut records = BTreeMap::new();
                while let Some((tool, versions)) =
                    map.next_entry::<String, Vec<MiseToolRecord>>()?
                {
                    if records.insert(tool, versions).is_some() {
                        return Err(A::Error::custom("duplicate mise tool key"));
                    }
                }
                Ok(MiseInventory(records))
            }
        }

        deserializer.deserialize_map(InventoryVisitor)
    }
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

pub(crate) type OperationStart = Result<Operation, ServiceError>;
type OperationStarted = mpsc::Sender<OperationStart>;

fn publish_operation(
    started: Option<OperationStarted>,
    id: OperationId,
    receiver: mpsc::Receiver<OperationEvent>,
) -> Option<mpsc::Receiver<OperationEvent>> {
    if let Some(started) = started {
        let _ = started.try_send(Ok(Operation {
            id,
            events: receiver,
        }));
        None
    } else {
        Some(receiver)
    }
}

pub struct SandboxService<B: RuntimeBackend> {
    runtime: B,
    store: Store,
    provisioner: Arc<dyn Provisioner>,
    workspace_image: Option<String>,
    locks: Mutex<HashMap<SandboxId, Weak<AsyncMutex<()>>>>,
    doctor: DoctorState,
    capabilities: tokio::sync::OnceCell<RuntimeCapabilities>,
}

#[derive(Clone)]
pub struct DoctorState {
    receiver: tokio::sync::watch::Receiver<Option<DoctorReport>>,
}

pub struct DoctorCompleter {
    sender: tokio::sync::watch::Sender<Option<DoctorReport>>,
}

impl DoctorState {
    pub fn ready(report: DoctorReport) -> Self {
        let (_sender, receiver) = tokio::sync::watch::channel(Some(report));
        Self { receiver }
    }

    pub fn pending() -> (Self, DoctorCompleter) {
        let (sender, receiver) = tokio::sync::watch::channel(None);
        (Self { receiver }, DoctorCompleter { sender })
    }

    pub fn collect<F>(timeout: std::time::Duration, collector: F) -> Self
    where
        F: std::future::Future<Output = DoctorReport> + Send + 'static,
    {
        let (state, completer) = Self::pending();
        tokio::spawn(async move {
            let report = tokio::time::timeout(timeout, collector)
                .await
                .unwrap_or_else(|_| {
                    DoctorFacts::unavailable(format!(
                        "runtime evidence collector exceeded its {} second bound",
                        timeout.as_secs()
                    ))
                    .into_report()
                });
            completer.complete(report);
        });
        state
    }

    pub async fn report(&self) -> DoctorReport {
        let mut receiver = self.receiver.clone();
        loop {
            if let Some(report) = receiver.borrow().clone() {
                return report;
            }
            if receiver.changed().await.is_err() {
                return DoctorFacts::unavailable("runtime evidence collection was abandoned")
                    .into_report();
            }
        }
    }
}

impl DoctorCompleter {
    pub fn complete(self, report: DoctorReport) {
        self.sender.send_replace(Some(report));
    }
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
    #[error("{}", format_storage_change_message(changes))]
    StorageChangeRequiresRecreate { changes: Vec<StorageCapacityChange> },
    #[error(
        "workspace image replacement cannot safely continue: {cause}; current image {current}; requested image {requested}; run `gascan apply` again"
    )]
    ImageUpgradeRequired {
        current: String,
        requested: String,
        cause: String,
    },
    #[error("compiled storage invariant failed: {0}")]
    StorageInvariant(&'static str),
    #[error("provisioning failed: {0}")]
    Provision(String),
    #[error(
        "provisioning failed: guest provisioning command failed (exit code {exit_code}, signal {signal}): {stderr_tail}"
    )]
    ProvisionCommandFailed {
        step: ProvisionStep,
        action: &'static str,
        exit_code: i32,
        signal: i32,
        stderr_tail: String,
    },
    #[error("mounted setup script changed before execution")]
    SetupChanged,
    #[error(
        "setup action {action} failed with exit code {exit_code}, signal {signal}: {stderr_tail}"
    )]
    SetupCommandFailed {
        action: &'static str,
        exit_code: i32,
        signal: i32,
        stderr_tail: String,
    },
    #[error("mounted setup script changed before execution; stopped state could not be confirmed")]
    SetupChangedStopUnconfirmed,
    #[error(
        "setup action {action} failed with exit code {exit_code}, signal {signal}: {stderr_tail}; stopped state could not be confirmed"
    )]
    SetupCommandFailedStopUnconfirmed {
        action: &'static str,
        exit_code: i32,
        signal: i32,
        stderr_tail: String,
    },
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
    #[error("{original}; rollback failed: {rollback}")]
    Rollback {
        original: Box<ServiceError>,
        rollback: RuntimeError,
    },
    #[error("{original}; rollback failed: {rollback}")]
    ImageRollback {
        original: Box<ServiceError>,
        rollback: Box<ServiceError>,
    },
    #[error("{original}; recording operation failure failed: {reporting}")]
    FailureReporting {
        original: Box<ServiceError>,
        reporting: Box<ServiceError>,
    },
}

impl<B: RuntimeBackend> SandboxService<B> {
    pub fn new(runtime: B, store: Store, provisioner: Arc<dyn Provisioner>) -> Self {
        Self::new_with_doctor(runtime, store, provisioner, default_doctor_report())
    }

    pub fn new_with_doctor(
        runtime: B,
        store: Store,
        provisioner: Arc<dyn Provisioner>,
        doctor: DoctorReport,
    ) -> Self {
        Self::new_with_doctor_state(runtime, store, provisioner, DoctorState::ready(doctor))
    }

    pub fn new_with_doctor_state(
        runtime: B,
        store: Store,
        provisioner: Arc<dyn Provisioner>,
        doctor: DoctorState,
    ) -> Self {
        Self {
            runtime,
            store,
            provisioner,
            workspace_image: None,
            locks: Mutex::new(HashMap::new()),
            doctor,
            capabilities: tokio::sync::OnceCell::new(),
        }
    }

    pub fn new_with_doctor_state_for_image(
        runtime: B,
        store: Store,
        provisioner: Arc<dyn Provisioner>,
        doctor: DoctorState,
        workspace_image: String,
    ) -> Result<Self, PolicyError> {
        if !immutable_image_reference(&workspace_image) {
            return Err(PolicyError::InvalidWorkspaceImage);
        }
        Ok(Self {
            runtime,
            store,
            provisioner,
            workspace_image: Some(workspace_image),
            locks: Mutex::new(HashMap::new()),
            doctor,
            capabilities: tokio::sync::OnceCell::new(),
        })
    }

    fn compile_policy(
        &self,
        spec: SandboxSpec,
        capabilities: &RuntimeCapabilities,
    ) -> Result<CreateRequest, PolicyError> {
        PolicyCompiler::compile_for_image(spec, capabilities, self.workspace_image())
    }

    #[must_use]
    pub fn workspace_image(&self) -> &str {
        match &self.workspace_image {
            Some(image) => image,
            None => PolicyCompiler::workspace_image(),
        }
    }

    pub const fn store(&self) -> &Store {
        &self.store
    }

    pub async fn exec(
        &self,
        id: &SandboxId,
        argv: Vec<String>,
        stdin: Vec<u8>,
        environment: std::collections::BTreeMap<String, String>,
        tty: bool,
    ) -> Result<gascan_core::runtime::ExecSession, ServiceError> {
        self.require_owned_running(id).await?;
        self.runtime
            .exec(gascan_core::runtime::ExecRequest {
                id: id.clone(),
                argv,
                stdin,
                environment,
                tty,
            })
            .await
            .map_err(Into::into)
    }

    pub async fn validate_exec(&self, id: &SandboxId) -> Result<(), ServiceError> {
        self.require_owned_running(id).await
    }

    pub async fn logs(
        &self,
        id: &SandboxId,
        since_millis: Option<i64>,
    ) -> Result<Vec<u8>, ServiceError> {
        self.require_owned(id).await?;
        self.runtime
            .logs(id, since_millis)
            .await
            .map_err(Into::into)
    }

    async fn require_owned_running(&self, id: &SandboxId) -> Result<(), ServiceError> {
        let sandbox = self.require_owned(id).await?;
        if sandbox.state != ContainerState::Running {
            return Err(ServiceError::Runtime(RuntimeError::InvalidState {
                resource: id.to_string(),
                message: "exec requires a running sandbox".to_owned(),
            }));
        }
        Ok(())
    }

    async fn require_owned(
        &self,
        id: &SandboxId,
    ) -> Result<gascan_core::runtime::RuntimeSandbox, ServiceError> {
        let sandbox = self
            .runtime
            .inspect(id)
            .await?
            .ok_or_else(|| ServiceError::Missing(id.clone()))?;
        if sandbox.ownership.managed_by != "gascan" || sandbox.ownership.sandbox_id != *id {
            return Err(ServiceError::Runtime(RuntimeError::OwnershipMismatch {
                resource: id.to_string(),
            }));
        }
        Ok(sandbox)
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

    pub async fn doctor_report(&self) -> DoctorReport {
        self.doctor.report().await
    }

    pub async fn require_runtime_ready(&self) -> Result<(), ServiceError> {
        let report = self.doctor_report().await;
        if let Some(check) = report
            .checks
            .into_iter()
            .find(|check| check.status != gascan_core::doctor::DoctorStatus::Pass)
        {
            return Err(ServiceError::Runtime(RuntimeError::UnsupportedCapability {
                capability: format!("{}: {}; remedy: {}", check.id, check.detail, check.remedy),
            }));
        }
        Ok(())
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

    async fn runtime_capabilities(&self) -> Result<&RuntimeCapabilities, ServiceError> {
        self.capabilities
            .get_or_try_init(|| async { self.runtime.capabilities().await })
            .await
            .map_err(ServiceError::Runtime)
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
        self.up_inner(request, None)
            .await?
            .ok_or(ServiceError::EventStreamUnavailable)
    }

    pub(crate) async fn up_started(
        &self,
        request: UpRequest,
        started: OperationStarted,
    ) -> Result<(), ServiceError> {
        self.up_inner(request, Some(started)).await.map(drop)
    }

    async fn up_inner(
        &self,
        request: UpRequest,
        started: Option<OperationStarted>,
    ) -> Result<Option<Operation>, ServiceError> {
        let id = request.spec.id().clone();
        let lock = self.keyed_lock(&id)?;
        let _guard = lock.lock().await;
        let capabilities = self.runtime_capabilities().await?;
        let create = self.compile_policy(request.spec.clone(), capabilities)?;
        let requested_storage = requested_storage(&create)?;
        let existing = self
            .database({
                let id = id.clone();
                move |store| store.sandbox(&id)
            })
            .await?;
        if let Some(prior) = existing
            .as_ref()
            .filter(|record| record.actual_state != ActualState::Absent)
        {
            validate_storage_capacities(prior, requested_storage)?;
        }
        let desired_fingerprint = desired_fingerprint(&request.spec).await?;
        let prior = existing.clone();
        let mut record = existing.unwrap_or_else(|| SandboxRecord {
            id: id.clone(),
            canonical_root: request.spec.canonical_root().to_owned(),
            desired_state: DesiredState::Running,
            actual_state: ActualState::Creating,
            setup_resolution: None,
            tool_resolution: None,
            image_resolution: Some(ImageResolution::new(1, json!({"digest": create.image()}))),
            storage_resolution: None,
            ssh_resolution: None,
            last_operation_id: None,
            updated_at_millis: 0,
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
        let receiver = publish_operation(started, operation.id, receiver);
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
                requested_storage,
                UpRuntimeContext {
                    operation_id: operation.id,
                    sender: &sender,
                    desired_fingerprint: &desired_fingerprint,
                },
            )
            .await;
        match result {
            Ok((actual, provisioned)) => {
                if let Some(provisioned) = provisioned {
                    let resolution = provisioned.resolution;
                    record.setup_resolution = Some(SetupResolution::new(
                        1,
                        json!({"desired_fingerprint":desired_fingerprint,"resolution":resolution.setup}),
                    ));
                    record.tool_resolution = Some(ToolResolution::new(
                        1,
                        json!({"desired_fingerprint":desired_fingerprint,"tool_hash":provisioned.tool_hash,"resolution":resolution.tools}),
                    ));
                }
                record.storage_resolution = Some(storage_resolution(requested_storage));
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
                Ok(receiver.map(|events| Operation {
                    id: operation.id,
                    events,
                }))
            }
            Err(error) => {
                let actual = if error.setup_stop_confirmed() {
                    ActualState::Stopped
                } else {
                    self.runtime_actual(&id, ActualState::Absent).await
                };
                let code = error.code();
                let details = failure_details(&error);
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
        requested_storage: StorageCapacities,
        context: UpRuntimeContext<'_>,
    ) -> Result<(ActualState, Option<ProvisionedResolution>), ServiceError> {
        let UpRuntimeContext {
            operation_id,
            sender,
            desired_fingerprint,
        } = context;
        let id = spec.id();
        let inspected = self.runtime.inspect(id).await?;
        let mut created = None;
        if let Some(runtime) = &inspected {
            if runtime.ownership.managed_by != "gascan" || runtime.ownership.sandbox_id != *id {
                return Err(ServiceError::Ownership(id.clone()));
            }
            let state = image_state(prior, &runtime.image, create.image())?;
            if state.change_required() {
                self.emit(
                    operation_id,
                    json!({
                        "phase": "apply_required",
                        "reason": "image_changed",
                        "recorded_image": state.recorded,
                        "running_image": state.running,
                        "approved_image": state.approved,
                    }),
                    sender,
                )
                .await?;
                let actual = match runtime.state {
                    ContainerState::Creating => ActualState::Creating,
                    ContainerState::Running => ActualState::Running,
                    ContainerState::Stopped => ActualState::Stopped,
                };
                return Ok((actual, None));
            }
        } else {
            match self.runtime.create(create.clone()).await {
                Ok(outcome) => {
                    if let Err(error) = self.persist_created_storage(id, requested_storage).await {
                        return Err(self.rollback_created(id, outcome, error).await);
                    }
                    created = Some(outcome);
                }
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
            let image_state = image_state(prior, &current.image, create.image())?;
            let durable_match = if let Some(record) = prior.filter(|_| inspected.is_some()) {
                resolution_matches(record, desired_fingerprint)
                    && tool_state_matches(record, spec.canonical_root(), spec.manifest())?
                    && !image_state.change_required()
            } else {
                false
            };
            let has_complete_durable_resolution = prior.is_some_and(|record| {
                record.setup_resolution.is_some() && record.tool_resolution.is_some()
            });
            let provisioned = if inspected.is_some()
                && !durable_match
                && has_complete_durable_resolution
            {
                let applied = applied_state(prior);
                let plan = ProvisioningPlanner::plan_for_root(
                    spec.canonical_root(),
                    spec.manifest(),
                    &applied,
                )
                    .map_err(|_| ServiceError::Provision("could not plan provisioning".to_owned()))?;
                let reason = if plan.setup_changed() {
                    "setup_changed"
                } else if plan.tools_changed() {
                    "tools_changed"
                } else {
                    "desired_content_changed"
                };
                self.emit(operation_id, json!({"phase":"apply_required","reason":reason,"desired_fingerprint":desired_fingerprint}), sender).await?;
                None
            } else if !durable_match {
                self.emit(operation_id, json!({"phase":"before_provision","desired_fingerprint":desired_fingerprint}), sender).await?;
                let prior_for_provision = if inspected.is_none() { None } else { prior };
                let provisioned = self
                    .provision_explicit(
                        spec,
                        create,
                        prior_for_provision,
                        operation_id,
                        sender,
                    )
                    .await?;
                self.emit(operation_id, json!({"phase":"after_provision","resolution_version":1,"desired_fingerprint":desired_fingerprint,"setup":provisioned.resolution.setup,"tools":provisioned.resolution.tools,"tool_hash":provisioned.tool_hash}), sender).await?;
                Some(provisioned)
            } else {
                None
            };
            self.emit(operation_id, json!({"phase":"before_health","step":ProvisionStep::HealthCheck.as_str()}), sender).await?;
            self.provisioner.health_check(id).await?;
            self.emit(operation_id, json!({"phase":"after_health","desired_fingerprint":desired_fingerprint}), sender).await?;
            Ok::<_, ServiceError>(provisioned)
        }
        .await;
        match result {
            Ok(provisioned) => Ok((ActualState::Running, provisioned)),
            Err(error) if error.is_setup_failure() => Err(error),
            Err(error) if created.is_some() => {
                if let Some(outcome) = created {
                    return Err(self.rollback_created(id, outcome, error).await);
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

    async fn provision_explicit(
        &self,
        spec: &SandboxSpec,
        create: &CreateRequest,
        prior: Option<&SandboxRecord>,
        operation_id: OperationId,
        sender: &mpsc::Sender<OperationEvent>,
    ) -> Result<ProvisionedResolution, ServiceError> {
        let applied = applied_state(prior);
        self.provision_with_applied(spec, create, prior, applied, operation_id, sender)
            .await
    }

    async fn provision_with_applied(
        &self,
        spec: &SandboxSpec,
        create: &CreateRequest,
        prior: Option<&SandboxRecord>,
        applied: AppliedState,
        operation_id: OperationId,
        sender: &mpsc::Sender<OperationEvent>,
    ) -> Result<ProvisionedResolution, ServiceError> {
        let plan =
            ProvisioningPlanner::plan_for_root(spec.canonical_root(), spec.manifest(), &applied)
                .map_err(|_| ServiceError::Provision("could not plan provisioning".to_owned()))?;
        self.initialize_managed_volume_roots(spec).await?;
        let resolved_tools = if plan.tools_changed() {
            Some(
                self.install_tools(spec, &plan, operation_id, sender)
                    .await?,
            )
        } else {
            prior.and_then(stored_tool_resolution)
        };
        if plan.steps().contains(&ProvisionStep::RunSetup) {
            self.emit_provision_step(operation_id, ProvisionStep::RunSetup, sender)
                .await?;
            let setup = plan.setup_script().ok_or_else(|| {
                ServiceError::Provision("setup execution was not planned".to_owned())
            })?;
            if let Err(error) = self.run_setup(spec.id(), setup).await {
                return Err(self.finalize_setup_failure(spec.id(), error).await);
            }
        }
        let resolved_setup = plan.setup_script().map(|setup| {
            json!({
                "canonical_relative_path": setup.canonical_relative_path(),
                "sha256": setup.sha256(),
            })
        });
        let mut resolution = self
            .provisioner
            .provision(ProvisionRequest { spec, create })
            .await?;
        if plan.setup_script().is_some() {
            resolution.setup = resolved_setup;
        }
        if let Some(tools) = resolved_tools {
            resolution.tools = Some(tools);
        }
        if plan.steps().contains(&ProvisionStep::VerifyGascamp) {
            self.emit_provision_step(operation_id, ProvisionStep::VerifyGascamp, sender)
                .await?;
            self.verify_gascamp(spec).await?;
        }
        Ok(ProvisionedResolution {
            resolution,
            tool_hash: plan.desired_tool_hash().to_owned(),
        })
    }

    async fn retained_resources(
        &self,
        create: &CreateRequest,
    ) -> Result<(RuntimeResource, RetainedResources), ServiceError> {
        let resources = self.runtime.list_resources().await?;
        let container = exact_owned_container(create, &resources)?.ok_or_else(|| {
            ServiceError::Runtime(RuntimeError::InvalidState {
                resource: create.id().to_string(),
                message: "expected exactly one owned container resource".to_owned(),
            })
        })?;
        let retained = resources
            .into_iter()
            .filter(|resource| {
                resource.ownership() == ResourceOwnership::GasCanOwned
                    && resource.sandbox_id() == Some(create.id())
                    && resource.kind() != ResourceKind::Container
            })
            .collect();
        Ok((
            container,
            RetainedResources::new(create, retained).map_err(ServiceError::Runtime)?,
        ))
    }

    async fn remove_exact_container(
        &self,
        create: &CreateRequest,
        container: RuntimeResource,
    ) -> Result<(), ServiceError> {
        if let Some(runtime) = self.runtime.inspect(create.id()).await? {
            if runtime.ownership.managed_by != "gascan"
                || runtime.ownership.sandbox_id != *create.id()
            {
                return Err(ServiceError::Ownership(create.id().clone()));
            }
            if runtime.state == ContainerState::Running {
                self.runtime.stop(create.id()).await?;
            }
        }
        self.runtime
            .remove(RemoveRequest::from_resources(vec![container])?)
            .await?;
        Ok(())
    }

    async fn rollback_image(
        &self,
        create: &CreateRequest,
        previous_image: &str,
        retained: RetainedResources,
        operation_id: OperationId,
        sender: &mpsc::Sender<OperationEvent>,
    ) -> Result<(), ServiceError> {
        let resources = self.runtime.list_resources().await?;
        let container = exact_owned_container(create, &resources)?;
        if let Some(container) = container {
            self.remove_exact_container(create, container).await?;
        }
        let rollback =
            RecreateRequest::for_image(create.clone(), previous_image.to_owned(), retained)?;
        self.runtime
            .create_container(rollback)
            .await
            .map_err(ServiceError::Create)?;
        self.runtime.start(create.id()).await?;
        self.emit(
            operation_id,
            json!({"phase":"image_rollback","image":previous_image}),
            sender,
        )
        .await
    }

    async fn replace_image(
        &self,
        spec: &SandboxSpec,
        create: &CreateRequest,
        previous_image: &str,
        operation_id: OperationId,
        sender: &mpsc::Sender<OperationEvent>,
    ) -> Result<ProvisionedResolution, ServiceError> {
        self.emit(
            operation_id,
            json!({"phase":"before_image_replace","previous_image":previous_image,"approved_image":create.image()}),
            sender,
        )
        .await?;
        self.runtime.prepare_image(create.image()).await?;
        let runtime = self
            .runtime
            .inspect(create.id())
            .await?
            .ok_or_else(|| ServiceError::Missing(create.id().clone()))?;
        let (container, retained) = self
            .replacement_evidence(create, &runtime, previous_image)
            .await?;
        let rollback_retained = retained.clone();
        self.emit(
            operation_id,
            json!({"phase":"image_replacing","image":create.image()}),
            sender,
        )
        .await?;
        let mut mutation_started = false;
        let result = async {
            mutation_started = true;
            if runtime.state == ContainerState::Running {
                self.runtime.stop(create.id()).await?;
            }
            self.runtime
                .remove(RemoveRequest::from_resources(vec![container])?)
                .await?;
            let recreate = RecreateRequest::new(create.clone(), retained)?;
            if let Err(failure) = self.runtime.create_container(recreate).await {
                let partial = failure.created().to_vec();
                let original = ServiceError::Create(failure);
                if !partial.is_empty() {
                    let [container] = partial.as_slice() else {
                        return Err(ServiceError::ImageRollback {
                            original: Box::new(original),
                            rollback: Box::new(ServiceError::Runtime(RuntimeError::InvalidState {
                                resource: create.id().to_string(),
                                message: "replacement failure returned non-container evidence"
                                    .to_owned(),
                            })),
                        });
                    };
                    let expected =
                        ResourceIdentity::new(ResourceKind::Container, create.id().to_string())?;
                    if container.identity() != &expected {
                        return Err(ServiceError::ImageRollback {
                            original: Box::new(original),
                            rollback: Box::new(ServiceError::Runtime(
                                RuntimeError::OwnershipMismatch {
                                    resource: container.name().to_owned(),
                                },
                            )),
                        });
                    }
                    if let Err(rollback) =
                        self.remove_exact_container(create, container.clone()).await
                    {
                        return Err(ServiceError::ImageRollback {
                            original: Box::new(original),
                            rollback: Box::new(rollback),
                        });
                    }
                }
                return Err(original);
            }
            self.runtime.start(create.id()).await?;
            let prior = self
                .database({
                    let id = create.id().clone();
                    move |store| store.sandbox(&id)
                })
                .await?
                .ok_or_else(|| ServiceError::Missing(create.id().clone()))?;
            self.emit(operation_id, json!({"phase":"before_provision"}), sender)
                .await?;
            let provisioned = self
                .provision_with_applied(
                    spec,
                    create,
                    Some(&prior),
                    replacement_applied_state(&prior),
                    operation_id,
                    sender,
                )
                .await?;
            self.emit(
                operation_id,
                json!({
                    "phase":"after_provision",
                    "resolution_version":1,
                    "setup":provisioned.resolution.setup,
                    "tools":provisioned.resolution.tools,
                    "tool_hash":provisioned.tool_hash,
                }),
                sender,
            )
            .await?;
            self.emit(
                operation_id,
                json!({"phase":"before_health","step":ProvisionStep::HealthCheck.as_str()}),
                sender,
            )
            .await?;
            self.provisioner.health_check(create.id()).await?;
            self.emit(operation_id, json!({"phase":"after_health"}), sender)
                .await?;
            self.emit(
                operation_id,
                json!({"phase":"image_replaced","image":create.image()}),
                sender,
            )
            .await?;
            self.emit(
                operation_id,
                json!({"phase":"after_image_replace","image":create.image()}),
                sender,
            )
            .await?;
            Ok::<_, ServiceError>(provisioned)
        }
        .await;
        match result {
            Ok(provisioned) => Ok(provisioned),
            Err(original) if mutation_started => {
                match self
                    .rollback_image(
                        create,
                        previous_image,
                        rollback_retained,
                        operation_id,
                        sender,
                    )
                    .await
                {
                    Ok(()) => Err(original),
                    Err(rollback) => Err(ServiceError::ImageRollback {
                        original: Box::new(original),
                        rollback: Box::new(rollback),
                    }),
                }
            }
            Err(error) => Err(error),
        }
    }

    async fn replacement_evidence(
        &self,
        create: &CreateRequest,
        runtime: &gascan_core::runtime::RuntimeSandbox,
        expected_previous: &str,
    ) -> Result<(RuntimeResource, RetainedResources), ServiceError> {
        let precondition = |cause: String| ServiceError::ImageUpgradeRequired {
            current: runtime.image.clone(),
            requested: create.image().to_owned(),
            cause,
        };
        if runtime.image != expected_previous {
            return Err(precondition(format!(
                "runtime image changed after preflight from {expected_previous}"
            )));
        }
        if runtime.ownership.managed_by != "gascan" || runtime.ownership.sandbox_id != *create.id()
        {
            return Err(precondition(
                "sandbox ownership changed after preflight".to_owned(),
            ));
        }
        self.retained_resources(create)
            .await
            .map_err(|error| precondition(error.to_string()))
    }

    async fn persist_created_storage(
        &self,
        id: &SandboxId,
        requested_storage: StorageCapacities,
    ) -> Result<(), ServiceError> {
        let mut record = self
            .database({
                let id = id.clone();
                move |store| store.sandbox(&id)
            })
            .await?
            .ok_or_else(|| ServiceError::Missing(id.clone()))?;
        record.storage_resolution = Some(storage_resolution(requested_storage));
        self.database(move |store| store.put_sandbox(&record)).await
    }

    async fn rollback_created(
        &self,
        id: &SandboxId,
        outcome: CreateOutcome,
        original: ServiceError,
    ) -> ServiceError {
        let rollback = async {
            let current =
                self.runtime
                    .inspect(id)
                    .await?
                    .ok_or_else(|| RuntimeError::NotFound {
                        resource: id.to_string(),
                    })?;
            if current.ownership.managed_by != "gascan" || current.ownership.sandbox_id != *id {
                return Err(RuntimeError::OwnershipMismatch {
                    resource: id.to_string(),
                });
            }
            if current.state == ContainerState::Running {
                self.runtime.stop(id).await?;
            }
            self.runtime
                .remove(RemoveRequest::from_resources(outcome.created().to_vec())?)
                .await
        }
        .await;
        match rollback {
            Ok(()) => original,
            Err(rollback) => ServiceError::Rollback {
                original: Box::new(original),
                rollback,
            },
        }
    }

    async fn run_setup(&self, id: &SandboxId, setup: &SetupScript) -> Result<(), ServiceError> {
        let guest_path = format!("/workspace/{}", setup.canonical_relative_path());
        let digest_outcome = self
            .exec_guest_raw(
                id,
                ["/usr/bin/sha256sum".to_owned(), guest_path.clone()],
                Vec::new(),
            )
            .await?;
        if digest_outcome.code != 0 || digest_outcome.signal != 0 {
            return Err(setup_command_failure("verify_setup_digest", digest_outcome));
        }
        let digest = std::str::from_utf8(&digest_outcome.stdout)
            .ok()
            .and_then(|output| output.split_ascii_whitespace().next())
            .filter(|digest| {
                digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
            .ok_or(ServiceError::SetupChanged)?;
        if setup.sha256().strip_prefix("sha256:") != Some(digest) {
            return Err(ServiceError::SetupChanged);
        }
        let outcome = self
            .exec_guest_raw(id, ["/bin/bash".to_owned(), guest_path], Vec::new())
            .await?;
        if outcome.code == 0 && outcome.signal == 0 {
            Ok(())
        } else {
            Err(setup_command_failure("run_setup", outcome))
        }
    }

    async fn finalize_setup_failure(&self, id: &SandboxId, error: ServiceError) -> ServiceError {
        let stop_succeeded = self.runtime.stop(id).await.is_ok();
        let stopped = matches!(
            self.runtime.inspect(id).await,
            Ok(Some(runtime)) if runtime.state == ContainerState::Stopped
        );
        if stop_succeeded && stopped {
            error
        } else {
            error.with_unconfirmed_setup_stop()
        }
    }

    async fn install_tools(
        &self,
        spec: &SandboxSpec,
        plan: &ProvisionPlan,
        operation_id: OperationId,
        sender: &mpsc::Sender<OperationEvent>,
    ) -> Result<Value, ServiceError> {
        self.emit_provision_step(operation_id, ProvisionStep::WriteSafeMiseConfig, sender)
            .await?;
        self.exec_guest(
            spec.id(),
            ProvisionStep::WriteSafeMiseConfig,
            "reset_safe_mise_workdir",
            [
                "/usr/bin/rm",
                "--recursive",
                "--force",
                "--",
                SAFE_MISE_WORKDIR,
            ],
            Vec::new(),
        )
        .await?;
        self.exec_guest(
            spec.id(),
            ProvisionStep::WriteSafeMiseConfig,
            "create_safe_mise_workdir",
            ["/usr/bin/install", "-d", "-m", "0700", SAFE_MISE_WORKDIR],
            Vec::new(),
        )
        .await?;
        let config = plan
            .safe_mise_toml()
            .map_err(|_| {
                ServiceError::Provision("could not serialize safe mise config".to_owned())
            })?
            .ok_or_else(|| {
                ServiceError::Provision("safe mise config was not planned".to_owned())
            })?;
        self.exec_guest(
            spec.id(),
            ProvisionStep::WriteSafeMiseConfig,
            "write_safe_mise_config",
            [
                "/usr/bin/install",
                "-m",
                "0600",
                "/dev/stdin",
                MISE_GLOBAL_CONFIG_FILE,
            ],
            config.into_bytes(),
        )
        .await?;

        self.emit_provision_step(operation_id, ProvisionStep::InstallTools, sender)
            .await?;
        self.exec_guest(
            spec.id(),
            ProvisionStep::InstallTools,
            "install_tools",
            mise_command(&["install", "--yes"]),
            Vec::new(),
        )
        .await?;
        let output = self
            .exec_guest(
                spec.id(),
                ProvisionStep::InstallTools,
                "list_installed_tools",
                mise_command(&["ls", "--current", "--installed", "--json"]),
                Vec::new(),
            )
            .await?;
        let resolved = parse_mise_versions(&output, spec.manifest().tools())?;
        serde_json::to_value(resolved)
            .map_err(|_| ServiceError::Provision("could not encode resolved tools".to_owned()))
    }

    async fn initialize_managed_volume_roots(
        &self,
        spec: &SandboxSpec,
    ) -> Result<(), ServiceError> {
        self.exec_guest(
            spec.id(),
            ProvisionStep::WriteSafeMiseConfig,
            "initialize_managed_volume_roots",
            [
                "/usr/bin/sudo",
                "-n",
                "/usr/bin/install",
                "-d",
                "-o",
                "workspace",
                "-g",
                "workspace",
                "-m",
                "0700",
                MISE_DATA_DIR,
                "/home/workspace/.cache",
                "/home/workspace/.config/gascan",
            ],
            Vec::new(),
        )
        .await?;
        self.exec_guest(
            spec.id(),
            ProvisionStep::WriteSafeMiseConfig,
            "initialize_workstation_home",
            [
                "/usr/bin/env",
                "HOME=/home/workspace",
                "/usr/local/bin/configure-workstation-home",
            ],
            Vec::new(),
        )
        .await
        .map(|_| ())
    }

    async fn verify_gascamp(&self, spec: &SandboxSpec) -> Result<(), ServiceError> {
        let requested = spec
            .manifest()
            .gascamp()
            .workspace_path()
            .map_or_else(|| "bundled".to_owned(), ToString::to_string);
        let output = self
            .exec_guest(
                spec.id(),
                ProvisionStep::VerifyGascamp,
                "verify_gascamp",
                ["/usr/local/bin/select-gascamp".to_owned(), requested],
                Vec::new(),
            )
            .await?;
        let value: Value = serde_json::from_slice(&output).map_err(|_| {
            ServiceError::Provision("invalid Gascamp verification output".to_owned())
        })?;
        if !value.is_object() {
            return Err(ServiceError::Provision(
                "invalid Gascamp verification output".to_owned(),
            ));
        }
        Ok(())
    }

    async fn exec_guest<I, S>(
        &self,
        id: &SandboxId,
        step: ProvisionStep,
        action: &'static str,
        argv: I,
        stdin: Vec<u8>,
    ) -> Result<Vec<u8>, ServiceError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let outcome = self.exec_guest_raw(id, argv, stdin).await?;
        if outcome.code == 0 && outcome.signal == 0 {
            Ok(outcome.stdout)
        } else {
            Err(ServiceError::ProvisionCommandFailed {
                step,
                action,
                exit_code: outcome.code,
                signal: outcome.signal,
                stderr_tail: sanitize_provision_stderr(&outcome.stderr_tail),
            })
        }
    }

    async fn exec_guest_raw<I, S>(
        &self,
        id: &SandboxId,
        argv: I,
        stdin: Vec<u8>,
    ) -> Result<GuestExecOutcome, ServiceError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut session = self
            .runtime
            .exec(ExecRequest {
                id: id.clone(),
                argv: argv.into_iter().map(Into::into).collect(),
                stdin,
                environment: BTreeMap::new(),
                tty: false,
            })
            .await
            .map_err(|_| provisioning_transport_error())?;
        session
            .send(ExecInput::Close)
            .await
            .map_err(|_| provisioning_transport_error())?;
        let mut stdout = Vec::new();
        let mut stderr_tail = BoundedTail::new(MAX_PROVISION_STDERR_TAIL_BYTES);
        while let Some(output) = session.next().await {
            match output.map_err(|_| provisioning_transport_error())? {
                ExecOutput::Stdout(bytes) => {
                    if stdout.len().saturating_add(bytes.len()) > MAX_PROVISION_STDOUT_BYTES {
                        session.cancel();
                        while session.next().await.is_some() {}
                        return Err(ServiceError::Provision(
                            "guest provisioning stdout exceeded its limit".to_owned(),
                        ));
                    }
                    stdout.extend(bytes);
                }
                ExecOutput::Stderr(bytes) => stderr_tail.extend(&bytes),
                ExecOutput::Exit { code, signal } => {
                    return Ok(GuestExecOutcome {
                        stdout,
                        stderr_tail: stderr_tail.into_bytes(),
                        code,
                        signal,
                    });
                }
            }
        }
        Err(ServiceError::Provision(
            "guest provisioning command ended without status".to_owned(),
        ))
    }

    async fn emit_provision_step(
        &self,
        operation_id: OperationId,
        step: ProvisionStep,
        sender: &mpsc::Sender<OperationEvent>,
    ) -> Result<(), ServiceError> {
        self.emit(
            operation_id,
            json!({"phase":"provision_step","step":step.as_str()}),
            sender,
        )
        .await
    }

    pub async fn start(&self, id: &SandboxId) -> Result<Operation, ServiceError> {
        self.simple_state(
            id,
            OperationKind::Start,
            DesiredState::Running,
            ActualState::Running,
            None,
        )
        .await?
        .ok_or(ServiceError::EventStreamUnavailable)
    }
    pub async fn stop(&self, id: &SandboxId) -> Result<Operation, ServiceError> {
        self.simple_state(
            id,
            OperationKind::Stop,
            DesiredState::Stopped,
            ActualState::Stopped,
            None,
        )
        .await?
        .ok_or(ServiceError::EventStreamUnavailable)
    }

    pub(crate) async fn stop_started(
        &self,
        id: &SandboxId,
        started: OperationStarted,
    ) -> Result<(), ServiceError> {
        self.simple_state(
            id,
            OperationKind::Stop,
            DesiredState::Stopped,
            ActualState::Stopped,
            Some(started),
        )
        .await
        .map(drop)
    }

    async fn simple_state(
        &self,
        id: &SandboxId,
        kind: OperationKind,
        desired: DesiredState,
        target: ActualState,
        started: Option<OperationStarted>,
    ) -> Result<Option<Operation>, ServiceError> {
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
        let receiver = publish_operation(started, operation.id, receiver);
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
        Ok(receiver.map(|events| Operation {
            id: operation.id,
            events,
        }))
    }

    pub async fn destroy(&self, id: &SandboxId) -> Result<Operation, ServiceError> {
        self.destroy_inner(id, None)
            .await?
            .ok_or(ServiceError::EventStreamUnavailable)
    }

    pub(crate) async fn destroy_started(
        &self,
        id: &SandboxId,
        started: OperationStarted,
    ) -> Result<(), ServiceError> {
        self.destroy_inner(id, Some(started)).await.map(drop)
    }

    async fn destroy_inner(
        &self,
        id: &SandboxId,
        started: Option<OperationStarted>,
    ) -> Result<Option<Operation>, ServiceError> {
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
        let receiver = publish_operation(started, operation.id, receiver);
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
        Ok(receiver.map(|events| Operation {
            id: operation.id,
            events,
        }))
    }

    pub async fn apply(&self, request: UpRequest) -> Result<Operation, ServiceError> {
        self.apply_inner(request, None)
            .await?
            .ok_or(ServiceError::EventStreamUnavailable)
    }

    pub(crate) async fn apply_started(
        &self,
        request: UpRequest,
        started: OperationStarted,
    ) -> Result<(), ServiceError> {
        self.apply_inner(request, Some(started)).await.map(drop)
    }

    async fn apply_inner(
        &self,
        request: UpRequest,
        started: Option<OperationStarted>,
    ) -> Result<Option<Operation>, ServiceError> {
        let id = request.spec.id().clone();
        let lock = self.keyed_lock(&id)?;
        let _guard = lock.lock().await;
        let capabilities = self.runtime_capabilities().await?;
        let create = self.compile_policy(request.spec.clone(), capabilities)?;
        let requested_storage = requested_storage(&create)?;
        let mut record = self
            .database({
                let id = id.clone();
                move |store| store.sandbox(&id)
            })
            .await?
            .ok_or_else(|| ServiceError::Missing(id.clone()))?;
        validate_storage_capacities(&record, requested_storage)?;
        let desired_fingerprint = desired_fingerprint(&request.spec).await?;
        let desired_plan = ProvisioningPlanner::plan_for_root(
            request.spec.canonical_root(),
            request.spec.manifest(),
            &applied_state(Some(&record)),
        )
        .map_err(|_| ServiceError::Provision("could not plan provisioning".to_owned()))?;
        let setup_changed = desired_plan.setup_changed();
        let unchanged = resolution_matches(&record, &desired_fingerprint)
            && !desired_plan.tools_changed()
            && !setup_changed;
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
            let image = image_state(Some(&record), &runtime.image, create.image())?;
            if image.change_required() {
                return Ok::<_, ServiceError>((runtime, image));
            }
            if runtime.state == ContainerState::Running && setup_changed {
                self.runtime.stop(&id).await?;
                self.runtime.start(&id).await?;
            } else if runtime.state != ContainerState::Running {
                self.runtime.start(&id).await?;
            }
            Ok::<_, ServiceError>((runtime, image))
        }
        .await;
        let (runtime, image) = match preflight {
            Ok(value) => value,
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
        if image.change_required() {
            if let Err(error) = self
                .replacement_evidence(&create, &runtime, &runtime.image)
                .await
            {
                let actual = self.runtime_actual(&id, prior_actual).await;
                let code = error.code();
                let details = failure_details(&error);
                self.database(move |store| {
                    store.fail_operation(operation.id, actual, code, details)
                })
                .await?;
                self.send_terminal(operation.id, &sender).await?;
                return Err(error);
            }
        }
        let receiver = publish_operation(started, operation.id, receiver);
        if image.change_required() {
            let previous_record = record.clone();
            let provisioned = match self
                .replace_image(
                    &request.spec,
                    &create,
                    &runtime.image,
                    operation.id,
                    &sender,
                )
                .await
            {
                Ok(provisioned) => provisioned,
                Err(error) => {
                    let actual = self.runtime_actual(&id, prior_actual).await;
                    let (code, details) =
                        if matches!(error, ServiceError::ImageUpgradeRequired { .. }) {
                            (error.code(), failure_details(&error))
                        } else {
                            (
                                gascan_proto::error_code::IMAGE_REPLACEMENT_FAILED,
                                image_replacement_failure_details(&error),
                            )
                        };
                    if let Err(reporting) = self
                        .database(move |store| {
                            store.fail_operation(operation.id, actual, code, details)
                        })
                        .await
                    {
                        return Err(ServiceError::FailureReporting {
                            original: Box::new(error),
                            reporting: Box::new(reporting),
                        });
                    }
                    if let Err(reporting) = self.send_terminal(operation.id, &sender).await {
                        return Err(ServiceError::FailureReporting {
                            original: Box::new(error),
                            reporting: Box::new(reporting),
                        });
                    }
                    return Err(error);
                }
            };
            record.setup_resolution = Some(SetupResolution::new(
                1,
                json!({"desired_fingerprint":desired_fingerprint,"resolution":provisioned.resolution.setup}),
            ));
            record.tool_resolution = Some(ToolResolution::new(
                1,
                json!({"desired_fingerprint":desired_fingerprint,"tool_hash":provisioned.tool_hash,"resolution":provisioned.resolution.tools}),
            ));
            record.image_resolution =
                Some(ImageResolution::new(1, json!({"digest": create.image()})));
            record.actual_state = ActualState::Running;
            let commit = async {
                self.database({
                    let record = record.clone();
                    move |store| store.put_sandbox(&record)
                })
                .await?;
                self.database(move |store| store.operation_events(operation.id))
                    .await?;
                let (_, terminal) = self
                    .database(move |store| {
                        store.complete_operation_with_event(operation.id, ActualState::Running)
                    })
                    .await?;
                let _ = sender.try_send(terminal);
                Ok::<_, ServiceError>(())
            }
            .await;
            if let Err(original) = commit {
                let runtime_rollback = async {
                    let (_, retained) = self.retained_resources(&create).await?;
                    self.rollback_image(&create, &runtime.image, retained, operation.id, &sender)
                        .await
                }
                .await;
                let record_rollback = self
                    .database(move |store| store.put_sandbox(&previous_record))
                    .await;
                let failure = match (runtime_rollback, record_rollback) {
                    (Ok(()), Ok(())) => original,
                    (Err(runtime), Ok(())) => ServiceError::ImageRollback {
                        original: Box::new(original),
                        rollback: Box::new(runtime),
                    },
                    (Ok(()), Err(store)) => ServiceError::ImageRollback {
                        original: Box::new(original),
                        rollback: Box::new(store),
                    },
                    (Err(runtime), Err(store)) => ServiceError::ImageRollback {
                        original: Box::new(original),
                        rollback: Box::new(ServiceError::ImageRollback {
                            original: Box::new(runtime),
                            rollback: Box::new(store),
                        }),
                    },
                };
                let actual = self.runtime_actual(&id, prior_actual).await;
                let code = gascan_proto::error_code::IMAGE_REPLACEMENT_FAILED;
                let details = image_replacement_failure_details(&failure);
                let terminal = self
                    .database(move |store| {
                        store.fail_operation_with_event(operation.id, actual, code, details)
                    })
                    .await;
                match terminal {
                    Ok((_, event)) => {
                        let _ = sender.try_send(event);
                        return Err(failure);
                    }
                    Err(reporting) => {
                        return Err(ServiceError::FailureReporting {
                            original: Box::new(failure),
                            reporting: Box::new(reporting),
                        });
                    }
                }
            }
            return Ok(receiver.map(|events| Operation {
                id: operation.id,
                events,
            }));
        }
        if unchanged {
            self.database(move |store| {
                store.complete_operation(operation.id, ActualState::Running)
            })
            .await?;
            self.send_terminal(operation.id, &sender).await?;
            return Ok(receiver.map(|events| Operation {
                id: operation.id,
                events,
            }));
        }
        let result = async {
            self.emit(operation.id, json!({"phase":"before_provision","desired_fingerprint":desired_fingerprint}), &sender).await?;
            let provisioned = self
                .provision_explicit(&request.spec, &create, Some(&record), operation.id, &sender)
                .await?;
            self.emit(operation.id, json!({"phase":"after_provision","resolution_version":1,"desired_fingerprint":desired_fingerprint,"setup":provisioned.resolution.setup,"tools":provisioned.resolution.tools,"tool_hash":provisioned.tool_hash}), &sender).await?;
            self.emit(operation.id, json!({"phase":"before_health","step":ProvisionStep::HealthCheck.as_str()}), &sender).await?;
            self.provisioner.health_check(&id).await?;
            self.emit(operation.id, json!({"phase":"after_health","desired_fingerprint":desired_fingerprint}), &sender).await?;
            Ok::<_, ServiceError>(provisioned)
        }
        .await;
        let provisioned = match result {
            Ok(provisioned) => provisioned,
            Err(error) => {
                let actual = if error.setup_stop_confirmed() {
                    ActualState::Stopped
                } else {
                    self.runtime_actual(&id, prior_actual).await
                };
                let code = error.code();
                let details = failure_details(&error);
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
            json!({"desired_fingerprint":desired_fingerprint,"resolution":provisioned.resolution.setup}),
        ));
        record.tool_resolution = Some(ToolResolution::new(
            1,
            json!({"desired_fingerprint":desired_fingerprint,"tool_hash":provisioned.tool_hash,"resolution":provisioned.resolution.tools}),
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
        Ok(receiver.map(|events| Operation {
            id: operation.id,
            events,
        }))
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
                            json!({"desired_fingerprint":fingerprint,"tool_hash":details.get("tool_hash").cloned().unwrap_or(Value::Null),"resolution":details.get("tools").cloned().unwrap_or(Value::Null)}),
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
        let _ = sender.try_send(event);
        Ok(())
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
            let _ = sender.try_send(event);
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
            let _ = sender.try_send(event);
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

#[cfg(debug_assertions)]
fn default_doctor_report() -> DoctorReport {
    DoctorFacts::all_supported_for_tests().into_report()
}

#[cfg(not(debug_assertions))]
fn default_doctor_report() -> DoctorReport {
    DoctorFacts::unavailable("no production doctor evidence was supplied").into_report()
}

impl ServiceError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Runtime(error) => error.code(),
            Self::Create(error) => error.code(),
            Self::Policy(error) => error.code(),
            Self::Missing(_) => "not_found",
            Self::Ownership(_) => "ownership_mismatch",
            Self::StorageChangeRequiresRecreate { .. } => "storage_change_requires_recreate",
            Self::ImageUpgradeRequired { .. } => gascan_proto::error_code::IMAGE_UPGRADE_REQUIRED,
            Self::StorageInvariant(_) => "storage_invariant_failed",
            Self::Provision(_)
            | Self::ProvisionCommandFailed { .. }
            | Self::SetupChanged
            | Self::SetupCommandFailed { .. }
            | Self::SetupChangedStopUnconfirmed
            | Self::SetupCommandFailedStopUnconfirmed { .. } => "provision_failed",
            Self::Store(_) => "store_error",
            Self::Sandbox(_) => "sandbox_error",
            Self::Manifest(_) => "manifest_error",
            Self::LockPoisoned => "lock_poisoned",
            Self::EventStreamUnavailable => "event_stream_unavailable",
            Self::DatabaseWorker(_) => "database_worker_failed",
            Self::Fingerprint(_) => "fingerprint_failed",
            Self::IncompleteDestroy(_) => "incomplete_destroy",
            Self::Rollback { original, .. }
            | Self::ImageRollback { original, .. }
            | Self::FailureReporting { original, .. } => original.code(),
        }
    }

    const fn is_setup_failure(&self) -> bool {
        matches!(
            self,
            Self::SetupChanged
                | Self::SetupCommandFailed { .. }
                | Self::SetupChangedStopUnconfirmed
                | Self::SetupCommandFailedStopUnconfirmed { .. }
        )
    }

    const fn setup_stop_confirmed(&self) -> bool {
        matches!(self, Self::SetupChanged | Self::SetupCommandFailed { .. })
    }

    const fn setup_exit_code(&self) -> Option<i32> {
        match self {
            Self::SetupCommandFailed { exit_code, .. }
            | Self::SetupCommandFailedStopUnconfirmed { exit_code, .. } => Some(*exit_code),
            _ => None,
        }
    }

    fn with_unconfirmed_setup_stop(self) -> Self {
        match self {
            Self::SetupChanged | Self::SetupChangedStopUnconfirmed => {
                Self::SetupChangedStopUnconfirmed
            }
            Self::SetupCommandFailed {
                action,
                exit_code,
                signal,
                stderr_tail,
            }
            | Self::SetupCommandFailedStopUnconfirmed {
                action,
                exit_code,
                signal,
                stderr_tail,
            } => Self::SetupCommandFailedStopUnconfirmed {
                action,
                exit_code,
                signal,
                stderr_tail,
            },
            other => other,
        }
    }
}

pub(crate) fn failure_details(error: &ServiceError) -> Value {
    if let ServiceError::StorageChangeRequiresRecreate { changes } = error {
        json!({
            "message": error.to_string(),
            "changes": changes.iter().map(|change| json!({
                "volume": change.volume,
                "recorded_bytes": change.recorded_bytes,
                "requested_bytes": change.requested_bytes,
            })).collect::<Vec<_>>(),
        })
    } else if let ServiceError::ImageUpgradeRequired {
        current,
        requested,
        cause,
    } = error
    {
        json!({
            "message": error.to_string(),
            "reason": "image_changed",
            "current": current,
            "requested": requested,
            "cause": cause,
            "recovery": "run `gascan apply` again",
        })
    } else if let ServiceError::ProvisionCommandFailed {
        step,
        action,
        exit_code,
        signal,
        stderr_tail,
    } = error
    {
        json!({
            "message": error.to_string(),
            "step": step.as_str(),
            "action": action,
            "exit_code": exit_code,
            "signal": signal,
            "stderr_tail": stderr_tail,
        })
    } else if error.is_setup_failure() {
        let mut details = json!({
            "message": error.to_string(),
            "phase": "setup",
            "retryable": true,
            "stopped": error.setup_stop_confirmed(),
        });
        if let Some(exit_code) = error.setup_exit_code() {
            details["exit_code"] = Value::from(exit_code);
        }
        if let ServiceError::SetupCommandFailed {
            action,
            signal,
            stderr_tail,
            ..
        }
        | ServiceError::SetupCommandFailedStopUnconfirmed {
            action,
            signal,
            stderr_tail,
            ..
        } = error
        {
            details["action"] = Value::from(*action);
            details["signal"] = Value::from(*signal);
            details["stderr_tail"] = Value::from(stderr_tail.clone());
        }
        details
    } else {
        json!({"message":error.to_string()})
    }
}

fn image_replacement_failure_details(error: &ServiceError) -> Value {
    let (primary, rollback) = match error {
        ServiceError::ImageRollback { original, rollback } => {
            (original.as_ref(), Some(rollback.as_ref()))
        }
        other => (other, None),
    };
    json!({
        "message": error.to_string(),
        "primary": {
            "code": primary.code(),
            "message": primary.to_string(),
        },
        "rollback": rollback.map(|rollback| json!({
            "code": rollback.code(),
            "message": rollback.to_string(),
        })),
    })
}

type StorageCapacities = [(&'static str, u64); 3];

fn storage_resolution(requested: StorageCapacities) -> StorageResolution {
    StorageResolution::new(
        1,
        json!({
            "tools_bytes": requested[0].1,
            "cache_bytes": requested[1].1,
            "config_bytes": requested[2].1,
        }),
    )
}

fn validate_storage_capacities(
    record: &SandboxRecord,
    requested: StorageCapacities,
) -> Result<(), ServiceError> {
    let recorded = record
        .storage_resolution
        .as_ref()
        .filter(|resolution| resolution.version == 1);
    let changes = requested
        .into_iter()
        .filter_map(|(volume, requested_bytes)| {
            let recorded_bytes = recorded
                .and_then(|resolution| resolution.details.get(format!("{volume}_bytes")))
                .and_then(Value::as_u64);
            (recorded_bytes != Some(requested_bytes)).then_some(StorageCapacityChange {
                volume,
                recorded_bytes,
                requested_bytes,
            })
        })
        .collect::<Vec<_>>();
    if changes.is_empty() {
        Ok(())
    } else {
        Err(ServiceError::StorageChangeRequiresRecreate { changes })
    }
}

fn requested_storage(create: &CreateRequest) -> Result<StorageCapacities, ServiceError> {
    requested_storage_from_volumes(create.volumes())
}

fn requested_storage_from_volumes(
    volumes: &[gascan_core::runtime::RuntimeVolume],
) -> Result<StorageCapacities, ServiceError> {
    [
        ("tools", "/home/workspace/.local/share/mise"),
        ("cache", "/home/workspace/.cache"),
        ("config", "/home/workspace/.config/gascan"),
    ]
    .into_iter()
    .map(|(volume, target)| {
        let mut matching = volumes
            .iter()
            .filter(|candidate| candidate.target.as_str() == target);
        let capacity = matching
            .next()
            .ok_or(ServiceError::StorageInvariant(
                "managed volume is missing from compiled create request",
            ))?
            .capacity_bytes;
        if matching.next().is_some() {
            return Err(ServiceError::StorageInvariant(
                "managed volume is duplicated in compiled create request",
            ));
        }
        Ok((volume, capacity))
    })
    .collect::<Result<Vec<_>, _>>()?
    .try_into()
    .map_err(|_| ServiceError::StorageInvariant("managed volume count is not three"))
}

fn format_storage_change_message(changes: &[StorageCapacityChange]) -> String {
    let changes = changes
        .iter()
        .map(|change| {
            let recorded = change
                .recorded_bytes
                .map_or_else(|| "unknown".to_owned(), format_binary_size);
            format!(
                "{} ({recorded} → {})",
                change.volume,
                format_binary_size(change.requested_bytes)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "storage settings changed for {changes}; run `gascan destroy --yes` and `gascan up` to recreate the sandbox"
    )
}

fn format_binary_size(bytes: u64) -> String {
    for (suffix, divisor) in [
        ("TiB", 1024_u64.pow(4)),
        ("GiB", 1024_u64.pow(3)),
        ("MiB", 1024_u64.pow(2)),
        ("KiB", 1024_u64),
    ] {
        if bytes % divisor == 0 {
            return format!("{}{suffix}", bytes / divisor);
        }
    }
    format!("{bytes} bytes")
}

fn applied_state(record: Option<&SandboxRecord>) -> AppliedState {
    let tool_hash = record
        .and_then(|record| record.tool_resolution.as_ref())
        .and_then(|resolution| resolution.details.get("tool_hash"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let setup_sha256 = record
        .and_then(|record| record.setup_resolution.as_ref())
        .and_then(|resolution| resolution.details.get("resolution"))
        .and_then(|resolution| resolution.get("sha256"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    AppliedState::with_hashes(tool_hash, setup_sha256)
}

fn replacement_applied_state(record: &SandboxRecord) -> AppliedState {
    let tool_hash = record
        .tool_resolution
        .as_ref()
        .and_then(|resolution| resolution.details.get("tool_hash"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    AppliedState::with_hashes(tool_hash, None)
}

fn exact_owned_container(
    create: &CreateRequest,
    resources: &[RuntimeResource],
) -> Result<Option<RuntimeResource>, ServiceError> {
    let mut containers = resources.iter().filter(|resource| {
        resource.kind() == ResourceKind::Container
            && resource.ownership() == ResourceOwnership::GasCanOwned
            && resource.sandbox_id() == Some(create.id())
    });
    let Some(container) = containers.next() else {
        return Ok(None);
    };
    if containers.next().is_some() {
        return Err(ServiceError::Runtime(RuntimeError::InvalidState {
            resource: create.id().to_string(),
            message: "expected at most one owned container resource".to_owned(),
        }));
    }
    let expected = ResourceIdentity::new(ResourceKind::Container, create.id().to_string())?;
    if container.identity() != &expected {
        return Err(ServiceError::Runtime(RuntimeError::OwnershipMismatch {
            resource: container.name().to_owned(),
        }));
    }
    Ok(Some(container.clone()))
}

fn image_state(
    record: Option<&SandboxRecord>,
    running: &str,
    approved: &str,
) -> Result<ImageState, RuntimeError> {
    if !immutable_image_reference(running) {
        return Err(RuntimeError::InvalidOutput {
            operation: "runtime image inspection".to_owned(),
            message: "running workspace image is not digest-qualified".to_owned(),
        });
    }
    if !immutable_image_reference(approved) {
        return Err(RuntimeError::InvalidOutput {
            operation: "workspace image policy".to_owned(),
            message: "approved workspace image is not digest-qualified".to_owned(),
        });
    }
    Ok(ImageState {
        recorded: record.and_then(stored_image),
        running: running.to_owned(),
        approved: approved.to_owned(),
    })
}

fn stored_image(record: &SandboxRecord) -> Option<String> {
    let resolution = record.image_resolution.as_ref()?;
    if resolution.version != 1 {
        return None;
    }
    resolution
        .details
        .get("digest")?
        .as_str()
        .filter(|value| immutable_image_reference(value))
        .map(ToOwned::to_owned)
}

fn provisioning_transport_error() -> ServiceError {
    ServiceError::Provision("guest provisioning transport failed".to_owned())
}

fn setup_command_failure(action: &'static str, outcome: GuestExecOutcome) -> ServiceError {
    ServiceError::SetupCommandFailed {
        action,
        exit_code: outcome.code,
        signal: outcome.signal,
        stderr_tail: sanitize_provision_stderr(&outcome.stderr_tail),
    }
}

fn sanitize_provision_stderr(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .chars()
        .map(|character| {
            if character.is_ascii_control() {
                ' '
            } else {
                character
            }
        })
        .collect()
}

fn mise_command(args: &[&str]) -> Vec<String> {
    let mut argv = vec![
        "/usr/bin/env".to_owned(),
        format!("HOME={WORKSPACE_HOME}"),
        format!("MISE_CACHE_DIR={MISE_CACHE_DIR}"),
        format!("MISE_CEILING_PATHS={SAFE_MISE_WORKDIR}"),
        format!("MISE_DATA_DIR={MISE_DATA_DIR}"),
        format!("MISE_GLOBAL_CONFIG_FILE={MISE_GLOBAL_CONFIG_FILE}"),
        format!("MISE_SYSTEM_CONFIG_FILE={MISE_GLOBAL_CONFIG_FILE}"),
        format!("MISE_SYSTEM_DATA_DIR={MISE_SYSTEM_DATA_DIR}"),
        format!("PATH={CONTAINER_PATH}"),
        "/usr/local/bin/mise".to_owned(),
        "--cd".to_owned(),
        SAFE_MISE_WORKDIR.to_owned(),
        "--no-env".to_owned(),
        "--no-hooks".to_owned(),
    ];
    argv.extend(args.iter().map(|arg| (*arg).to_owned()));
    argv
}

fn stored_tool_resolution(record: &SandboxRecord) -> Option<Value> {
    record
        .tool_resolution
        .as_ref()
        .and_then(|resolution| resolution.details.get("resolution"))
        .cloned()
}

fn tool_state_matches(
    record: &SandboxRecord,
    canonical_root: &camino::Utf8Path,
    manifest: &gascan_core::manifest::Manifest,
) -> Result<bool, ServiceError> {
    ProvisioningPlanner::plan_for_root(canonical_root, manifest, &applied_state(Some(record)))
        .map(|plan| !plan.tools_changed())
        .map_err(|_| ServiceError::Provision("could not plan provisioning".to_owned()))
}

fn parse_mise_versions(
    output: &[u8],
    desired: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, ServiceError> {
    let MiseInventory(records) = serde_json::from_slice(output)
        .map_err(|_| ServiceError::Provision("invalid mise tool inventory".to_owned()))?;
    if !records.keys().eq(desired.keys()) {
        return Err(ServiceError::Provision(
            "mise returned an unexpected tool set".to_owned(),
        ));
    }
    records
        .into_iter()
        .map(|(tool, records)| {
            let [record] = records.as_slice() else {
                return Err(ServiceError::Provision(
                    "mise returned an invalid tool record".to_owned(),
                ));
            };
            if !record.installed
                || !record.active
                || record.version.trim().is_empty()
                || record.version.chars().any(char::is_control)
            {
                return Err(ServiceError::Provision(
                    "mise returned an invalid tool record".to_owned(),
                ));
            }
            Ok((tool, record.version.clone()))
        })
        .collect()
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
    let manifest = spec.manifest().clone();
    tokio::task::spawn_blocking(move || {
        let plan = ProvisioningPlanner::plan_for_root(&root, &manifest, &AppliedState::empty())
            .map_err(|_| {
                ServiceError::Fingerprint("workspace setup could not be read safely".to_owned())
            })?;
        let mut hash = Sha256::new();
        hash.update(plan.desired_tool_hash().as_bytes());
        if let Some(setup) = plan.setup_script() {
            hash.update(setup.canonical_relative_path().as_str().as_bytes());
            hash.update(setup.sha256().as_bytes());
        }
        Ok(format!("sha256:{:x}", hash.finalize()))
    })
    .await
    .map_err(|error| ServiceError::DatabaseWorker(error.to_string()))?
}

#[cfg(test)]
mod storage_tests {
    use super::{
        BoundedTail, NoopProvisioner, SandboxService, ServiceError, Store,
        image_replacement_failure_details, requested_storage_from_volumes,
    };
    use camino::Utf8Path;
    use gascan_core::fake_runtime::FakeRuntime;
    use gascan_core::manifest::Manifest;
    use gascan_core::policy::PolicyCompiler;
    use gascan_core::runtime::{RuntimeBackend, RuntimeCall};
    use gascan_core::sandbox::SandboxSpec;
    use std::sync::Arc;

    #[test]
    fn bounded_tail_keeps_exact_suffix_across_chunks() {
        let mut tail = BoundedTail::new(5);
        tail.extend(b"\xffab");
        tail.extend(b"cdef");
        assert_eq!(tail.into_bytes(), b"bcdef");
    }

    #[test]
    fn zero_length_bounded_tail_discards_input() {
        let mut tail = BoundedTail::new(0);
        tail.extend(b"ignored");
        assert!(tail.into_bytes().is_empty());
    }

    #[test]
    fn missing_compiled_managed_volume_is_an_internal_invariant_error() {
        assert!(matches!(
            requested_storage_from_volumes(&[]),
            Err(ServiceError::StorageInvariant(
                "managed volume is missing from compiled create request"
            ))
        ));
    }

    #[test]
    fn image_replacement_failure_details_preserve_primary_and_rollback() {
        let error = ServiceError::ImageRollback {
            original: Box::new(ServiceError::Provision("primary failure".to_owned())),
            rollback: Box::new(ServiceError::Provision("rollback failure".to_owned())),
        };

        assert_eq!(
            image_replacement_failure_details(&error),
            serde_json::json!({
                "message": error.to_string(),
                "primary": {
                    "code": "provision_failed",
                    "message": "provisioning failed: primary failure",
                },
                "rollback": {
                    "code": "provision_failed",
                    "message": "provisioning failed: rollback failure",
                },
            })
        );
    }

    #[tokio::test]
    async fn created_rollback_stops_running_runtime_before_exact_remove()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let root = Utf8Path::from_path(temp.path()).ok_or("UTF-8 root")?;
        let spec = SandboxSpec::from_root("persist-created-rollback", root, Manifest::load(root)?)?;
        let id = spec.id().clone();
        let runtime = FakeRuntime::default();
        let create = PolicyCompiler::compile(spec, &runtime.capabilities().await?)?;
        let outcome = runtime.create(create).await?;
        let exact_created = outcome.created().to_vec();
        runtime.start(&id).await?;
        let service = SandboxService::new(
            runtime.clone(),
            Store::open(root.join("state.db"))?,
            Arc::new(NoopProvisioner),
        );

        let error = service
            .rollback_created(
                &id,
                outcome,
                ServiceError::Provision("injected persistence failure".to_owned()),
            )
            .await;

        assert_eq!(
            error.to_string(),
            "provisioning failed: injected persistence failure"
        );
        assert!(runtime.inspect(&id).await?.is_none());
        assert!(runtime.list_resources().await?.is_empty());
        let calls = runtime.calls().await;
        let stopped = calls
            .iter()
            .position(|call| matches!(call, RuntimeCall::Stop(call_id) if call_id == &id))
            .ok_or("stop")?;
        let (removed, removed_resources) = calls
            .iter()
            .enumerate()
            .find_map(|(index, call)| match call {
                RuntimeCall::Remove(request) => Some((index, request.resources())),
                _ => None,
            })
            .ok_or("remove")?;
        assert!(matches!(
            &calls[stopped - 1],
            RuntimeCall::Inspect(call_id) if call_id == &id
        ));
        assert!(stopped < removed);
        assert_eq!(removed_resources, exact_created);
        Ok(())
    }
}
