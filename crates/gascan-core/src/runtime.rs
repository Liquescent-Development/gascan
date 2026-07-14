use crate::sandbox::SandboxId;
use async_trait::async_trait;
use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::net::IpAddr;
use thiserror::Error;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RuntimeVersion {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl RuntimeVersion {
    pub const fn new(major: u64, minor: u64, patch: u64) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkIsolation {
    Proven,
    Unsupported,
    Unverified,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RuntimeCapabilities {
    pub version: RuntimeVersion,
    pub bind_mounts: bool,
    pub named_volumes: bool,
    pub tty: bool,
    pub signals: bool,
    pub loopback_publish: bool,
    pub resource_limits: bool,
    pub offline: NetworkIsolation,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OwnershipMetadata {
    pub managed_by: String,
    pub sandbox_id: SandboxId,
}

/// A policy-validated request accepted by [`RuntimeBackend::create`].
///
/// Its nested request-shape types remain public for backend inspection, but
/// they cannot be inserted into or used to mutate this sealed request. The
/// compiler and the fixed fixture are the only construction paths.
///
/// External callers cannot replace fields with an unchecked struct update.
///
/// ```compile_fail
/// use gascan_core::runtime::CreateRequest;
/// use gascan_core::sandbox::SandboxId;
///
/// let fixture = CreateRequest::fixture(SandboxId::test("sealed"));
/// let _unchecked = CreateRequest {
///     image: "mutable.example/workspace:latest".to_owned(),
///     ..fixture
/// };
/// ```
///
/// Serialized requests are output-only and cannot bypass policy validation.
///
/// ```compile_fail
/// use gascan_core::runtime::CreateRequest;
///
/// let _unchecked: CreateRequest = serde_json::from_str("{}").unwrap();
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CreateRequest {
    pub(crate) id: SandboxId,
    pub(crate) image: String,
    pub(crate) bind_mounts: Vec<RuntimeBindMount>,
    pub(crate) volumes: Vec<RuntimeVolume>,
    pub(crate) ports: Vec<RuntimePort>,
    pub(crate) environment: BTreeMap<String, String>,
    pub(crate) resources: RuntimeResourceLimits,
    pub(crate) network: RuntimeNetwork,
    pub(crate) user: RuntimeUser,
    /// Requires the backend to run the workspace under an init process.
    pub(crate) init: bool,
    pub(crate) ownership: OwnershipMetadata,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RuntimeBindMount {
    pub source: Utf8PathBuf,
    pub target: Utf8PathBuf,
    pub writable: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RuntimeVolume {
    pub name: String,
    pub target: Utf8PathBuf,
    pub writable: bool,
    pub ownership: OwnershipMetadata,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RuntimePort {
    pub host_address: IpAddr,
    pub host_port: u16,
    pub guest_port: u16,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct RuntimeResourceLimits {
    pub cpus: Option<u16>,
    pub memory_bytes: Option<u64>,
    pub disk_bytes: Option<u64>,
    pub process_count: Option<u32>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeNetwork {
    Networked,
    Offline,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeUser {
    Workspace,
    Root,
}

impl CreateRequest {
    /// Policy-shaped request for backend contract tests and downstream adapter fixtures.
    pub fn fixture(id: SandboxId) -> Self {
        Self {
            ownership: OwnershipMetadata {
                managed_by: "gascan".to_owned(),
                sandbox_id: id.clone(),
            },
            id,
            image: "fixture.invalid/workspace@sha256:0000000000000000000000000000000000000000000000000000000000000000".to_owned(),
            bind_mounts: vec![RuntimeBindMount {
                source: Utf8PathBuf::from("/tmp/code"),
                target: Utf8PathBuf::from("/workspace"),
                writable: true,
            }],
            volumes: Vec::new(),
            ports: vec![RuntimePort {
                host_address: IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
                host_port: 3000,
                guest_port: 3000,
            }],
            environment: BTreeMap::new(),
            resources: RuntimeResourceLimits::default(),
            network: RuntimeNetwork::Offline,
            user: RuntimeUser::Workspace,
            init: true,
        }
    }

    pub const fn id(&self) -> &SandboxId {
        &self.id
    }

    pub fn image(&self) -> &str {
        &self.image
    }

    pub fn bind_mounts(&self) -> &[RuntimeBindMount] {
        &self.bind_mounts
    }

    pub fn volumes(&self) -> &[RuntimeVolume] {
        &self.volumes
    }

    pub fn ports(&self) -> &[RuntimePort] {
        &self.ports
    }

    pub const fn environment(&self) -> &BTreeMap<String, String> {
        &self.environment
    }

    pub const fn resources(&self) -> &RuntimeResourceLimits {
        &self.resources
    }

    pub const fn network(&self) -> RuntimeNetwork {
        self.network
    }

    pub const fn user(&self) -> RuntimeUser {
        self.user
    }

    pub const fn init(&self) -> bool {
        self.init
    }

    pub const fn ownership(&self) -> &OwnershipMetadata {
        &self.ownership
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContainerState {
    Creating,
    Running,
    Stopped,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RuntimeSandbox {
    pub id: SandboxId,
    pub state: ContainerState,
    pub ownership: OwnershipMetadata,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExecRequest {
    pub id: SandboxId,
    pub argv: Vec<String>,
    pub stdin: Vec<u8>,
    pub environment: BTreeMap<String, String>,
    pub tty: bool,
}

impl ExecRequest {
    /// Byte-safe request fixture; it deliberately bypasses no sandbox-ID validation.
    pub fn fixture<I, S>(id: SandboxId, argv: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            id,
            argv: argv.into_iter().map(Into::into).collect(),
            stdin: Vec::new(),
            environment: BTreeMap::new(),
            tty: false,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecSession {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    exit_code: i32,
}

impl ExecSession {
    pub fn from_output(stdout: Vec<u8>, stderr: Vec<u8>, exit_code: i32) -> Self {
        Self {
            stdout,
            stderr,
            exit_code,
        }
    }

    pub fn stdout(&self) -> &[u8] {
        &self.stdout
    }
    pub fn stderr(&self) -> &[u8] {
        &self.stderr
    }
    pub const fn exit_code(&self) -> i32 {
        self.exit_code
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OwnedResource {
    pub id: SandboxId,
    pub ownership: OwnershipMetadata,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RuntimeCall {
    Capabilities,
    Inspect(SandboxId),
    Create(CreateRequest),
    Start(SandboxId),
    Stop(SandboxId),
    Remove(SandboxId),
    Exec(ExecRequest),
    Logs(SandboxId),
    ListOwned,
}

#[async_trait]
pub trait RuntimeBackend: Send + Sync {
    async fn capabilities(&self) -> Result<RuntimeCapabilities, RuntimeError>;
    async fn inspect(&self, id: &SandboxId) -> Result<Option<RuntimeSandbox>, RuntimeError>;
    async fn create(&self, request: CreateRequest) -> Result<(), RuntimeError>;
    async fn start(&self, id: &SandboxId) -> Result<(), RuntimeError>;
    async fn stop(&self, id: &SandboxId) -> Result<(), RuntimeError>;
    async fn remove(&self, id: &SandboxId) -> Result<(), RuntimeError>;
    async fn exec(&self, request: ExecRequest) -> Result<ExecSession, RuntimeError>;
    async fn logs(&self, id: &SandboxId) -> Result<Vec<u8>, RuntimeError>;
    async fn list_owned(&self) -> Result<Vec<OwnedResource>, RuntimeError>;
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RuntimeError {
    #[error("{operation}: {message}")]
    CommandIo { operation: String, message: String },
    #[error("{operation} failed with exit code {exit_code:?}: {stderr}")]
    CommandFailed {
        operation: String,
        exit_code: Option<i32>,
        stderr: String,
    },
    #[error("invalid output from {operation}: {message}")]
    InvalidOutput { operation: String, message: String },
    #[error("{operation} helper error {code}: {message}")]
    HelperError {
        operation: String,
        code: String,
        message: String,
    },
    #[error("unsupported runtime version {found:?}; supported versions: {supported}")]
    UnsupportedVersion {
        found: RuntimeVersion,
        supported: String,
    },
    #[error("unsupported capability: {capability}")]
    UnsupportedCapability { capability: String },
    #[error("resource ownership mismatch: {resource}")]
    OwnershipMismatch { resource: String },
    #[error("resource conflict for {resource}: {message}")]
    Conflict { resource: String, message: String },
    #[error("resource not found: {resource}")]
    NotFound { resource: String },
    #[error("invalid state for {resource}: {message}")]
    InvalidState { resource: String, message: String },
    #[error("unknown actual state for {resource}: {state}")]
    UnknownActualState { resource: String, state: String },
    #[error("injected failure at {boundary}")]
    InjectedFailure { boundary: String },
}

impl RuntimeError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::CommandIo { .. } => "command_io",
            Self::CommandFailed { .. } => "command_failed",
            Self::InvalidOutput { .. } => "invalid_output",
            Self::HelperError { .. } => "helper_error",
            Self::UnsupportedVersion { .. } => "unsupported_version",
            Self::UnsupportedCapability { .. } => "unsupported_capability",
            Self::OwnershipMismatch { .. } => "ownership_mismatch",
            Self::Conflict { .. } => "resource_conflict",
            Self::NotFound { .. } => "not_found",
            Self::InvalidState { .. } => "invalid_state",
            Self::UnknownActualState { .. } => "unknown_actual_state",
            Self::InjectedFailure { .. } => "injected_failure",
        }
    }
}
