use crate::client::{Client, ClientError};
use crate::terminal::RawTerminal;
use clap::{Parser, Subcommand};
use gascan_proto::v1;
use std::io::{IsTerminal, Write};
use std::os::fd::AsFd;

const EXIT_USAGE: i32 = 64;
const EXIT_DAEMON: i32 = 69;
const EXIT_RUNTIME: i32 = 70;
const EXIT_API: i32 = 76;

#[derive(Parser)]
#[command(name = "gascan", disable_help_subcommand = true)]
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
}

#[derive(Debug)]
pub enum CliError {
    Client(ClientError),
    Usage(String),
    Runtime(String),
    Io(std::io::Error),
}
impl CliError {
    pub const fn exit_code(&self) -> i32 {
        match self {
            Self::Usage(_) => EXIT_USAGE,
            Self::Client(ClientError::Api(_)) => EXIT_API,
            Self::Client(ClientError::Rpc(_)) => EXIT_RUNTIME,
            Self::Client(_) => EXIT_DAEMON,
            Self::Runtime(_) | Self::Io(_) => EXIT_RUNTIME,
        }
    }
}
impl std::fmt::Display for CliError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Client(e) => e.fmt(formatter),
            Self::Usage(e) | Self::Runtime(e) => formatter.write_str(e),
            Self::Io(e) => e.fmt(formatter),
        }
    }
}
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

pub async fn execute() -> Result<i32, CliError> {
    let arguments = Arguments::try_parse().map_err(|error| CliError::Usage(error.to_string()))?;
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
    let mut client = Client::connect_or_start().await?;
    match arguments.command {
        Command::DaemonAttest => Ok(0),
        Command::Up { project_root, json } => {
            operation(
                client
                    .api
                    .up(v1::UpRequest { project_root })
                    .await?
                    .into_inner(),
                json,
            )
            .await
        }
        Command::Apply { project_root, json } => {
            let root = match project_root {
                Some(root) => root,
                None => std::env::current_dir()?.to_string_lossy().into_owned(),
            };
            operation(
                client
                    .api
                    .apply(v1::ApplyRequest { project_root: root })
                    .await?
                    .into_inner(),
                json,
            )
            .await
        }
        Command::Down { json } => {
            let selector = selector(&mut client, arguments.sandbox).await?;
            operation(
                client
                    .api
                    .down(v1::DownRequest {
                        sandbox: Some(selector),
                    })
                    .await?
                    .into_inner(),
                json,
            )
            .await
        }
        Command::Destroy { yes, json } => {
            if !yes {
                confirm_destroy()?;
            }
            let selector = selector(&mut client, arguments.sandbox).await?;
            operation(
                client
                    .api
                    .destroy(v1::DestroyRequest {
                        sandbox: Some(selector),
                    })
                    .await?
                    .into_inner(),
                json,
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
                        .unwrap_or_else(|_| serde_json::json!({"detail": capability.detail, "remedy": ""}));
                    serde_json::json!({
                        "id": capability.name,
                        "status": detail.get("status").and_then(serde_json::Value::as_str).unwrap_or(if capability.available { "pass" } else { "fail" }),
                        "detail": detail.get("detail").and_then(serde_json::Value::as_str).unwrap_or(""),
                        "remedy": detail.get("remedy").and_then(serde_json::Value::as_str).unwrap_or(""),
                    })
                })
                .collect::<Vec<_>>();
            if json {
                println!("{}", serde_json::json!({"checks": checks}));
            } else {
                for check in &checks {
                    println!(
                        "{} {:<4} {}",
                        check["id"].as_str().unwrap_or("unknown"),
                        check["status"].as_str().unwrap_or("fail"),
                        check["detail"].as_str().unwrap_or("")
                    );
                    if check["status"] != "pass" {
                        println!("  remedy: {}", check["remedy"].as_str().unwrap_or(""));
                    }
                }
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
        [] => Err(CliError::Usage(
            "no sandbox is available; run `gascan up` first".to_owned(),
        )),
        _ => Err(CliError::Usage(
            "multiple sandboxes exist; pass --sandbox".to_owned(),
        )),
    }
}

fn event_phase_label(event: &v1::OperationEvent) -> Option<String> {
    if event.phase.is_empty() {
        return None;
    }
    if event.phase != "provision_step" {
        return Some(event.phase.clone());
    }
    let step = match v1::ProvisionStep::try_from(event.provision_step).ok()? {
        v1::ProvisionStep::WriteSafeMiseConfig => "write_safe_mise_config",
        v1::ProvisionStep::InstallTools => "install_tools",
        v1::ProvisionStep::RunSetup => "run_setup",
        v1::ProvisionStep::VerifyGascamp => "verify_gascamp",
        v1::ProvisionStep::HealthCheck => "health_check",
        v1::ProvisionStep::Unspecified => return Some(event.phase.clone()),
    };
    Some(format!("{}: {step}", event.phase))
}

async fn operation(
    mut stream: tonic::Streaming<v1::OperationEvent>,
    json: bool,
) -> Result<i32, CliError> {
    while let Some(event) = stream.message().await? {
        if json {
            println!(
                "{}",
                serde_json::json!({"operation_id":event.operation_id.map(|id|id.value),"sequence":event.sequence,"phase":event.phase,"status":event.status,"error":event.error.as_ref().map(|error|serde_json::json!({"code":error.code,"message":error.message}))})
            );
        }
        if let Some(error) = event.error {
            return Err(CliError::Runtime(format!(
                "{}: {}",
                error.code, error.message
            )));
        }
        if !json {
            if let Some(label) = event_phase_label(&event) {
                eprintln!("{label}");
            }
        }
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provisioning_phase_renders_only_the_stable_step_identifier() {
        let event = v1::OperationEvent {
            phase: "provision_step".to_owned(),
            provision_step: v1::ProvisionStep::WriteSafeMiseConfig as i32,
            payload: br#"{"step":"write_safe_mise_config","secret":"must-not-render"}"#.to_vec(),
            ..Default::default()
        };

        assert_eq!(
            event_phase_label(&event).as_deref(),
            Some("provision_step: write_safe_mise_config")
        );
        assert!(
            !event_phase_label(&event)
                .unwrap_or_default()
                .contains("secret")
        );
    }
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
                return Err(CliError::Runtime(format!(
                    "{}: {}",
                    error.code, error.message
                )));
            }
            None => {}
        }
    }
    Err(CliError::Runtime(
        "attach ended without exit status".to_owned(),
    ))
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
fn render_status(status: &v1::SandboxStatus, json: bool) -> Result<(), CliError> {
    if json {
        println!(
            "{}",
            serde_json::json!({"sandbox_id":status.sandbox_id,"actual_state":actual_name(status.actual_state)})
        );
    } else {
        println!("{} {}", status.sandbox_id, actual_name(status.actual_state));
    }
    Ok(())
}
fn render_list(sandboxes: &[v1::SandboxStatus], json: bool) -> Result<(), CliError> {
    if json {
        let values = sandboxes.iter().map(|s| serde_json::json!({"sandbox_id":s.sandbox_id,"actual_state":actual_name(s.actual_state)})).collect::<Vec<_>>();
        println!(
            "{}",
            serde_json::to_string(&values).map_err(|e| CliError::Runtime(e.to_string()))?
        );
    } else {
        for sandbox in sandboxes {
            println!(
                "{} {}",
                sandbox.sandbox_id,
                actual_name(sandbox.actual_state)
            );
        }
    }
    Ok(())
}
fn confirm_destroy() -> Result<(), CliError> {
    if !std::io::stdin().is_terminal() {
        return Err(CliError::Usage(
            "destroy requires --yes when stdin is not a TTY".to_owned(),
        ));
    }
    eprint!("Destroy sandbox? [y/N] ");
    std::io::stderr().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    if answer.trim().eq_ignore_ascii_case("y") {
        Ok(())
    } else {
        Err(CliError::Usage("destroy cancelled".to_owned()))
    }
}
