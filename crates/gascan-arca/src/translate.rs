use gascan_core::runtime::{
    ContainerState, CreateRequest, ExecRequest, MANAGED_BY, NetworkIsolation, OwnershipMetadata,
    RecreateRequest, RemoveRequest, ResourceIdentity, ResourceKind, RuntimeBindMount,
    RuntimeCapabilities, RuntimeError, RuntimeNetwork, RuntimePort, RuntimeResource,
    RuntimeResourceLimits, RuntimeSandbox, RuntimeUser, RuntimeVersion, RuntimeVolume,
    SandboxLabel, classify_resource_ownership, immutable_image_identity, immutable_image_reference,
};
use gascan_core::sandbox::SandboxId;
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

pub(crate) fn resource_limits(limits: &RuntimeResourceLimits) -> v1::ResourceLimits {
    v1::ResourceLimits {
        cpus: limits.cpus.map(u32::from),
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

/// A response whose `oneof` is unset. proto3 makes that representable, and it
/// means the engine sent a message this client cannot interpret.
pub(crate) fn missing_outcome(operation: &str) -> RuntimeError {
    invalid_output(operation, "response carried no outcome")
}

pub(crate) fn runtime_capabilities(
    capabilities: &v1::Capabilities,
) -> Result<RuntimeCapabilities, RuntimeError> {
    let version = capabilities
        .engine_version
        .as_ref()
        .ok_or_else(|| invalid_output("capabilities", "response carried no engine version"))?;
    let offline = match v1::Isolation::try_from(capabilities.offline) {
        Ok(v1::Isolation::Proven) => NetworkIsolation::Proven,
        Ok(v1::Isolation::Unsupported) => NetworkIsolation::Unsupported,
        Ok(v1::Isolation::Unverified) => NetworkIsolation::Unverified,
        Ok(v1::Isolation::Unspecified) | Err(_) => {
            return Err(invalid_output(
                "capabilities",
                format!("offline isolation {} is not a value", capabilities.offline),
            ));
        }
    };
    // contract_minor is deliberately read and dropped: this client populates no
    // additive fields yet, so knowing which it may find tells it nothing.
    Ok(RuntimeCapabilities {
        version: RuntimeVersion::new(
            u64::from(version.major),
            u64::from(version.minor),
            u64::from(version.patch),
        ),
        bind_mounts: capabilities.project_mount,
        named_volumes: capabilities.named_volumes,
        tty: capabilities.tty,
        signals: capabilities.signals,
        loopback_publish: capabilities.loopback_publish,
        resource_limits: capabilities.resource_limits,
        offline,
    })
}

/// Reassembles the canonical reference, then asserts it is one.
///
/// The result is deterministic, which is what lets the daemon compare one
/// observation against another by exact string.
pub(crate) fn runtime_image(image: Option<&v1::ImageDigest>) -> Result<String, RuntimeError> {
    let image =
        image.ok_or_else(|| invalid_output("inspect", "response carried no image digest"))?;
    let reference = format!("{}@sha256:{}", image.repository, image.sha256_hex);
    if !immutable_image_reference(&reference) {
        return Err(invalid_output(
            "inspect",
            format!("engine image {reference:?} is not a named sha256 digest reference"),
        ));
    }
    Ok(reference)
}

/// Loopback is not on the wire because it is the only case, so it is restored
/// here. Every construction site in the policy compiler uses the same address,
/// so this round-trips exactly.
pub(crate) fn runtime_ports(ports: &[v1::PortMapping]) -> Result<Vec<RuntimePort>, RuntimeError> {
    let mut seen = BTreeSet::new();
    ports
        .iter()
        .map(|port| {
            let host_port = u16::try_from(port.host_port).map_err(|_| {
                invalid_output(
                    "inspect",
                    format!("host port {} is out of range", port.host_port),
                )
            })?;
            let guest_port = u16::try_from(port.guest_port).map_err(|_| {
                invalid_output(
                    "inspect",
                    format!("guest port {} is out of range", port.guest_port),
                )
            })?;
            if host_port == 0 || guest_port == 0 {
                return Err(invalid_output(
                    "inspect",
                    format!("port 0 is not a mapping: {host_port}:{guest_port}"),
                ));
            }
            if !seen.insert(host_port) {
                return Err(invalid_output(
                    "inspect",
                    format!("host port {host_port} is published twice"),
                ));
            }
            Ok(RuntimePort {
                host_address: IpAddr::V4(Ipv4Addr::LOCALHOST),
                host_port,
                guest_port,
            })
        })
        .collect()
}

pub(crate) fn runtime_sandbox(sandbox: &v1::Sandbox) -> Result<RuntimeSandbox, RuntimeError> {
    let id = SandboxId::try_from(sandbox.sandbox_id.clone()).map_err(|error| {
        invalid_output(
            "inspect",
            format!("sandbox id {:?} is invalid: {error}", sandbox.sandbox_id),
        )
    })?;
    let image = runtime_image(sandbox.image.as_ref())?;
    let state = match v1::SandboxState::try_from(sandbox.state) {
        Ok(v1::SandboxState::Creating) => ContainerState::Creating,
        Ok(v1::SandboxState::Running) => ContainerState::Running,
        Ok(v1::SandboxState::Stopped) => ContainerState::Stopped,
        Ok(v1::SandboxState::Unspecified) | Err(_) => {
            return Err(RuntimeError::UnknownActualState {
                resource: id.to_string(),
                state: sandbox.state.to_string(),
            });
        }
    };
    // A sandbox must be labelled, as the Apple backend also requires: an
    // unlabelled container is not one this client may claim to own.
    let owner = sandbox.owner.as_ref().ok_or_else(|| {
        invalid_output("inspect", format!("sandbox {id} carries no owner labels"))
    })?;
    let sandbox_id = SandboxId::try_from(owner.sandbox_id.clone()).map_err(|error| {
        invalid_output(
            "inspect",
            format!("sandbox {id} has an invalid sandbox-id label: {error}"),
        )
    })?;
    if sandbox_id != id {
        return Err(RuntimeError::OwnershipMismatch {
            resource: id.to_string(),
        });
    }
    let ownership = OwnershipMetadata {
        managed_by: owner.managed_by.clone(),
        sandbox_id,
    };
    Ok(RuntimeSandbox::observed(
        id,
        image,
        state,
        ownership,
        runtime_ports(&sandbox.ports)?,
    ))
}

/// `operation` is the RPC whose response carried this resource, not the RPC that
/// happens to list them. `ListResources`, `Create`, and `CreateContainer` all
/// return `Resource`, so a hardcoded name here would report an RPC the caller
/// never made — a malformed resource in a create response would blame
/// `list_resources` and send an operator looking in the wrong place.
pub(crate) fn runtime_resource(
    operation: &str,
    resource: &v1::Resource,
) -> Result<RuntimeResource, RuntimeError> {
    let identity = resource
        .identity
        .as_ref()
        .ok_or_else(|| invalid_output(operation, "resource carried no identity"))?;
    let kind = match v1::ResourceKind::try_from(identity.kind) {
        Ok(v1::ResourceKind::Container) => ResourceKind::Container,
        Ok(v1::ResourceKind::Volume) => ResourceKind::Volume,
        Ok(v1::ResourceKind::Network) => ResourceKind::Network,
        Ok(v1::ResourceKind::Unspecified) | Err(_) => {
            return Err(invalid_output(
                operation,
                format!("resource kind {} is not a value", identity.kind),
            ));
        }
    };
    let core_identity = ResourceIdentity::new(kind, identity.name.clone())?;
    let owner = resource.owner.as_ref();
    // An unparseable label is Mismatched, not a failed call: ListResources
    // returns every resource the engine holds so that drift detection can see
    // them, and one malformed foreign label must not hide the rest.
    let parsed = owner.and_then(|owner| SandboxId::try_from(owner.sandbox_id.clone()).ok());
    let label = match (owner, &parsed) {
        (None, _) => SandboxLabel::Absent,
        (Some(_), Some(id)) => SandboxLabel::Parsed(id),
        (Some(_), None) => SandboxLabel::Unparseable,
    };
    let ownership = classify_resource_ownership(
        kind,
        &identity.name,
        owner.map(|owner| owner.managed_by.as_str()),
        label,
    );
    // Some(id) whenever OUR label parsed, including when the resource is
    // Mismatched. **CORRECTED 2026-08-08 — this file previously said
    // `match ownership { GasCanOwned => parsed, _ => None }`, which is the exact
    // rule Task 1's fix round reverted as a regression.** `gascan-apple`'s
    // `inspect.rs` reports the claimed id for a mismatched resource because the
    // reconciler at `gascand/src/service.rs:3001-3012` finds it by that claim.
    // The two backends MUST agree here — a divergence is what Task 1 exists to
    // prevent, and it would mean one backend reports an ownership mismatch that
    // the other silently drops.
    let sandbox_id = if owner.map(|owner| owner.managed_by.as_str()) == Some(MANAGED_BY) {
        parsed
    } else {
        None
    };
    Ok(RuntimeResource::discovered(
        core_identity,
        sandbox_id,
        ownership,
    ))
}

pub(crate) fn runtime_resources(
    operation: &str,
    resources: &[v1::Resource],
) -> Result<Vec<RuntimeResource>, RuntimeError> {
    resources
        .iter()
        .map(|resource| runtime_resource(operation, resource))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gascan_core::runtime::{
        ContainerState, NetworkIsolation, ResourceOwnership, RuntimeBindMount, RuntimePort,
    };
    use gascan_core::sandbox::SandboxId;
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

    #[test]
    fn a_read_only_volume_is_refused_rather_than_silently_made_writable() {
        let volume = RuntimeVolume {
            name: "workspace-data".to_owned(),
            target: "/workspace/.data".into(),
            writable: true,
            capacity_bytes: 1 << 30,
            ownership: OwnershipMetadata {
                managed_by: "gascan".to_owned(),
                sandbox_id: SandboxId::test("volumes"),
            },
        };
        let wire = volumes(std::slice::from_ref(&volume)).expect("a writable volume maps");
        assert_eq!(wire.len(), 1);
        assert_eq!(wire[0].name, "workspace-data");
        assert_eq!(wire[0].guest_path, "/workspace/.data");
        assert_eq!(wire[0].capacity_bytes, 1 << 30);

        let read_only = RuntimeVolume {
            writable: false,
            ..volume
        };
        assert_eq!(
            volumes(std::slice::from_ref(&read_only))
                .expect_err("the contract has no field for a read-only volume")
                .code(),
            "invalid_state",
        );
    }

    #[test]
    fn a_remove_request_spanning_two_sandboxes_is_refused() {
        let first = SandboxId::test("first");
        let second = SandboxId::test("second");
        let resource = |id: &SandboxId, name: &str| {
            RuntimeResource::discovered(
                ResourceIdentity::new(ResourceKind::Volume, name).expect("a valid identity"),
                Some(id.clone()),
                ResourceOwnership::GasCanOwned,
            )
        };

        let single = RemoveRequest::from_resources(vec![
            resource(&first, "first-data"),
            resource(&first, "first-cache"),
        ])
        .expect("one sandbox's resources");
        let wire = remove_request(&single).expect("a single-sandbox request maps");
        assert_eq!(wire.resources.len(), 2);
        assert_eq!(
            wire.owner.as_ref().map(|owner| owner.sandbox_id.as_str()),
            Some(first.as_str()),
            "the call carries the one sandbox's labels",
        );

        let mixed = RemoveRequest::from_resources(vec![
            resource(&first, "first-data"),
            resource(&second, "second-data"),
        ])
        .expect("core permits a mixed request; only the wire cannot express one");
        assert_eq!(
            remove_request(&mixed)
                .expect_err("one remove call carries one sandbox's labels")
                .code(),
            "invalid_state",
        );
    }

    fn wire_owner(sandbox_id: &str) -> v1::OwnerLabels {
        v1::OwnerLabels {
            managed_by: "gascan".to_owned(),
            sandbox_id: sandbox_id.to_owned(),
        }
    }

    /// A capability set with every flag off, which the one-hot test below raises
    /// one at a time.
    fn no_capabilities() -> v1::Capabilities {
        v1::Capabilities {
            engine_version: Some(v1::Version {
                major: 1,
                minor: 2,
                patch: 3,
            }),
            contract_minor: 0,
            project_mount: false,
            named_volumes: false,
            tty: false,
            signals: false,
            loopback_publish: false,
            resource_limits: false,
            offline: v1::Isolation::Proven as i32,
            // Empty, because nothing reads it yet and a plausible-looking
            // revision here would be a claim this test does not make. Field 20
            // arrived with the schema-2 pin; the certified-revision comparison
            // that turns it into a Proven/Unverified verdict is still to come,
            // and the tests that earn it belong with it rather than here.
            build_revision: String::new(),
        }
    }

    #[test]
    fn capabilities_widen_the_version_and_carry_the_isolation_verdict() {
        let capabilities = runtime_capabilities(&no_capabilities())
            .expect("a capability set with no optional flags still maps");
        assert_eq!(
            capabilities.version,
            gascan_core::runtime::RuntimeVersion::new(1, 2, 3)
        );
        assert_eq!(capabilities.offline, NetworkIsolation::Proven);
    }

    #[test]
    fn each_capability_flag_maps_to_exactly_one_field() {
        // No single fixture can separate six booleans, whatever mix of `true` and
        // `false` it uses: any two flags that share a value can be transposed
        // invisibly, and an all-true fixture cannot tell a mapped field from a
        // hardcoded `true` at all. So each flag is raised alone and all six fields
        // are read every time. That pins the `project_mount -> bind_mounts` rename
        // five times over -- five of the six cases require `bind_mounts` to be
        // false -- and any transposition lights up the wrong field.
        type Raise = fn(&mut v1::Capabilities);
        type Read = fn(&RuntimeCapabilities) -> bool;

        let flags: [(&str, Raise, Read); 6] = [
            (
                "project_mount -> bind_mounts",
                |wire| wire.project_mount = true,
                |mapped| mapped.bind_mounts,
            ),
            (
                "named_volumes",
                |wire| wire.named_volumes = true,
                |mapped| mapped.named_volumes,
            ),
            ("tty", |wire| wire.tty = true, |mapped| mapped.tty),
            (
                "signals",
                |wire| wire.signals = true,
                |mapped| mapped.signals,
            ),
            (
                "loopback_publish",
                |wire| wire.loopback_publish = true,
                |mapped| mapped.loopback_publish,
            ),
            (
                "resource_limits",
                |wire| wire.resource_limits = true,
                |mapped| mapped.resource_limits,
            ),
        ];

        for (raised, raise, _) in flags {
            let mut wire = no_capabilities();
            raise(&mut wire);
            let mapped = runtime_capabilities(&wire).expect("a one-hot capability set maps");
            for (read_back, _, read) in flags {
                assert_eq!(
                    read(&mapped),
                    raised == read_back,
                    "the wire set {raised} alone, so {read_back} must be {}",
                    raised == read_back,
                );
            }
        }
    }

    #[test]
    fn an_unspecified_isolation_or_absent_version_is_refused() {
        let unspecified = v1::Capabilities {
            engine_version: Some(v1::Version {
                major: 1,
                minor: 0,
                patch: 0,
            }),
            contract_minor: 0,
            project_mount: true,
            named_volumes: true,
            tty: true,
            signals: true,
            loopback_publish: true,
            resource_limits: true,
            offline: v1::Isolation::Unspecified as i32,
            build_revision: String::new(),
        };
        assert_eq!(
            runtime_capabilities(&unspecified)
                .expect_err("unspecified is not a value")
                .code(),
            "invalid_output",
        );

        let versionless = v1::Capabilities {
            engine_version: None,
            ..unspecified
        };
        assert_eq!(
            runtime_capabilities(&versionless)
                .expect_err("no version")
                .code(),
            "invalid_output",
        );
    }

    #[test]
    fn an_image_digest_reassembles_into_a_canonical_reference() {
        let image = runtime_image(Some(&v1::ImageDigest {
            repository: "registry.example/workspace".to_owned(),
            sha256_hex: DIGEST.to_owned(),
        }))
        .expect("a digest reassembles");
        assert_eq!(image, format!("registry.example/workspace@sha256:{DIGEST}"));
    }

    #[test]
    fn a_malformed_digest_is_refused_rather_than_concatenated() {
        assert_eq!(
            runtime_image(Some(&v1::ImageDigest {
                repository: "registry.example/workspace".to_owned(),
                sha256_hex: "not-a-digest".to_owned(),
            }))
            .expect_err("a short digest is not a reference")
            .code(),
            "invalid_output",
        );
        assert_eq!(
            runtime_image(None).expect_err("no image at all").code(),
            "invalid_output",
        );
    }

    #[test]
    fn inbound_ports_regain_the_loopback_address_they_never_sent() {
        let ports = runtime_ports(&[v1::PortMapping {
            host_port: 22222,
            guest_port: 22,
        }])
        .expect("a port maps");
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].host_address, IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_eq!((ports[0].host_port, ports[0].guest_port), (22222, 22));
    }

    #[test]
    fn an_out_of_range_zero_or_duplicated_inbound_port_is_refused() {
        for ports in [
            vec![v1::PortMapping {
                host_port: 65_536,
                guest_port: 22,
            }],
            vec![v1::PortMapping {
                host_port: 22222,
                guest_port: 70_000,
            }],
            vec![v1::PortMapping {
                host_port: 0,
                guest_port: 22,
            }],
            vec![v1::PortMapping {
                host_port: 22222,
                guest_port: 0,
            }],
            vec![
                v1::PortMapping {
                    host_port: 22222,
                    guest_port: 22,
                },
                v1::PortMapping {
                    host_port: 22222,
                    guest_port: 80,
                },
            ],
        ] {
            assert_eq!(
                runtime_ports(&ports).expect_err("must fail closed").code(),
                "invalid_output",
                "ports: {ports:?}",
            );
        }
    }

    #[test]
    fn a_sandbox_maps_and_its_labels_must_agree_with_its_id() {
        let id = gascan_core::sandbox::SandboxId::test("observed");
        let sandbox = v1::Sandbox {
            sandbox_id: id.as_str().to_owned(),
            image: Some(v1::ImageDigest {
                repository: "registry.example/workspace".to_owned(),
                sha256_hex: DIGEST.to_owned(),
            }),
            state: v1::SandboxState::Running as i32,
            owner: Some(wire_owner(id.as_str())),
            ports: Vec::new(),
        };
        let observed = runtime_sandbox(&sandbox).expect("a labelled running sandbox maps");
        assert_eq!(observed.state, ContainerState::Running);
        assert_eq!(observed.ownership.managed_by, "gascan");

        let disagreeing = v1::Sandbox {
            owner: Some(wire_owner(
                gascan_core::sandbox::SandboxId::test("other").as_str(),
            )),
            ..sandbox.clone()
        };
        assert_eq!(
            runtime_sandbox(&disagreeing)
                .expect_err("labels must describe this sandbox")
                .code(),
            "ownership_mismatch",
        );

        let unlabelled = v1::Sandbox {
            owner: None,
            ..sandbox.clone()
        };
        assert_eq!(
            runtime_sandbox(&unlabelled)
                .expect_err("a sandbox must be labelled")
                .code(),
            "invalid_output",
        );

        // The label-equals-id check IS the ownership guard on this path:
        // `managed_by` is copied verbatim with no MANAGED_BY check on either
        // backend, so an unparseable label that is tolerated instead of refused
        // yields an OwnershipMetadata whose sandbox_id is the sandbox's own id and
        // whose managed_by is whatever the engine claimed -- ownership fabricated
        // out of a garbage label, which the equality check below can never catch
        // because an unparseable label can never equal a parsed id.
        //
        // The regression is a specific one, not a hypothetical: `runtime_resource`
        // sixty lines below deliberately uses `.ok()` and carries on, because
        // ListResources must return foreign resources rather than fail on them.
        // Anyone unifying the two idioms would write `.unwrap_or_else(|_| id.clone())`
        // here and see every other test still pass.
        let unparseable = v1::Sandbox {
            owner: Some(v1::OwnerLabels {
                managed_by: "gascan".to_owned(),
                sandbox_id: "not a valid id".to_owned(),
            }),
            ..sandbox.clone()
        };
        assert_eq!(
            runtime_sandbox(&unparseable)
                .expect_err("a label that does not parse cannot be shown to describe this sandbox")
                .code(),
            "invalid_output",
        );

        let stateless = v1::Sandbox {
            state: v1::SandboxState::Unspecified as i32,
            ..sandbox
        };
        assert_eq!(
            runtime_sandbox(&stateless)
                .expect_err("unspecified is not a state")
                .code(),
            "unknown_actual_state",
        );
    }

    #[test]
    fn a_resource_is_classified_by_the_shared_rule() {
        let id = gascan_core::sandbox::SandboxId::test("owned");
        let container = v1::Resource {
            identity: Some(v1::ResourceIdentity {
                kind: v1::ResourceKind::Container as i32,
                name: id.as_str().to_owned(),
            }),
            owner: Some(wire_owner(id.as_str())),
        };
        assert_eq!(
            runtime_resource("list_resources", &container)
                .expect("maps")
                .ownership(),
            ResourceOwnership::GasCanOwned,
        );

        let unlabelled = v1::Resource {
            owner: None,
            ..container.clone()
        };
        assert_eq!(
            runtime_resource("list_resources", &unlabelled)
                .expect("maps")
                .ownership(),
            ResourceOwnership::Foreign,
            "ListResources returns unlabelled resources on purpose; they are not an error",
        );

        let unparseable = v1::Resource {
            owner: Some(v1::OwnerLabels {
                managed_by: "gascan".to_owned(),
                sandbox_id: "not a valid id".to_owned(),
            }),
            ..container.clone()
        };
        assert_eq!(
            runtime_resource("list_resources", &unparseable)
                .expect("maps")
                .ownership(),
            ResourceOwnership::Mismatched,
            "one malformed label must not blind the consumer to the rest of the inventory",
        );

        let kindless = v1::Resource {
            identity: Some(v1::ResourceIdentity {
                kind: v1::ResourceKind::Unspecified as i32,
                name: id.as_str().to_owned(),
            }),
            ..container.clone()
        };
        assert_eq!(
            runtime_resource("list_resources", &kindless)
                .expect_err("unspecified is not a kind")
                .code(),
            "invalid_output",
        );

        let identityless = v1::Resource {
            identity: None,
            ..container
        };
        assert_eq!(
            runtime_resource("list_resources", &identityless)
                .expect_err("a resource with no identity is not addressable")
                .code(),
            "invalid_output",
        );
    }

    #[test]
    fn a_mismatched_resource_still_reports_the_sandbox_id_it_claims() {
        // Parity with gascan-apple's inspect.rs, which reports the claimed id for a
        // mismatched resource because the reconciler finds it by that claim. If the
        // two backends disagree here, one reports an ownership mismatch the other
        // drops -- the divergence Task 1's shared classifier exists to prevent.
        let claimed = gascan_core::sandbox::SandboxId::test("claimed");
        let collision = v1::Resource {
            identity: Some(v1::ResourceIdentity {
                kind: v1::ResourceKind::Container as i32,
                name: "a-name-that-is-not-the-label".to_owned(),
            }),
            owner: Some(wire_owner(claimed.as_str())),
        };
        let resource =
            runtime_resource("list_resources", &collision).expect("a collision still maps");
        assert_eq!(resource.ownership(), ResourceOwnership::Mismatched);
        assert_eq!(
            resource
                .sandbox_id()
                .map(gascan_core::sandbox::SandboxId::as_str),
            Some(claimed.as_str()),
        );
    }
}
