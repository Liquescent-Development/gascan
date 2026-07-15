use std::net::{IpAddr, Ipv4Addr};

use gascan_core::runtime::{CreateRequest, RuntimeNetwork, RuntimeUser};
use gascan_core::sandbox::SandboxId;
use thiserror::Error;

use crate::CommandSpec;

const MANAGED_BY: &str = "gascan";
const WORKSPACE_TARGET: &str = "/workspace";

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
        validate_request(request)?;

        let id = request.id().as_str();
        let mut args = vec![
            "run".to_owned(),
            "--name".to_owned(),
            id.to_owned(),
            "--label".to_owned(),
            format!("dev.gascan.managed-by={MANAGED_BY}"),
            "--label".to_owned(),
            format!("dev.gascan.sandbox-id={id}"),
        ];

        let mount = &request.bind_mounts()[0];
        args.extend([
            "--mount".to_owned(),
            format!(
                "type=bind,source={},target={WORKSPACE_TARGET}",
                mount.source
            ),
        ]);
        for volume in request.volumes() {
            args.extend([
                "--volume".to_owned(),
                format!("{}:{}", volume.name, volume.target),
            ]);
        }
        for (key, value) in request.environment() {
            args.extend(["--env".to_owned(), format!("{key}={value}")]);
        }

        let resources = request.resources();
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

        for port in request.ports() {
            args.extend([
                "--publish".to_owned(),
                format!(
                    "{}:{}:{}",
                    port.host_address, port.host_port, port.guest_port
                ),
            ]);
        }
        if request.network() == RuntimeNetwork::Offline {
            args.extend(["--network".to_owned(), "none".to_owned()]);
        }
        args.push(request.image().to_owned());

        Ok(CommandSpec::new("container", args))
    }
}

fn validate_request(request: &CreateRequest) -> Result<(), TranslationError> {
    validate_image(request.image())?;
    let id = request.id();
    let ownership = request.ownership();
    if ownership.managed_by != MANAGED_BY || ownership.sandbox_id != *id {
        return Err(TranslationError::InvalidOwnership);
    }
    let [mount] = request.bind_mounts() else {
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
    if request.volumes().len() != expected_volumes.len() {
        return Err(TranslationError::InvalidOwnedVolume);
    }
    for (volume, (expected_name, expected_target)) in request.volumes().iter().zip(expected_volumes)
    {
        if volume.name != expected_name
            || volume.target.as_str() != expected_target
            || !volume.writable
            || volume.ownership != *ownership
        {
            return Err(TranslationError::InvalidOwnedVolume);
        }
    }
    if request
        .ports()
        .iter()
        .any(|port| port.host_address != IpAddr::V4(Ipv4Addr::LOCALHOST))
    {
        return Err(TranslationError::NonLoopbackPort);
    }
    if request.resources().disk_bytes.is_some() {
        return Err(TranslationError::UnsupportedControl("disk"));
    }
    if request.resources().process_count.is_some() {
        return Err(TranslationError::UnsupportedControl("process_count"));
    }
    if request.user() != RuntimeUser::Workspace {
        return Err(TranslationError::UnsupportedUser);
    }
    if !request.init() {
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
