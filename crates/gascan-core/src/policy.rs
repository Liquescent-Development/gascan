use crate::manifest::{NetworkMode, UserMode};
use crate::runtime::{
    CreateRequest, NetworkIsolation, OwnershipMetadata, ResourceIdentity, ResourceKind,
    RuntimeBindMount, RuntimeCapabilities, RuntimeError, RuntimeNetwork, RuntimePort,
    RuntimeResourceLimits, RuntimeUser, RuntimeVolume,
};
use crate::sandbox::{SandboxSpec, WORKSPACE_TARGET};
use camino::{Utf8Path, Utf8PathBuf};
use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, Ipv4Addr};
use thiserror::Error;

pub const DEFAULT_CPUS: u16 = 4;
pub const MAX_CPUS: u16 = 16;
pub const DEFAULT_MEMORY_BYTES: u64 = 8 * 1024_u64.pow(3);
pub const MAX_MEMORY_BYTES: u64 = 64 * 1024_u64.pow(3);
pub const DEFAULT_DISK_BYTES: u64 = 64 * 1024_u64.pow(3);
pub const MAX_DISK_BYTES: u64 = 512 * 1024_u64.pow(3);
pub const DEFAULT_PROCESS_COUNT: u32 = 1_024;

const MANAGED_BY: &str = "gascan";
const WORKSPACE_IMAGE: &str = include_str!("../../../images/workspace/approved-image.txt");
pub const WORKSPACE_HOME: &str = "/home/workspace";
pub const MISE_DATA_DIR: &str = "/home/workspace/.local/share/mise";
pub const MISE_CACHE_DIR: &str = "/home/workspace/.cache/mise";
pub const MISE_GLOBAL_CONFIG_FILE: &str = "/home/workspace/.config/gascan/mise.toml";
pub const MISE_SYSTEM_DATA_DIR: &str = "/opt/gascan/mise";
pub const CONTAINER_PATH: &str = "/home/workspace/.local/share/mise/shims:/opt/gascan/mise/shims:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

pub struct PolicyCompiler;

impl PolicyCompiler {
    pub fn expected_resource_identities(
        id: &crate::sandbox::SandboxId,
    ) -> Result<Vec<ResourceIdentity>, RuntimeError> {
        let mut identities = vec![ResourceIdentity::new(
            ResourceKind::Container,
            id.to_string(),
        )?];
        for name in managed_volume_names(id.as_str()) {
            identities.push(ResourceIdentity::new(ResourceKind::Volume, name)?);
        }
        Ok(identities)
    }

    pub fn compile(
        spec: SandboxSpec,
        capabilities: &RuntimeCapabilities,
    ) -> Result<CreateRequest, PolicyError> {
        validate_spec(&spec)?;
        validate_capabilities(&spec, capabilities)?;

        let manifest = spec.manifest();
        let ports = compile_ports(manifest.network(), manifest.ports())?;
        let resources = compile_resources(manifest.resources())?;
        let ownership = OwnershipMetadata {
            managed_by: MANAGED_BY.to_owned(),
            sandbox_id: spec.id().clone(),
        };
        let bind_mounts = spec
            .bind_mounts()
            .iter()
            .map(|mount| RuntimeBindMount {
                source: mount.source().to_owned(),
                target: mount.target().to_owned(),
                writable: mount.is_writable(),
            })
            .collect();
        let volumes = managed_volumes(spec.id().as_str(), &ownership);
        let network = match manifest.network() {
            NetworkMode::Networked => RuntimeNetwork::Networked,
            NetworkMode::Offline => RuntimeNetwork::Offline,
        };
        let user = match manifest.user() {
            UserMode::Workspace => RuntimeUser::Workspace,
            UserMode::Root => RuntimeUser::Root,
        };

        Ok(CreateRequest {
            id: spec.id().clone(),
            image: WORKSPACE_IMAGE.to_owned(),
            bind_mounts,
            volumes,
            ports,
            environment: BTreeMap::from([
                ("HOME".to_owned(), WORKSPACE_HOME.to_owned()),
                ("MISE_CACHE_DIR".to_owned(), MISE_CACHE_DIR.to_owned()),
                ("MISE_DATA_DIR".to_owned(), MISE_DATA_DIR.to_owned()),
                (
                    "MISE_GLOBAL_CONFIG_FILE".to_owned(),
                    MISE_GLOBAL_CONFIG_FILE.to_owned(),
                ),
                (
                    "MISE_SYSTEM_DATA_DIR".to_owned(),
                    MISE_SYSTEM_DATA_DIR.to_owned(),
                ),
                ("PATH".to_owned(), CONTAINER_PATH.to_owned()),
            ]),
            resources,
            network,
            user,
            init: true,
            ownership,
        })
    }
}

pub fn filtered_host_environment<I, K, V>(environment: I) -> BTreeMap<String, String>
where
    I: IntoIterator<Item = (K, V)>,
    K: Into<String>,
    V: Into<String>,
{
    environment
        .into_iter()
        .map(|(key, value)| (key.into(), value.into()))
        .filter(|(key, _)| is_allowed_environment_key(key))
        .collect()
}

fn is_allowed_environment_key(key: &str) -> bool {
    matches!(key, "TERM" | "COLORTERM" | "LANG")
        || key
            .strip_prefix("LC_")
            .is_some_and(|suffix| !suffix.is_empty())
}

fn validate_spec(spec: &SandboxSpec) -> Result<(), PolicyError> {
    let [mount] = spec.bind_mounts() else {
        return Err(PolicyError::InvalidMount);
    };
    if mount.source() != spec.canonical_root()
        || mount.target() != Utf8Path::new(WORKSPACE_TARGET)
        || !mount.is_writable()
    {
        return Err(PolicyError::InvalidMount);
    }
    Ok(())
}

fn validate_capabilities(
    spec: &SandboxSpec,
    capabilities: &RuntimeCapabilities,
) -> Result<(), PolicyError> {
    if !capabilities.bind_mounts {
        return Err(PolicyError::BindMountsUnavailable);
    }
    if !capabilities.named_volumes {
        return Err(PolicyError::NamedVolumesUnavailable);
    }
    if !capabilities.resource_limits {
        return Err(PolicyError::ResourceLimitsUnavailable);
    }
    if !spec.manifest().ports().is_empty() && !capabilities.loopback_publish {
        return Err(PolicyError::LoopbackPublishUnavailable);
    }
    if spec.manifest().network() == NetworkMode::Offline
        && capabilities.offline != NetworkIsolation::Proven
    {
        return Err(PolicyError::OfflineUnavailable);
    }
    Ok(())
}

fn compile_ports(
    network: NetworkMode,
    declared: &BTreeMap<String, u16>,
) -> Result<Vec<RuntimePort>, PolicyError> {
    if network == NetworkMode::Offline && !declared.is_empty() {
        return Err(PolicyError::OfflinePortsForbidden);
    }
    let mut seen = BTreeSet::new();
    declared
        .values()
        .map(|port| {
            if *port == 0 {
                return Err(PolicyError::InvalidPort);
            }
            if !seen.insert(*port) {
                return Err(PolicyError::DuplicatePort(*port));
            }
            Ok(RuntimePort {
                host_address: IpAddr::V4(Ipv4Addr::LOCALHOST),
                host_port: *port,
                guest_port: *port,
            })
        })
        .collect()
}

fn compile_resources(
    declared: &crate::manifest::Resources,
) -> Result<RuntimeResourceLimits, PolicyError> {
    let cpus = declared.cpus().unwrap_or(DEFAULT_CPUS);
    if cpus > MAX_CPUS {
        return Err(PolicyError::CpusExceedMaximum { requested: cpus });
    }
    let memory = declared
        .memory()
        .map_or(DEFAULT_MEMORY_BYTES, |value| value.bytes());
    if memory > MAX_MEMORY_BYTES {
        return Err(PolicyError::MemoryExceedsMaximum { requested: memory });
    }
    if declared.disk().is_some() {
        return Err(PolicyError::DiskControlUnsupported);
    }
    Ok(RuntimeResourceLimits {
        cpus: Some(cpus),
        memory_bytes: Some(memory),
        disk_bytes: None,
        process_count: None,
    })
}

fn managed_volumes(sandbox_id: &str, ownership: &OwnershipMetadata) -> Vec<RuntimeVolume> {
    managed_volume_names(sandbox_id)
        .into_iter()
        .zip([
            "/home/workspace/.local/share/mise",
            "/home/workspace/.cache",
            "/home/workspace/.config/gascan",
        ])
        .map(|(name, target)| RuntimeVolume {
            name,
            target: Utf8PathBuf::from(target),
            writable: true,
            ownership: ownership.clone(),
        })
        .collect()
}

fn managed_volume_names(sandbox_id: &str) -> [String; 3] {
    ["mise", "cache", "config"].map(|kind| format!("gascan-{kind}-{sandbox_id}"))
}

#[derive(Debug, Error, Eq, PartialEq)]
#[non_exhaustive]
pub enum PolicyError {
    #[error("sandbox must contain exactly the canonical writable /workspace mount")]
    InvalidMount,
    #[error("runtime cannot provide bind mounts")]
    BindMountsUnavailable,
    #[error("runtime cannot provide named volumes")]
    NamedVolumesUnavailable,
    #[error("runtime cannot enforce resource limits")]
    ResourceLimitsUnavailable,
    #[error("runtime cannot publish ports exclusively on loopback")]
    LoopbackPublishUnavailable,
    #[error("runtime cannot prove offline network isolation")]
    OfflineUnavailable,
    #[error("offline sandboxes cannot publish ports")]
    OfflinePortsForbidden,
    #[error("published ports must be nonzero")]
    InvalidPort,
    #[error("published port {0} is declared more than once")]
    DuplicatePort(u16),
    #[error("requested CPU count {requested} exceeds maximum {MAX_CPUS}")]
    CpusExceedMaximum { requested: u16 },
    #[error("requested memory {requested} exceeds maximum {MAX_MEMORY_BYTES}")]
    MemoryExceedsMaximum { requested: u64 },
    #[error("requested disk {requested} exceeds maximum {MAX_DISK_BYTES}")]
    DiskExceedsMaximum { requested: u64 },
    #[error("the current macOS backend cannot enforce a sandbox disk ceiling")]
    DiskControlUnsupported,
}

impl PolicyError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidMount => "invalid_mount",
            Self::BindMountsUnavailable => "bind_mounts_unavailable",
            Self::NamedVolumesUnavailable => "named_volumes_unavailable",
            Self::ResourceLimitsUnavailable => "resource_limits_unavailable",
            Self::LoopbackPublishUnavailable => "loopback_publish_unavailable",
            Self::OfflineUnavailable => "offline_unavailable",
            Self::OfflinePortsForbidden => "offline_ports_forbidden",
            Self::InvalidPort => "invalid_port",
            Self::DuplicatePort(_) => "duplicate_port",
            Self::CpusExceedMaximum { .. } => "cpus_exceed_maximum",
            Self::MemoryExceedsMaximum { .. } => "memory_exceeds_maximum",
            Self::DiskExceedsMaximum { .. } => "disk_exceeds_maximum",
            Self::DiskControlUnsupported => "disk_control_unsupported",
        }
    }
}
