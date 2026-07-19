use crate::reconcile::{ReconcileFinding, ReconcileReport};
use crate::{
    ActualState, DesiredState, ImageResolution, OperationEvent, OperationId, OperationKind,
    OperationRecord, SandboxRecord, SetupResolution, Store, StoreError, ToolResolution,
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
    ContainerState, CreateFailure, CreateRequest, ExecInput, ExecOutput, ExecRequest,
    RemoveRequest, ResourceKind, ResourceOwnership, RuntimeBackend, RuntimeError,
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
const MAX_PROVISION_OUTPUT_BYTES: usize = 1024 * 1024;

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

#[derive(Clone, Copy, Debug)]
pub(crate) enum PreBeginFailure {
    Conflict,
    Missing,
    Runtime,
    Invalid,
    Internal,
}

impl From<&ServiceError> for PreBeginFailure {
    fn from(error: &ServiceError) -> Self {
        match error {
            ServiceError::Store(StoreError::PendingOperationExists { .. }) => Self::Conflict,
            ServiceError::Missing(_) => Self::Missing,
            ServiceError::Runtime(_) => Self::Runtime,
            ServiceError::Policy(_) | ServiceError::Sandbox(_) | ServiceError::Manifest(_) => {
                Self::Invalid
            }
            _ => Self::Internal,
        }
    }
}

pub(crate) type OperationStart = Result<Operation, PreBeginFailure>;
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
    locks: Mutex<HashMap<SandboxId, Weak<AsyncMutex<()>>>>,
    doctor: DoctorState,
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
    #[error("provisioning failed: {0}")]
    Provision(String),
    #[error("mounted setup script changed before execution")]
    SetupChanged,
    #[error("setup script failed with exit code {0}")]
    SetupExit(i32),
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
            locks: Mutex::new(HashMap::new()),
            doctor,
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
        let mut record = existing.unwrap_or_else(|| SandboxRecord {
            id: id.clone(),
            canonical_root: request.spec.canonical_root().to_owned(),
            desired_state: DesiredState::Running,
            actual_state: ActualState::Creating,
            setup_resolution: None,
            tool_resolution: None,
            image_resolution: Some(ImageResolution::new(1, json!({"digest": create.image()}))),
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
                operation.id,
                &sender,
                &desired_fingerprint,
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
    ) -> Result<(ActualState, Option<ProvisionedResolution>), ServiceError> {
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
            let durable_match = if let Some(record) = prior.filter(|_| inspected.is_some()) {
                resolution_matches(record, desired_fingerprint)
                    && tool_state_matches(record, spec.canonical_root(), spec.manifest())?
            } else {
                false
            };
            let provisioned = if inspected.is_some() && !durable_match {
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
            Err(error) if error.is_setup_failure() => {
                if self.runtime.inspect(id).await?.is_some() {
                    let _ = self.runtime.stop(id).await;
                }
                Err(error)
            }
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

    async fn provision_explicit(
        &self,
        spec: &SandboxSpec,
        create: &CreateRequest,
        prior: Option<&SandboxRecord>,
        operation_id: OperationId,
        sender: &mpsc::Sender<OperationEvent>,
    ) -> Result<ProvisionedResolution, ServiceError> {
        let applied = applied_state(prior);
        let plan =
            ProvisioningPlanner::plan_for_root(spec.canonical_root(), spec.manifest(), &applied)
                .map_err(|_| ServiceError::Provision("could not plan provisioning".to_owned()))?;
        let resolved_tools = if plan.tools_changed() {
            Some(
                self.install_tools(spec, &plan, operation_id, sender)
                    .await?,
            )
        } else {
            prior.and_then(stored_tool_resolution)
        };
        let resolved_setup = if plan.steps().contains(&ProvisionStep::RunSetup) {
            self.emit_provision_step(operation_id, ProvisionStep::RunSetup, sender)
                .await?;
            let setup = plan.setup_script().ok_or_else(|| {
                ServiceError::Provision("setup execution was not planned".to_owned())
            })?;
            self.run_setup(spec.id(), setup).await?;
            Some(json!({
                "canonical_relative_path": setup.canonical_relative_path(),
                "sha256": setup.sha256(),
            }))
        } else {
            prior.and_then(stored_setup_resolution)
        };
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

    async fn run_setup(&self, id: &SandboxId, setup: &SetupScript) -> Result<(), ServiceError> {
        let guest_path = format!("/workspace/{}", setup.canonical_relative_path());
        let (digest_output, digest_code, digest_signal) = self
            .exec_guest_raw(
                id,
                ["/usr/bin/sha256sum".to_owned(), guest_path.clone()],
                Vec::new(),
            )
            .await?;
        if digest_code != 0 || digest_signal != 0 {
            return Err(ServiceError::SetupChanged);
        }
        let digest = std::str::from_utf8(&digest_output)
            .ok()
            .and_then(|output| output.split_ascii_whitespace().next())
            .filter(|digest| {
                digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
            .ok_or(ServiceError::SetupChanged)?;
        if setup.sha256().strip_prefix("sha256:") != Some(digest) {
            return Err(ServiceError::SetupChanged);
        }
        let (_, code, signal) = self
            .exec_guest_raw(id, ["/bin/bash".to_owned(), guest_path], Vec::new())
            .await?;
        if code == 0 && signal == 0 {
            Ok(())
        } else {
            Err(ServiceError::SetupExit(code))
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
        self.exec_guest(spec.id(), mise_command(&["install", "--yes"]), Vec::new())
            .await?;
        let output = self
            .exec_guest(
                spec.id(),
                mise_command(&["ls", "--current", "--installed", "--json"]),
                Vec::new(),
            )
            .await?;
        let resolved = parse_mise_versions(&output, spec.manifest().tools())?;
        serde_json::to_value(resolved)
            .map_err(|_| ServiceError::Provision("could not encode resolved tools".to_owned()))
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
        argv: I,
        stdin: Vec<u8>,
    ) -> Result<Vec<u8>, ServiceError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let (stdout, code, signal) = self.exec_guest_raw(id, argv, stdin).await?;
        if code == 0 && signal == 0 {
            Ok(stdout)
        } else {
            Err(ServiceError::Provision(
                "guest provisioning command failed".to_owned(),
            ))
        }
    }

    async fn exec_guest_raw<I, S>(
        &self,
        id: &SandboxId,
        argv: I,
        stdin: Vec<u8>,
    ) -> Result<(Vec<u8>, i32, i32), ServiceError>
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
        let mut output_bytes = 0_usize;
        while let Some(output) = session.next().await {
            match output.map_err(|_| provisioning_transport_error())? {
                ExecOutput::Stdout(bytes) => {
                    output_bytes = output_bytes.saturating_add(bytes.len());
                    if output_bytes > MAX_PROVISION_OUTPUT_BYTES {
                        return Err(ServiceError::Provision(
                            "guest provisioning output exceeded its limit".to_owned(),
                        ));
                    }
                    stdout.extend(bytes);
                }
                ExecOutput::Stderr(bytes) => {
                    output_bytes = output_bytes.saturating_add(bytes.len());
                    if output_bytes > MAX_PROVISION_OUTPUT_BYTES {
                        return Err(ServiceError::Provision(
                            "guest provisioning output exceeded its limit".to_owned(),
                        ));
                    }
                }
                ExecOutput::Exit { code, signal } => return Ok((stdout, code, signal)),
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
        let unchanged = resolution_matches(&record, &desired_fingerprint)
            && tool_state_matches(
                &record,
                request.spec.canonical_root(),
                request.spec.manifest(),
            )?;
        let operation = self
            .database({
                let record = record.clone();
                move |store| store.begin_operation(&record, OperationKind::Apply)
            })
            .await?;
        let (sender, receiver) = mpsc::channel(16);
        self.initialize_operation(operation.id, &id, record.actual_state, &sender)
            .await?;
        let receiver = publish_operation(started, operation.id, receiver);
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
                if error.is_setup_failure() {
                    let _ = self.runtime.stop(&id).await;
                }
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
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::Runtime(error) => error.code(),
            Self::Create(error) => error.code(),
            Self::Policy(error) => error.code(),
            Self::Missing(_) => "not_found",
            Self::Ownership(_) => "ownership_mismatch",
            Self::Provision(_) | Self::SetupChanged | Self::SetupExit(_) => "provision_failed",
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

    const fn is_setup_failure(&self) -> bool {
        matches!(self, Self::SetupChanged | Self::SetupExit(_))
    }
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

fn provisioning_transport_error() -> ServiceError {
    ServiceError::Provision("guest provisioning transport failed".to_owned())
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

fn stored_setup_resolution(record: &SandboxRecord) -> Option<Value> {
    record
        .setup_resolution
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
