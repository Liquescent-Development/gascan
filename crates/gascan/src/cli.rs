use crate::client::{Client, ClientError};
use crate::terminal::RawTerminal;
use clap::{Parser, Subcommand};
use gascan_proto::v1;
use std::io::{IsTerminal, Read, Write};

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
    Up {
        project_root: String,
    },
    Apply {
        project_root: Option<String>,
    },
    Shell {
        #[arg(last = true)]
        argv: Vec<String>,
    },
    Run {
        #[arg(last = true, required = true)]
        argv: Vec<String>,
    },
    Down,
    Destroy {
        #[arg(long)]
        yes: bool,
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
    let mut client = Client::connect_or_start().await?;
    match arguments.command {
        Command::Up { project_root } => {
            operation(
                client
                    .api
                    .up(v1::UpRequest { project_root })
                    .await?
                    .into_inner(),
            )
            .await
        }
        Command::Apply { project_root } => {
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
            )
            .await
        }
        Command::Down => {
            let selector = selector(&mut client, arguments.sandbox).await?;
            operation(
                client
                    .api
                    .down(v1::DownRequest {
                        sandbox: Some(selector),
                    })
                    .await?
                    .into_inner(),
            )
            .await
        }
        Command::Destroy { yes } => {
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
            if json {
                println!(
                    "{}",
                    serde_json::json!({"capabilities": doctor.capabilities.len(), "findings": doctor.findings.len()})
                );
            } else {
                println!("Gas Can daemon: ready");
            }
            Ok(0)
        }
        Command::Run { argv } => run(&mut client, arguments.sandbox, argv, false).await,
        Command::Shell { argv } => run(&mut client, arguments.sandbox, argv, true).await,
        Command::Logs { follow } => logs(&mut client, arguments.sandbox, follow).await,
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

async fn operation(mut stream: tonic::Streaming<v1::OperationEvent>) -> Result<i32, CliError> {
    while let Some(event) = stream.message().await? {
        if let Some(error) = event.error {
            return Err(CliError::Runtime(format!(
                "{}: {}",
                error.code, error.message
            )));
        }
        if !event.phase.is_empty() {
            eprintln!("{}", event.phase);
        }
    }
    Ok(0)
}

async fn run(
    client: &mut Client,
    explicit: Option<String>,
    argv: Vec<String>,
    shell: bool,
) -> Result<i32, CliError> {
    let selector = selector(client, explicit).await?;
    let mut events = if shell {
        client
            .api
            .shell(v1::ShellRequest {
                sandbox: Some(selector),
                argv: argv.into_iter().map(String::into_bytes).collect(),
            })
            .await?
            .into_inner()
    } else {
        let mut stdin = Vec::new();
        if !std::io::stdin().is_terminal() {
            std::io::stdin().read_to_end(&mut stdin)?;
        }
        client
            .api
            .run(v1::RunRequest {
                sandbox: Some(selector),
                command: Some(v1::CommandPayload {
                    argv: argv.into_iter().map(String::into_bytes).collect(),
                    stdin,
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
    let frame = v1::ClientFrame {
        frame: Some(v1::client_frame::Frame::Close(v1::Close {})),
        session_token: event.session_token,
    };
    let mut attached = client
        .api
        .attach(tokio_stream::iter([frame]))
        .await?
        .into_inner();
    while let Some(frame) = attached.message().await? {
        match frame.frame {
            Some(v1::server_frame::Frame::Stdout(bytes)) => {
                std::io::stdout().write_all(&bytes)?;
            }
            Some(v1::server_frame::Frame::Stderr(bytes)) => {
                std::io::stderr().write_all(&bytes)?;
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

async fn logs(
    client: &mut Client,
    explicit: Option<String>,
    follow: bool,
) -> Result<i32, CliError> {
    let selector = selector(client, explicit).await?;
    let mut stream = client
        .api
        .logs(v1::LogsRequest {
            sandbox: Some(selector),
            since: None,
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
