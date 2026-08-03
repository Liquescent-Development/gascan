use crate::client::{Client, ClientError};
use crate::configure::{
    ConfigureError, ConfigureIo, ConfigureOutcome, Forge, GitProtocol, OfferResult,
    SystemHostDiscovery, TerminalPrompter, configure_all, configure_forge_interactive,
    configure_git_interactive, offer_after_up,
};
use crate::guest::{
    ClientGuestRunner, Secret, SensitiveBytes, allowed_environment, attach_to_stdio,
    first_session_token,
};
use crate::presentation::{
    DoctorCheck, OperationKind, OperationProgress, OutputCapabilities, daemon_force_warning,
    daemon_lifecycle_json, daemon_status_json, render_daemon_lifecycle, render_daemon_status,
    render_doctor as render_human_doctor, render_error as render_human_error,
    render_list as render_human_list, render_status as render_human_status,
};
use crate::ssh_config::{
    IncludeChange, OfferAnswer, SshConfig, answer_first_use_offer, first_use_offer,
};
use clap::{CommandFactory as _, Parser, Subcommand, error::ErrorKind};
use gascan_proto::ssh_status::{SshState, classify as classify_ssh};
use gascan_proto::v1;
use std::ffi::{OsStr, OsString};
use std::io::{IsTerminal as _, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const EXIT_USAGE: i32 = 64;
const EXIT_DAEMON: i32 = 69;
const EXIT_RUNTIME: i32 = 70;
const EXIT_API: i32 = 76;
const MAX_CONFIGURE_TOKEN_BYTES: usize = 1024 * 1024;
const CONFIGURE_TOKEN_SCRATCH_BYTES: usize = 16 * 1024;

#[derive(Parser)]
#[command(name = "gascan", version, disable_help_subcommand = true)]
struct Arguments {
    #[arg(long, global = true)]
    sandbox: Option<String>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    #[command(hide = true)]
    DaemonAttest,
    Daemon {
        #[command(subcommand)]
        command: DaemonCommand,
    },
    Up {
        project_root: String,
        #[arg(long)]
        json: bool,
    },
    Apply {
        project_root: Option<String>,
        #[arg(long)]
        json: bool,
    },
    Shell {
        #[arg(last = true)]
        argv: Vec<String>,
    },
    Run {
        #[arg(last = true, required = true)]
        argv: Vec<String>,
    },
    Down {
        #[arg(long)]
        json: bool,
    },
    Destroy {
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        json: bool,
    },
    List {
        #[arg(long)]
        json: bool,
    },
    Status {
        #[arg(long)]
        json: bool,
    },
    Logs {
        #[arg(long)]
        follow: bool,
        #[arg(long)]
        since_millis: Option<i64>,
    },
    Doctor {
        #[arg(long)]
        json: bool,
    },
    Ssh {
        #[arg(last = true)]
        argv: Vec<OsString>,
    },
    SshConfig {
        #[command(subcommand)]
        command: SshConfigCommand,
    },
    Configure {
        #[command(subcommand)]
        command: Option<ConfigureCommand>,
    },
}

#[derive(Subcommand)]
enum ConfigureCommand {
    Git,
    Gh {
        #[arg(long)]
        hostname: Option<String>,
        #[arg(long)]
        token_stdin: bool,
        #[arg(long, value_enum, default_value_t = GitProtocol::Ssh)]
        git_protocol: GitProtocol,
    },
    Glab {
        #[arg(long)]
        hostname: Option<String>,
        #[arg(long)]
        token_stdin: bool,
        #[arg(long, value_enum, default_value_t = GitProtocol::Ssh)]
        git_protocol: GitProtocol,
    },
}

#[derive(Subcommand, Clone)]
enum DaemonCommand {
    Status {
        #[arg(long)]
        json: bool,
    },
    Start {
        #[arg(long)]
        json: bool,
    },
    Stop {
        #[arg(
            long,
            help = "Force shutdown if graceful shutdown times out; may interrupt active sandbox operations and attachments"
        )]
        force: bool,
        #[arg(long)]
        json: bool,
    },
    Restart {
        #[arg(
            long,
            help = "Force shutdown if graceful shutdown times out; may interrupt active sandbox operations and attachments"
        )]
        force: bool,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum SshConfigCommand {
    Install,
    Remove,
    Path,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SshInvocation {
    pub program: PathBuf,
    pub arguments: Vec<OsString>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParsedSshArguments {
    pub sandbox: Option<String>,
    pub remote: Vec<OsString>,
}

#[derive(Debug)]
pub enum UsageKind {
    NoSandbox,
    MultipleSandboxes,
    Other,
}

#[derive(Debug)]
pub enum CliError {
    Client(ClientError),
    Usage {
        kind: UsageKind,
        message: String,
    },
    Operation {
        code: String,
        message: String,
    },
    DaemonOperation {
        code: String,
        message: String,
        suggestion: Option<&'static str>,
    },
    Runtime(String),
    Io(std::io::Error),
}
impl CliError {
    pub const fn exit_code(&self) -> i32 {
        match self {
            Self::Usage { .. } => EXIT_USAGE,
            Self::Client(ClientError::Api(_)) => EXIT_API,
            Self::Client(ClientError::Rpc(_)) => EXIT_RUNTIME,
            Self::Client(_) => EXIT_DAEMON,
            Self::Operation { .. }
            | Self::DaemonOperation { .. }
            | Self::Runtime(_)
            | Self::Io(_) => EXIT_RUNTIME,
        }
    }

    pub fn stable_code(&self) -> Option<&str> {
        match self {
            Self::Client(error) => error.stable_code(),
            Self::Operation { code, .. } | Self::DaemonOperation { code, .. } => Some(code),
            Self::Usage { .. } | Self::Runtime(_) | Self::Io(_) => None,
        }
    }

    pub fn message(&self) -> String {
        let stable_code = self.stable_code();
        if stable_code == Some(gascan_proto::error_code::SANDBOX_NOT_FOUND) {
            return "sandbox not found".to_owned();
        }
        let message = match self {
            Self::Client(error) => error.cause().unwrap_or_else(|| {
                stable_code.map_or_else(|| error.to_string(), ToOwned::to_owned)
            }),
            Self::Usage { message, .. }
            | Self::Operation { message, .. }
            | Self::DaemonOperation { message, .. }
            | Self::Runtime(message) => message.clone(),
            Self::Io(error) => error.to_string(),
        };
        if message.trim().is_empty() {
            return stable_code.unwrap_or_default().to_owned();
        }
        if stable_code == Some("resource_conflict") {
            return format!("a managed runtime resource already exists: {message}");
        }
        message
    }

    pub fn suggestion(&self) -> Option<&'static str> {
        let contextual = match self {
            Self::DaemonOperation {
                suggestion: Some(suggestion),
                ..
            } => Some(*suggestion),
            Self::Usage {
                kind: UsageKind::NoSandbox,
                ..
            } => Some("gascan up <project-root>"),
            Self::Usage {
                kind: UsageKind::MultipleSandboxes,
                ..
            } => Some("run `gascan list`, then pass `--sandbox <sandbox-id>`"),
            Self::Client(_) | Self::Operation { .. } | Self::DaemonOperation { .. }
                if matches!(
                    self.stable_code(),
                    Some(gascan_proto::error_code::SANDBOX_NOT_FOUND)
                ) =>
            {
                Some("run `gascan list` and use the sandbox ID shown there")
            }
            Self::Client(_)
            | Self::Usage {
                kind: UsageKind::Other,
                ..
            }
            | Self::Operation { .. }
            | Self::DaemonOperation { .. }
            | Self::Runtime(_)
            | Self::Io(_) => None,
        };
        contextual.or_else(|| match self.stable_code() {
            Some("daemon_outdated") => Some("run `gascan daemon restart` to replace it"),
            Some("daemon_io") => {
                Some("run `gascan daemon status` after checking the local runtime directory")
            }
            Some("daemon_graceful_shutdown_timeout") => Some(
                "retry with `gascan daemon restart --force` if it is safe to interrupt active work",
            ),
            Some("daemon_invalid_state")
            | Some("daemon_readiness_failed")
            | Some("daemon_identity_changed")
            | Some("daemon_exit_timeout")
            | Some("daemon_lifecycle_busy")
            | Some("daemon_lifecycle_changed") => {
                Some("run `gascan daemon status` for the current daemon state")
            }
            _ => None,
        })
    }
}
impl std::fmt::Display for CliError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message())
    }
}
impl std::error::Error for CliError {}
impl From<ClientError> for CliError {
    fn from(value: ClientError) -> Self {
        Self::Client(value)
    }
}
impl From<tonic::Status> for CliError {
    fn from(value: tonic::Status) -> Self {
        Self::Client(ClientError::Rpc(Box::new(value)))
    }
}
impl From<std::io::Error> for CliError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

pub fn render_error(error: &CliError) -> String {
    render_human_error(
        &error.message(),
        error.suggestion(),
        OutputCapabilities::for_stderr(),
    )
}

/// Resolve a project root to the absolute path the daemon requires.
///
/// A relative path names a directory relative to *this* process. The daemon
/// runs with a different working directory, so resolving there would mount the
/// wrong directory; resolution has to happen on this side. The daemon still
/// rejects a relative root, and that check stays: it is the boundary, not a
/// fallback for this function.
fn resolve_project_root(project_root: &str) -> Result<String, CliError> {
    if project_root.is_empty() {
        return Err(CliError::Usage {
            kind: UsageKind::Other,
            message: "project root must not be empty".to_owned(),
        });
    }
    let resolved = std::fs::canonicalize(project_root).map_err(|error| CliError::Usage {
        kind: UsageKind::Other,
        message: format!("cannot use `{project_root}` as a project root: {error}"),
    })?;
    let metadata = resolved.metadata().map_err(|error| CliError::Usage {
        kind: UsageKind::Other,
        message: format!("cannot use `{project_root}` as a project root: {error}"),
    })?;
    if !metadata.is_dir() {
        return Err(CliError::Usage {
            kind: UsageKind::Other,
            message: format!("cannot use `{project_root}` as a project root: not a directory"),
        });
    }
    resolved
        .to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| CliError::Usage {
            kind: UsageKind::Other,
            message: format!("project root `{project_root}` is not valid UTF-8"),
        })
}

fn doctor_request(current_directory: std::io::Result<PathBuf>) -> v1::DoctorRequest {
    let workspace_result = match current_directory {
        Ok(path) if path.is_absolute() => match path.to_str() {
            Some(path) => v1::doctor_request::WorkspaceResult::Workspace(path.to_owned()),
            None => v1::doctor_request::WorkspaceResult::WorkspaceError(
                "caller directory is not valid UTF-8".to_owned(),
            ),
        },
        Ok(path) => v1::doctor_request::WorkspaceResult::WorkspaceError(format!(
            "caller directory is not absolute: {}",
            path.display()
        )),
        Err(error) => v1::doctor_request::WorkspaceResult::WorkspaceError(format!(
            "could not resolve caller directory: {error}"
        )),
    };
    v1::DoctorRequest {
        workspace_result: Some(workspace_result),
    }
}

pub async fn execute() -> Result<i32, CliError> {
    let arguments = match Arguments::try_parse() {
        Ok(arguments) => arguments,
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::DisplayVersion | ErrorKind::DisplayHelp
            ) =>
        {
            print!("{error}");
            return Ok(0);
        }
        Err(error) => {
            return Err(CliError::Usage {
                kind: UsageKind::Other,
                message: error.to_string(),
            });
        }
    };
    if matches!(arguments.command, Command::DaemonAttest) {
        let attestation = Client::daemon_attestation().await?;
        println!(
            "{}",
            serde_json::json!({
                "instance_token": attestation.daemon_instance_token,
                "pid": attestation.daemon_pid,
                "executable": attestation.daemon_executable,
                "start_identity": attestation.daemon_start_identity,
            })
        );
        return Ok(0);
    }
    if let Command::Daemon { command } = &arguments.command {
        return execute_daemon(command.clone()).await;
    }
    if let Command::SshConfig { command } = arguments.command {
        return execute_ssh_config(command);
    }
    let configure_io = if let Command::Configure { command } = &arguments.command {
        let io = TerminalPrompter::new().map_err(configure_cli_error)?;
        preflight_configure(command, io.stdin_is_terminal(), io.stderr_is_terminal())?;
        Some(io)
    } else {
        None
    };
    let doctor_request = matches!(&arguments.command, Command::Doctor { .. })
        .then(|| doctor_request(std::env::current_dir()));
    let connected = connect_with_recovery_progress(command_uses_json(&arguments.command)).await?;
    let mut client = connected.daemon.connection;
    match arguments.command {
        Command::DaemonAttest => Ok(0),
        Command::Daemon { .. } => Ok(0),
        Command::Configure { command } => {
            let io = configure_io.ok_or_else(|| {
                CliError::Runtime("configuration terminal was not initialized".to_owned())
            })?;
            execute_configure(&mut client, arguments.sandbox, command, io).await
        }
        Command::Up { project_root, json } => {
            let project_root = resolve_project_root(&project_root)?;
            let developer_offer_ci = continuous_integration();
            let developer_stdin_is_terminal = std::io::stdin().is_terminal();
            let developer_stderr_is_terminal = std::io::stderr().is_terminal();
            match client
                .api
                .up(v1::UpRequest {
                    project_root: project_root.clone(),
                })
                .await
            {
                Ok(response) => {
                    let result =
                        operation(response.into_inner(), json, OperationKind::Up, None).await;
                    let result = preserve_up_result_with_optional_offer(
                        result,
                        json,
                        &mut std::io::stderr(),
                        try_offer_ssh_config_include,
                    );
                    let mut warning = std::io::stderr();
                    preserve_up_result_with_developer_offer_gate(
                        result,
                        DeveloperOfferEligibility {
                            json,
                            continuous_integration: developer_offer_ci,
                            stdin_is_terminal: developer_stdin_is_terminal,
                            stderr_is_terminal: developer_stderr_is_terminal,
                        },
                        &mut warning,
                        || selector_for_project_root(&project_root),
                        |selector| async {
                            let mut io = TerminalPrompter::new()?;
                            offer_after_up(&mut client, selector, &mut io).await
                        },
                    )
                    .await
                }
                Err(status) => pre_stream_operation_failure(status.into(), json),
            }
        }
        Command::Apply { project_root, json } => {
            let root = match project_root {
                Some(root) => resolve_project_root(&root)?,
                None => resolve_project_root(".")?,
            };
            match client
                .api
                .apply(v1::ApplyRequest { project_root: root })
                .await
            {
                Ok(response) => {
                    operation(response.into_inner(), json, OperationKind::Apply, None).await
                }
                Err(status) => pre_stream_operation_failure(status.into(), json),
            }
        }
        Command::Down { json } => {
            let selector = selector(&mut client, arguments.sandbox).await?;
            let sandbox_id = Some(selector.sandbox_id.clone());
            operation(
                client
                    .api
                    .down(v1::DownRequest {
                        sandbox: Some(selector),
                    })
                    .await?
                    .into_inner(),
                json,
                OperationKind::Down,
                sandbox_id,
            )
            .await
        }
        Command::Destroy { yes, json } => {
            if !yes {
                confirm_destroy()?;
            }
            let selector = selector(&mut client, arguments.sandbox).await?;
            let sandbox_id = Some(selector.sandbox_id.clone());
            operation(
                client
                    .api
                    .destroy(v1::DestroyRequest {
                        sandbox: Some(selector),
                    })
                    .await?
                    .into_inner(),
                json,
                OperationKind::Destroy,
                sandbox_id,
            )
            .await
        }
        Command::Status { json } => {
            let selector = selector(&mut client, arguments.sandbox).await?;
            let status = client
                .api
                .status(v1::StatusRequest {
                    sandbox: Some(selector),
                })
                .await?
                .into_inner()
                .sandbox
                .ok_or_else(|| CliError::Runtime("daemon returned no sandbox status".to_owned()))?;
            render_status(&status, json)?;
            Ok(0)
        }
        Command::List { json } => {
            let list = client.api.list(v1::ListRequest {}).await?.into_inner();
            render_list(&list.sandboxes, json)?;
            Ok(0)
        }
        Command::Doctor { json } => {
            let request = doctor_request.ok_or_else(|| {
                CliError::Runtime("Doctor request was not prepared before connecting".to_owned())
            })?;
            let doctor = client.api.doctor(request).await?.into_inner();
            let checks = doctor
                .capabilities
                .iter()
                .map(|capability| {
                    let detail: serde_json::Value = serde_json::from_str(&capability.detail)
                        .unwrap_or_else(
                            |_| serde_json::json!({"detail": capability.detail, "remedy": ""}),
                        );
                    DoctorCheck {
                        id: capability.name.clone(),
                        status: detail
                            .get("status")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or(if capability.available { "pass" } else { "fail" })
                            .to_owned(),
                        detail: detail
                            .get("detail")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("")
                            .to_owned(),
                        remedy: detail
                            .get("remedy")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("")
                            .to_owned(),
                    }
                })
                .collect::<Vec<_>>();
            if json {
                let checks = checks
                    .iter()
                    .map(|check| {
                        serde_json::json!({
                            "id": check.id,
                            "status": check.status,
                            "detail": check.detail,
                            "remedy": check.remedy,
                        })
                    })
                    .collect::<Vec<_>>();
                println!("{}", serde_json::json!({"checks": checks}));
            } else {
                print!(
                    "{}",
                    render_human_doctor(&checks, OutputCapabilities::for_stdout())
                );
            }
            Ok(if doctor.findings.is_empty() {
                0
            } else {
                EXIT_RUNTIME
            })
        }
        Command::Run { argv } => run(&mut client, arguments.sandbox, argv, false).await,
        Command::Shell { argv } => run(&mut client, arguments.sandbox, argv, true).await,
        Command::Logs {
            follow,
            since_millis,
        } => logs(&mut client, arguments.sandbox, follow, since_millis).await,
        Command::Ssh { argv } => ssh(&mut client, arguments.sandbox, argv).await,
        Command::SshConfig { .. } => Ok(0),
    }
}

fn preflight_configure(
    command: &Option<ConfigureCommand>,
    stdin_is_terminal: bool,
    stderr_is_terminal: bool,
) -> Result<(), CliError> {
    let token_stdin_protocol = match command {
        Some(
            ConfigureCommand::Gh {
                token_stdin: true,
                git_protocol,
                ..
            }
            | ConfigureCommand::Glab {
                token_stdin: true,
                git_protocol,
                ..
            },
        ) => Some(*git_protocol),
        _ => None,
    };
    if let Some(protocol) = token_stdin_protocol {
        if stdin_is_terminal {
            return Err(CliError::Usage {
                kind: UsageKind::Other,
                message:
                    "--token-stdin requires piped stdin; omit --token-stdin for hidden token entry"
                        .to_owned(),
            });
        }
        if protocol == GitProtocol::Ssh {
            return Err(CliError::Usage {
                kind: UsageKind::Other,
                message: "--token-stdin cannot perform SSH first-use verification; rerun interactively without --token-stdin, or pass --git-protocol https".to_owned(),
            });
        }
        return Ok(());
    }
    if !stdin_is_terminal || !stderr_is_terminal {
        return Err(CliError::Usage {
            kind: UsageKind::Other,
            message: "interactive configuration requires an interactive terminal".to_owned(),
        });
    }
    Ok(())
}

async fn execute_configure(
    client: &mut Client,
    explicit_sandbox: Option<String>,
    command: Option<ConfigureCommand>,
    mut io: TerminalPrompter,
) -> Result<i32, CliError> {
    let selector = selector(client, explicit_sandbox).await?;
    let status = client
        .api
        .status(v1::StatusRequest {
            sandbox: Some(selector.clone()),
        })
        .await?
        .into_inner()
        .sandbox
        .ok_or_else(|| CliError::Runtime("daemon returned no sandbox status".to_owned()))?;
    let (piped_token, mut runner) =
        prepare_configure_dispatch(&selector, &status, &command, read_token_stdin, move || {
            ClientGuestRunner::new(client)
        })?;
    let discovery = SystemHostDiscovery::new();
    let outcome = match command {
        None => configure_all(&mut runner, selector, &discovery, &mut io).await,
        Some(ConfigureCommand::Git) => {
            configure_git_interactive(&mut runner, selector, &discovery, &mut io).await
        }
        Some(ConfigureCommand::Gh {
            hostname,
            git_protocol,
            ..
        }) => {
            configure_forge_interactive(
                &mut runner,
                selector,
                Forge::GitHub,
                hostname.unwrap_or_else(|| "github.com".to_owned()),
                git_protocol,
                piped_token,
                &mut io,
            )
            .await
        }
        Some(ConfigureCommand::Glab {
            hostname,
            git_protocol,
            ..
        }) => {
            configure_forge_interactive(
                &mut runner,
                selector,
                Forge::GitLab,
                hostname.unwrap_or_else(|| "gitlab.com".to_owned()),
                git_protocol,
                piped_token,
                &mut io,
            )
            .await
        }
    }
    .map_err(configure_cli_error)?;
    Ok(match outcome {
        ConfigureOutcome::Completed | ConfigureOutcome::Cancelled => 0,
        ConfigureOutcome::Partial => EXIT_RUNTIME,
    })
}

fn configure_cli_error(error: ConfigureError) -> CliError {
    match error {
        ConfigureError::Io(error) => CliError::Io(error),
        error => CliError::Runtime(error.to_string()),
    }
}

fn read_token_stdin() -> Result<Secret, CliError> {
    read_token_stdin_from(&mut std::io::stdin().lock())
}

fn read_token_stdin_from(reader: &mut impl std::io::Read) -> Result<Secret, CliError> {
    let mut token = SensitiveBytes::zeroed(MAX_CONFIGURE_TOKEN_BYTES);
    let mut scratch = SensitiveBytes::zeroed(CONFIGURE_TOKEN_SCRATCH_BYTES);
    loop {
        let count = match reader.read(scratch.storage_mut()) {
            Ok(count) => count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(CliError::Io(error)),
        };
        if count == 0 {
            break;
        }
        let exceeded = token.append_bounded(&scratch.storage()[..count]);
        scratch.clear_storage();
        if exceeded {
            return Err(CliError::Usage {
                kind: UsageKind::Other,
                message: format!("piped token exceeds the {MAX_CONFIGURE_TOKEN_BYTES}-byte limit"),
            });
        }
    }
    if token.is_empty() {
        return Err(CliError::Usage {
            kind: UsageKind::Other,
            message: "piped token must not be empty".to_owned(),
        });
    }
    Ok(Secret::from_sensitive(token))
}

fn prepare_configure_dispatch<R>(
    selector: &v1::SandboxSelector,
    status: &v1::SandboxStatus,
    command: &Option<ConfigureCommand>,
    read_token: impl FnOnce() -> Result<Secret, CliError>,
    make_runner: impl FnOnce() -> R,
) -> Result<(Option<Secret>, R), CliError> {
    require_running_sandbox(&selector.sandbox_id, status)?;
    let piped_token = match command {
        Some(
            ConfigureCommand::Gh {
                token_stdin: true, ..
            }
            | ConfigureCommand::Glab {
                token_stdin: true, ..
            },
        ) => Some(read_token()?),
        None
        | Some(ConfigureCommand::Git)
        | Some(ConfigureCommand::Gh {
            token_stdin: false, ..
        })
        | Some(ConfigureCommand::Glab {
            token_stdin: false, ..
        }) => None,
    };
    Ok((piped_token, make_runner()))
}

fn require_running_sandbox(
    expected_sandbox_id: &str,
    status: &v1::SandboxStatus,
) -> Result<(), CliError> {
    if status.sandbox_id != expected_sandbox_id {
        return Err(CliError::Runtime(format!(
            "daemon returned status for a different sandbox than selected `{expected_sandbox_id}`; retry the command, then run `gascan daemon restart` if the mismatch persists"
        )));
    }
    if status.actual_state == v1::ActualState::Running as i32 {
        return Ok(());
    }
    Err(CliError::Usage {
        kind: UsageKind::Other,
        message: format!(
            "sandbox `{}` is not running; run `gascan up <project-root>`",
            expected_sandbox_id
        ),
    })
}

async fn execute_daemon(command: DaemonCommand) -> Result<i32, CliError> {
    let json = daemon_command_uses_json(&command);
    match command {
        DaemonCommand::Status { .. } => {
            let status = crate::daemon::inspect().await.map_err(supervisor_error)?;
            let now_millis = daemon_now_millis()?;
            if json {
                println!("{}", daemon_status_json(&status, now_millis));
            } else {
                print!(
                    "{}",
                    render_daemon_status(&status, now_millis, OutputCapabilities::for_stdout())
                );
            }
        }
        DaemonCommand::Start { .. } => {
            let outcome = crate::daemon::start().await.map_err(supervisor_error)?;
            let now_millis = daemon_now_millis()?;
            render_daemon_outcome(&outcome, now_millis, json);
        }
        DaemonCommand::Stop { force, .. } => {
            if force && !json {
                eprint!("{}", daemon_force_warning());
            }
            let outcome = crate::daemon::stop(force)
                .await
                .map_err(|error| supervisor_error_for_action(error, DaemonErrorContext::Stop))?;
            let now_millis = daemon_now_millis()?;
            render_daemon_outcome(&outcome, now_millis, json);
        }
        DaemonCommand::Restart { force, .. } => {
            if force && !json {
                eprint!("{}", daemon_force_warning());
            }
            let outcome = crate::daemon::restart(force)
                .await
                .map_err(|error| supervisor_error_for_action(error, DaemonErrorContext::Restart))?;
            let now_millis = daemon_now_millis()?;
            render_daemon_outcome(&outcome, now_millis, json);
        }
    }
    Ok(0)
}

fn render_daemon_outcome(outcome: &crate::daemon::LifecycleOutcome, now_millis: i64, json: bool) {
    if json {
        println!("{}", daemon_lifecycle_json(outcome, now_millis));
    } else {
        print!(
            "{}",
            render_daemon_lifecycle(outcome, now_millis, OutputCapabilities::for_stdout())
        );
    }
}

fn daemon_now_millis() -> Result<i64, CliError> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| {
            CliError::Runtime(format!("system clock is before Unix epoch: {error}"))
        })?;
    i64::try_from(elapsed.as_millis())
        .map_err(|_| CliError::Runtime("system clock exceeds supported timestamp range".to_owned()))
}

fn daemon_command_uses_json(command: &DaemonCommand) -> bool {
    match command {
        DaemonCommand::Status { json }
        | DaemonCommand::Start { json }
        | DaemonCommand::Stop { json, .. }
        | DaemonCommand::Restart { json, .. } => *json,
    }
}

fn command_uses_json(command: &Command) -> bool {
    match command {
        Command::Up { json, .. }
        | Command::Apply { json, .. }
        | Command::Down { json }
        | Command::Destroy { json, .. }
        | Command::List { json }
        | Command::Status { json }
        | Command::Doctor { json } => *json,
        Command::Daemon { command } => daemon_command_uses_json(command),
        Command::DaemonAttest
        | Command::Shell { .. }
        | Command::Run { .. }
        | Command::Logs { .. }
        | Command::Ssh { .. }
        | Command::SshConfig { .. }
        | Command::Configure { .. } => false,
    }
}

#[derive(Clone, Copy)]
enum RecoveryOutputStream {
    #[allow(
        dead_code,
        reason = "the JSON regression sink records accidental stdout writes even though recovery uses stderr only"
    )]
    Stdout,
    Stderr,
}

trait RecoveryOutputSink {
    fn write(&mut self, stream: RecoveryOutputStream, line: &str);
}

struct TerminalRecoveryOutput;

impl RecoveryOutputSink for TerminalRecoveryOutput {
    fn write(&mut self, stream: RecoveryOutputStream, line: &str) {
        match stream {
            RecoveryOutputStream::Stdout => {
                let _ = writeln!(std::io::stdout(), "{line}");
            }
            RecoveryOutputStream::Stderr => {
                let _ = writeln!(std::io::stderr(), "{line}");
            }
        }
    }
}

struct CliRecoveryObserver<Output> {
    mode: CliRecoveryProgressMode,
    output: Output,
}

enum CliRecoveryProgressMode {
    Human {
        capabilities: OutputCapabilities,
        progress: Option<OperationProgress>,
    },
    Suppressed,
}

impl<Output: RecoveryOutputSink> CliRecoveryObserver<Output> {
    fn new(json: bool, capabilities: OutputCapabilities, output: Output) -> Self {
        Self {
            mode: if json {
                CliRecoveryProgressMode::Suppressed
            } else {
                CliRecoveryProgressMode::Human {
                    capabilities,
                    progress: None,
                }
            },
            output,
        }
    }

    fn finish(&mut self) {
        let CliRecoveryProgressMode::Human { progress, .. } = &mut self.mode else {
            return;
        };
        if let Some(progress) = progress.take() {
            if let Some(line) = progress.finish_success() {
                self.output.write(RecoveryOutputStream::Stderr, &line);
            }
        }
    }

    #[cfg(test)]
    fn is_presenting(&self) -> bool {
        matches!(
            self.mode,
            CliRecoveryProgressMode::Human {
                progress: Some(_),
                ..
            }
        )
    }
}

#[tonic::async_trait]
impl<Output: RecoveryOutputSink + Send> crate::daemon::DaemonLifecycleObserver
    for CliRecoveryObserver<Output>
{
    async fn transition_started(&mut self, transition: crate::daemon::DaemonTransition) {
        if transition != crate::daemon::DaemonTransition::Recovered {
            return;
        }
        let CliRecoveryProgressMode::Human {
            capabilities,
            progress,
        } = &mut self.mode
        else {
            return;
        };
        let (next, initial) =
            OperationProgress::new(OperationKind::DaemonRecovery, None, *capabilities);
        if let Some(line) = initial {
            self.output.write(RecoveryOutputStream::Stderr, &line);
        }
        *progress = Some(next);
    }
}

async fn connect_with_recovery_progress(
    json: bool,
) -> Result<crate::daemon::ConnectionOutcome<Client>, CliError> {
    let mut observer = CliRecoveryObserver::new(
        json,
        OutputCapabilities::for_stderr(),
        TerminalRecoveryOutput,
    );
    let connected = crate::daemon::connect_current_or_recover_observing(&mut observer)
        .await
        .map_err(supervisor_error)?;
    observer.finish();
    Ok(connected)
}

fn supervisor_error(error: crate::daemon::SupervisorError) -> CliError {
    supervisor_error_for_action(error, DaemonErrorContext::Automatic)
}

#[derive(Clone, Copy)]
enum DaemonErrorContext {
    Automatic,
    Stop,
    Restart,
}

impl DaemonErrorContext {
    const fn graceful_timeout_suggestion(self) -> &'static str {
        match self {
            Self::Automatic => {
                "run `gascan daemon restart --force` if it is safe to interrupt active work"
            }
            Self::Stop => {
                "retry with `gascan daemon stop --force` if it is safe to interrupt active work"
            }
            Self::Restart => {
                "retry with `gascan daemon restart --force` if it is safe to interrupt active work"
            }
        }
    }
}

fn supervisor_error_for_action(
    error: crate::daemon::SupervisorError,
    context: DaemonErrorContext,
) -> CliError {
    if let crate::daemon::SupervisorError::Client(error) = error {
        return CliError::Client(error);
    }
    if let crate::daemon::SupervisorError::ControllerStartup { code, message } = error {
        return CliError::DaemonOperation {
            message: format!("{code}: {message}"),
            code,
            suggestion: None,
        };
    }
    let suggestion = matches!(
        error,
        crate::daemon::SupervisorError::GracefulTimeout { .. }
    )
    .then(|| context.graceful_timeout_suggestion());
    let code = match &error {
        crate::daemon::SupervisorError::Client(_) => unreachable!("client errors returned above"),
        crate::daemon::SupervisorError::Io(_) => "daemon_io",
        crate::daemon::SupervisorError::Outdated { .. } => "daemon_outdated",
        crate::daemon::SupervisorError::InvalidState { .. } => "daemon_invalid_state",
        crate::daemon::SupervisorError::Readiness { .. } => "daemon_readiness_failed",
        crate::daemon::SupervisorError::ControllerStartup { .. } => {
            unreachable!("controller startup errors returned above")
        }
        crate::daemon::SupervisorError::GracefulTimeout { .. } => {
            "daemon_graceful_shutdown_timeout"
        }
        crate::daemon::SupervisorError::IdentityChanged { .. } => "daemon_identity_changed",
        crate::daemon::SupervisorError::ExitTimeout { .. } => "daemon_exit_timeout",
        crate::daemon::SupervisorError::TombstoneBusy { .. } => "daemon_lifecycle_busy",
        crate::daemon::SupervisorError::TombstoneChanged { .. } => "daemon_lifecycle_changed",
    };
    CliError::DaemonOperation {
        code: code.to_owned(),
        message: error.to_string(),
        suggestion,
    }
}

async fn ssh(
    client: &mut Client,
    explicit: Option<String>,
    remote: Vec<OsString>,
) -> Result<i32, CliError> {
    let selector = selector(client, explicit).await?;
    let status = client
        .api
        .status(v1::StatusRequest {
            sandbox: Some(selector),
        })
        .await?
        .into_inner()
        .sandbox
        .ok_or_else(|| CliError::Runtime("daemon returned no sandbox status".to_owned()))?;
    let config = SshConfig::for_user().map_err(ssh_config_error)?;
    let invocation = ssh_invocation(&status, config.managed_config_path(), remote)?;
    wait_for_ssh(
        &invocation.program,
        &invocation.arguments,
        std::iter::empty::<(OsString, OsString)>(),
    )
    .map_err(|_| CliError::Operation {
        code: gascan_proto::error_code::SSH_CLIENT_UNAVAILABLE.to_owned(),
        message: "the system OpenSSH client could not be started".to_owned(),
    })
}

fn execute_ssh_config(command: SshConfigCommand) -> Result<i32, CliError> {
    let config = SshConfig::for_user().map_err(ssh_config_error)?;
    match command {
        SshConfigCommand::Install => match config.install().map_err(ssh_config_error)? {
            IncludeChange::Changed => println!("Installed the Gas Can SSH include."),
            IncludeChange::Unchanged => println!("The Gas Can SSH include is already installed."),
        },
        SshConfigCommand::Remove => match config.remove().map_err(ssh_config_error)? {
            IncludeChange::Changed => println!("Removed the Gas Can SSH include."),
            IncludeChange::Unchanged => println!("The Gas Can SSH include is not installed."),
        },
        SshConfigCommand::Path => println!("{}", config.managed_config_path().display()),
    }
    Ok(0)
}

fn ssh_config_error(error: crate::ssh_config::SshConfigError) -> CliError {
    let code = error.stable_code().to_owned();
    let context = if code == gascan_proto::error_code::SSH_CONFIG_UNSAFE {
        "SSH configuration is unsafe"
    } else {
        "SSH configuration could not be updated"
    };
    CliError::Operation {
        code,
        message: format!("{context}: {error}"),
    }
}

fn preserve_up_result_with_optional_offer<F, W>(
    result: Result<i32, CliError>,
    json: bool,
    warning: &mut W,
    offer: F,
) -> Result<i32, CliError>
where
    F: FnOnce() -> Result<(), CliError>,
    W: Write,
{
    result.inspect(|code| {
        if *code == 0 && !json && offer().is_err() {
            let _ = writeln!(
                warning,
                "Warning: automatic SSH config setup failed; run `gascan ssh-config install` to try again."
            );
        }
    })
}

async fn preserve_up_result_with_developer_offer<F, Future, W>(
    result: Result<i32, CliError>,
    json: bool,
    continuous_integration: bool,
    warning: &mut W,
    offer: F,
) -> Result<i32, CliError>
where
    F: FnOnce() -> Future,
    Future: std::future::Future<Output = Result<OfferResult, ConfigureError>>,
    W: Write,
{
    if !matches!(result, Ok(0)) || json || continuous_integration {
        return result;
    }
    if matches!(offer().await, Err(_) | Ok(OfferResult::Pending)) {
        let _ = writeln!(
            warning,
            "Warning: developer setup was not completed; run `gascan configure` to try again."
        );
    }
    result
}

#[derive(Clone, Copy)]
struct DeveloperOfferEligibility {
    json: bool,
    continuous_integration: bool,
    stdin_is_terminal: bool,
    stderr_is_terminal: bool,
}

async fn preserve_up_result_with_developer_offer_gate<Prepare, Offer, Future, W>(
    result: Result<i32, CliError>,
    eligibility: DeveloperOfferEligibility,
    warning: &mut W,
    prepare_selector: Prepare,
    offer: Offer,
) -> Result<i32, CliError>
where
    Prepare: FnOnce() -> Result<v1::SandboxSelector, ConfigureError>,
    Offer: FnOnce(v1::SandboxSelector) -> Future,
    Future: std::future::Future<Output = Result<OfferResult, ConfigureError>>,
    W: Write,
{
    if !matches!(result, Ok(0))
        || eligibility.json
        || eligibility.continuous_integration
        || !eligibility.stdin_is_terminal
        || !eligibility.stderr_is_terminal
    {
        return result;
    }
    let selector = prepare_selector();
    preserve_up_result_with_developer_offer(result, false, false, warning, || async move {
        offer(selector?).await
    })
    .await
}

fn selector_for_project_root(project_root: &str) -> Result<v1::SandboxSelector, ConfigureError> {
    let manifest = gascan_core::manifest::Manifest::load(project_root.as_ref()).map_err(|_| {
        ConfigureError::HostCommand {
            category: "developer onboarding selector",
            message: "the project manifest could not be loaded for developer onboarding".to_owned(),
        }
    })?;
    let name = manifest
        .name()
        .map(ToOwned::to_owned)
        .or_else(|| {
            Path::new(project_root)
                .file_name()
                .and_then(OsStr::to_str)
                .map(ToOwned::to_owned)
        })
        .ok_or_else(|| ConfigureError::HostCommand {
            category: "developer onboarding selector",
            message: "the successful project sandbox name could not be resolved".to_owned(),
        })?;
    let spec = gascan_core::sandbox::SandboxSpec::from_root(&name, project_root.as_ref(), manifest)
        .map_err(|_| ConfigureError::HostCommand {
            category: "developer onboarding selector",
            message: "the successful project sandbox could not be resolved".to_owned(),
        })?;
    Ok(v1::SandboxSelector {
        sandbox_id: spec.id().as_str().to_owned(),
    })
}

fn try_offer_ssh_config_include() -> Result<(), CliError> {
    if !std::io::stdin().is_terminal()
        || !std::io::stderr().is_terminal()
        || continuous_integration()
    {
        return Ok(());
    }
    let config = SshConfig::for_user().map_err(ssh_config_error)?;
    if !first_use_offer(&config, true, true).map_err(ssh_config_error)? {
        return Ok(());
    }
    eprint!("Add Gas Can's generated SSH hosts to ~/.ssh/config? [Y/n] ");
    std::io::stderr().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    if answer_first_use_offer(&config, &answer).map_err(ssh_config_error)? == OfferAnswer::Declined
    {
        eprintln!("Run `gascan ssh-config install` to add it later.");
    }
    Ok(())
}

fn continuous_integration() -> bool {
    continuous_integration_from(|name| std::env::var_os(name))
}

fn continuous_integration_from(mut environment: impl FnMut(&str) -> Option<OsString>) -> bool {
    ["CI", "GITHUB_ACTIONS", "BUILD_BUILDID"]
        .into_iter()
        .any(|name| environment(name).is_some_and(|value| !value.is_empty()))
}

pub fn ssh_invocation<I>(
    status: &v1::SandboxStatus,
    managed_config: &Path,
    remote: I,
) -> Result<SshInvocation, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    match classify_ssh(status) {
        SshState::Disabled => {
            return Err(CliError::Operation {
                code: gascan_proto::error_code::SSH_DISABLED.to_owned(),
                message: "SSH requires a networked sandbox with SSH enabled".to_owned(),
            });
        }
        SshState::Ready => {}
        SshState::Starting | SshState::Unavailable => {
            return Err(CliError::Operation {
                code: gascan_proto::error_code::SSH_NOT_READY.to_owned(),
                message: "SSH is not ready; run `gascan up <project-root>`".to_owned(),
            });
        }
        SshState::Unhealthy => {
            return Err(CliError::Operation {
                code: gascan_proto::error_code::SSH_NOT_READY.to_owned(),
                message: "SSH status is incomplete or unsafe; run `gascan up` again".to_owned(),
            });
        }
    }
    if !managed_config.is_absolute() {
        return Err(CliError::Operation {
            code: gascan_proto::error_code::SSH_NOT_READY.to_owned(),
            message: "managed SSH configuration path is unsafe".to_owned(),
        });
    }
    let expected_alias = format!("gascan-{}", status.sandbox_id);
    let mut arguments = vec![
        OsString::from("-F"),
        managed_config.as_os_str().to_owned(),
        OsString::from(expected_alias),
    ];
    arguments.extend(remote);
    Ok(SshInvocation {
        program: PathBuf::from("/usr/bin/ssh"),
        arguments,
    })
}

pub fn wait_for_ssh<I, K, V>(
    program: &Path,
    arguments: &[OsString],
    environment: I,
) -> Result<i32, std::io::Error>
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    let status = std::process::Command::new(program)
        .args(arguments)
        .envs(environment)
        .status()?;
    if let Some(code) = status.code() {
        return Ok(code);
    }
    use std::os::unix::process::ExitStatusExt as _;
    let Some(signal) = status.signal() else {
        return Ok(EXIT_RUNTIME);
    };
    signal_hook::low_level::emulate_default_handler(signal)?;
    Ok(EXIT_RUNTIME)
}

pub fn ssh_arguments_from<I, T>(arguments: I) -> Result<ParsedSshArguments, clap::Error>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let parsed = Arguments::try_parse_from(arguments)?;
    match parsed.command {
        Command::Ssh { argv } => Ok(ParsedSshArguments {
            sandbox: parsed.sandbox,
            remote: argv,
        }),
        _ => {
            Err(Arguments::command()
                .error(ErrorKind::InvalidSubcommand, "expected the ssh command"))
        }
    }
}

async fn selector(
    client: &mut Client,
    explicit: Option<String>,
) -> Result<v1::SandboxSelector, CliError> {
    if let Some(sandbox_id) = explicit {
        return Ok(v1::SandboxSelector { sandbox_id });
    }
    let sandboxes = client
        .api
        .list(v1::ListRequest {})
        .await?
        .into_inner()
        .sandboxes
        .into_iter()
        .filter(|sandbox| sandbox.actual_state != v1::ActualState::Absent as i32)
        .collect::<Vec<_>>();
    selector_from_sandboxes(sandboxes)
}

fn selector_from_sandboxes(
    sandboxes: Vec<v1::SandboxStatus>,
) -> Result<v1::SandboxSelector, CliError> {
    match sandboxes.as_slice() {
        [sandbox] => Ok(v1::SandboxSelector {
            sandbox_id: sandbox.sandbox_id.clone(),
        }),
        [] => Err(CliError::Usage {
            kind: UsageKind::NoSandbox,
            message: "no sandbox is available".to_owned(),
        }),
        _ => Err(CliError::Usage {
            kind: UsageKind::MultipleSandboxes,
            message: "multiple sandboxes are available".to_owned(),
        }),
    }
}

async fn operation(
    mut stream: tonic::Streaming<v1::OperationEvent>,
    json: bool,
    kind: OperationKind,
    sandbox_id: Option<String>,
) -> Result<i32, CliError> {
    if json {
        while let Some(event) = stream.message().await? {
            println!(
                "{}",
                serde_json::json!({"operation_id":event.operation_id.map(|id|id.value),"sequence":event.sequence,"phase":event.phase,"status":event.status,"error":event.error.as_ref().map(json_operation_error)})
            );
            if event.error.is_some() {
                return Ok(EXIT_RUNTIME);
            }
        }
        return Ok(0);
    }

    let (mut progress, initial) =
        OperationProgress::new(kind, sandbox_id, OutputCapabilities::for_stderr());
    if let Some(line) = initial {
        writeln!(std::io::stderr(), "{line}")?;
    }
    while let Some(event) = stream.message().await? {
        if let Some(error) = event.error {
            progress.clear();
            return Err(CliError::Operation {
                code: error.code,
                message: error.message,
            });
        }
        if let Some(line) = progress.update(&event) {
            writeln!(std::io::stderr(), "{line}")?;
        }
    }
    if let Some(line) = progress.finish_success() {
        writeln!(std::io::stderr(), "{line}")?;
    }
    Ok(0)
}

fn pre_stream_operation_failure(error: ClientError, json: bool) -> Result<i32, CliError> {
    let rendered = render_pre_stream_client_error(error, json)?;
    println!("{rendered}");
    Ok(EXIT_RUNTIME)
}

fn render_pre_stream_client_error(error: ClientError, json: bool) -> Result<String, CliError> {
    if !json {
        return Err(CliError::Client(error));
    }
    let code = error.stable_code().unwrap_or("unknown_error");
    let message = error.cause().unwrap_or_else(|| code.to_owned());
    let details = error.failure_details().unwrap_or(serde_json::Value::Null);
    Ok(serde_json::json!({
        "error": {
            "code": code,
            "message": message,
            "details": details,
        }
    })
    .to_string())
}

fn json_operation_error(error: &v1::Error) -> serde_json::Value {
    let details = serde_json::from_slice::<serde_json::Value>(&error.details)
        .unwrap_or(serde_json::Value::Null);
    serde_json::json!({
        "code": error.code,
        "message": error.message,
        "details": details,
    })
}

async fn run(
    client: &mut Client,
    explicit: Option<String>,
    argv: Vec<String>,
    shell: bool,
) -> Result<i32, CliError> {
    let selector = selector(client, explicit).await?;
    let environment = allowed_environment();
    let stdin_is_tty = std::io::stdin().is_terminal();
    let mut events = if shell {
        client
            .api
            .shell(v1::ShellRequest {
                sandbox: Some(selector),
                command: Some(v1::CommandPayload {
                    argv: argv.into_iter().map(String::into_bytes).collect(),
                    environment,
                    tty: true,
                }),
            })
            .await?
            .into_inner()
    } else {
        client
            .api
            .run(v1::RunRequest {
                sandbox: Some(selector),
                command: Some(v1::CommandPayload {
                    argv: argv.into_iter().map(String::into_bytes).collect(),
                    environment,
                    tty: false,
                }),
            })
            .await?
            .into_inner()
    };
    let token = first_session_token(&mut events).await?;
    attach_to_stdio(client, token, shell, stdin_is_tty).await
}

async fn logs(
    client: &mut Client,
    explicit: Option<String>,
    follow: bool,
    since_millis: Option<i64>,
) -> Result<i32, CliError> {
    let selector = selector(client, explicit).await?;
    let mut stream = client
        .api
        .logs(v1::LogsRequest {
            sandbox: Some(selector),
            since: since_millis.map(|millis| prost_types::Timestamp {
                seconds: millis.div_euclid(1_000),
                nanos: (millis.rem_euclid(1_000) * 1_000_000) as i32,
            }),
            follow,
        })
        .await?
        .into_inner();
    while let Some(event) = stream.message().await? {
        std::io::stdout().write_all(&event.payload)?;
    }
    Ok(0)
}

fn actual_name(value: i32) -> &'static str {
    match v1::ActualState::try_from(value).unwrap_or(v1::ActualState::Unknown) {
        v1::ActualState::Pending => "pending",
        v1::ActualState::Running => "running",
        v1::ActualState::Stopped => "stopped",
        v1::ActualState::Absent => "absent",
        v1::ActualState::Failed => "failed",
        _ => "unknown",
    }
}
fn status_json(status: &v1::SandboxStatus) -> serde_json::Value {
    let ssh = status.ssh.as_ref();
    let ssh_state = classify_ssh(status);
    let ready = ssh_state == SshState::Ready;
    serde_json::json!({
        "sandbox_id": status.sandbox_id,
        "actual_state": actual_name(status.actual_state),
        "apply_requirements": status.apply_requirements.iter().map(|requirement| {
            serde_json::json!({
                "reason": requirement.reason,
                "current": requirement.current,
                "requested": requirement.requested,
            })
        }).collect::<Vec<_>>(),
        "ssh": {
            "enabled": ssh.is_some_and(|ssh| ssh.enabled),
            "active": ready,
            "state": ssh_state.as_str(),
            "host": ready.then(|| ssh.and_then(|ssh| ssh.host.as_deref())).flatten(),
            "port": ready.then(|| ssh.and_then(|ssh| ssh.port)).flatten(),
            "alias": ready.then(|| ssh.and_then(|ssh| ssh.alias.as_deref())).flatten(),
            "host_key_fingerprint": ready.then(|| ssh.and_then(|ssh| ssh.host_key_fingerprint.as_deref())).flatten(),
            "client_key_fingerprint": ready.then(|| ssh.and_then(|ssh| ssh.client_key_fingerprint.as_deref())).flatten(),
        },
    })
}
fn render_status(status: &v1::SandboxStatus, json: bool) -> Result<(), CliError> {
    if json {
        println!("{}", status_json(status));
    } else {
        print!(
            "{}",
            render_human_status(status, OutputCapabilities::for_stdout())
        );
    }
    Ok(())
}
fn render_list(sandboxes: &[v1::SandboxStatus], json: bool) -> Result<(), CliError> {
    if json {
        let values = sandboxes.iter().map(status_json).collect::<Vec<_>>();
        println!(
            "{}",
            serde_json::to_string(&values).map_err(|e| CliError::Runtime(e.to_string()))?
        );
    } else {
        print!(
            "{}",
            render_human_list(sandboxes, OutputCapabilities::for_stdout())
        );
    }
    Ok(())
}
fn confirm_destroy() -> Result<(), CliError> {
    if !std::io::stdin().is_terminal() {
        return Err(CliError::Usage {
            kind: UsageKind::Other,
            message: "destroy requires --yes when stdin is not a TTY".to_owned(),
        });
    }
    eprint!("Destroy sandbox? [y/N] ");
    std::io::stderr().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    if answer.trim().eq_ignore_ascii_case("y") {
        Ok(())
    } else {
        Err(CliError::Usage {
            kind: UsageKind::Other,
            message: "destroy cancelled".to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optional_include_offer_failure_preserves_successful_up_result()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let home = temp.path().join("home");
        let ssh = home.join(".ssh");
        std::fs::create_dir(&home)?;
        std::fs::create_dir(&ssh)?;
        std::fs::set_permissions(
            &ssh,
            <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o775),
        )?;
        let config = SshConfig::for_environment(None, Some(&home))?;
        let mut warning = Vec::new();

        let result = preserve_up_result_with_optional_offer(
            Ok(0),
            false,
            &mut warning,
            || -> Result<(), CliError> {
                let _ = first_use_offer(&config, true, true).map_err(ssh_config_error)?;
                Ok(())
            },
        );

        assert_eq!(result?, 0);
        assert_eq!(
            String::from_utf8(warning)?,
            "Warning: automatic SSH config setup failed; run `gascan ssh-config install` to try again.\n"
        );
        assert_eq!(config.user_config_path(), home.join(".ssh/config"));
        assert_eq!(
            config.managed_config_path(),
            home.join(".config/gascan/ssh/config")
        );
        assert!(!home.join(".config/gascan/ssh/include-offer-v1").exists());
        Ok(())
    }

    #[test]
    fn optional_include_offer_accepts_conventional_0755_directory_without_warning()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let home = temp.path().join("home");
        let ssh = home.join(".ssh");
        std::fs::create_dir(&home)?;
        std::fs::create_dir(&ssh)?;
        std::fs::set_permissions(
            &ssh,
            <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o755),
        )?;
        let config = SshConfig::for_environment(None, Some(&home))?;
        let mut warning = Vec::new();

        let result = preserve_up_result_with_optional_offer(
            Ok(0),
            false,
            &mut warning,
            || -> Result<(), CliError> {
                let _ = first_use_offer(&config, true, true).map_err(ssh_config_error)?;
                Ok(())
            },
        );

        assert_eq!(result?, 0);
        assert!(warning.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn first_up_developer_offer_error_warns_once_and_preserves_success()
    -> Result<(), Box<dyn std::error::Error>> {
        let calls = std::cell::Cell::new(0_u32);
        let mut warning = Vec::new();

        let result =
            preserve_up_result_with_developer_offer(Ok(0), false, false, &mut warning, || async {
                calls.set(calls.get() + 1);
                Err(ConfigureError::GuestCommand {
                    category: "developer-home receipt",
                    message: "injected failure".to_owned(),
                })
            })
            .await;

        assert_eq!(result?, 0);
        assert_eq!(calls.get(), 1);
        assert_eq!(
            String::from_utf8(warning)?,
            "Warning: developer setup was not completed; run `gascan configure` to try again.\n"
        );
        Ok(())
    }

    #[tokio::test]
    async fn first_up_pending_setup_warns_once_and_preserves_success()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut warning = Vec::new();

        let result =
            preserve_up_result_with_developer_offer(Ok(0), false, false, &mut warning, || async {
                Ok(OfferResult::Pending)
            })
            .await;

        assert_eq!(result?, 0);
        assert_eq!(
            String::from_utf8(warning)?,
            "Warning: developer setup was not completed; run `gascan configure` to try again.\n"
        );
        Ok(())
    }

    #[tokio::test]
    async fn first_up_offer_runs_once_only_for_successful_human_non_ci_up()
    -> Result<(), Box<dyn std::error::Error>> {
        async fn exercise(
            result: Result<i32, CliError>,
            json: bool,
            ci: bool,
        ) -> Result<(Result<i32, CliError>, u32, u32, Vec<u8>), Box<dyn std::error::Error>>
        {
            let selector_preparations = std::cell::Cell::new(0_u32);
            let calls = std::cell::Cell::new(0_u32);
            let mut warning = Vec::new();
            let result = preserve_up_result_with_developer_offer_gate(
                result,
                DeveloperOfferEligibility {
                    json,
                    continuous_integration: ci,
                    stdin_is_terminal: true,
                    stderr_is_terminal: true,
                },
                &mut warning,
                || {
                    selector_preparations.set(selector_preparations.get() + 1);
                    Ok(v1::SandboxSelector {
                        sandbox_id: "selected-0123456789ab".to_owned(),
                    })
                },
                |_| async {
                    calls.set(calls.get() + 1);
                    Ok(OfferResult::Completed)
                },
            )
            .await;
            Ok((result, selector_preparations.get(), calls.get(), warning))
        }

        let (success, selector_preparations, calls, warning) =
            exercise(Ok(0), false, false).await?;
        assert_eq!(success?, 0);
        assert_eq!(selector_preparations, 1);
        assert_eq!(calls, 1);
        assert!(warning.is_empty());

        for (result, json, ci) in [
            (Ok(0), true, false),
            (Ok(0), false, true),
            (Ok(EXIT_RUNTIME), false, false),
        ] {
            let (result, selector_preparations, calls, warning) =
                exercise(result, json, ci).await?;
            assert!(result.is_ok());
            assert_eq!(selector_preparations, 0);
            assert_eq!(calls, 0);
            assert!(warning.is_empty());
        }

        let original = CliError::Runtime("injected up failure".to_owned());
        let (failed, selector_preparations, calls, warning) =
            exercise(Err(original), false, false).await?;
        let failed = match failed {
            Err(error) => error,
            Ok(_) => return Err("failed up unexpectedly succeeded".into()),
        };
        assert_eq!(failed.message(), "injected up failure");
        assert_eq!(selector_preparations, 0);
        assert_eq!(calls, 0);
        assert!(warning.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn first_up_redirected_io_suppresses_before_selector_preparation()
    -> Result<(), Box<dyn std::error::Error>> {
        for (stdin_is_terminal, stderr_is_terminal) in [(false, true), (true, false)] {
            let selector_preparations = std::cell::Cell::new(0_u32);
            let offer_calls = std::cell::Cell::new(0_u32);
            let mut warning = Vec::new();

            let result = preserve_up_result_with_developer_offer_gate(
                Ok(0),
                DeveloperOfferEligibility {
                    json: false,
                    continuous_integration: false,
                    stdin_is_terminal,
                    stderr_is_terminal,
                },
                &mut warning,
                || {
                    selector_preparations.set(selector_preparations.get() + 1);
                    Err(ConfigureError::HostCommand {
                        category: "developer onboarding selector",
                        message: "injected selector failure".to_owned(),
                    })
                },
                |_| async {
                    offer_calls.set(offer_calls.get() + 1);
                    Ok(OfferResult::Completed)
                },
            )
            .await;

            assert_eq!(result?, 0);
            assert_eq!(selector_preparations.get(), 0);
            assert_eq!(offer_calls.get(), 0);
            assert!(warning.is_empty());
        }
        Ok(())
    }

    #[test]
    fn first_up_ci_suppression_recognizes_each_supported_environment() {
        for active in ["CI", "GITHUB_ACTIONS", "BUILD_BUILDID"] {
            assert!(continuous_integration_from(|name| {
                (name == active).then(|| OsString::from("1"))
            }));
        }
        assert!(!continuous_integration_from(|_| None));
        assert!(!continuous_integration_from(|_| Some(OsString::new())));
    }

    #[test]
    fn first_up_selector_is_derived_from_the_resolved_project_not_sandbox_count()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempfile::tempdir()?;
        let selected = temporary.path().join("selected");
        let unrelated = temporary.path().join("unrelated");
        std::fs::create_dir(&selected)?;
        std::fs::create_dir(&unrelated)?;
        std::fs::write(
            selected.join("gascan.toml"),
            "version = 1\nname = 'selected-project'\n",
        )?;
        std::fs::write(
            unrelated.join("gascan.toml"),
            "version = 1\nname = 'unrelated-project'\n",
        )?;
        let selected = std::fs::canonicalize(selected)?;
        let unrelated = std::fs::canonicalize(unrelated)?;

        let selector =
            selector_for_project_root(selected.to_str().ok_or("selected root was not UTF-8")?)?;
        let unrelated_selector =
            selector_for_project_root(unrelated.to_str().ok_or("unrelated root was not UTF-8")?)?;

        assert_ne!(selector.sandbox_id, unrelated_selector.sandbox_id);
        assert!(selector.sandbox_id.starts_with("selected-project-"));
        Ok(())
    }

    #[test]
    fn root_help_advertises_the_standard_version_flags() {
        let help = Arguments::command().render_help().to_string();
        assert!(
            help.contains("-V, --version"),
            "version option missing: {help}"
        );
    }

    #[test]
    fn configure_clap_accepts_all_forms_global_selector_and_protocol_default()
    -> Result<(), Box<dyn std::error::Error>> {
        let aggregate =
            Arguments::try_parse_from(["gascan", "--sandbox", "demo-0123456789ab", "configure"])?;
        assert_eq!(aggregate.sandbox.as_deref(), Some("demo-0123456789ab"));
        assert!(matches!(
            aggregate.command,
            Command::Configure { command: None }
        ));

        let git = Arguments::try_parse_from(["gascan", "configure", "git"])?;
        assert!(matches!(
            git.command,
            Command::Configure {
                command: Some(ConfigureCommand::Git)
            }
        ));

        let github = Arguments::try_parse_from(["gascan", "configure", "gh"])?;
        assert!(matches!(
            github.command,
            Command::Configure {
                command: Some(ConfigureCommand::Gh {
                    hostname: None,
                    token_stdin: false,
                    git_protocol: GitProtocol::Ssh,
                })
            }
        ));

        let gitlab = Arguments::try_parse_from([
            "gascan",
            "configure",
            "glab",
            "--hostname",
            "gitlab.enterprise.test",
            "--token-stdin",
            "--git-protocol",
            "https",
        ])?;
        assert!(matches!(
            gitlab.command,
            Command::Configure {
                command: Some(ConfigureCommand::Glab {
                    hostname: Some(hostname),
                    token_stdin: true,
                    git_protocol: GitProtocol::Https,
                })
            } if hostname == "gitlab.enterprise.test"
        ));
        Ok(())
    }

    #[test]
    fn configure_preflight_enforces_interactive_and_token_stdin_modes() {
        let aggregate = None;
        assert!(preflight_configure(&aggregate, true, true).is_ok());
        assert!(preflight_configure(&aggregate, false, true).is_err());
        assert!(preflight_configure(&aggregate, true, false).is_err());

        let git = Some(ConfigureCommand::Git);
        assert!(preflight_configure(&git, true, true).is_ok());
        assert!(preflight_configure(&git, false, true).is_err());

        let piped_ssh = Some(ConfigureCommand::Gh {
            hostname: None,
            token_stdin: true,
            git_protocol: GitProtocol::Ssh,
        });
        let ssh_error = preflight_configure(&piped_ssh, false, false)
            .err()
            .map(|error| error.message().to_owned());
        assert_eq!(
            ssh_error.as_deref(),
            Some(
                "--token-stdin cannot perform SSH first-use verification; rerun interactively without --token-stdin, or pass --git-protocol https"
            )
        );
        assert!(preflight_configure(&piped_ssh, true, true).is_err());

        let piped_https = Some(ConfigureCommand::Gh {
            hostname: None,
            token_stdin: true,
            git_protocol: GitProtocol::Https,
        });
        assert!(preflight_configure(&piped_https, false, false).is_ok());

        let hidden = Some(ConfigureCommand::Glab {
            hostname: None,
            token_stdin: false,
            git_protocol: GitProtocol::Ssh,
        });
        assert!(preflight_configure(&hidden, true, true).is_ok());
        assert!(preflight_configure(&hidden, false, true).is_err());
    }

    #[test]
    fn configure_selector_and_running_state_fail_with_existing_guidance()
    -> Result<(), Box<dyn std::error::Error>> {
        let none = match selector_from_sandboxes(Vec::new()) {
            Err(error) => error,
            Ok(_) => return Err("no sandbox unexpectedly selected".into()),
        };
        assert!(matches!(
            none,
            CliError::Usage {
                kind: UsageKind::NoSandbox,
                ..
            }
        ));

        let statuses = ["one", "two"].map(|sandbox_id| v1::SandboxStatus {
            sandbox_id: sandbox_id.to_owned(),
            actual_state: v1::ActualState::Running as i32,
            ..Default::default()
        });
        let multiple = match selector_from_sandboxes(statuses.to_vec()) {
            Err(error) => error,
            Ok(_) => return Err("multiple sandboxes unexpectedly selected".into()),
        };
        assert!(matches!(
            multiple,
            CliError::Usage {
                kind: UsageKind::MultipleSandboxes,
                ..
            }
        ));

        let stopped = v1::SandboxStatus {
            sandbox_id: "demo-0123456789ab".to_owned(),
            actual_state: v1::ActualState::Stopped as i32,
            ..Default::default()
        };
        let error = match require_running_sandbox("demo-0123456789ab", &stopped) {
            Err(error) => error,
            Ok(()) => return Err("stopped sandbox accepted configuration".into()),
        };
        assert_eq!(
            error.message(),
            "sandbox `demo-0123456789ab` is not running; run `gascan up <project-root>`"
        );
        Ok(())
    }

    #[test]
    fn mismatched_running_status_stops_before_token_read_or_runner_creation()
    -> Result<(), Box<dyn std::error::Error>> {
        const TOKEN_SENTINEL: &str = "configure-dispatch-token-never-read-42a7";

        let selector = v1::SandboxSelector {
            sandbox_id: "selected-0123456789ab".to_owned(),
        };
        let status = v1::SandboxStatus {
            sandbox_id: "different-0123456789ab".to_owned(),
            actual_state: v1::ActualState::Running as i32,
            ..Default::default()
        };
        let command = Some(ConfigureCommand::Gh {
            hostname: None,
            token_stdin: true,
            git_protocol: GitProtocol::Https,
        });
        let token_reads = std::cell::Cell::new(0_u32);
        let runner_constructions = std::cell::Cell::new(0_u32);

        let prepared: Result<(Option<Secret>, ()), CliError> = prepare_configure_dispatch(
            &selector,
            &status,
            &command,
            || {
                token_reads.set(token_reads.get() + 1);
                Ok(Secret::new(TOKEN_SENTINEL.as_bytes().to_vec()))
            },
            || {
                runner_constructions.set(runner_constructions.get() + 1);
            },
        );

        let error = match prepared {
            Err(error) => error,
            Ok(_) => return Err("mismatched running status reached configure dispatch".into()),
        };
        assert_eq!(
            error.message(),
            "daemon returned status for a different sandbox than selected `selected-0123456789ab`; retry the command, then run `gascan daemon restart` if the mismatch persists"
        );
        assert_eq!(token_reads.get(), 0);
        assert_eq!(runner_constructions.get(), 0);
        assert!(!error.message().contains(TOKEN_SENTINEL));
        Ok(())
    }

    #[test]
    fn stopped_status_stops_before_token_read_or_runner_creation()
    -> Result<(), Box<dyn std::error::Error>> {
        const TOKEN_SENTINEL: &str = "configure-dispatch-token-never-read-42a7";

        let selector = v1::SandboxSelector {
            sandbox_id: "selected-0123456789ab".to_owned(),
        };
        let status = v1::SandboxStatus {
            sandbox_id: selector.sandbox_id.clone(),
            actual_state: v1::ActualState::Stopped as i32,
            ..Default::default()
        };
        let command = Some(ConfigureCommand::Glab {
            hostname: None,
            token_stdin: true,
            git_protocol: GitProtocol::Https,
        });
        let token_reads = std::cell::Cell::new(0_u32);
        let runner_constructions = std::cell::Cell::new(0_u32);

        let prepared: Result<(Option<Secret>, ()), CliError> = prepare_configure_dispatch(
            &selector,
            &status,
            &command,
            || {
                token_reads.set(token_reads.get() + 1);
                Ok(Secret::new(TOKEN_SENTINEL.as_bytes().to_vec()))
            },
            || {
                runner_constructions.set(runner_constructions.get() + 1);
            },
        );

        let error = match prepared {
            Err(error) => error,
            Ok(_) => return Err("stopped status reached configure dispatch".into()),
        };
        assert_eq!(
            error.message(),
            "sandbox `selected-0123456789ab` is not running; run `gascan up <project-root>`"
        );
        assert_eq!(token_reads.get(), 0);
        assert_eq!(runner_constructions.get(), 0);
        assert!(!error.message().contains(TOKEN_SENTINEL));
        Ok(())
    }

    #[test]
    fn piped_token_capture_is_exact_bounded_and_redacted() -> Result<(), Box<dyn std::error::Error>>
    {
        let token = read_token_stdin_from(&mut std::io::Cursor::new(b"token-bytes\n"))
            .map_err(|error| format!("valid token was rejected: {error}"))?;
        assert_eq!(token.expose(), b"token-bytes\n");
        assert!(!format!("{token:?}").contains("token-bytes"));

        assert!(read_token_stdin_from(&mut std::io::Cursor::new(Vec::<u8>::new())).is_err());
        assert!(
            read_token_stdin_from(&mut std::io::Cursor::new(vec![
                b'x';
                MAX_CONFIGURE_TOKEN_BYTES + 1
            ]))
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn daemon_parses_the_public_management_commands() -> Result<(), Box<dyn std::error::Error>> {
        let status = Arguments::try_parse_from(["gascan", "daemon", "status", "--json"])?;
        assert!(matches!(
            status.command,
            Command::Daemon {
                command: DaemonCommand::Status { json: true }
            }
        ));

        let start = Arguments::try_parse_from(["gascan", "daemon", "start", "--json"])?;
        assert!(matches!(
            start.command,
            Command::Daemon {
                command: DaemonCommand::Start { json: true }
            }
        ));

        let stop = Arguments::try_parse_from(["gascan", "daemon", "stop", "--force", "--json"])?;
        assert!(matches!(
            stop.command,
            Command::Daemon {
                command: DaemonCommand::Stop {
                    force: true,
                    json: true,
                }
            }
        ));

        let restart =
            Arguments::try_parse_from(["gascan", "daemon", "restart", "--force", "--json"])?;
        assert!(matches!(
            restart.command,
            Command::Daemon {
                command: DaemonCommand::Restart {
                    force: true,
                    json: true,
                }
            }
        ));
        Ok(())
    }

    #[test]
    fn daemon_rejects_force_for_status_and_start() {
        for subcommand in ["status", "start"] {
            assert!(matches!(
                Arguments::try_parse_from(["gascan", "daemon", subcommand, "--force"]),
                Err(error) if error.kind() == ErrorKind::UnknownArgument
            ));
        }
    }

    #[test]
    fn daemon_attest_remains_hidden_from_public_help() {
        let help = Arguments::command().render_long_help().to_string();
        assert!(help.contains("daemon"), "daemon command missing: {help}");
        assert!(
            !help.contains("daemon-attest"),
            "internal daemon command leaked into help: {help}"
        );
    }

    #[test]
    fn daemon_supervisor_io_errors_have_a_stable_actionable_cli_contract() {
        let error = supervisor_error(crate::daemon::SupervisorError::Io(std::io::Error::other(
            "runtime directory is unavailable",
        )));

        assert_eq!(error.stable_code(), Some("daemon_io"));
        assert_eq!(
            error.suggestion(),
            Some("run `gascan daemon status` after checking the local runtime directory")
        );
    }

    #[test]
    fn daemon_graceful_timeout_guidance_preserves_the_requested_command() {
        let timeout = || crate::daemon::SupervisorError::GracefulTimeout {
            identity: Box::new(crate::daemon::DaemonIdentity {
                pid: 42,
                executable: "/trusted/gascand".into(),
                start_identity: "start:42".to_owned(),
                instance_token: "11".repeat(32),
                release_version: Some(env!("CARGO_PKG_VERSION").to_owned()),
                started_at: Some(crate::daemon::InstanceTimestamp {
                    seconds: 1_785_264_100,
                    nanos: 0,
                }),
            }),
        };

        let stop = supervisor_error_for_action(timeout(), DaemonErrorContext::Stop);
        assert_eq!(stop.stable_code(), Some("daemon_graceful_shutdown_timeout"));
        assert_eq!(
            stop.suggestion(),
            Some("retry with `gascan daemon stop --force` if it is safe to interrupt active work")
        );

        let restart = supervisor_error_for_action(timeout(), DaemonErrorContext::Restart);
        assert_eq!(
            restart.stable_code(),
            Some("daemon_graceful_shutdown_timeout")
        );
        assert_eq!(
            restart.suggestion(),
            Some(
                "retry with `gascan daemon restart --force` if it is safe to interrupt active work"
            )
        );

        let automatic = supervisor_error_for_action(timeout(), DaemonErrorContext::Automatic);
        assert_eq!(
            automatic.suggestion(),
            Some("run `gascan daemon restart --force` if it is safe to interrupt active work")
        );
    }

    #[tokio::test]
    async fn daemon_json_recovery_observer_writes_no_progress_to_either_output_stream()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::daemon::DaemonLifecycleObserver as _;

        let sink = CapturingRecoveryOutput::default();
        let mut observer =
            CliRecoveryObserver::new(true, OutputCapabilities::for_stderr(), sink.clone());
        observer
            .transition_started(crate::daemon::DaemonTransition::Recovered)
            .await;

        assert!(!observer.is_presenting());
        let stderr_is_empty = {
            let stderr = sink
                .stderr
                .lock()
                .map_err(|_| std::io::Error::other("captured stderr was poisoned"))?;
            stderr.is_empty()
        };
        let stdout_is_empty = {
            let stdout = sink
                .stdout
                .lock()
                .map_err(|_| std::io::Error::other("captured stdout was poisoned"))?;
            stdout.is_empty()
        };
        assert!(stderr_is_empty);
        assert!(stdout_is_empty);
        Ok(())
    }

    #[derive(Clone, Default)]
    struct CapturingRecoveryOutput {
        stderr: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        stdout: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    }

    impl RecoveryOutputSink for CapturingRecoveryOutput {
        fn write(&mut self, stream: RecoveryOutputStream, line: &str) {
            let output = match stream {
                RecoveryOutputStream::Stdout => &self.stdout,
                RecoveryOutputStream::Stderr => &self.stderr,
            };
            if let Ok(mut output) = output.lock() {
                output.push(line.to_owned());
            }
        }
    }

    #[test]
    fn status_json_preserves_exact_image_references() {
        let current = "registry.example/workspace:old@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let requested = "registry.example/workspace:new@sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let status = v1::SandboxStatus {
            sandbox_id: "code-123".to_owned(),
            actual_state: v1::ActualState::Running as i32,
            apply_requirements: vec![v1::ApplyRequirement {
                reason: "image_changed".to_owned(),
                current: current.to_owned(),
                requested: requested.to_owned(),
            }],
            ..Default::default()
        };

        assert_eq!(
            status_json(&status),
            serde_json::json!({
                "sandbox_id": "code-123",
                "actual_state": "running",
                "apply_requirements": [{
                    "reason": "image_changed",
                    "current": current,
                    "requested": requested,
                }],
                "ssh": {
                    "enabled": false,
                    "active": false,
                    "state": "unavailable",
                    "host": null,
                    "port": null,
                    "alias": null,
                    "host_key_fingerprint": null,
                    "client_key_fingerprint": null,
                },
            })
        );
    }

    #[test]
    fn status_json_exposes_active_ssh_as_separate_structured_fields() {
        let status = v1::SandboxStatus {
            sandbox_id: "code-123".to_owned(),
            actual_state: v1::ActualState::Running as i32,
            ssh: Some(v1::SshStatus {
                enabled: true,
                active: true,
                host: Some("127.0.0.1".to_owned()),
                port: Some(22222),
                alias: Some("gascan-code-123".to_owned()),
                host_key_fingerprint: Some("SHA256:host".to_owned()),
                client_key_fingerprint: Some("SHA256:client".to_owned()),
            }),
            ..Default::default()
        };

        assert_eq!(
            status_json(&status)["ssh"],
            serde_json::json!({
                "enabled": true,
                "active": true,
                "state": "ready",
                "host": "127.0.0.1",
                "port": 22222,
                "alias": "gascan-code-123",
                "host_key_fingerprint": "SHA256:host",
                "client_key_fingerprint": "SHA256:client",
            })
        );
    }

    #[test]
    fn status_json_suppresses_incomplete_active_endpoint_and_uses_shared_state() {
        let status = v1::SandboxStatus {
            sandbox_id: "code-123".to_owned(),
            actual_state: v1::ActualState::Running as i32,
            ssh: Some(v1::SshStatus {
                enabled: true,
                active: true,
                host: Some("127.0.0.1".to_owned()),
                port: Some(22222),
                alias: Some("gascan-code-123".to_owned()),
                host_key_fingerprint: None,
                client_key_fingerprint: Some("SHA256:client".to_owned()),
            }),
            ..Default::default()
        };

        assert_eq!(
            status_json(&status)["ssh"],
            serde_json::json!({
                "enabled": true,
                "active": false,
                "state": "unhealthy",
                "host": null,
                "port": null,
                "alias": null,
                "host_key_fingerprint": null,
                "client_key_fingerprint": null,
            })
        );
    }

    #[test]
    fn clap_formats_the_package_version() -> Result<(), Box<dyn std::error::Error>> {
        let error = Arguments::try_parse_from(["gascan", "--version"])
            .err()
            .ok_or("version did not produce an early display result")?;
        assert_eq!(error.kind(), clap::error::ErrorKind::DisplayVersion);
        assert_eq!(
            error.to_string(),
            format!("gascan {}\n", env!("CARGO_PKG_VERSION"))
        );
        Ok(())
    }

    #[test]
    fn selector_usage_errors_choose_suggestions_structurally() {
        let no_sandbox = CliError::Usage {
            kind: UsageKind::NoSandbox,
            message: "no sandbox is available".to_owned(),
        };
        assert_eq!(no_sandbox.message(), "no sandbox is available");
        assert_eq!(no_sandbox.suggestion(), Some("gascan up <project-root>"));

        let multiple = CliError::Usage {
            kind: UsageKind::MultipleSandboxes,
            message: "multiple sandboxes are available".to_owned(),
        };
        assert_eq!(multiple.message(), "multiple sandboxes are available");
        assert_eq!(
            multiple.suggestion(),
            Some("run `gascan list`, then pass `--sandbox <sandbox-id>`")
        );
    }

    #[test]
    fn sandbox_not_found_uses_its_stable_code_for_the_suggestion() {
        let error = CliError::Client(ClientError::Rpc(Box::new(tonic::Status::not_found(
            gascan_proto::error_code::SANDBOX_NOT_FOUND,
        ))));
        assert_eq!(error.stable_code(), Some("sandbox_not_found"));
        assert_eq!(error.message(), "sandbox not found");
        assert_eq!(
            error.suggestion(),
            Some("run `gascan list` and use the sandbox ID shown there")
        );
    }

    #[test]
    fn empty_operation_message_falls_back_to_its_stable_code() {
        for message in ["", "  \n\t"] {
            let error = CliError::Operation {
                code: "injected_failure".to_owned(),
                message: message.to_owned(),
            };

            assert_eq!(error.message(), "injected_failure");
        }
    }

    #[test]
    fn resource_conflict_explains_managed_resource_and_keeps_daemon_cause() {
        let error = CliError::Operation {
            code: "resource_conflict".to_owned(),
            message: "resource conflict for port 3000: already reserved".to_owned(),
        };
        assert_eq!(error.stable_code(), Some("resource_conflict"));
        assert_eq!(
            error.message(),
            concat!(
                "a managed runtime resource already exists: ",
                "resource conflict for port 3000: already reserved",
            )
        );
    }

    #[test]
    fn empty_resource_conflict_cause_falls_back_to_stable_code() {
        let error = CliError::Operation {
            code: "resource_conflict".to_owned(),
            message: " \n".to_owned(),
        };

        assert_eq!(error.message(), "resource_conflict");
    }

    #[test]
    fn attach_frame_error_retains_code_and_message_on_runtime_path() {
        let error = crate::guest::attach_frame_error(v1::Error {
            code: "process_failed".to_owned(),
            message: "command exited before setup completed".to_owned(),
            ..Default::default()
        });

        assert!(matches!(error, CliError::Runtime(_)));
        assert_eq!(
            error.message(),
            "process_failed: command exited before setup completed"
        );
    }

    #[test]
    fn json_operation_error_retains_storage_change_details()
    -> Result<(), Box<dyn std::error::Error>> {
        let error = v1::Error {
            code: gascan_proto::error_code::STORAGE_CHANGE_REQUIRES_RECREATE.to_owned(),
            message: "storage settings changed".to_owned(),
            details: serde_json::to_vec(&serde_json::json!({"changes":[{
                "volume":"tools",
                "recorded_bytes":10 * 1024_u64.pow(3),
                "requested_bytes":20 * 1024_u64.pow(3),
            }]}))?,
        };
        assert_eq!(
            super::json_operation_error(&error),
            serde_json::json!({
                "code":"storage_change_requires_recreate",
                "message":"storage settings changed",
                "details":{"changes":[{
                    "volume":"tools",
                    "recorded_bytes":10 * 1024_u64.pow(3),
                    "requested_bytes":20 * 1024_u64.pow(3),
                }]}
            })
        );
        Ok(())
    }

    #[test]
    fn storage_change_human_error_is_actionable() {
        let message = "storage settings changed for tools (10GiB → 20GiB); run `gascan destroy --yes` and `gascan up` to recreate the sandbox";
        let error = CliError::Operation {
            code: gascan_proto::error_code::STORAGE_CHANGE_REQUIRES_RECREATE.to_owned(),
            message: message.to_owned(),
        };
        assert_eq!(error.message(), message);
        assert!(render_error(&error).contains(message));
    }

    #[test]
    fn ssh_not_ready_human_and_json_errors_keep_the_cause_and_stable_code()
    -> Result<(), Box<dyn std::error::Error>> {
        let message = concat!(
            "strict SSH readiness for 127.0.0.1:2222 failed; ",
            "last OpenSSH stderr tail: Host key verification failed\n",
            "Run `gascan doctor` for managed SSH configuration details."
        );
        let details =
            gascan_proto::error_detail::encode(gascan_proto::error_code::SSH_NOT_READY, message);
        let human_error = CliError::Client(ClientError::from(tonic::Status::with_details(
            tonic::Code::FailedPrecondition,
            gascan_proto::error_code::SSH_NOT_READY,
            tonic::codegen::Bytes::from(details.clone()),
        )));

        let human = render_error(&human_error);
        assert!(
            human.contains("127.0.0.1:2222"),
            "missing endpoint: {human}"
        );
        assert!(
            human.contains("Host key verification failed"),
            "missing OpenSSH cause: {human}"
        );
        assert!(
            human.contains("gascan doctor"),
            "missing doctor guidance: {human}"
        );
        assert!(
            !human.contains(gascan_proto::error_code::SSH_NOT_READY),
            "human error must show its cause instead of its stable code: {human}"
        );

        let json_error = ClientError::from(tonic::Status::with_details(
            tonic::Code::FailedPrecondition,
            gascan_proto::error_code::SSH_NOT_READY,
            tonic::codegen::Bytes::from(details),
        ));
        let json = super::render_pre_stream_client_error(json_error, true)?;
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&json)?,
            serde_json::json!({"error": {
                "code": "ssh_not_ready",
                "message": message,
                "details": null,
            }})
        );
        Ok(())
    }

    #[test]
    fn offline_unavailable_human_and_json_errors_keep_the_cause_and_stable_code()
    -> Result<(), Box<dyn std::error::Error>> {
        let message = concat!(
            "hard offline isolation has not been verified with Apple Container 1.2.0; ",
            "use networked mode or install the certified 1.1.0 release"
        );
        let details = gascan_proto::error_detail::encode("offline_unavailable", message);
        let status = || {
            tonic::Status::with_details(
                tonic::Code::InvalidArgument,
                "offline_unavailable",
                tonic::codegen::Bytes::from(details.clone()),
            )
        };

        let human = render_error(&CliError::Client(ClientError::from(status())));
        assert!(
            human.contains(message),
            "human error did not preserve the offline cause: {human}"
        );
        assert!(
            !human.contains("offline_unavailable"),
            "human error showed the stable code instead of the cause: {human}"
        );

        let json = super::render_pre_stream_client_error(ClientError::from(status()), true)?;
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&json)?,
            serde_json::json!({"error": {
                "code": "offline_unavailable",
                "message": message,
                "details": null,
            }})
        );
        Ok(())
    }

    #[test]
    fn pre_stream_client_error_renders_structured_json_when_requested()
    -> Result<(), Box<dyn std::error::Error>> {
        let message = "storage settings changed for tools (10GiB → 20GiB); run `gascan destroy --yes` and `gascan up` to recreate the sandbox";
        let changes = serde_json::json!({"changes":[{
            "volume":"tools",
            "recorded_bytes":10 * 1024_u64.pow(3),
            "requested_bytes":20 * 1024_u64.pow(3),
        }]});
        let details = gascan_proto::error_detail::encode_with_details(
            gascan_proto::error_code::STORAGE_CHANGE_REQUIRES_RECREATE,
            message,
            &serde_json::to_vec(&changes)?,
        );
        let error = ClientError::from(tonic::Status::with_details(
            tonic::Code::FailedPrecondition,
            gascan_proto::error_code::STORAGE_CHANGE_REQUIRES_RECREATE,
            tonic::codegen::Bytes::from(details),
        ));

        let rendered = super::render_pre_stream_client_error(error, true)?;
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&rendered)?,
            serde_json::json!({"error":{
                "code":"storage_change_requires_recreate",
                "message":message,
                "details":changes,
            }})
        );
        Ok(())
    }

    #[test]
    fn pre_stream_client_error_keeps_human_mode_unchanged() {
        let status = tonic::Status::failed_precondition(
            gascan_proto::error_code::STORAGE_CHANGE_REQUIRES_RECREATE,
        );
        let error = super::render_pre_stream_client_error(ClientError::from(status), false)
            .err()
            .ok_or("human mode unexpectedly rendered JSON");
        assert!(matches!(
            error,
            Ok(CliError::Client(ClientError::Rpc(status)))
                if status.message()
                    == gascan_proto::error_code::STORAGE_CHANGE_REQUIRES_RECREATE
        ));
    }

    #[test]
    fn relative_roots_resolve_against_this_process() -> Result<(), Box<dyn std::error::Error>> {
        let resolved = resolve_project_root(".")?;
        assert_eq!(
            std::path::Path::new(&resolved),
            std::env::current_dir()?.canonicalize()?
        );
        assert!(std::path::Path::new(&resolved).is_absolute());
        Ok(())
    }

    #[test]
    fn doctor_request_carries_the_callers_absolute_workspace()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?.path().canonicalize()?;
        let request = doctor_request(Ok(directory.clone()));

        assert_eq!(
            request.workspace_result,
            Some(v1::doctor_request::WorkspaceResult::Workspace(
                directory.to_str().ok_or("UTF-8 workspace")?.to_owned(),
            ))
        );
        Ok(())
    }

    #[test]
    fn doctor_request_reports_caller_directory_errors() {
        let request = doctor_request(Err(std::io::Error::other("launch directory was removed")));

        assert!(matches!(
            request.workspace_result,
            Some(v1::doctor_request::WorkspaceResult::WorkspaceError(error))
                if error.contains("launch directory was removed")
        ));
    }

    #[test]
    fn absolute_roots_survive_resolution() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let canonical = directory.path().canonicalize()?;
        let resolved = resolve_project_root(canonical.to_str().ok_or("non-UTF-8 fixture")?)?;
        assert_eq!(std::path::Path::new(&resolved), canonical);
        Ok(())
    }

    #[test]
    fn dot_segments_and_trailing_slashes_normalize() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let canonical = directory.path().canonicalize()?;
        let base = canonical.to_str().ok_or("non-UTF-8 fixture")?;
        for variant in [
            format!("{base}/"),
            format!("{base}/."),
            format!("{base}/./"),
        ] {
            assert_eq!(
                std::path::Path::new(&resolve_project_root(&variant)?),
                canonical,
                "variant {variant} must normalize"
            );
        }
        Ok(())
    }

    #[test]
    fn parent_and_nested_segments_resolve() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let canonical = directory.path().canonicalize()?;
        std::fs::create_dir(canonical.join("nested"))?;
        let base = canonical.to_str().ok_or("non-UTF-8 fixture")?;

        // A nested relative segment.
        assert_eq!(
            std::path::Path::new(&resolve_project_root(&format!("{base}/nested"))?),
            canonical.join("nested")
        );
        // A parent segment that climbs back out of it.
        assert_eq!(
            std::path::Path::new(&resolve_project_root(&format!("{base}/nested/.."))?),
            canonical
        );
        Ok(())
    }

    #[test]
    fn a_symlinked_root_resolves_to_its_target() -> Result<(), Box<dyn std::error::Error>> {
        // The daemon canonicalizes too, so the client must agree with it about
        // which directory a symlink names; otherwise the same project could
        // produce two sandbox identities.
        let directory = tempfile::tempdir()?;
        let canonical = directory.path().canonicalize()?;
        let target = canonical.join("project");
        std::fs::create_dir(&target)?;
        let link = canonical.join("link");
        std::os::unix::fs::symlink(&target, &link)?;

        let resolved = resolve_project_root(link.to_str().ok_or("non-UTF-8 fixture")?)?;
        assert_eq!(std::path::Path::new(&resolved), target);
        Ok(())
    }

    #[test]
    #[allow(
        clippy::expect_used,
        reason = "asserting on the Err variant is the test"
    )]
    fn a_missing_root_fails_here_rather_than_at_the_daemon() {
        let error = resolve_project_root("/definitely/not/a/real/project/root")
            .expect_err("a missing root must be rejected");
        assert_eq!(error.exit_code(), super::EXIT_USAGE);
        assert!(
            format!("{error}").contains("/definitely/not/a/real/project/root"),
            "the message must name the offending path"
        );
    }

    #[test]
    #[allow(
        clippy::expect_used,
        reason = "asserting on the Err variant is the test"
    )]
    fn an_empty_root_is_rejected() {
        let error = resolve_project_root("").expect_err("an empty root must be rejected");
        assert_eq!(error.exit_code(), super::EXIT_USAGE);
    }

    #[test]
    #[allow(
        clippy::expect_used,
        reason = "asserting on the Err variant is the test"
    )]
    fn a_file_root_is_rejected_locally() -> Result<(), Box<dyn std::error::Error>> {
        let file = tempfile::NamedTempFile::new()?;
        let path = file.path().to_str().ok_or("non-UTF-8 fixture")?;
        let error = resolve_project_root(path).expect_err("a file root must be rejected");
        assert_eq!(error.exit_code(), super::EXIT_USAGE);
        assert!(
            format!("{error}").contains(path),
            "the message must name the offending path"
        );
        Ok(())
    }
}
