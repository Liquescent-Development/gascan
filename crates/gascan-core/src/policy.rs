use crate::manifest::{NetworkMode, Storage, UserMode};
use crate::runtime::{
    CreateRequest, NetworkIsolation, OwnershipMetadata, ResourceIdentity, ResourceKind,
    RuntimeBindMount, RuntimeCapabilities, RuntimeError, RuntimeNetwork, RuntimePort,
    RuntimeResourceLimits, RuntimeUser, RuntimeVolume, immutable_image_reference,
};
use crate::sandbox::{SandboxSpec, WORKSPACE_TARGET};
use base64::{Engine as _, engine::general_purpose::STANDARD};
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
pub const TOOLS_ROOT: &str = "/home/workspace/.local";
pub const CACHE_ROOT: &str = "/home/workspace/.cache";
pub const CONFIG_ROOT: &str = "/home/workspace/.config";
pub const CARGO_HOME: &str = "/home/workspace/.local/share/cargo";
pub const RUSTUP_HOME: &str = "/home/workspace/.local/share/rustup";
pub const NPM_CACHE_DIR: &str = "/home/workspace/.cache/npm";
pub const GO_PATH: &str = "/home/workspace/.local/share/go";
pub const GO_BIN: &str = "/home/workspace/.local/bin";
pub const MISE_DATA_DIR: &str = "/home/workspace/.local/share/mise";
pub const MISE_CACHE_DIR: &str = "/home/workspace/.cache/mise";
pub const MISE_GLOBAL_CONFIG_FILE: &str = "/home/workspace/.config/gascan/mise.toml";
pub const MISE_STATE_DIR: &str = "/home/workspace/.config/gascan/mise-state";
pub const MISE_SYSTEM_CONFIG_FILE: &str = "/etc/mise/config.toml";
pub const MISE_SYSTEM_DATA_DIR: &str = "/opt/gascan/mise";
pub const CONTAINER_PATH: &str = concat!(
    "/home/workspace/.local/bin:",
    "/home/workspace/.local/share/cargo/bin:",
    "/home/workspace/.local/share/go/bin:",
    "/home/workspace/.local/share/gem/bin:",
    "/home/workspace/.local/share/mise/shims:",
    "/opt/gascan/mise/shims:",
    "/usr/local/sbin:/usr/local/bin:",
    "/opt/gascan/workstation/bin:",
    "/usr/sbin:/usr/bin:/sbin:/bin"
);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ControlPlanePolicy<'a> {
    pub ssh_authorized_key: Option<&'a str>,
    pub ssh_host_port: Option<u16>,
}

pub struct PolicyCompiler;

impl PolicyCompiler {
    #[must_use]
    pub const fn workspace_image() -> &'static str {
        WORKSPACE_IMAGE
    }

    pub fn managed_network_name(id: &crate::sandbox::SandboxId) -> String {
        format!("gascan-network-{id}")
    }

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
        identities.push(ResourceIdentity::new(
            ResourceKind::Network,
            Self::managed_network_name(id),
        )?);
        Ok(identities)
    }

    pub fn compile(
        spec: SandboxSpec,
        capabilities: &RuntimeCapabilities,
    ) -> Result<CreateRequest, PolicyError> {
        Self::compile_for_image(spec, capabilities, Self::workspace_image())
    }

    pub fn compile_for_image(
        spec: SandboxSpec,
        capabilities: &RuntimeCapabilities,
        workspace_image: &str,
    ) -> Result<CreateRequest, PolicyError> {
        Self::compile_for_image_internal(spec, capabilities, workspace_image, None)
    }

    pub fn compile_with_control_plane(
        spec: SandboxSpec,
        capabilities: &RuntimeCapabilities,
        control: ControlPlanePolicy<'_>,
    ) -> Result<CreateRequest, PolicyError> {
        Self::compile_for_image_with_control_plane(
            spec,
            capabilities,
            Self::workspace_image(),
            control,
        )
    }

    pub fn compile_for_image_with_control_plane(
        spec: SandboxSpec,
        capabilities: &RuntimeCapabilities,
        workspace_image: &str,
        control: ControlPlanePolicy<'_>,
    ) -> Result<CreateRequest, PolicyError> {
        Self::compile_for_image_internal(spec, capabilities, workspace_image, Some(control))
    }

    /// Replaces only the native SSH transport fields on an already validated request.
    ///
    /// This is used to reconstruct a previously inspected transport during rollback,
    /// including when the newly requested manifest disabled SSH.
    pub fn restore_ssh_transport(
        mut request: CreateRequest,
        control: Option<ControlPlanePolicy<'_>>,
    ) -> Result<CreateRequest, PolicyError> {
        request.ports.retain(|mapping| mapping.guest_port != 22);
        request.environment.remove("GASCAN_SSH_AUTHORIZED_KEY");
        request.environment.insert(
            "GASCAN_SSH_ENABLED".to_owned(),
            if control.is_some() { "1" } else { "0" }.to_owned(),
        );
        let Some(control) = control else {
            return Ok(request);
        };
        if matches!(request.network, RuntimeNetwork::Offline) {
            return Err(PolicyError::OfflinePortsForbidden);
        }
        let authorized_key = control
            .ssh_authorized_key
            .ok_or(PolicyError::MissingSshAuthorizedKey)?;
        if !is_ssh_public_key(authorized_key) {
            return Err(PolicyError::InvalidSshAuthorizedKey);
        }
        let host_port = control
            .ssh_host_port
            .ok_or(PolicyError::MissingSshHostPort)?;
        if host_port < 1024 {
            return Err(PolicyError::InvalidSshHostPort);
        }
        if request
            .ports
            .iter()
            .any(|mapping| mapping.host_port == host_port)
        {
            return Err(PolicyError::DuplicatePort(host_port));
        }
        request.ports.push(RuntimePort {
            host_address: IpAddr::V4(Ipv4Addr::LOCALHOST),
            host_port,
            guest_port: 22,
        });
        request.environment.insert(
            "GASCAN_SSH_AUTHORIZED_KEY".to_owned(),
            authorized_key.to_owned(),
        );
        Ok(request)
    }

    fn compile_for_image_internal(
        spec: SandboxSpec,
        capabilities: &RuntimeCapabilities,
        workspace_image: &str,
        control: Option<ControlPlanePolicy<'_>>,
    ) -> Result<CreateRequest, PolicyError> {
        if !immutable_image_reference(workspace_image) {
            return Err(PolicyError::InvalidWorkspaceImage);
        }
        validate_spec(&spec)?;

        let manifest = spec.manifest();
        let ssh_enabled = manifest.ssh().enabled() && control.is_some();
        validate_capabilities(&spec, capabilities, ssh_enabled)?;
        let control = control.unwrap_or_default();
        let ssh_host_port = if ssh_enabled {
            let authorized_key = control
                .ssh_authorized_key
                .ok_or(PolicyError::MissingSshAuthorizedKey)?;
            if !is_ssh_public_key(authorized_key) {
                return Err(PolicyError::InvalidSshAuthorizedKey);
            }
            let host_port = control
                .ssh_host_port
                .ok_or(PolicyError::MissingSshHostPort)?;
            if host_port < 1024 {
                return Err(PolicyError::InvalidSshHostPort);
            }
            if let Some(manifest_host_port) = manifest.ssh().host_port() {
                if manifest_host_port != host_port {
                    return Err(PolicyError::SshHostPortMismatch {
                        manifest: manifest_host_port,
                        control_plane: host_port,
                    });
                }
            }
            Some(host_port)
        } else {
            None
        };
        let ports = compile_ports(manifest.network(), manifest.ports(), ssh_host_port)?;
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
        let volumes = managed_volumes(spec.id().as_str(), manifest.storage(), &ownership);
        let network = match manifest.network() {
            NetworkMode::Networked => RuntimeNetwork::Networked {
                name: Self::managed_network_name(spec.id()),
            },
            NetworkMode::Offline => RuntimeNetwork::Offline,
        };
        let user = match manifest.user() {
            UserMode::Workspace => RuntimeUser::Workspace,
            UserMode::Root => RuntimeUser::Root,
        };

        Ok(CreateRequest {
            id: spec.id().clone(),
            image: workspace_image.to_owned(),
            bind_mounts,
            volumes,
            ports,
            environment: guest_environment(ssh_enabled, control),
            resources,
            network,
            user,
            init: true,
            ownership,
        })
    }
}

fn guest_environment(
    ssh_enabled: bool,
    control: ControlPlanePolicy<'_>,
) -> BTreeMap<String, String> {
    let mut environment = workspace_environment();
    environment.insert(
        "GASCAN_SSH_ENABLED".to_owned(),
        if ssh_enabled { "1" } else { "0" }.to_owned(),
    );
    if ssh_enabled {
        if let Some(authorized_key) = control.ssh_authorized_key {
            environment.insert(
                "GASCAN_SSH_AUTHORIZED_KEY".to_owned(),
                authorized_key.to_owned(),
            );
        }
    }
    environment
}

pub fn workspace_environment() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("CARGO_HOME".to_owned(), CARGO_HOME.to_owned()),
        (
            "GEM_HOME".to_owned(),
            "/home/workspace/.local/share/gem".to_owned(),
        ),
        (
            "GOCACHE".to_owned(),
            "/home/workspace/.cache/go-build".to_owned(),
        ),
        ("GOBIN".to_owned(), GO_BIN.to_owned()),
        (
            "GOMODCACHE".to_owned(),
            "/home/workspace/.cache/go-mod".to_owned(),
        ),
        ("GOPATH".to_owned(), GO_PATH.to_owned()),
        (
            "HEX_HOME".to_owned(),
            "/home/workspace/.local/share/hex".to_owned(),
        ),
        ("HOME".to_owned(), WORKSPACE_HOME.to_owned()),
        ("MISE_CARGO_HOME".to_owned(), CARGO_HOME.to_owned()),
        ("MISE_CACHE_DIR".to_owned(), MISE_CACHE_DIR.to_owned()),
        ("MISE_DATA_DIR".to_owned(), MISE_DATA_DIR.to_owned()),
        (
            "MISE_GLOBAL_CONFIG_FILE".to_owned(),
            MISE_GLOBAL_CONFIG_FILE.to_owned(),
        ),
        ("MISE_STATE_DIR".to_owned(), MISE_STATE_DIR.to_owned()),
        (
            "MISE_SYSTEM_CONFIG_FILE".to_owned(),
            MISE_SYSTEM_CONFIG_FILE.to_owned(),
        ),
        (
            "MISE_SYSTEM_DATA_DIR".to_owned(),
            MISE_SYSTEM_DATA_DIR.to_owned(),
        ),
        ("MISE_RUSTUP_HOME".to_owned(), RUSTUP_HOME.to_owned()),
        (
            "MIX_HOME".to_owned(),
            "/home/workspace/.local/share/mix".to_owned(),
        ),
        ("NPM_CONFIG_CACHE".to_owned(), NPM_CACHE_DIR.to_owned()),
        ("NPM_CONFIG_PREFIX".to_owned(), TOOLS_ROOT.to_owned()),
        ("PATH".to_owned(), CONTAINER_PATH.to_owned()),
        ("PYTHONUSERBASE".to_owned(), TOOLS_ROOT.to_owned()),
        (
            "REBAR_CACHE_DIR".to_owned(),
            "/home/workspace/.cache/rebar3".to_owned(),
        ),
        ("RUSTUP_HOME".to_owned(), RUSTUP_HOME.to_owned()),
        ("XDG_CACHE_HOME".to_owned(), CACHE_ROOT.to_owned()),
        ("XDG_CONFIG_HOME".to_owned(), CONFIG_ROOT.to_owned()),
        (
            "XDG_DATA_HOME".to_owned(),
            "/home/workspace/.local/share".to_owned(),
        ),
    ])
}

fn is_ssh_public_key(key: &str) -> bool {
    let mut fields = key.split_ascii_whitespace();
    let (Some(kind), Some(encoded)) = (fields.next(), fields.next()) else {
        return false;
    };
    if kind != "ssh-ed25519"
        || fields.next().is_some()
        || key.len() != kind.len() + 1 + encoded.len()
        || key.as_bytes().get(kind.len()) != Some(&b' ')
    {
        return false;
    }
    let Ok(blob) = STANDARD.decode(encoded) else {
        return false;
    };
    let Some((wire_kind, key_data)) = ssh_wire_string(&blob) else {
        return false;
    };
    let Some((public_key, trailing)) = ssh_wire_string(key_data) else {
        return false;
    };
    wire_kind == kind.as_bytes() && public_key.len() == 32 && trailing.is_empty()
}

fn ssh_wire_string(bytes: &[u8]) -> Option<(&[u8], &[u8])> {
    let length = u32::from_be_bytes(bytes.get(..4)?.try_into().ok()?) as usize;
    let content = bytes.get(4..)?;
    Some((content.get(..length)?, content.get(length..)?))
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
    matches!(
        key,
        "TERM" | "COLORTERM" | "LANG" | "GH_NO_UPDATE_NOTIFIER" | "GLAB_CHECK_UPDATE" | "NO_COLOR"
    ) || key
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
    ssh_enabled: bool,
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
    if (!spec.manifest().ports().is_empty() || ssh_enabled) && !capabilities.loopback_publish {
        return Err(PolicyError::LoopbackPublishUnavailable);
    }
    if spec.manifest().network() == NetworkMode::Offline {
        match capabilities.offline {
            NetworkIsolation::Proven => {}
            NetworkIsolation::Unsupported => {
                return Err(PolicyError::OfflineUnsupported {
                    version: capabilities.version.clone(),
                });
            }
            NetworkIsolation::Unverified => return Err(PolicyError::OfflineUnavailable),
        }
    }
    Ok(())
}

fn compile_ports(
    network: NetworkMode,
    declared: &BTreeMap<String, u16>,
    ssh_host_port: Option<u16>,
) -> Result<Vec<RuntimePort>, PolicyError> {
    if network == NetworkMode::Offline && !declared.is_empty() {
        return Err(PolicyError::OfflinePortsForbidden);
    }
    let mut seen = BTreeSet::new();
    let mut ports = declared
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
        .collect::<Result<Vec<_>, _>>()?;
    if let Some(host_port) = ssh_host_port {
        if !seen.insert(host_port) {
            return Err(PolicyError::DuplicatePort(host_port));
        }
        ports.push(RuntimePort {
            host_address: IpAddr::V4(Ipv4Addr::LOCALHOST),
            host_port,
            guest_port: 22,
        });
    }
    Ok(ports)
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

fn managed_volumes(
    sandbox_id: &str,
    storage: &Storage,
    ownership: &OwnershipMetadata,
) -> Vec<RuntimeVolume> {
    managed_volume_names(sandbox_id)
        .into_iter()
        .zip([
            (TOOLS_ROOT, storage.tools().bytes()),
            (CACHE_ROOT, storage.cache().bytes()),
            (CONFIG_ROOT, storage.config().bytes()),
        ])
        .map(|(name, (target, capacity_bytes))| RuntimeVolume {
            name,
            target: Utf8PathBuf::from(target),
            writable: true,
            capacity_bytes,
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
    #[error("workspace image must be an immutable digest-qualified reference")]
    InvalidWorkspaceImage,
    #[error("SSH authorized key must be an OpenSSH public key")]
    InvalidSshAuthorizedKey,
    #[error("enabled SSH requires an authorized public key")]
    MissingSshAuthorizedKey,
    #[error("enabled SSH requires a host port")]
    MissingSshHostPort,
    #[error("SSH host port must be in 1024..=65535")]
    InvalidSshHostPort,
    #[error(
        "control-plane SSH host port {control_plane} does not match manifest SSH host port {manifest}"
    )]
    SshHostPortMismatch { manifest: u16, control_plane: u16 },
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
    #[error(
        "hard offline isolation has not been verified with Apple Container {version_major}.{version_minor}.{version_patch}; use networked mode or install the certified 1.1.0 release",
        version_major = .version.major,
        version_minor = .version.minor,
        version_patch = .version.patch,
    )]
    OfflineUnsupported {
        version: crate::runtime::RuntimeVersion,
    },
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
            Self::InvalidWorkspaceImage => "invalid_workspace_image",
            Self::InvalidSshAuthorizedKey => "invalid_ssh_authorized_key",
            Self::MissingSshAuthorizedKey => "missing_ssh_authorized_key",
            Self::MissingSshHostPort => "missing_ssh_host_port",
            Self::InvalidSshHostPort => "invalid_ssh_host_port",
            Self::SshHostPortMismatch { .. } => "ssh_host_port_mismatch",
            Self::InvalidMount => "invalid_mount",
            Self::BindMountsUnavailable => "bind_mounts_unavailable",
            Self::NamedVolumesUnavailable => "named_volumes_unavailable",
            Self::ResourceLimitsUnavailable => "resource_limits_unavailable",
            Self::LoopbackPublishUnavailable => "loopback_publish_unavailable",
            Self::OfflineUnavailable | Self::OfflineUnsupported { .. } => "offline_unavailable",
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
