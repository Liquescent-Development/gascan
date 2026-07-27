use crate::client::{Client, ClientError};
use crate::presentation::{
    DoctorCheck, OperationKind, OperationProgress, OutputCapabilities,
    render_doctor as render_human_doctor, render_error as render_human_error,
    render_list as render_human_list, render_status as render_human_status,
};
use crate::ssh_config::{
    IncludeChange, OfferAnswer, SshConfig, answer_first_use_offer, first_use_offer,
};
use crate::terminal::RawTerminal;
use clap::{CommandFactory as _, Parser, Subcommand, error::ErrorKind};
use gascan_proto::ssh_status::{SshState, classify as classify_ssh};
use gascan_proto::v1;
use std::ffi::{OsStr, OsString};
use std::io::{IsTerminal, Write};
use std::os::fd::AsFd;
use std::path::{Path, PathBuf};

const EXIT_USAGE: i32 = 64;
const EXIT_DAEMON: i32 = 69;
const EXIT_RUNTIME: i32 = 70;
const EXIT_API: i32 = 76;

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
    Usage { kind: UsageKind, message: String },
    Operation { code: String, message: String },
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
            Self::Operation { .. } | Self::Runtime(_) | Self::Io(_) => EXIT_RUNTIME,
        }
    }

    pub fn stable_code(&self) -> Option<&str> {
        match self {
            Self::Client(error) => error.stable_code(),
            Self::Operation { code, .. } => Some(code),
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
        match self {
            Self::Usage {
                kind: UsageKind::NoSandbox,
                ..
            } => Some("gascan up <project-root>"),
            Self::Usage {
                kind: UsageKind::MultipleSandboxes,
                ..
            } => Some("run `gascan list`, then pass `--sandbox <sandbox-id>`"),
            Self::Client(_) | Self::Operation { .. }
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
            | Self::Runtime(_)
            | Self::Io(_) => None,
        }
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

pub async fn execute() -> Result<i32, CliError> {
    let arguments = match Arguments::try_parse() {
        Ok(arguments) => arguments,
        Err(error) if error.kind() == ErrorKind::DisplayVersion => {
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
    if let Command::SshConfig { command } = arguments.command {
        return execute_ssh_config(command);
    }
    let mut client = Client::connect_or_start().await?;
    match arguments.command {
        Command::DaemonAttest => Ok(0),
        Command::Up { project_root, json } => {
            let project_root = resolve_project_root(&project_root)?;
            match client.api.up(v1::UpRequest { project_root }).await {
                Ok(response) => {
                    let result =
                        operation(response.into_inner(), json, OperationKind::Up, None).await;
                    preserve_up_result_with_optional_offer(
                        result,
                        json,
                        &mut std::io::stderr(),
                        try_offer_ssh_config_include,
                    )
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
            let doctor = client.api.doctor(v1::DoctorRequest {}).await?.into_inner();
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
    ["CI", "GITHUB_ACTIONS", "BUILD_BUILDID"]
        .into_iter()
        .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
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
    let event = events
        .message()
        .await?
        .ok_or_else(|| CliError::Runtime("daemon returned no session".to_owned()))?;
    if event.session_token.is_empty() {
        return Err(CliError::Runtime(
            "daemon returned an empty session token".to_owned(),
        ));
    }
    let _terminal = if shell {
        Some(RawTerminal::acquire()?)
    } else {
        None
    };
    let token = event.session_token;
    let (input_sender, input_receiver) = tokio::sync::mpsc::channel(16);
    if shell && stdin_is_tty {
        let producer = input_sender.clone();
        let producer_token = token.clone();
        let restore = _terminal.as_ref().map(RawTerminal::restore_handle);
        tokio::spawn(async move {
            forward_terminal_input(producer, producer_token, restore).await;
        });
    } else if !stdin_is_tty {
        let producer = input_sender.clone();
        let producer_token = token.clone();
        tokio::spawn(async move {
            forward_piped_input(producer, producer_token).await;
        });
    } else {
        input_sender
            .send(v1::ClientFrame {
                frame: Some(v1::client_frame::Frame::Close(v1::Close {})),
                session_token: token,
            })
            .await
            .map_err(|_| CliError::Runtime("attach input closed".to_owned()))?;
    }
    drop(input_sender);
    let mut attached = client
        .api
        .attach(tokio_stream::wrappers::ReceiverStream::new(input_receiver))
        .await?
        .into_inner();
    while let Some(frame) = attached.message().await? {
        match frame.frame {
            Some(v1::server_frame::Frame::Stdout(bytes)) => {
                std::io::stdout().write_all(&bytes)?;
                std::io::stdout().flush()?;
            }
            Some(v1::server_frame::Frame::Stderr(bytes)) => {
                std::io::stderr().write_all(&bytes)?;
                std::io::stderr().flush()?;
            }
            Some(v1::server_frame::Frame::Exit(exit)) => return Ok(exit.code),
            Some(v1::server_frame::Frame::Error(error)) => {
                return Err(attach_frame_error(error));
            }
            None => {}
        }
    }
    Err(CliError::Runtime(
        "attach ended without exit status".to_owned(),
    ))
}

fn attach_frame_error(error: v1::Error) -> CliError {
    CliError::Runtime(format!("{}: {}", error.code, error.message))
}

async fn forward_piped_input(sender: tokio::sync::mpsc::Sender<v1::ClientFrame>, token: Vec<u8>) {
    use tokio::io::AsyncReadExt as _;
    let mut stdin = tokio::io::stdin();
    let mut bytes = vec![0_u8; 16 * 1024];
    loop {
        let frame = match stdin.read(&mut bytes).await {
            Ok(0) | Err(_) => v1::client_frame::Frame::Close(v1::Close {}),
            Ok(count) => v1::client_frame::Frame::Stdin(bytes[..count].to_vec()),
        };
        let terminal = matches!(frame, v1::client_frame::Frame::Close(_));
        if sender
            .send(v1::ClientFrame {
                frame: Some(frame),
                session_token: token.clone(),
            })
            .await
            .is_err()
        {
            return;
        }
        if terminal {
            return;
        }
    }
}

async fn forward_terminal_input(
    sender: tokio::sync::mpsc::Sender<v1::ClientFrame>,
    token: Vec<u8>,
    restore: Option<crate::terminal::TerminalRestore>,
) {
    use tokio::io::AsyncReadExt;
    let size = rustix::termios::tcgetwinsize(std::io::stdin().as_fd()).ok();
    if let Some(size) = size {
        if sender
            .send(v1::ClientFrame {
                frame: Some(v1::client_frame::Frame::Resize(v1::Resize {
                    columns: u32::from(size.ws_col),
                    rows: u32::from(size.ws_row),
                })),
                session_token: token.clone(),
            })
            .await
            .is_err()
        {
            return;
        }
    }
    let mut interrupt =
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt()) {
            Ok(signal) => signal,
            Err(_) => return,
        };
    let mut terminate =
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(signal) => signal,
            Err(_) => return,
        };
    let mut resize =
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::window_change()) {
            Ok(signal) => signal,
            Err(_) => return,
        };
    let mut stdin = tokio::io::stdin();
    let mut bytes = vec![0_u8; 4096];
    loop {
        let frame = tokio::select! {
            read = stdin.read(&mut bytes) => match read { Ok(0) | Err(_) => v1::client_frame::Frame::Close(v1::Close {}), Ok(count) => v1::client_frame::Frame::Stdin(bytes[..count].to_vec()) },
            _ = interrupt.recv() => v1::client_frame::Frame::Signal(v1::Signal { number: 2 }),
            _ = terminate.recv() => v1::client_frame::Frame::Signal(v1::Signal { number: 15 }),
            _ = resize.recv() => {
                let size = rustix::termios::tcgetwinsize(std::io::stdin().as_fd()).ok();
                let Some(size) = size else { continue; };
                v1::client_frame::Frame::Resize(v1::Resize { columns: u32::from(size.ws_col), rows: u32::from(size.ws_row) })
            }
        };
        let terminal = matches!(
            frame,
            v1::client_frame::Frame::Close(_) | v1::client_frame::Frame::Signal(_)
        );
        if matches!(frame, v1::client_frame::Frame::Signal(_)) {
            if let Some(restore) = &restore {
                restore.restore();
            }
        }
        if sender
            .send(v1::ClientFrame {
                frame: Some(frame),
                session_token: token.clone(),
            })
            .await
            .is_err()
        {
            return;
        }
        if terminal {
            let _ = sender
                .send(v1::ClientFrame {
                    frame: Some(v1::client_frame::Frame::Close(v1::Close {})),
                    session_token: token.clone(),
                })
                .await;
            return;
        }
    }
}

fn allowed_environment() -> Vec<v1::EnvironmentVariable> {
    gascan_core::policy::filtered_host_environment(std::env::vars())
        .into_iter()
        .map(|(name, value)| v1::EnvironmentVariable { name, value })
        .collect()
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

    #[test]
    fn root_help_advertises_the_standard_version_flags() {
        let help = Arguments::command().render_help().to_string();
        assert!(
            help.contains("-V, --version"),
            "version option missing: {help}"
        );
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
        let error = attach_frame_error(v1::Error {
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
