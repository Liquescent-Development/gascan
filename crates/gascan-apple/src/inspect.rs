use std::{
    collections::{BTreeMap, BTreeSet},
    net::{IpAddr, Ipv4Addr},
};

use gascan_core::{
    runtime::{
        ContainerState, MANAGED_BY_LABEL, OwnershipMetadata, ResourceIdentity, ResourceKind,
        ResourceOwnership, RuntimeError, RuntimeResource, RuntimeSandbox, SANDBOX_ID_LABEL,
        SandboxLabel, classify_resource_ownership,
    },
    sandbox::SandboxId,
};
use serde::Deserialize;

use crate::{AppleCommandBuilder, CommandRunner, CommandSpec};

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
        let ContainerConfiguration {
            id: configured_id,
            image,
            labels,
            published_ports,
        } = record.configuration;
        let observed_id = parse_id("container inspect", configured_id)?;
        if &observed_id != id {
            return Err(RuntimeError::OwnershipMismatch {
                resource: observed_id.to_string(),
            });
        }
        let state = map_state(&observed_id, &record.status.state)?;
        let ownership = ownership_metadata(&observed_id, &labels)?;
        let image = parse_image(image)?;
        let ports = parse_published_ports(published_ports)?;
        Ok(Some(RuntimeSandbox::observed(
            observed_id,
            image,
            state,
            ownership,
            ports,
        )))
    }

    pub async fn list_resources(&self) -> Result<Vec<RuntimeResource>, RuntimeError> {
        let spec = CommandSpec::new("container", ["list", "--all", "--format", "json"]);
        let output = self.runner.run(spec).await?;
        parse_records("container list", &output.stdout)?
            .into_iter()
            .map(|record| {
                let name = record.configuration.id;
                map_state(&name, &record.status.state)?;
                let labels = &record.configuration.labels;
                let parsed = labels
                    .get(SANDBOX_ID_LABEL)
                    .and_then(|value| SandboxId::try_from(value.clone()).ok());
                let label = match (labels.get(SANDBOX_ID_LABEL), &parsed) {
                    (None, _) => SandboxLabel::Absent,
                    (Some(_), Some(id)) => SandboxLabel::Parsed(id),
                    (Some(_), None) => SandboxLabel::Unparseable,
                };
                let ownership = classify_resource_ownership(
                    ResourceKind::Container,
                    &name,
                    labels.get(MANAGED_BY_LABEL).map(String::as_str),
                    label,
                );
                let sandbox_id = match ownership {
                    ResourceOwnership::GasCanOwned => parsed,
                    _ => None,
                };
                let identity = ResourceIdentity::new(ResourceKind::Container, name)?;
                Ok(RuntimeResource::discovered(identity, sandbox_id, ownership))
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
    image: Option<ContainerImage>,
    #[serde(default)]
    labels: BTreeMap<String, String>,
    #[serde(default, rename = "publishedPorts")]
    published_ports: Vec<PublishedPort>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PublishedPort {
    host_address: IpAddr,
    host_port: u16,
    container_port: u16,
    count: u16,
    proto: String,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ContainerImage {
    Reference(String),
    Structured {
        reference: String,
        descriptor: ContainerImageDescriptor,
    },
}

#[derive(Deserialize)]
struct ContainerImageDescriptor {
    digest: String,
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

fn parse_published_ports(
    published_ports: Vec<PublishedPort>,
) -> Result<Vec<gascan_core::runtime::RuntimePort>, RuntimeError> {
    let mut seen = BTreeSet::new();
    published_ports
        .into_iter()
        .map(|port| {
            if port.host_address != IpAddr::V4(Ipv4Addr::LOCALHOST) {
                return Err(invalid_output(
                    "container inspect",
                    "published port does not bind to IPv4 loopback".to_owned(),
                ));
            }
            if port.proto != "tcp" {
                return Err(invalid_output(
                    "container inspect",
                    "published port protocol is not TCP".to_owned(),
                ));
            }
            if port.host_port == 0 || port.container_port == 0 || port.count != 1 {
                return Err(invalid_output(
                    "container inspect",
                    "published port values must be nonzero single-port mappings".to_owned(),
                ));
            }
            if !seen.insert((port.host_address, port.host_port)) {
                return Err(invalid_output(
                    "container inspect",
                    "published port mapping is duplicated".to_owned(),
                ));
            }
            Ok(gascan_core::runtime::RuntimePort {
                host_address: port.host_address,
                host_port: port.host_port,
                guest_port: port.container_port,
            })
        })
        .collect()
}

fn parse_image(image: Option<ContainerImage>) -> Result<String, RuntimeError> {
    let image = image.ok_or_else(|| {
        invalid_output(
            "container inspect",
            "missing inspected container image".to_owned(),
        )
    })?;
    let image = match image {
        ContainerImage::Reference(reference) => reference,
        ContainerImage::Structured {
            reference,
            descriptor,
        } => {
            let expected = reference
                .rsplit_once('@')
                .map(|(_, digest)| digest)
                .ok_or_else(|| {
                    invalid_output(
                        "container inspect",
                        "structured container image reference is not immutable".to_owned(),
                    )
                })?;
            if descriptor.digest != expected {
                return Err(invalid_output(
                    "container inspect",
                    "structured container image descriptor differs from its reference".to_owned(),
                ));
            }
            reference
        }
    };
    let Some((name, digest)) = image.split_once("@sha256:") else {
        return Err(invalid_output(
            "container inspect",
            "container image is not digest-qualified".to_owned(),
        ));
    };
    if name.is_empty()
        || digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid_output(
            "container inspect",
            "container image is not digest-qualified".to_owned(),
        ));
    }
    Ok(image)
}

fn map_state(id: impl std::fmt::Display, state: &str) -> Result<ContainerState, RuntimeError> {
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
