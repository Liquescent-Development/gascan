use std::net::{IpAddr, Ipv4Addr};

use std::collections::BTreeMap;

use camino::Utf8PathBuf;
use gascan_core::runtime::{
    CreateRequest, OwnershipMetadata, RuntimeNetwork, RuntimePort, RuntimeResourceLimits,
    RuntimeUser,
};
use gascan_core::sandbox::SandboxId;
use thiserror::Error;

use crate::CommandSpec;

const MANAGED_BY: &str = "gascan";
const WORKSPACE_TARGET: &str = "/workspace";

#[derive(Clone)]
struct CreateView {
    id: SandboxId,
    image: String,
    bind_mounts: Vec<BindMountView>,
    volumes: Vec<VolumeView>,
    ports: Vec<RuntimePort>,
    environment: BTreeMap<String, String>,
    resources: RuntimeResourceLimits,
    network: RuntimeNetwork,
    user: RuntimeUser,
    init: bool,
    ownership: OwnershipMetadata,
}

#[derive(Clone)]
struct BindMountView {
    source: Utf8PathBuf,
    target: Utf8PathBuf,
    writable: bool,
}

#[derive(Clone)]
struct VolumeView {
    name: String,
    target: Utf8PathBuf,
    writable: bool,
    ownership: OwnershipMetadata,
}

impl CreateView {
    fn from_request(request: &CreateRequest) -> Self {
        Self {
            id: request.id().clone(),
            image: request.image().to_owned(),
            bind_mounts: request
                .bind_mounts()
                .iter()
                .map(|mount| BindMountView {
                    source: mount.source.clone(),
                    target: mount.target.clone(),
                    writable: mount.writable,
                })
                .collect(),
            volumes: request
                .volumes()
                .iter()
                .map(|volume| VolumeView {
                    name: volume.name.clone(),
                    target: volume.target.clone(),
                    writable: volume.writable,
                    ownership: volume.ownership.clone(),
                })
                .collect(),
            ports: request.ports().to_vec(),
            environment: request.environment().clone(),
            resources: *request.resources(),
            network: request.network().clone(),
            user: request.user(),
            init: request.init(),
            ownership: request.ownership().clone(),
        }
    }
}

pub struct AppleCommandBuilder;

impl AppleCommandBuilder {
    pub fn pull(image: &str) -> Result<CommandSpec, TranslationError> {
        validate_image(image)?;
        Ok(CommandSpec::new("container", ["image", "pull", image]))
    }

    pub fn inspect(id: &SandboxId) -> CommandSpec {
        CommandSpec::new("container", ["inspect", id.as_str()])
    }

    pub fn create(request: &CreateRequest) -> Result<CommandSpec, TranslationError> {
        let view = CreateView::from_request(request);
        validate_view(&view)?;

        let id = view.id.as_str();
        let mut args = vec![
            "run".to_owned(),
            "--name".to_owned(),
            id.to_owned(),
            "--label".to_owned(),
            format!("dev.gascan.managed-by={MANAGED_BY}"),
            "--label".to_owned(),
            format!("dev.gascan.sandbox-id={id}"),
        ];

        let mount = &view.bind_mounts[0];
        args.extend([
            "--mount".to_owned(),
            format!(
                "type=bind,source={},target={WORKSPACE_TARGET}",
                mount.source
            ),
        ]);
        for volume in &view.volumes {
            args.extend([
                "--volume".to_owned(),
                format!("{}:{}", volume.name, volume.target),
            ]);
        }
        for (key, value) in &view.environment {
            args.extend(["--env".to_owned(), format!("{key}={value}")]);
        }

        let resources = &view.resources;
        args.extend([
            "--cpus".to_owned(),
            resources
                .cpus
                .ok_or(TranslationError::MissingControl("cpus"))?
                .to_string(),
            "--memory".to_owned(),
            resources
                .memory_bytes
                .ok_or(TranslationError::MissingControl("memory"))?
                .to_string(),
            "--init".to_owned(),
            "--detach".to_owned(),
        ]);

        for port in &view.ports {
            args.extend([
                "--publish".to_owned(),
                format!(
                    "{}:{}:{}",
                    port.host_address, port.host_port, port.guest_port
                ),
            ]);
        }
        match &view.network {
            RuntimeNetwork::Networked { name } => {
                args.extend(["--network".to_owned(), name.clone()]);
            }
            RuntimeNetwork::Offline => {
                args.extend(["--network".to_owned(), "none".to_owned()]);
            }
        }
        args.push(view.image);

        Ok(CommandSpec::new("container", args))
    }
}

fn validate_view(view: &CreateView) -> Result<(), TranslationError> {
    validate_image(&view.image)?;
    let id = &view.id;
    let ownership = &view.ownership;
    if ownership.managed_by != MANAGED_BY || ownership.sandbox_id != *id {
        return Err(TranslationError::InvalidOwnership);
    }
    let [mount] = view.bind_mounts.as_slice() else {
        return Err(TranslationError::InvalidWorkspaceMount);
    };
    let canonical_source = std::fs::canonicalize(mount.source.as_std_path()).ok();
    if mount.target.as_str() != WORKSPACE_TARGET
        || !mount.writable
        || canonical_source.as_deref() != Some(mount.source.as_std_path())
    {
        return Err(TranslationError::InvalidWorkspaceMount);
    }

    let expected_volumes = [
        (
            format!("gascan-mise-{id}"),
            "/home/workspace/.local/share/mise",
        ),
        (format!("gascan-cache-{id}"), "/home/workspace/.cache"),
        (
            format!("gascan-config-{id}"),
            "/home/workspace/.config/gascan",
        ),
    ];
    if view.volumes.len() != expected_volumes.len() {
        return Err(TranslationError::InvalidOwnedVolume);
    }
    for (volume, (expected_name, expected_target)) in view.volumes.iter().zip(expected_volumes) {
        if volume.name != expected_name
            || volume.target.as_str() != expected_target
            || !volume.writable
            || volume.ownership != *ownership
        {
            return Err(TranslationError::InvalidOwnedVolume);
        }
    }
    if view
        .ports
        .iter()
        .any(|port| port.host_address != IpAddr::V4(Ipv4Addr::LOCALHOST))
    {
        return Err(TranslationError::NonLoopbackPort);
    }
    if view.resources.disk_bytes.is_some() {
        return Err(TranslationError::UnsupportedControl("disk"));
    }
    if view.resources.process_count.is_some() {
        return Err(TranslationError::UnsupportedControl("process_count"));
    }
    if view.resources.cpus.is_none() {
        return Err(TranslationError::MissingControl("cpus"));
    }
    if view.resources.memory_bytes.is_none() {
        return Err(TranslationError::MissingControl("memory"));
    }
    if view.user != RuntimeUser::Workspace {
        return Err(TranslationError::UnsupportedUser);
    }
    if !view.init {
        return Err(TranslationError::InitRequired);
    }
    Ok(())
}

fn validate_image(image: &str) -> Result<(), TranslationError> {
    let Some((name, digest)) = image.split_once("@sha256:") else {
        return Err(TranslationError::MissingImageDigest);
    };
    if name.is_empty()
        || digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(TranslationError::MissingImageDigest);
    }
    Ok(())
}

#[derive(Debug, Error, Eq, PartialEq)]
#[non_exhaustive]
pub enum TranslationError {
    #[error("image reference must contain an immutable sha256 digest")]
    MissingImageDigest,
    #[error("request ownership does not match the Gas Can sandbox")]
    InvalidOwnership,
    #[error("request must contain one canonical writable /workspace mount")]
    InvalidWorkspaceMount,
    #[error("request contains a missing, unowned, or unexpected managed volume")]
    InvalidOwnedVolume,
    #[error("published ports must bind to IPv4 loopback")]
    NonLoopbackPort,
    #[error("Apple runtime translation does not support the {0} control")]
    UnsupportedControl(&'static str),
    #[error("Apple runtime translation requires the {0} control")]
    MissingControl(&'static str),
    #[error("Apple runtime translation supports only the locked image workspace user")]
    UnsupportedUser,
    #[error("Apple runtime translation requires init")]
    InitRequired,
}

impl TranslationError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::MissingImageDigest => "missing_image_digest",
            Self::InvalidOwnership => "invalid_ownership",
            Self::InvalidWorkspaceMount => "invalid_workspace_mount",
            Self::InvalidOwnedVolume => "invalid_owned_volume",
            Self::NonLoopbackPort => "non_loopback_port",
            Self::UnsupportedControl(_) => "unsupported_control",
            Self::MissingControl(_) => "missing_control",
            Self::UnsupportedUser => "unsupported_user",
            Self::InitRequired => "init_required",
        }
    }
}

#[cfg(test)]
mod tests {
    use camino::Utf8Path;
    use gascan_core::manifest::Manifest;
    use gascan_core::policy::PolicyCompiler;
    use gascan_core::runtime::{
        CreateRequest, NetworkIsolation, RuntimeCapabilities, RuntimeVersion,
    };
    use gascan_core::sandbox::SandboxSpec;

    use super::{AppleCommandBuilder, CreateView, TranslationError, validate_view};

    fn request() -> (tempfile::TempDir, CreateRequest) {
        let temp = tempfile::tempdir().expect("temporary translation validation root");
        let root = Utf8Path::from_path(temp.path()).expect("UTF-8 temporary path");
        std::fs::write(
            root.join("gascan.toml"),
            "version = 1\nnetwork = 'networked'\n[ports]\nweb = 3000\n",
        )
        .expect("write validation manifest");
        let manifest = Manifest::load(root).expect("load validation manifest");
        let spec = SandboxSpec::from_root("validation", root, manifest)
            .expect("build sealed sandbox spec");
        let capabilities = RuntimeCapabilities {
            version: RuntimeVersion::new(1, 1, 0),
            bind_mounts: true,
            named_volumes: true,
            tty: true,
            signals: true,
            loopback_publish: true,
            resource_limits: true,
            offline: NetworkIsolation::Proven,
        };
        let request = PolicyCompiler::compile(spec, &capabilities).expect("compile policy");
        (temp, request)
    }

    fn assert_view_error(mutate: impl FnOnce(&mut CreateView), expected: TranslationError) {
        let (_root, request) = request();
        let mut view = CreateView::from_request(&request);
        mutate(&mut view);
        let error = validate_view(&view).expect_err("mutation must fail closed");
        assert_eq!(error, expected);
        assert_eq!(error.code(), expected.code());
    }

    #[test]
    fn rejects_every_ownership_mount_and_volume_invariant() {
        assert_view_error(
            |view| view.ownership.managed_by = "foreign".to_owned(),
            TranslationError::InvalidOwnership,
        );
        assert_view_error(
            |view| view.ownership.sandbox_id = gascan_core::sandbox::SandboxId::test("mismatch"),
            TranslationError::InvalidOwnership,
        );
        assert_view_error(
            |view| view.bind_mounts.clear(),
            TranslationError::InvalidWorkspaceMount,
        );
        assert_view_error(
            |view| view.bind_mounts.push(view.bind_mounts[0].clone()),
            TranslationError::InvalidWorkspaceMount,
        );
        assert_view_error(
            |view| view.bind_mounts[0].source.push(".."),
            TranslationError::InvalidWorkspaceMount,
        );
        assert_view_error(
            |view| view.bind_mounts[0].writable = false,
            TranslationError::InvalidWorkspaceMount,
        );
        assert_view_error(
            |view| {
                view.volumes.pop();
            },
            TranslationError::InvalidOwnedVolume,
        );
        assert_view_error(
            |view| view.volumes.push(view.volumes[0].clone()),
            TranslationError::InvalidOwnedVolume,
        );
        assert_view_error(
            |view| view.volumes[0].ownership.managed_by = "foreign".to_owned(),
            TranslationError::InvalidOwnedVolume,
        );
        assert_view_error(
            |view| view.volumes[0].target = "/unexpected".into(),
            TranslationError::InvalidOwnedVolume,
        );
        assert_view_error(
            |view| view.volumes[0].writable = false,
            TranslationError::InvalidOwnedVolume,
        );
    }

    #[test]
    fn rejects_wrong_bind_target_with_the_exact_typed_error() {
        assert_view_error(
            |view| view.bind_mounts[0].target = "/not-workspace".into(),
            TranslationError::InvalidWorkspaceMount,
        );
    }

    #[test]
    fn rejects_unexpected_volume_name_without_changing_required_count() {
        assert_view_error(
            |view| view.volumes[0].name = "gascan-unexpected".to_owned(),
            TranslationError::InvalidOwnedVolume,
        );
    }

    #[test]
    fn rejects_volume_ownership_sandbox_mismatch() {
        assert_view_error(
            |view| {
                view.volumes[0].ownership.sandbox_id =
                    gascan_core::sandbox::SandboxId::test("mismatch")
            },
            TranslationError::InvalidOwnedVolume,
        );
    }

    #[test]
    fn rejects_every_port_resource_user_and_init_invariant() {
        assert_view_error(
            |view| view.ports[0].host_address = "0.0.0.0".parse().expect("valid test IP"),
            TranslationError::NonLoopbackPort,
        );
        assert_view_error(
            |view| view.resources.disk_bytes = Some(1),
            TranslationError::UnsupportedControl("disk"),
        );
        assert_view_error(
            |view| view.resources.process_count = Some(1),
            TranslationError::UnsupportedControl("process_count"),
        );
        assert_view_error(
            |view| view.resources.cpus = None,
            TranslationError::MissingControl("cpus"),
        );
        assert_view_error(
            |view| view.resources.memory_bytes = None,
            TranslationError::MissingControl("memory"),
        );
        assert_view_error(
            |view| view.user = gascan_core::runtime::RuntimeUser::Root,
            TranslationError::UnsupportedUser,
        );
        assert_view_error(|view| view.init = false, TranslationError::InitRequired);
    }

    #[test]
    fn workspace_user_translation_never_emits_a_user_override() {
        let (_root, request) = request();
        let spec = AppleCommandBuilder::create(&request).expect("workspace user is supported");
        assert!(!spec.args.iter().any(|argument| argument == "--user"));
    }
}
