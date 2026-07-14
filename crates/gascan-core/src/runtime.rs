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
/// [`crate::policy::PolicyCompiler`] is the only construction path.
///
/// External callers cannot construct a request through a test fixture.
///
/// ```compile_fail
/// use gascan_core::runtime::CreateRequest;
/// use gascan_core::sandbox::SandboxId;
///
/// let _unchecked = CreateRequest::fixture(SandboxId::test("unchecked"));
/// ```
///
/// There is no generic constructor or builder API.
///
/// ```compile_fail
/// use gascan_core::runtime::CreateRequest;
///
/// let _unchecked = CreateRequest::new();
/// ```
///
/// ```compile_fail
/// use gascan_core::runtime::CreateRequest;
///
/// let _unchecked = CreateRequest::builder();
/// ```
///
/// External callers cannot construct requests with a struct literal.
///
/// ```compile_fail
/// use gascan_core::runtime::CreateRequest;
/// use gascan_core::sandbox::SandboxId;
///
/// let _unchecked = CreateRequest {
///     image: "mutable.example/workspace:latest".to_owned(),
///     id: SandboxId::test("sealed"),
///     bind_mounts: Vec::new(),
///     volumes: Vec::new(),
///     ports: Vec::new(),
///     environment: Default::default(),
///     resources: Default::default(),
///     network: gascan_core::runtime::RuntimeNetwork::Offline,
///     user: gascan_core::runtime::RuntimeUser::Workspace,
///     init: true,
///     ownership: gascan_core::runtime::OwnershipMetadata {
///         managed_by: "gascan".to_owned(),
///         sandbox_id: SandboxId::test("sealed"),
///     },
/// };
/// ```
///
/// Nor can callers replace fields on a validated request with struct update syntax.
///
/// ```compile_fail
/// use gascan_core::runtime::CreateRequest;
///
/// fn validated_request() -> CreateRequest {
///     todo!()
/// }
///
/// let _unchecked = CreateRequest {
///     image: "mutable.example/workspace:latest".to_owned(),
///     ..validated_request()
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

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    Container,
    Volume,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceOwnership {
    GasCanOwned,
    Foreign,
    Mismatched,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct ResourceIdentity {
    kind: ResourceKind,
    name: String,
}

impl ResourceIdentity {
    pub fn new(kind: ResourceKind, name: impl Into<String>) -> Result<Self, RuntimeError> {
        let name = name.into();
        if name.trim().is_empty() || name.chars().any(char::is_control) {
            return Err(RuntimeError::InvalidResourceIdentity { name });
        }
        Ok(Self { kind, name })
    }

    pub const fn kind(&self) -> ResourceKind {
        self.kind
    }
    pub fn name(&self) -> &str {
        &self.name
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RuntimeResource {
    identity: ResourceIdentity,
    sandbox_id: Option<SandboxId>,
    ownership: ResourceOwnership,
}

impl RuntimeResource {
    pub fn discovered(
        identity: ResourceIdentity,
        sandbox_id: Option<SandboxId>,
        ownership: ResourceOwnership,
    ) -> Self {
        Self {
            identity,
            sandbox_id,
            ownership,
        }
    }

    pub const fn identity(&self) -> &ResourceIdentity {
        &self.identity
    }
    pub const fn kind(&self) -> ResourceKind {
        self.identity.kind
    }
    pub fn name(&self) -> &str {
        &self.identity.name
    }
    pub const fn sandbox_id(&self) -> Option<&SandboxId> {
        self.sandbox_id.as_ref()
    }
    pub const fn ownership(&self) -> ResourceOwnership {
        self.ownership
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreateOutcome {
    pub created: Vec<RuntimeResource>,
}

impl CreateOutcome {
    pub fn new(created: Vec<RuntimeResource>) -> Result<Self, RuntimeError> {
        if created
            .iter()
            .any(|resource| resource.ownership != ResourceOwnership::GasCanOwned)
        {
            return Err(RuntimeError::OwnershipMismatch {
                resource: "create outcome".to_owned(),
            });
        }
        Ok(Self { created })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoveRequest {
    resources: Vec<RuntimeResource>,
}

impl RemoveRequest {
    pub fn from_resources(resources: Vec<RuntimeResource>) -> Result<Self, RuntimeError> {
        if resources.is_empty() {
            return Err(RuntimeError::InvalidState {
                resource: "remove request".to_owned(),
                message: "at least one exact resource is required".to_owned(),
            });
        }
        Ok(Self { resources })
    }

    pub fn resources(&self) -> &[RuntimeResource] {
        &self.resources
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RuntimeCall {
    Capabilities,
    Inspect(SandboxId),
    Create(CreateRequest),
    Start(SandboxId),
    Stop(SandboxId),
    Remove(RemoveRequest),
    Exec(ExecRequest),
    Logs(SandboxId),
    ListResources,
}

#[async_trait]
pub trait RuntimeBackend: Send + Sync {
    async fn capabilities(&self) -> Result<RuntimeCapabilities, RuntimeError>;
    async fn inspect(&self, id: &SandboxId) -> Result<Option<RuntimeSandbox>, RuntimeError>;
    async fn create(&self, request: CreateRequest) -> Result<CreateOutcome, RuntimeError>;
    async fn start(&self, id: &SandboxId) -> Result<(), RuntimeError>;
    async fn stop(&self, id: &SandboxId) -> Result<(), RuntimeError>;
    async fn remove(&self, request: RemoveRequest) -> Result<(), RuntimeError>;
    async fn exec(&self, request: ExecRequest) -> Result<ExecSession, RuntimeError>;
    async fn logs(&self, id: &SandboxId) -> Result<Vec<u8>, RuntimeError>;
    async fn list_resources(&self) -> Result<Vec<RuntimeResource>, RuntimeError>;
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
    #[error("refusing to remove foreign resource: {resource}")]
    ForeignResourceRefused { resource: String },
    #[error("invalid resource identity: {name:?}")]
    InvalidResourceIdentity { name: String },
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
            Self::ForeignResourceRefused { .. } => "foreign_resource_refused",
            Self::InvalidResourceIdentity { .. } => "invalid_resource_identity",
            Self::Conflict { .. } => "resource_conflict",
            Self::NotFound { .. } => "not_found",
            Self::InvalidState { .. } => "invalid_state",
            Self::UnknownActualState { .. } => "unknown_actual_state",
            Self::InjectedFailure { .. } => "injected_failure",
        }
    }
}
