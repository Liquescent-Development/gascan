use std::{collections::BTreeMap, sync::Mutex};

use async_trait::async_trait;
use gascan_core::{
    runtime::{
        ContainerState, CreateFailure, CreateOutcome, CreateRequest, ExecCancellation, ExecInput,
        ExecOutput, ExecRequest, ExecSession, RemoveRequest, ResourceIdentity, ResourceKind,
        ResourceOwnership, RuntimeBackend, RuntimeCapabilities, RuntimeError, RuntimeResource,
        RuntimeSandbox,
    },
    sandbox::SandboxId,
};
use serde::Deserialize;

use crate::{
    AppleAttach, AppleCommandBuilder, AppleInspector, AppleProbe, AttachInput, AttachOutput,
    CommandRunner, CommandSpec, TranslationError,
};

const MANAGED_BY: &str = "gascan";
const MANAGED_BY_LABEL: &str = "dev.gascan.managed-by";
const SANDBOX_ID_LABEL: &str = "dev.gascan.sandbox-id";
const MANAGED_VOLUME_SIZE_BYTES: &str = "104857600";

pub struct AppleBackend<R> {
    runner: R,
    attach: AppleAttach,
    observations: Mutex<BTreeMap<ResourceIdentity, RuntimeResource>>,
}

impl<R> AppleBackend<R> {
    pub fn new(runner: R) -> Self {
        Self {
            runner,
            attach: AppleAttach::default(),
            observations: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn with_attach(runner: R, attach: AppleAttach) -> Self {
        Self {
            runner,
            attach,
            observations: Mutex::new(BTreeMap::new()),
        }
    }
}

impl<R: CommandRunner + Clone> AppleBackend<R> {
    pub async fn pull(&self, image: &str) -> Result<(), RuntimeError> {
        let spec = AppleCommandBuilder::pull(image).map_err(translation_error)?;
        self.runner.run(spec).await.map(|_| ())
    }

    async fn inventory(&self) -> Result<Vec<RuntimeResource>, RuntimeError> {
        let mut resources = AppleInspector::new(self.runner.clone())
            .list_resources()
            .await?;
        let output = self
            .runner
            .run(CommandSpec::new(
                "container",
                ["volume", "list", "--format", "json"],
            ))
            .await?;
        let records: Vec<VolumeRecord> = serde_json::from_slice(&output.stdout)
            .map_err(|error| invalid_output("container volume list", error.to_string()))?;
        for record in records {
            if record.id != record.configuration.name {
                return Err(invalid_output(
                    "container volume list",
                    "volume id and name differ".into(),
                ));
            }
            let sandbox_id = record
                .configuration
                .labels
                .get(SANDBOX_ID_LABEL)
                .map(|value| SandboxId::try_from(value.clone()))
                .transpose()
                .map_err(|error| invalid_output("container volume list", error.to_string()))?;
            let ownership = classify(sandbox_id.as_ref(), &record.configuration.labels);
            let identity = ResourceIdentity::new(ResourceKind::Volume, record.id)?;
            resources.push(RuntimeResource::discovered(identity, sandbox_id, ownership));
        }
        let output = self
            .runner
            .run(CommandSpec::new(
                "container",
                ["network", "list", "--format", "json"],
            ))
            .await?;
        let records: Vec<NetworkRecord> = serde_json::from_slice(&output.stdout)
            .map_err(|error| invalid_output("container network list", error.to_string()))?;
        for record in records {
            if record.id != record.configuration.name {
                return Err(invalid_output(
                    "container network list",
                    "network id and name differ".into(),
                ));
            }
            let sandbox_id = record
                .configuration
                .labels
                .get(SANDBOX_ID_LABEL)
                .map(|value| SandboxId::try_from(value.clone()))
                .transpose()
                .map_err(|error| invalid_output("container network list", error.to_string()))?;
            let ownership = classify(sandbox_id.as_ref(), &record.configuration.labels);
            let identity = ResourceIdentity::new(ResourceKind::Network, record.id)?;
            resources.push(RuntimeResource::discovered(identity, sandbox_id, ownership));
        }
        let mut observations = self
            .observations
            .lock()
            .map_err(|_| RuntimeError::CommandIo {
                operation: "inventory proof cache".into(),
                message: "lock poisoned".into(),
            })?;
        let mut reconciled = Vec::with_capacity(resources.len());
        for resource in resources {
            let stable = match observations
                .get(resource.identity())
                .filter(|prior| {
                    prior.sandbox_id() == resource.sandbox_id()
                        && prior.ownership() == resource.ownership()
                })
                .cloned()
            {
                Some(prior) => prior,
                None => resource,
            };
            observations.insert(stable.identity().clone(), stable.clone());
            reconciled.push(stable);
        }
        observations.retain(|_, prior| reconciled.iter().any(|item| item == prior));
        Ok(reconciled)
    }

    async fn current_for(
        &self,
        identity: &ResourceIdentity,
    ) -> Result<Option<RuntimeResource>, RuntimeError> {
        Ok(self
            .inventory()
            .await?
            .into_iter()
            .find(|item| item.identity() == identity))
    }

    async fn reconcile_created(
        &self,
        request: &CreateRequest,
        before: &[RuntimeResource],
        mut created: Vec<RuntimeResource>,
    ) -> Vec<RuntimeResource> {
        let Ok(current) = self.inventory().await else {
            return created;
        };
        for resource in current.into_iter().filter(|resource| {
            resource.ownership() == ResourceOwnership::GasCanOwned
                && resource.sandbox_id() == Some(request.id())
                && !before
                    .iter()
                    .any(|old| old.identity() == resource.identity())
                && (resource.name() == request.id().as_str()
                    || request
                        .volumes()
                        .iter()
                        .any(|volume| volume.name == resource.name())
                    || request.network().managed_name() == Some(resource.name()))
        }) {
            if !created
                .iter()
                .any(|prior| prior.identity() == resource.identity())
            {
                created.push(resource);
            }
        }
        created
    }
}

#[async_trait]
impl<R> RuntimeBackend for AppleBackend<R>
where
    R: CommandRunner + Clone + Send + Sync,
{
    async fn capabilities(&self) -> Result<RuntimeCapabilities, RuntimeError> {
        AppleProbe::new(self.runner.clone())
            .base_capabilities()
            .await
    }

    async fn inspect(&self, id: &SandboxId) -> Result<Option<RuntimeSandbox>, RuntimeError> {
        AppleInspector::new(self.runner.clone()).inspect(id).await
    }

    async fn create(&self, request: CreateRequest) -> Result<CreateOutcome, CreateFailure> {
        let before = self.inventory().await.map_err(CreateFailure::from_source)?;
        if let Some(resource) = before.iter().find(|resource| {
            resource.name() == request.id().as_str()
                || request
                    .volumes()
                    .iter()
                    .any(|volume| volume.name == resource.name())
                || request.network().managed_name() == Some(resource.name())
        }) {
            return Err(CreateFailure::from_source(RuntimeError::Conflict {
                resource: resource.name().to_owned(),
                message: "resource already exists".into(),
            }));
        }
        let mut created = Vec::new();
        if let Some(name) = request.network().managed_name() {
            let manager = format!("{MANAGED_BY_LABEL}={MANAGED_BY}");
            let sandbox = format!("{SANDBOX_ID_LABEL}={}", request.id());
            let spec = CommandSpec::new(
                "container",
                [
                    "network", "create", "--label", &manager, "--label", &sandbox, name,
                ],
            );
            if let Err(error) = self.runner.run(spec).await {
                if matches!(&error, RuntimeError::CommandIo { .. }) {
                    created = self.reconcile_created(&request, &before, created).await;
                }
                return Err(create_failure(&request, created, error));
            }
            let identity = match ResourceIdentity::new(ResourceKind::Network, name) {
                Ok(identity) => identity,
                Err(error) => return Err(create_failure(&request, created, error)),
            };
            let resource = match self.current_for(&identity).await {
                Ok(Some(resource))
                    if resource.ownership() == ResourceOwnership::GasCanOwned
                        && resource.sandbox_id() == Some(request.id()) =>
                {
                    resource
                }
                Ok(_) => {
                    created = self.reconcile_created(&request, &before, created).await;
                    return Err(create_failure(
                        &request,
                        created,
                        RuntimeError::OwnershipMismatch {
                            resource: name.to_owned(),
                        },
                    ));
                }
                Err(error) => {
                    created = self.reconcile_created(&request, &before, created).await;
                    return Err(create_failure(&request, created, error));
                }
            };
            created.push(resource);
        }
        for volume in request.volumes() {
            let manager = format!("{MANAGED_BY_LABEL}={MANAGED_BY}");
            let sandbox = format!("{SANDBOX_ID_LABEL}={}", request.id());
            let spec = CommandSpec::new(
                "container",
                [
                    "volume",
                    "create",
                    "--label",
                    &manager,
                    "--label",
                    &sandbox,
                    "-s",
                    MANAGED_VOLUME_SIZE_BYTES,
                    &volume.name,
                ],
            );
            if let Err(error) = self.runner.run(spec).await {
                if matches!(&error, RuntimeError::CommandIo { .. }) {
                    created = self.reconcile_created(&request, &before, created).await;
                }
                return Err(create_failure(&request, created, error));
            }
            let identity = match ResourceIdentity::new(ResourceKind::Volume, volume.name.clone()) {
                Ok(identity) => identity,
                Err(error) => return Err(create_failure(&request, created, error)),
            };
            let resource = match self.current_for(&identity).await {
                Ok(Some(resource))
                    if resource.ownership() == ResourceOwnership::GasCanOwned
                        && resource.sandbox_id() == Some(request.id()) =>
                {
                    resource
                }
                Ok(_) => {
                    created = self.reconcile_created(&request, &before, created).await;
                    return Err(create_failure(
                        &request,
                        created,
                        RuntimeError::OwnershipMismatch {
                            resource: volume.name.clone(),
                        },
                    ));
                }
                Err(error) => {
                    created = self.reconcile_created(&request, &before, created).await;
                    return Err(create_failure(&request, created, error));
                }
            };
            created.push(resource);
        }
        let spec = match AppleCommandBuilder::create(&request) {
            Ok(spec) => spec,
            Err(error) => return Err(create_failure(&request, created, translation_error(error))),
        };
        if let Err(error) = self.runner.run(spec).await {
            if matches!(&error, RuntimeError::CommandIo { .. }) {
                created = self.reconcile_created(&request, &before, created).await;
            }
            return Err(create_failure(&request, created, error));
        }
        let identity =
            match ResourceIdentity::new(ResourceKind::Container, request.id().to_string()) {
                Ok(identity) => identity,
                Err(error) => return Err(create_failure(&request, created, error)),
            };
        let resource = match self.current_for(&identity).await {
            Ok(Some(resource))
                if resource.ownership() == ResourceOwnership::GasCanOwned
                    && resource.sandbox_id() == Some(request.id()) =>
            {
                resource
            }
            Ok(_) => {
                created = self.reconcile_created(&request, &before, created).await;
                return Err(create_failure(
                    &request,
                    created,
                    RuntimeError::OwnershipMismatch {
                        resource: request.id().to_string(),
                    },
                ));
            }
            Err(error) => {
                created = self.reconcile_created(&request, &before, created).await;
                return Err(create_failure(&request, created, error));
            }
        };
        created.push(resource);
        match CreateOutcome::new(&request, created.clone()) {
            Ok(outcome) => Ok(outcome),
            Err(error) => Err(create_failure(&request, created, error)),
        }
    }

    async fn start(&self, id: &SandboxId) -> Result<(), RuntimeError> {
        match self.inspect(id).await? {
            Some(sandbox) if sandbox.state == ContainerState::Running => Ok(()),
            Some(_) => self
                .runner
                .run(CommandSpec::new("container", ["start", id.as_str()]))
                .await
                .map(|_| ()),
            None => Err(RuntimeError::NotFound {
                resource: id.to_string(),
            }),
        }
    }

    async fn stop(&self, id: &SandboxId) -> Result<(), RuntimeError> {
        match self.inspect(id).await? {
            Some(sandbox) if sandbox.state == ContainerState::Stopped => Ok(()),
            Some(_) => self
                .runner
                .run(CommandSpec::new(
                    "container",
                    ["stop", "--time", "5", id.as_str()],
                ))
                .await
                .map(|_| ()),
            None => Err(RuntimeError::NotFound {
                resource: id.to_string(),
            }),
        }
    }

    async fn remove(&self, request: RemoveRequest) -> Result<(), RuntimeError> {
        let ordered = [
            ResourceKind::Container,
            ResourceKind::Volume,
            ResourceKind::Network,
        ];
        for kind in ordered {
            for recorded in request
                .resources()
                .iter()
                .filter(|resource| resource.kind() == kind)
            {
                let current = self
                    .current_for(recorded.identity())
                    .await?
                    .ok_or_else(|| RuntimeError::OwnershipMismatch {
                        resource: recorded.name().to_owned(),
                    })?;
                if &current != recorded
                    || current.ownership() != ResourceOwnership::GasCanOwned
                    || current.sandbox_id() != recorded.sandbox_id()
                {
                    return Err(RuntimeError::OwnershipMismatch {
                        resource: recorded.name().to_owned(),
                    });
                }
                let spec = match recorded.kind() {
                    ResourceKind::Container => {
                        CommandSpec::new("container", ["delete", recorded.name()])
                    }
                    ResourceKind::Volume => {
                        CommandSpec::new("container", ["volume", "delete", recorded.name()])
                    }
                    ResourceKind::Network => {
                        CommandSpec::new("container", ["network", "delete", recorded.name()])
                    }
                };
                self.runner.run(spec).await?;
                self.observations
                    .lock()
                    .map_err(|_| RuntimeError::CommandIo {
                        operation: "inventory proof cache".into(),
                        message: "lock poisoned".into(),
                    })?
                    .remove(recorded.identity());
            }
        }
        Ok(())
    }

    async fn exec(&self, request: ExecRequest) -> Result<ExecSession, RuntimeError> {
        let initial_stdin = request.stdin;
        let mut session = self
            .attach
            .exec_with_environment(
                request.id.as_str(),
                request.argv,
                request.tty,
                request.environment,
            )
            .await?;
        let (input, mut inputs) = tokio::sync::mpsc::channel(16);
        let (outputs, output) = tokio::sync::mpsc::channel(32);
        let (cancellation, mut cancelled) = ExecCancellation::channel();
        let writer = session.input_handle();
        tokio::spawn(async move {
            if !initial_stdin.is_empty() {
                tokio::select! {
                    result = writer.send(AttachInput::Stdin(initial_stdin)) => {
                        if let Err(error) = result {
                            let _ = outputs.send(Err(error)).await;
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
                    frame = inputs.recv() => {
                        let Some(frame) = frame else { break };
                        let frame = match frame {
                            ExecInput::Stdin(bytes) => Ok(AttachInput::Stdin(bytes)),
                            ExecInput::Resize { columns, rows } => {
                                match (u16::try_from(columns), u16::try_from(rows)) {
                                    (Ok(cols), Ok(rows)) => Ok(AttachInput::Resize { rows, cols }),
                                    _ => Err(RuntimeError::UnsupportedCapability {
                                        capability: format!("attachment size {columns}x{rows} exceeds Apple ContainerAPIClient limits"),
                                    }),
                                }
                            }
                            ExecInput::Signal(signal) => Ok(AttachInput::Signal(signal)),
                            ExecInput::Close => Ok(AttachInput::Close),
                        };
                        let result = match frame {
                            Ok(frame) => tokio::select! {
                                result = writer.send(frame) => result,
                                result = cancelled.changed() => {
                                    if result.is_ok() && *cancelled.borrow() { break; }
                                    continue;
                                }
                            },
                            Err(error) => Err(error),
                        };
                        if let Err(error) = result {
                            let _ = outputs.send(Err(error)).await;
                            break;
                        }
                    }
                    next = session.recv() => {
                        let (mapped, terminal) = match next {
                            Ok(Some(AttachOutput::Stdout(bytes))) => (Ok(ExecOutput::Stdout(bytes)), false),
                            Ok(Some(AttachOutput::Stderr(bytes))) => (Ok(ExecOutput::Stderr(bytes)), false),
                            Ok(Some(AttachOutput::Exit(code))) => (Ok(ExecOutput::Exit { code, signal: 0 }), true),
                            Ok(None) => break,
                            Err(error) => (Err(error), true),
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

    async fn logs(
        &self,
        id: &SandboxId,
        since_millis: Option<i64>,
    ) -> Result<Vec<u8>, RuntimeError> {
        let mut args = vec!["logs".to_owned()];
        if let Some(since) = since_millis {
            args.extend(["--since".into(), format!("{since}ms")]);
        }
        args.push(id.to_string());
        self.runner
            .run(CommandSpec::new("container", args))
            .await
            .map(|output| output.stdout)
    }

    async fn list_resources(&self) -> Result<Vec<RuntimeResource>, RuntimeError> {
        self.inventory().await
    }
}

#[derive(Deserialize)]
struct VolumeRecord {
    id: String,
    configuration: VolumeConfiguration,
}
#[derive(Deserialize)]
struct VolumeConfiguration {
    name: String,
    #[serde(default)]
    labels: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct NetworkRecord {
    id: String,
    configuration: NetworkConfiguration,
}
#[derive(Deserialize)]
struct NetworkConfiguration {
    name: String,
    #[serde(default)]
    labels: BTreeMap<String, String>,
}

fn classify(id: Option<&SandboxId>, labels: &BTreeMap<String, String>) -> ResourceOwnership {
    match (
        labels.get(MANAGED_BY_LABEL).map(String::as_str),
        labels.get(SANDBOX_ID_LABEL),
        id,
    ) {
        (Some(MANAGED_BY), Some(annotation), Some(id)) if annotation == id.as_str() => {
            ResourceOwnership::GasCanOwned
        }
        (None, None, None) => ResourceOwnership::Foreign,
        (Some(manager), _, _) if manager != MANAGED_BY => ResourceOwnership::Foreign,
        _ => ResourceOwnership::Mismatched,
    }
}

fn translation_error(error: TranslationError) -> RuntimeError {
    RuntimeError::UnsupportedCapability {
        capability: format!("{}: {error}", error.code()),
    }
}
fn invalid_output(operation: &str, message: String) -> RuntimeError {
    RuntimeError::InvalidOutput {
        operation: operation.into(),
        message,
    }
}
fn create_failure(
    request: &CreateRequest,
    created: Vec<RuntimeResource>,
    source: RuntimeError,
) -> CreateFailure {
    CreateFailure::from_created_evidence(request, created, source)
}
