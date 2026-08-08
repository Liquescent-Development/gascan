use gascan_core::runtime::{
    CreateRequest, ExecRequest, MANAGED_BY, OwnershipMetadata, RecreateRequest, RemoveRequest,
    ResourceIdentity, ResourceKind, RuntimeBindMount, RuntimeError, RuntimeNetwork, RuntimePort,
    RuntimeResource, RuntimeResourceLimits, RuntimeUser, RuntimeVolume, immutable_image_identity,
};
use gascan_engine_proto::v1;
use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr};

/// A request or response shape the contract cannot express, or must not coerce.
pub(crate) fn boundary(resource: &str, message: impl Into<String>) -> RuntimeError {
    RuntimeError::InvalidState {
        resource: resource.to_owned(),
        message: message.into(),
    }
}

/// The engine sent something this client cannot read.
pub(crate) fn invalid_output(operation: &str, message: impl Into<String>) -> RuntimeError {
    RuntimeError::InvalidOutput {
        operation: operation.to_owned(),
        message: message.into(),
    }
}

pub(crate) fn image_digest(image: &str) -> Result<v1::ImageDigest, RuntimeError> {
    let (repository, sha256_hex) = immutable_image_identity(image).ok_or_else(|| {
        boundary(
            "engine image",
            format!("image {image:?} is not a named sha256 digest reference"),
        )
    })?;
    Ok(v1::ImageDigest {
        repository: repository.to_owned(),
        sha256_hex: sha256_hex.to_owned(),
    })
}

pub(crate) fn owner_labels(ownership: &OwnershipMetadata) -> v1::OwnerLabels {
    v1::OwnerLabels {
        managed_by: ownership.managed_by.clone(),
        sandbox_id: ownership.sandbox_id.to_string(),
    }
}

pub(crate) fn project_mount(mounts: &[RuntimeBindMount]) -> Result<v1::ProjectMount, RuntimeError> {
    let [mount] = mounts else {
        return Err(boundary(
            "engine project mount",
            format!(
                "exactly one project mount is expressible, found {}",
                mounts.len()
            ),
        ));
    };
    if !mount.writable {
        return Err(boundary(
            "engine project mount",
            "a read-only project mount is not expressible",
        ));
    }
    Ok(v1::ProjectMount {
        host_path: mount.source.to_string(),
        guest_path: mount.target.to_string(),
    })
}

pub(crate) fn volumes(volumes: &[RuntimeVolume]) -> Result<Vec<v1::Volume>, RuntimeError> {
    volumes
        .iter()
        .map(|volume| {
            if !volume.writable {
                return Err(boundary(
                    "engine volume",
                    format!(
                        "volume {:?} is read-only, which is not expressible",
                        volume.name
                    ),
                ));
            }
            Ok(v1::Volume {
                name: volume.name.clone(),
                guest_path: volume.target.to_string(),
                capacity_bytes: volume.capacity_bytes,
            })
        })
        .collect()
}

/// Loopback is implied by the contract, so a routable address is refused rather
/// than dropped: publishing on loopback when the caller named another address
/// would be a silent change of meaning.
pub(crate) fn port_mappings(ports: &[RuntimePort]) -> Result<Vec<v1::PortMapping>, RuntimeError> {
    let mut seen = BTreeSet::new();
    ports
        .iter()
        .map(|port| {
            if port.host_address != IpAddr::V4(Ipv4Addr::LOCALHOST) {
                return Err(boundary(
                    "engine port mapping",
                    format!(
                        "loopback is implied, so host address {} cannot be requested",
                        port.host_address
                    ),
                ));
            }
            if port.host_port == 0 || port.guest_port == 0 {
                return Err(boundary(
                    "engine port mapping",
                    format!(
                        "port 0 is not a mapping: {}:{}",
                        port.host_port, port.guest_port
                    ),
                ));
            }
            if !seen.insert(port.host_port) {
                return Err(boundary(
                    "engine port mapping",
                    format!("host port {} is mapped twice", port.host_port),
                ));
            }
            Ok(v1::PortMapping {
                host_port: u32::from(port.host_port),
                guest_port: u32::from(port.guest_port),
            })
        })
        .collect()
}

pub(crate) const fn resource_limits(limits: &RuntimeResourceLimits) -> v1::ResourceLimits {
    v1::ResourceLimits {
        cpus: match limits.cpus {
            Some(cpus) => Some(cpus as u32),
            None => None,
        },
        memory_bytes: limits.memory_bytes,
        disk_bytes: limits.disk_bytes,
        process_count: limits.process_count,
    }
}

pub(crate) fn network(network: &RuntimeNetwork) -> v1::Network {
    v1::Network {
        mode: Some(match network {
            RuntimeNetwork::Offline => v1::network::Mode::Offline(v1::Offline {}),
            RuntimeNetwork::Networked { name } => v1::network::Mode::NetworkedName(name.clone()),
        }),
    }
}

pub(crate) const fn user(user: RuntimeUser) -> v1::User {
    match user {
        RuntimeUser::Workspace => v1::User::Workspace,
        RuntimeUser::Root => v1::User::Root,
    }
}

pub(crate) const fn resource_kind(kind: ResourceKind) -> v1::ResourceKind {
    match kind {
        ResourceKind::Container => v1::ResourceKind::Container,
        ResourceKind::Volume => v1::ResourceKind::Volume,
        ResourceKind::Network => v1::ResourceKind::Network,
    }
}

pub(crate) fn resource_identity(identity: &ResourceIdentity) -> v1::ResourceIdentity {
    v1::ResourceIdentity {
        kind: resource_kind(identity.kind()) as i32,
        name: identity.name().to_owned(),
    }
}

/// A resource on the way out, for `CreateContainerRequest.retained`.
pub(crate) fn wire_resource(resource: &RuntimeResource) -> Result<v1::Resource, RuntimeError> {
    let sandbox_id = resource.sandbox_id().ok_or_else(|| {
        boundary(
            "engine retained resource",
            format!("resource {:?} carries no sandbox id", resource.name()),
        )
    })?;
    Ok(v1::Resource {
        identity: Some(resource_identity(resource.identity())),
        owner: Some(v1::OwnerLabels {
            managed_by: MANAGED_BY.to_owned(),
            sandbox_id: sandbox_id.to_string(),
        }),
    })
}

pub(crate) fn create_request(request: &CreateRequest) -> Result<v1::CreateRequest, RuntimeError> {
    Ok(v1::CreateRequest {
        sandbox_id: request.id().to_string(),
        image: Some(image_digest(request.image())?),
        project: Some(project_mount(request.bind_mounts())?),
        volumes: volumes(request.volumes())?,
        ports: port_mappings(request.ports())?,
        environment: request
            .environment()
            .iter()
            .map(|(name, value)| v1::EnvironmentVariable {
                name: name.clone(),
                value: value.clone(),
            })
            .collect(),
        resources: Some(resource_limits(request.resources())),
        network: Some(network(request.network())),
        user: user(request.user()) as i32,
        init: request.init(),
        owner: Some(owner_labels(request.ownership())),
    })
}

pub(crate) fn create_container_request(
    request: &RecreateRequest,
) -> Result<v1::CreateContainerRequest, RuntimeError> {
    Ok(v1::CreateContainerRequest {
        create: Some(create_request(request.create())?),
        retained: request
            .retained()
            .resources()
            .iter()
            .map(wire_resource)
            .collect::<Result<Vec<_>, _>>()?,
    })
}

/// One call carries one sandbox's labels.
///
/// Core's `RemoveRequest` holds resources that each carry their own sandbox id
/// and does not require them to agree; the wire carries a single `OwnerLabels`
/// for the whole call. Sending the first resource's labels for a mixed request
/// would ask the engine to delete under labels that do not describe every named
/// resource, so a mixed request is refused instead.
pub(crate) fn remove_request(request: &RemoveRequest) -> Result<v1::RemoveRequest, RuntimeError> {
    let mut owner = None;
    for resource in request.resources() {
        let id = resource
            .sandbox_id()
            .ok_or_else(|| RuntimeError::OwnershipMismatch {
                resource: resource.name().to_owned(),
            })?;
        match owner {
            None => owner = Some(id),
            Some(existing) if existing == id => {}
            Some(existing) => {
                return Err(boundary(
                    "engine remove request",
                    format!(
                        "one remove call carries one sandbox's labels; found {existing} and {id}"
                    ),
                ));
            }
        }
    }
    let owner = owner
        .ok_or_else(|| boundary("engine remove request", "at least one resource is required"))?;
    Ok(v1::RemoveRequest {
        resources: request
            .resources()
            .iter()
            .map(|resource| resource_identity(resource.identity()))
            .collect(),
        owner: Some(v1::OwnerLabels {
            managed_by: MANAGED_BY.to_owned(),
            sandbox_id: owner.to_string(),
        }),
    })
}

/// `argv` widens losslessly: the wire takes bytes because `execve` does.
pub(crate) fn exec_start(request: &ExecRequest) -> v1::ExecStart {
    v1::ExecStart {
        sandbox_id: request.id.to_string(),
        argv: request
            .argv
            .iter()
            .map(|argument| argument.clone().into_bytes())
            .collect(),
        environment: request
            .environment
            .iter()
            .map(|(name, value)| v1::EnvironmentVariable {
                name: name.clone(),
                value: value.clone(),
            })
            .collect(),
        tty: request.tty,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gascan_core::runtime::{RuntimeBindMount, RuntimePort};
    use std::net::{IpAddr, Ipv4Addr};

    const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn an_image_reference_splits_into_repository_and_digest_and_drops_the_tag() {
        let digest = image_digest(&format!("registry.example/workspace:1.2@sha256:{DIGEST}"))
            .expect("a named sha256 reference maps");
        assert_eq!(digest.repository, "registry.example/workspace");
        assert_eq!(digest.sha256_hex, DIGEST);
    }

    #[test]
    fn a_reference_without_a_digest_is_refused_rather_than_coerced() {
        let error = image_digest("registry.example/workspace:latest")
            .expect_err("a tag-only reference is not expressible");
        assert_eq!(error.code(), "invalid_state");
    }

    #[test]
    fn exactly_one_writable_project_mount_is_expressible() {
        let mount = RuntimeBindMount {
            source: "/host/project".into(),
            target: "/workspace".into(),
            writable: true,
        };
        let wire = project_mount(std::slice::from_ref(&mount)).expect("one writable mount maps");
        assert_eq!(wire.host_path, "/host/project");
        assert_eq!(wire.guest_path, "/workspace");

        assert_eq!(
            project_mount(&[])
                .expect_err("zero mounts is not expressible")
                .code(),
            "invalid_state",
        );
        assert_eq!(
            project_mount(&[mount.clone(), mount.clone()])
                .expect_err("two mounts is not expressible")
                .code(),
            "invalid_state",
        );

        let read_only = RuntimeBindMount {
            writable: false,
            ..mount
        };
        assert_eq!(
            project_mount(std::slice::from_ref(&read_only))
                .expect_err("a read-only project mount is not expressible")
                .code(),
            "invalid_state",
        );
    }

    fn port(host: u16, guest: u16) -> RuntimePort {
        RuntimePort {
            host_address: IpAddr::V4(Ipv4Addr::LOCALHOST),
            host_port: host,
            guest_port: guest,
        }
    }

    #[test]
    fn ports_widen_to_uint32_and_keep_their_order() {
        let wire = port_mappings(&[port(22222, 22), port(33333, 80)]).expect("ports map");
        assert_eq!(
            wire.iter()
                .map(|p| (p.host_port, p.guest_port))
                .collect::<Vec<_>>(),
            [(22222, 22), (33333, 80)],
        );
    }

    #[test]
    fn a_zero_port_a_duplicate_or_a_non_loopback_address_is_refused() {
        assert_eq!(
            port_mappings(&[port(0, 22)])
                .expect_err("zero host port")
                .code(),
            "invalid_state"
        );
        assert_eq!(
            port_mappings(&[port(22222, 0)])
                .expect_err("zero guest port")
                .code(),
            "invalid_state"
        );
        assert_eq!(
            port_mappings(&[port(22222, 22), port(22222, 80)])
                .expect_err("a duplicated host port")
                .code(),
            "invalid_state",
        );

        let routable = RuntimePort {
            host_address: IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)),
            host_port: 22222,
            guest_port: 22,
        };
        assert_eq!(
            port_mappings(std::slice::from_ref(&routable))
                .expect_err("loopback is implied, so a routable address cannot be honoured")
                .code(),
            "invalid_state",
        );
    }
}
