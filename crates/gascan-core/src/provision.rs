use crate::manifest::{Manifest, ShellPrompt};
use camino::{Utf8Component, Utf8Path, Utf8PathBuf};
use rustix::fd::OwnedFd;
use rustix::fs::{FileType, Mode, OFlags};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Read as _;
use thiserror::Error;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppliedState {
    tool_hash: Option<String>,
    setup_sha256: Option<String>,
    shell_hash: Option<String>,
}

impl AppliedState {
    pub const fn empty() -> Self {
        Self {
            tool_hash: None,
            setup_sha256: None,
            shell_hash: None,
        }
    }

    pub fn with_tool_hash(hash: impl Into<String>) -> Self {
        Self {
            tool_hash: Some(hash.into()),
            setup_sha256: None,
            shell_hash: None,
        }
    }

    pub fn with_setup_sha256(sha256: impl Into<String>) -> Self {
        Self {
            tool_hash: None,
            setup_sha256: Some(sha256.into()),
            shell_hash: None,
        }
    }

    pub fn with_hashes(
        tool_hash: Option<String>,
        setup_sha256: Option<String>,
        shell_hash: Option<String>,
    ) -> Self {
        Self {
            tool_hash,
            setup_sha256,
            shell_hash,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SetupScript {
    canonical_relative_path: Utf8PathBuf,
    sha256: String,
}

impl SetupScript {
    pub fn canonical_relative_path(&self) -> &Utf8Path {
        &self.canonical_relative_path
    }

    pub fn sha256(&self) -> &str {
        &self.sha256
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProvisionStep {
    WriteSafeMiseConfig,
    InstallTools,
    ConfigureShell,
    RunSetup,
    VerifyGascamp,
    HealthCheck,
    InitializeRuntimeHome,
}

impl ProvisionStep {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WriteSafeMiseConfig => "write_safe_mise_config",
            Self::InstallTools => "install_tools",
            Self::ConfigureShell => "configure_shell",
            Self::RunSetup => "run_setup",
            Self::VerifyGascamp => "verify_gascamp",
            Self::HealthCheck => "health_check",
            Self::InitializeRuntimeHome => "initialize_runtime_home",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProvisionPlan {
    steps: Vec<ProvisionStep>,
    desired_tools: BTreeMap<String, String>,
    desired_tool_hash: String,
    tools_changed: bool,
    desired_shell_hash: String,
    shell_prompt: ShellPrompt,
    shell_changed: bool,
    setup_script: Option<SetupScript>,
    setup_changed: bool,
}

impl ProvisionPlan {
    pub fn steps(&self) -> &[ProvisionStep] {
        &self.steps
    }

    pub fn desired_tool_hash(&self) -> &str {
        &self.desired_tool_hash
    }

    pub const fn tools_changed(&self) -> bool {
        self.tools_changed
    }

    pub fn desired_shell_hash(&self) -> &str {
        &self.desired_shell_hash
    }

    pub const fn shell_prompt(&self) -> ShellPrompt {
        self.shell_prompt
    }

    pub const fn shell_changed(&self) -> bool {
        self.shell_changed
    }

    pub const fn setup_script(&self) -> Option<&SetupScript> {
        self.setup_script.as_ref()
    }

    pub const fn setup_changed(&self) -> bool {
        self.setup_changed
    }

    pub fn safe_mise_toml(&self) -> Result<Option<String>, ProvisionError> {
        if !self.tools_changed {
            return Ok(None);
        }
        #[derive(Serialize)]
        struct SafeMiseConfig<'a> {
            tools: &'a BTreeMap<String, String>,
        }
        toml::to_string(&SafeMiseConfig {
            tools: &self.desired_tools,
        })
        .map(Some)
        .map_err(ProvisionError::SerializeConfig)
    }
}

pub struct ProvisioningPlanner;

impl ProvisioningPlanner {
    pub fn plan(
        manifest: &Manifest,
        applied: &AppliedState,
    ) -> Result<ProvisionPlan, ProvisionError> {
        let desired_tools = manifest.tools().clone();
        let serialized = serde_json::to_vec(&desired_tools).map_err(ProvisionError::HashTools)?;
        let desired_tool_hash = format!("sha256:{:x}", Sha256::digest(serialized));
        let tools_changed = match applied.tool_hash.as_deref() {
            Some(applied_hash) => applied_hash != desired_tool_hash,
            None => !desired_tools.is_empty(),
        };
        let shell_prompt = manifest.shell().prompt();
        let desired_shell_hash = desired_shell_hash(shell_prompt);
        let shell_changed = applied.shell_hash.as_deref() != Some(&desired_shell_hash);
        let mut steps = Vec::new();
        if tools_changed {
            steps.push(ProvisionStep::WriteSafeMiseConfig);
            steps.push(ProvisionStep::InstallTools);
        }
        if shell_changed {
            steps.push(ProvisionStep::ConfigureShell);
        }
        if manifest.setup().is_some() {
            steps.push(ProvisionStep::RunSetup);
        }
        steps.push(ProvisionStep::VerifyGascamp);
        steps.push(ProvisionStep::HealthCheck);
        Ok(ProvisionPlan {
            steps,
            desired_tools,
            desired_tool_hash,
            tools_changed,
            desired_shell_hash,
            shell_prompt,
            shell_changed,
            setup_script: None,
            setup_changed: false,
        })
    }

    pub fn plan_for_root(
        canonical_root: &Utf8Path,
        manifest: &Manifest,
        applied: &AppliedState,
    ) -> Result<ProvisionPlan, ProvisionError> {
        let mut plan = Self::plan(manifest, applied)?;
        plan.steps.retain(|step| *step != ProvisionStep::RunSetup);
        let setup_script = manifest
            .setup()
            .map(|path| resolve_setup(canonical_root, path))
            .transpose()?;
        let setup_changed = setup_script
            .as_ref()
            .is_some_and(|setup| applied.setup_sha256.as_deref() != Some(setup.sha256()));
        if setup_changed {
            let verification = plan
                .steps
                .iter()
                .position(|step| *step == ProvisionStep::VerifyGascamp)
                .unwrap_or(plan.steps.len());
            plan.steps.insert(verification, ProvisionStep::RunSetup);
        }
        plan.setup_script = setup_script;
        plan.setup_changed = setup_changed;
        Ok(plan)
    }
}

const MANAGED_SHELL_CONFIG_VERSION: &str = "gascan-managed-shell-v1";

fn desired_shell_hash(prompt: ShellPrompt) -> String {
    let mut hash = Sha256::new();
    hash.update(MANAGED_SHELL_CONFIG_VERSION.as_bytes());
    hash.update([0]);
    hash.update(match prompt {
        ShellPrompt::Standard => b"standard".as_slice(),
        ShellPrompt::Starship => b"starship".as_slice(),
        ShellPrompt::StarshipNerdFont => b"starship-nerd-font".as_slice(),
    });
    format!("sha256:{:x}", hash.finalize())
}

fn resolve_setup(
    canonical_root: &Utf8Path,
    relative_path: &Utf8Path,
) -> Result<SetupScript, ProvisionError> {
    let mut directory = open_root_no_follow(canonical_root)?;
    let mut components = relative_path.components().peekable();
    let mut canonical_relative_path = Utf8PathBuf::new();
    let mut file = None;
    while let Some(component) = components.next() {
        match component {
            Utf8Component::CurDir => continue,
            Utf8Component::Normal(component) => {
                canonical_relative_path.push(component);
                let final_component = components.peek().is_none();
                let flags = OFlags::RDONLY
                    | OFlags::NOFOLLOW
                    | OFlags::CLOEXEC
                    | OFlags::NONBLOCK
                    | if final_component {
                        OFlags::empty()
                    } else {
                        OFlags::DIRECTORY
                    };
                let opened = rustix::fs::openat(&directory, component, flags, Mode::empty())
                    .map_err(map_setup_open_error)?;
                if final_component {
                    file = Some(opened);
                } else {
                    directory = opened;
                }
            }
            Utf8Component::ParentDir | Utf8Component::RootDir | Utf8Component::Prefix(_) => {
                return Err(ProvisionError::SetupOutsideRoot);
            }
        }
    }
    let file = file.ok_or(ProvisionError::SetupNotRegular)?;
    let stat = rustix::fs::fstat(&file).map_err(|_| ProvisionError::SetupUnreadable)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
        return Err(ProvisionError::SetupNotRegular);
    }
    let mut file = std::fs::File::from(file);
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|_| ProvisionError::SetupUnreadable)?;
    Ok(SetupScript {
        canonical_relative_path,
        sha256: format!("sha256:{:x}", Sha256::digest(bytes)),
    })
}

fn open_root_no_follow(path: &Utf8Path) -> Result<OwnedFd, ProvisionError> {
    let mut components = path.components();
    if components.next() != Some(Utf8Component::RootDir) {
        return Err(ProvisionError::SetupOutsideRoot);
    }
    let mut directory = rustix::fs::open(
        "/",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(map_setup_open_error)?;
    for component in components {
        let Utf8Component::Normal(component) = component else {
            return Err(ProvisionError::SetupOutsideRoot);
        };
        directory = rustix::fs::openat(
            &directory,
            component,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(map_setup_open_error)?;
    }
    Ok(directory)
}

fn map_setup_open_error(error: rustix::io::Errno) -> ProvisionError {
    if matches!(error, rustix::io::Errno::LOOP | rustix::io::Errno::NOTDIR) {
        ProvisionError::SetupSymlink
    } else {
        ProvisionError::SetupUnreadable
    }
}

#[derive(Debug, Error)]
pub enum ProvisionError {
    #[error("failed to hash desired tools: {0}")]
    HashTools(serde_json::Error),
    #[error("failed to serialize safe mise configuration: {0}")]
    SerializeConfig(toml::ser::Error),
    #[error("setup script path escapes the workspace root")]
    SetupOutsideRoot,
    #[error("setup script path contains a symbolic link")]
    SetupSymlink,
    #[error("setup script is not a regular file")]
    SetupNotRegular,
    #[error("setup script is not readable")]
    SetupUnreadable,
}
