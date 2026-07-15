use std::collections::BTreeMap;

use gascan_core::{
    runtime::{
        ContainerState, OwnershipMetadata, ResourceIdentity, ResourceKind, ResourceOwnership,
        RuntimeError, RuntimeResource, RuntimeSandbox,
    },
    sandbox::SandboxId,
};
use serde::Deserialize;

use crate::{AppleCommandBuilder, CommandRunner, CommandSpec};

const MANAGED_BY_LABEL: &str = "dev.gascan.managed-by";
const SANDBOX_ID_LABEL: &str = "dev.gascan.sandbox-id";
const MANAGED_BY_GASCAN: &str = "gascan";
const CONTAINER_NOT_FOUND_EXIT_CODE: i32 = 1;

pub struct AppleInspector<R> {
    runner: R,
}

impl<R> AppleInspector<R>
where
    R: CommandRunner,
{
    pub const fn new(runner: R) -> Self {
        Self { runner }
    }

    pub async fn inspect(&self, id: &SandboxId) -> Result<Option<RuntimeSandbox>, RuntimeError> {
        let output = match self.runner.run(AppleCommandBuilder::inspect(id)).await {
            Ok(output) => output,
            Err(RuntimeError::CommandFailed {
                operation,
                exit_code: Some(CONTAINER_NOT_FOUND_EXIT_CODE),
                ..
            }) if operation == "container" => return Ok(None),
            Err(error) => return Err(error),
        };
        let mut records = parse_records("container inspect", &output.stdout)?;
        if records.len() != 1 {
            return Err(invalid_output(
                "container inspect",
                format!("expected exactly one container, found {}", records.len()),
            ));
        }
        let record = records.pop().ok_or_else(|| {
            invalid_output(
                "container inspect",
                "missing inspected container".to_owned(),
            )
        })?;
        let observed_id = parse_id("container inspect", record.configuration.id)?;
        if &observed_id != id {
            return Err(RuntimeError::OwnershipMismatch {
                resource: observed_id.to_string(),
            });
        }
        let state = map_state(&observed_id, &record.status.state)?;
        let ownership = ownership_metadata(&observed_id, &record.configuration.labels)?;
        Ok(Some(RuntimeSandbox {
            id: observed_id,
            state,
            ownership,
        }))
    }

    pub async fn list_resources(&self) -> Result<Vec<RuntimeResource>, RuntimeError> {
        let spec = CommandSpec::new("container", ["list", "--all", "--format", "json"]);
        let output = self.runner.run(spec).await?;
        parse_records("container list", &output.stdout)?
            .into_iter()
            .map(|record| {
                let id = parse_id("container list", record.configuration.id)?;
                map_state(&id, &record.status.state)?;
                let ownership = classify_ownership(&id, &record.configuration.labels);
                let identity = ResourceIdentity::new(ResourceKind::Container, id.to_string())?;
                Ok(RuntimeResource::discovered(identity, Some(id), ownership))
            })
            .collect()
    }
}

#[derive(Deserialize)]
struct ContainerRecord {
    configuration: ContainerConfiguration,
    status: ContainerStatus,
}

#[derive(Deserialize)]
struct ContainerConfiguration {
    id: String,
    #[serde(default)]
    labels: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct ContainerStatus {
    state: String,
}

fn parse_records(operation: &str, bytes: &[u8]) -> Result<Vec<ContainerRecord>, RuntimeError> {
    serde_json::from_slice(bytes).map_err(|error| invalid_output(operation, error.to_string()))
}

fn parse_id(operation: &str, id: String) -> Result<SandboxId, RuntimeError> {
    SandboxId::try_from(id).map_err(|error| invalid_output(operation, error.to_string()))
}

fn map_state(id: &SandboxId, state: &str) -> Result<ContainerState, RuntimeError> {
    match state {
        "creating" => Ok(ContainerState::Creating),
        "running" => Ok(ContainerState::Running),
        "stopped" => Ok(ContainerState::Stopped),
        state => Err(RuntimeError::UnknownActualState {
            resource: id.to_string(),
            state: state.to_owned(),
        }),
    }
}

fn classify_ownership(id: &SandboxId, labels: &BTreeMap<String, String>) -> ResourceOwnership {
    let manager = labels.get(MANAGED_BY_LABEL).map(String::as_str);
    let sandbox = labels.get(SANDBOX_ID_LABEL).map(String::as_str);
    match (manager, sandbox) {
        (Some(MANAGED_BY_GASCAN), Some(annotation)) if annotation == id.as_str() => {
            ResourceOwnership::GasCanOwned
        }
        (None, None) => ResourceOwnership::Foreign,
        (Some(manager), _) if manager != MANAGED_BY_GASCAN => ResourceOwnership::Foreign,
        _ => ResourceOwnership::Mismatched,
    }
}

fn ownership_metadata(
    id: &SandboxId,
    labels: &BTreeMap<String, String>,
) -> Result<OwnershipMetadata, RuntimeError> {
    let managed_by = labels.get(MANAGED_BY_LABEL).cloned().ok_or_else(|| {
        invalid_output(
            "container inspect",
            format!("container {id} is missing required label {MANAGED_BY_LABEL}"),
        )
    })?;
    let annotation = labels.get(SANDBOX_ID_LABEL).cloned().ok_or_else(|| {
        invalid_output(
            "container inspect",
            format!("container {id} is missing required label {SANDBOX_ID_LABEL}"),
        )
    })?;
    let sandbox_id = SandboxId::try_from(annotation).map_err(|error| {
        invalid_output(
            "container inspect",
            format!("container {id} has invalid {SANDBOX_ID_LABEL}: {error}"),
        )
    })?;
    if &sandbox_id != id {
        return Err(RuntimeError::OwnershipMismatch {
            resource: id.to_string(),
        });
    }
    Ok(OwnershipMetadata {
        managed_by,
        sandbox_id,
    })
}

fn invalid_output(operation: &str, message: String) -> RuntimeError {
    RuntimeError::InvalidOutput {
        operation: operation.to_owned(),
        message,
    }
}
