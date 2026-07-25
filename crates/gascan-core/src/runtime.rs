use crate::sandbox::SandboxId;
use async_trait::async_trait;
use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::net::IpAddr;
use std::sync::Arc;
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
    pub capacity_bytes: u64,
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeNetwork {
    Networked { name: String },
    Offline,
}

impl RuntimeNetwork {
    pub fn managed_name(&self) -> Option<&str> {
        match self {
            Self::Networked { name } => Some(name),
            Self::Offline => None,
        }
    }
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

    pub const fn network(&self) -> &RuntimeNetwork {
        &self.network
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
    pub image: String,
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
pub enum ExecInput {
    Stdin(Vec<u8>),
    Resize { columns: u32, rows: u32 },
    Signal(i32),
    Close,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExecOutput {
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
    Exit { code: i32, signal: i32 },
}

#[derive(Debug)]
pub struct ExecSession {
    input: tokio::sync::mpsc::Sender<ExecInput>,
    output: tokio::sync::mpsc::Receiver<Result<ExecOutput, RuntimeError>>,
    cancellation: Option<ExecCancellation>,
}

#[derive(Clone, Debug)]
/// Backend-neutral cancellation ownership for a live [`ExecSession`].
///
/// Backends must arrange for cancellation to interrupt pending input/output
/// work and release the underlying guest process. Cancellation is idempotent.
pub struct ExecCancellation(tokio::sync::watch::Sender<bool>);

impl ExecCancellation {
    pub fn channel() -> (Self, tokio::sync::watch::Receiver<bool>) {
        let (sender, receiver) = tokio::sync::watch::channel(false);
        (Self(sender), receiver)
    }

    pub fn cancel(&self) {
        let _ = self.0.send(true);
    }
}

impl ExecSession {
    pub fn from_output(stdout: Vec<u8>, stderr: Vec<u8>, exit_code: i32) -> Self {
        let (input, _inputs) = tokio::sync::mpsc::channel(1);
        let (outputs, output) = tokio::sync::mpsc::channel(3);
        let _ = outputs.try_send(Ok(ExecOutput::Stdout(stdout)));
        let _ = outputs.try_send(Ok(ExecOutput::Stderr(stderr)));
        let _ = outputs.try_send(Ok(ExecOutput::Exit {
            code: exit_code,
            signal: 0,
        }));
        Self {
            input,
            output,
            cancellation: None,
        }
    }

    pub fn live(
        input: tokio::sync::mpsc::Sender<ExecInput>,
        output: tokio::sync::mpsc::Receiver<Result<ExecOutput, RuntimeError>>,
    ) -> Self {
        Self {
            input,
            output,
            cancellation: None,
        }
    }

    /// Creates a live session whose explicit cancellation or drop interrupts
    /// backend work through `cancellation`.
    pub fn live_cancellable(
        input: tokio::sync::mpsc::Sender<ExecInput>,
        output: tokio::sync::mpsc::Receiver<Result<ExecOutput, RuntimeError>>,
        cancellation: ExecCancellation,
    ) -> Self {
        Self {
            input,
            output,
            cancellation: Some(cancellation),
        }
    }

    pub async fn send(&self, input: ExecInput) -> Result<(), RuntimeError> {
        self.input
            .send(input)
            .await
            .map_err(|_| RuntimeError::CommandIo {
                operation: "exec_input".to_owned(),
                message: "session input is closed".to_owned(),
            })
    }

    pub async fn next(&mut self) -> Option<Result<ExecOutput, RuntimeError>> {
        self.output.recv().await
    }

    /// Requests backend cancellation. Calling this more than once is safe.
    pub fn cancel(&self) {
        if let Some(cancellation) = &self.cancellation {
            cancellation.cancel();
        }
    }
}

impl Drop for ExecSession {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    Container,
    Volume,
    Network,
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

#[derive(Clone)]
struct RemovalProof(Arc<()>);

impl RemovalProof {
    fn new() -> Self {
        Self(Arc::new(()))
    }
}

impl std::fmt::Debug for RemovalProof {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("RemovalProof(<opaque>)")
    }
}

impl PartialEq for RemovalProof {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for RemovalProof {}

/// A runtime observation carrying an opaque, process-local removal proof.
///
/// Serialized observations are diagnostic-only and cannot be deserialized into
/// a deletion capability.
///
/// ```compile_fail
/// use gascan_core::runtime::RuntimeResource;
///
/// let _: RuntimeResource = serde_json::from_str("{}").unwrap();
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RuntimeResource {
    identity: ResourceIdentity,
    sandbox_id: Option<SandboxId>,
    ownership: ResourceOwnership,
    #[serde(skip)]
    removal_proof: RemovalProof,
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
            removal_proof: RemovalProof::new(),
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

/// Exact retained runtime observations authorized by a validated create request.
///
/// Callers cannot forge retained evidence with a struct literal.
///
/// ```compile_fail
/// use gascan_core::runtime::RetainedResources;
///
/// let _forged = RetainedResources { resources: Vec::new() };
/// ```
///
/// Serialized diagnostics cannot be deserialized into retained evidence.
///
/// ```compile_fail
/// use gascan_core::runtime::RetainedResources;
///
/// let _forged: RetainedResources = serde_json::from_str("{}").unwrap();
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RetainedResources {
    resources: Vec<RuntimeResource>,
}

impl RetainedResources {
    pub fn new(
        request: &CreateRequest,
        resources: Vec<RuntimeResource>,
    ) -> Result<Self, RuntimeError> {
        validate_retained_resources(request, &resources)?;
        Ok(Self { resources })
    }

    pub fn resources(&self) -> &[RuntimeResource] {
        &self.resources
    }
}

/// A validated desired topology paired with exact retained runtime evidence.
///
/// Callers cannot forge recreation requests with a struct literal.
///
/// ```compile_fail
/// use gascan_core::runtime::{CreateRequest, RecreateRequest, RetainedResources};
///
/// fn parts() -> (CreateRequest, RetainedResources) {
///     todo!()
/// }
/// let (create, retained) = parts();
/// let _forged = RecreateRequest { create, retained };
/// ```
///
/// Serialized diagnostics cannot be deserialized into recreation authority.
///
/// ```compile_fail
/// use gascan_core::runtime::RecreateRequest;
///
/// let _forged: RecreateRequest = serde_json::from_str("{}").unwrap();
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RecreateRequest {
    create: CreateRequest,
    retained: RetainedResources,
}

impl RecreateRequest {
    pub fn new(create: CreateRequest, retained: RetainedResources) -> Result<Self, RuntimeError> {
        validate_retained_resources(&create, retained.resources())?;
        Ok(Self { create, retained })
    }

    pub fn for_image(
        mut create: CreateRequest,
        image: String,
        retained: RetainedResources,
    ) -> Result<Self, RuntimeError> {
        if !immutable_image_reference(&image) {
            return Err(RuntimeError::InvalidState {
                resource: "recreate image".to_owned(),
                message: "image must be a nonempty named sha256 digest reference".to_owned(),
            });
        }
        create.image = image;
        Self::new(create, retained)
    }

    pub const fn create(&self) -> &CreateRequest {
        &self.create
    }

    pub const fn retained(&self) -> &RetainedResources {
        &self.retained
    }
}

pub fn immutable_image_reference(image: &str) -> bool {
    let Some((name, digest)) = image.split_once("@sha256:") else {
        return false;
    };
    valid_image_name(name)
        && digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub fn same_immutable_image(left: &str, right: &str) -> bool {
    let Some(left) = immutable_image_identity(left) else {
        return false;
    };
    let Some(right) = immutable_image_identity(right) else {
        return false;
    };
    left == right
}

fn immutable_image_identity(image: &str) -> Option<(&str, &str)> {
    if !immutable_image_reference(image) {
        return None;
    }
    let (name, digest) = image.split_once("@sha256:")?;
    let last_slash = name.rfind('/');
    let tag_separator = name
        .rfind(':')
        .filter(|separator| last_slash.is_none_or(|slash| *separator > slash));
    let repository = tag_separator.map_or(name, |separator| &name[..separator]);
    Some((repository, digest))
}

fn valid_image_name(name: &str) -> bool {
    if name.is_empty() || !name.is_ascii() || name.bytes().any(|byte| byte.is_ascii_whitespace()) {
        return false;
    }
    let last_slash = name.rfind('/');
    let tag_separator = name
        .rfind(':')
        .filter(|separator| last_slash.is_none_or(|slash| *separator > slash));
    let (repository, tag) = tag_separator.map_or((name, None), |separator| {
        (&name[..separator], Some(&name[separator + 1..]))
    });
    if tag.is_some_and(|tag| {
        tag.is_empty()
            || tag.len() > 128
            || !tag
                .bytes()
                .next()
                .is_some_and(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            || !tag
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
    }) {
        return false;
    }
    repository
        .split('/')
        .enumerate()
        .all(|(index, component)| valid_repository_component(component, index == 0))
}

fn valid_repository_component(component: &str, allow_port: bool) -> bool {
    let (component, port) = if allow_port {
        component
            .split_once(':')
            .map_or((component, None), |(host, port)| (host, Some(port)))
    } else {
        (component, None)
    };
    if port.is_some_and(|port| port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit())) {
        return false;
    }
    !component.is_empty()
        && component
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && component
            .bytes()
            .last()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && component.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreateOutcome {
    created: Vec<RuntimeResource>,
}

impl CreateOutcome {
    pub fn new(
        request: &CreateRequest,
        created: Vec<RuntimeResource>,
    ) -> Result<Self, RuntimeError> {
        validate_created_resources(request, &created, true)?;
        Ok(Self { created })
    }

    pub fn created(&self) -> &[RuntimeResource] {
        &self.created
    }

    pub fn for_recreate(
        request: &RecreateRequest,
        created: Vec<RuntimeResource>,
    ) -> Result<Self, RuntimeError> {
        validate_recreated_container(request.create(), &created)?;
        Ok(Self { created })
    }
}

#[derive(Debug)]
pub struct CreateFailure {
    created: Vec<RuntimeResource>,
    source: RuntimeError,
}

impl CreateFailure {
    pub fn new(
        request: &CreateRequest,
        created: Vec<RuntimeResource>,
        source: RuntimeError,
    ) -> Result<Self, RuntimeError> {
        validate_created_resources(request, &created, false)?;
        Ok(Self { created, source })
    }

    pub fn from_source(source: RuntimeError) -> Self {
        Self {
            created: Vec::new(),
            source,
        }
    }

    /// Retains every independently valid piece of create evidence and drops any
    /// malformed, duplicate, foreign, or request-unrelated observation.
    pub fn from_created_evidence(
        request: &CreateRequest,
        created: Vec<RuntimeResource>,
        source: RuntimeError,
    ) -> Self {
        let container = ResourceIdentity {
            kind: ResourceKind::Container,
            name: request.id.to_string(),
        };
        let allowed_volumes: BTreeSet<_> = request
            .volumes()
            .iter()
            .map(|volume| ResourceIdentity {
                kind: ResourceKind::Volume,
                name: volume.name.clone(),
            })
            .collect();
        let allowed_network = allowed_network(request);
        let mut identities = BTreeSet::new();
        let created = created
            .into_iter()
            .filter(|resource| {
                let allowed = resource.identity == container
                    || allowed_volumes.contains(&resource.identity)
                    || allowed_network.as_ref() == Some(&resource.identity);
                allowed
                    && resource.ownership == ResourceOwnership::GasCanOwned
                    && resource.sandbox_id.as_ref() == Some(&request.id)
                    && identities.insert(resource.identity.clone())
            })
            .collect();
        Self { created, source }
    }

    pub fn created(&self) -> &[RuntimeResource] {
        &self.created
    }

    pub const fn source(&self) -> &RuntimeError {
        &self.source
    }

    pub const fn code(&self) -> &'static str {
        self.source.code()
    }
}

impl std::fmt::Display for CreateFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.source.fmt(formatter)
    }
}

impl std::error::Error for CreateFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

fn allowed_network(request: &CreateRequest) -> Option<ResourceIdentity> {
    request
        .network()
        .managed_name()
        .map(|name| ResourceIdentity {
            kind: ResourceKind::Network,
            name: name.to_owned(),
        })
}

fn validate_retained_resources(
    request: &CreateRequest,
    resources: &[RuntimeResource],
) -> Result<(), RuntimeError> {
    let expected_volumes = request
        .volumes()
        .iter()
        .map(|volume| ResourceIdentity::new(ResourceKind::Volume, volume.name.clone()))
        .collect::<Result<BTreeSet<_>, _>>()?;
    let expected_network = allowed_network(request);
    let mut identities = BTreeSet::new();
    for resource in resources {
        let expected = expected_volumes.contains(resource.identity())
            || expected_network.as_ref() == Some(resource.identity());
        if !expected
            || resource.ownership() != ResourceOwnership::GasCanOwned
            || resource.sandbox_id() != Some(request.id())
            || !identities.insert(resource.identity().clone())
        {
            return Err(RuntimeError::OwnershipMismatch {
                resource: resource.name().to_owned(),
            });
        }
    }
    let expected_count = expected_volumes.len() + usize::from(expected_network.is_some());
    if identities.len() != expected_count {
        return Err(RuntimeError::InvalidState {
            resource: request.id().to_string(),
            message: "retained resources do not exactly match the requested topology".to_owned(),
        });
    }
    Ok(())
}

fn validate_recreated_container(
    request: &CreateRequest,
    created: &[RuntimeResource],
) -> Result<(), RuntimeError> {
    let expected = ResourceIdentity::new(ResourceKind::Container, request.id().to_string())?;
    let [container] = created else {
        return Err(RuntimeError::InvalidState {
            resource: request.id().to_string(),
            message: "recreate outcome must contain exactly the requested container".to_owned(),
        });
    };
    if container.identity() != &expected
        || container.ownership() != ResourceOwnership::GasCanOwned
        || container.sandbox_id() != Some(request.id())
    {
        return Err(RuntimeError::OwnershipMismatch {
            resource: container.name().to_owned(),
        });
    }
    Ok(())
}

fn validate_created_resources(
    request: &CreateRequest,
    created: &[RuntimeResource],
    require_container: bool,
) -> Result<(), RuntimeError> {
    let container = ResourceIdentity::new(ResourceKind::Container, request.id.to_string())?;
    let allowed_volumes = request
        .volumes()
        .iter()
        .map(|volume| ResourceIdentity::new(ResourceKind::Volume, volume.name.clone()))
        .collect::<Result<BTreeSet<_>, _>>()?;
    let allowed_network = allowed_network(request);
    let mut identities = BTreeSet::new();
    for resource in created {
        let allowed = resource.identity == container
            || allowed_volumes.contains(&resource.identity)
            || allowed_network.as_ref() == Some(&resource.identity);
        if !allowed
            || resource.ownership != ResourceOwnership::GasCanOwned
            || resource.sandbox_id.as_ref() != Some(&request.id)
            || !identities.insert(resource.identity.clone())
        {
            return Err(RuntimeError::OwnershipMismatch {
                resource: resource.name().to_owned(),
            });
        }
    }
    if require_container && !identities.contains(&container) {
        return Err(RuntimeError::InvalidState {
            resource: request.id.to_string(),
            message: "create outcome does not contain the requested container".to_owned(),
        });
    }
    if require_container
        && allowed_network
            .as_ref()
            .is_some_and(|network| !identities.contains(network))
    {
        return Err(RuntimeError::InvalidState {
            resource: request.id.to_string(),
            message: "create outcome does not contain the requested managed network".to_owned(),
        });
    }
    Ok(())
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
        if resources
            .iter()
            .any(|resource| resource.ownership != ResourceOwnership::GasCanOwned)
        {
            return Err(RuntimeError::OwnershipMismatch {
                resource: "remove request".to_owned(),
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
    PrepareImage(String),
    CreateContainer(RecreateRequest),
    Start(SandboxId),
    Stop(SandboxId),
    Remove(RemoveRequest),
    Exec(ExecRequest),
    Logs(SandboxId),
    ListResources,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RuntimeOutcome {
    Created(CreateOutcome),
    Removed(RemoveRequest),
    Failure { boundary: String, code: String },
}

#[async_trait]
pub trait RuntimeBackend: Send + Sync {
    async fn capabilities(&self) -> Result<RuntimeCapabilities, RuntimeError>;
    async fn inspect(&self, id: &SandboxId) -> Result<Option<RuntimeSandbox>, RuntimeError>;
    async fn create(&self, request: CreateRequest) -> Result<CreateOutcome, CreateFailure>;
    async fn prepare_image(&self, image: &str) -> Result<(), RuntimeError>;
    async fn create_container(
        &self,
        request: RecreateRequest,
    ) -> Result<CreateOutcome, CreateFailure>;
    async fn start(&self, id: &SandboxId) -> Result<(), RuntimeError>;
    async fn stop(&self, id: &SandboxId) -> Result<(), RuntimeError>;
    async fn remove(&self, request: RemoveRequest) -> Result<(), RuntimeError>;
    async fn exec(&self, request: ExecRequest) -> Result<ExecSession, RuntimeError>;
    async fn logs(
        &self,
        id: &SandboxId,
        since_millis: Option<i64>,
    ) -> Result<Vec<u8>, RuntimeError>;
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
