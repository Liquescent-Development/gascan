use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    process::Stdio,
};

use gascan_core::runtime::RuntimeError;
use rustix::fs::{Access, AtFlags, CWD, accessat};
use serde::Serialize;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{ChildStdin, Command},
    sync::{mpsc, oneshot, watch},
};

use crate::{HelperInput, HelperOutput};

const OPERATION: &str = "gascan-apple-attach";
const SIGINT: i32 = 2;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AttachInput {
    Stdin(Vec<u8>),
    Resize { rows: u16, cols: u16 },
    Signal(i32),
    Close,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AttachOutput {
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
    Exit(i32),
}

#[derive(Clone, Debug)]
pub struct AppleAttach {
    helper: PathBuf,
    helper_args: Vec<String>,
}

impl Default for AppleAttach {
    fn default() -> Self {
        Self::new("gascan-apple-attach")
    }
}

impl AppleAttach {
    pub fn new(helper: impl AsRef<OsStr>) -> Self {
        Self {
            helper: PathBuf::from(helper.as_ref()),
            helper_args: Vec::new(),
        }
    }

    pub fn configured(helper_override: Option<OsString>) -> Result<Self, RuntimeError> {
        let Some(helper) = helper_override else {
            return Ok(Self::default());
        };
        if helper.is_empty() {
            return Err(configuration_error("must not be empty"));
        }
        let requested = PathBuf::from(helper);
        let canonical = requested.canonicalize().map_err(|error| {
            configuration_error(format!("cannot resolve {}: {error}", requested.display()))
        })?;
        let metadata = canonical.metadata().map_err(|error| {
            configuration_error(format!("cannot inspect {}: {error}", canonical.display()))
        })?;
        if !metadata.is_file() {
            return Err(configuration_error(format!(
                "{} is not a regular file",
                canonical.display()
            )));
        }
        accessat(CWD, &canonical, Access::EXEC_OK, AtFlags::EACCESS).map_err(|error| {
            configuration_error(format!(
                "{} is not executable by the effective user: {error}",
                canonical.display()
            ))
        })?;
        Ok(Self::new(canonical))
    }

    pub fn configured_from_environment() -> Result<Self, RuntimeError> {
        Self::configured(std::env::var_os("GASCAN_APPLE_ATTACH_HELPER"))
    }

    pub fn helper_path(&self) -> &Path {
        &self.helper
    }

    pub fn with_helper_args<I, A>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = A>,
        A: Into<String>,
    {
        self.helper_args = args.into_iter().map(Into::into).collect();
        self
    }

    pub async fn exec<I, A>(
        &self,
        container: impl AsRef<str>,
        argv: I,
        tty: bool,
    ) -> Result<AttachSession, RuntimeError>
    where
        I: IntoIterator<Item = A>,
        A: AsRef<str>,
    {
        self.exec_with_environment(container, argv, tty, BTreeMap::new())
            .await
    }

    pub async fn exec_with_environment<I, A>(
        &self,
        container: impl AsRef<str>,
        argv: I,
        tty: bool,
        environment: BTreeMap<String, String>,
    ) -> Result<AttachSession, RuntimeError>
    where
        I: IntoIterator<Item = A>,
        A: AsRef<str>,
    {
        let argv: Vec<String> = argv
            .into_iter()
            .map(|value| value.as_ref().to_owned())
            .collect();
        if argv.is_empty() {
            return Err(io_error("guest argv must not be empty"));
        }
        validate_environment(&environment)?;

        let mut command = Command::new(&self.helper);
        command
            .args(&self.helper_args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(if std::env::var_os("GASCAN_ATTACH_DIAGNOSTICS").is_some() {
                Stdio::inherit()
            } else {
                Stdio::null()
            })
            .kill_on_drop(true);
        let mut child = command.spawn().map_err(command_error)?;
        let mut stdin = child.stdin.take().ok_or_else(|| missing_pipe("stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| missing_pipe("stdout"))?;

        write_frame(
            &mut stdin,
            &HelperInput::start(container.as_ref().to_owned(), argv, tty, environment),
        )
        .await?;

        let (input_tx, input_rx) = mpsc::channel(16);
        let (output_tx, output_rx) = mpsc::channel(32);
        let (cancel_tx, mut cancel_rx) = watch::channel(false);
        tokio::spawn(write_inputs(input_rx, stdin));
        tokio::spawn(read_outputs(BufReader::new(stdout), output_tx));
        tokio::spawn(async move {
            tokio::select! {
                _ = child.wait() => {}
                result = cancel_rx.changed() => {
                    if result.is_ok() && *cancel_rx.borrow() {
                        let _ = child.kill().await;
                        let _ = child.wait().await;
                    }
                }
            }
        });

        Ok(AttachSession {
            input: input_tx,
            output: output_rx,
            exit: None,
            tty,
            _cleanup: CleanupGuard { cancel: cancel_tx },
        })
    }
}

pub struct AttachSession {
    input: mpsc::Sender<InputRequest>,
    output: mpsc::Receiver<Result<AttachOutput, RuntimeError>>,
    exit: Option<i32>,
    tty: bool,
    _cleanup: CleanupGuard,
}

impl AttachSession {
    pub async fn send(&self, input: AttachInput) -> Result<(), RuntimeError> {
        self.input_handle().send(input).await
    }

    pub(crate) fn input_handle(&self) -> AttachInputHandle {
        AttachInputHandle {
            input: self.input.clone(),
            tty: self.tty,
        }
    }

    pub async fn recv(&mut self) -> Result<Option<AttachOutput>, RuntimeError> {
        match self.output.recv().await {
            Some(Ok(AttachOutput::Exit(code))) => {
                self.exit = Some(code);
                Ok(Some(AttachOutput::Exit(code)))
            }
            Some(result) => result.map(Some),
            None => Ok(None),
        }
    }

    pub async fn read_until(&mut self, needle: &[u8]) -> Result<Option<Vec<u8>>, RuntimeError> {
        if needle.is_empty() {
            return Ok(Some(Vec::new()));
        }
        let mut bytes = Vec::new();
        while let Some(output) = self.recv().await? {
            match output {
                AttachOutput::Stdout(chunk) | AttachOutput::Stderr(chunk) => {
                    bytes.extend(chunk);
                    if bytes.windows(needle.len()).any(|window| window == needle) {
                        return Ok(Some(bytes));
                    }
                }
                AttachOutput::Exit(_) => return Ok(None),
            }
        }
        Ok(None)
    }

    pub async fn exit(&mut self) -> Result<i32, RuntimeError> {
        if let Some(code) = self.exit {
            return Ok(code);
        }
        while let Some(output) = self.recv().await? {
            if let AttachOutput::Exit(code) = output {
                return Ok(code);
            }
        }
        Err(io_error("helper closed without a terminal frame"))
    }
}

#[derive(Clone)]
pub(crate) struct AttachInputHandle {
    input: mpsc::Sender<InputRequest>,
    tty: bool,
}

impl AttachInputHandle {
    pub(crate) async fn send(&self, input: AttachInput) -> Result<(), RuntimeError> {
        if matches!(input, AttachInput::Resize { .. }) && !self.tty {
            return Err(RuntimeError::UnsupportedCapability {
                capability: "resize requires a TTY attachment".to_owned(),
            });
        }
        let input = match input {
            AttachInput::Signal(SIGINT) if self.tty => AttachInput::Stdin(vec![0x03]),
            AttachInput::Signal(signal) => {
                return Err(RuntimeError::UnsupportedCapability {
                    capability: format!(
                        "attachment signal {signal}; Apple ContainerAPIClient 1.1.0 supports only SIGINT for TTY attachments"
                    ),
                });
            }
            input => input,
        };
        let (reply, response) = oneshot::channel();
        self.input
            .send(InputRequest { input, reply })
            .await
            .map_err(|_| io_error("helper input is closed"))?;
        response
            .await
            .map_err(|_| io_error("helper input closed without acknowledging the frame"))?
    }
}

struct InputRequest {
    input: AttachInput,
    reply: oneshot::Sender<Result<(), RuntimeError>>,
}

struct CleanupGuard {
    cancel: watch::Sender<bool>,
}

impl Drop for CleanupGuard {
    fn drop(&mut self) {
        let _ = self.cancel.send(true);
    }
}

async fn write_inputs(mut requests: mpsc::Receiver<InputRequest>, mut stdin: ChildStdin) {
    while let Some(request) = requests.recv().await {
        let close = matches!(&request.input, AttachInput::Close);
        let result = write_frame(&mut stdin, &HelperInput::from(request.input)).await;
        let failed = result.is_err();
        let _ = request.reply.send(result);
        if close || failed {
            break;
        }
    }
}

async fn write_frame<T: Serialize>(stdin: &mut ChildStdin, frame: &T) -> Result<(), RuntimeError> {
    let mut encoded = serde_json::to_vec(frame).map_err(|error| RuntimeError::InvalidOutput {
        operation: OPERATION.to_owned(),
        message: format!("failed to encode helper frame: {error}"),
    })?;
    encoded.push(b'\n');
    stdin.write_all(&encoded).await.map_err(command_error)?;
    stdin.flush().await.map_err(command_error)
}

async fn read_outputs<R>(
    reader: BufReader<R>,
    output: mpsc::Sender<Result<AttachOutput, RuntimeError>>,
) where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut lines = reader.lines();
    let mut terminal = false;
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                if terminal {
                    let _ = output
                        .send(Err(protocol_error("frame received after terminal event")))
                        .await;
                    break;
                }
                let frame = match serde_json::from_str::<HelperOutput>(&line) {
                    Ok(frame) => frame,
                    Err(error) => {
                        let _ = output
                            .send(Err(protocol_error(format!(
                                "invalid helper frame: {error}"
                            ))))
                            .await;
                        break;
                    }
                };
                let is_terminal = matches!(
                    frame,
                    HelperOutput::Exit { .. } | HelperOutput::Error { .. }
                );
                match frame.into_attach_output() {
                    Ok(value) => {
                        terminal = is_terminal;
                        if output.send(Ok(value)).await.is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = output.send(Err(error)).await;
                        break;
                    }
                }
            }
            Ok(None) => {
                if !terminal {
                    let _ = output
                        .send(Err(protocol_error(
                            "helper closed without a terminal frame",
                        )))
                        .await;
                }
                break;
            }
            Err(error) => {
                let _ = output.send(Err(command_error(error))).await;
                break;
            }
        }
    }
}

fn command_error(error: std::io::Error) -> RuntimeError {
    RuntimeError::CommandIo {
        operation: OPERATION.to_owned(),
        message: error.to_string(),
    }
}

fn configuration_error(message: impl Into<String>) -> RuntimeError {
    RuntimeError::CommandIo {
        operation: "GASCAN_APPLE_ATTACH_HELPER".to_owned(),
        message: message.into(),
    }
}

fn protocol_error(message: impl Into<String>) -> RuntimeError {
    RuntimeError::InvalidOutput {
        operation: OPERATION.to_owned(),
        message: message.into(),
    }
}

fn io_error(message: impl Into<String>) -> RuntimeError {
    RuntimeError::CommandIo {
        operation: OPERATION.to_owned(),
        message: message.into(),
    }
}

fn validate_environment(environment: &BTreeMap<String, String>) -> Result<(), RuntimeError> {
    if environment.iter().any(|(name, value)| {
        !is_allowed_environment_name(name)
            || name.chars().any(char::is_control)
            || name.contains('=')
            || value.contains('\0')
    }) {
        return Err(io_error("invalid environment variable"));
    }
    Ok(())
}

fn is_allowed_environment_name(name: &str) -> bool {
    matches!(name, "TERM" | "COLORTERM" | "LANG")
        || name
            .strip_prefix("LC_")
            .is_some_and(|suffix| !suffix.is_empty())
}

fn missing_pipe(stream: &str) -> RuntimeError {
    io_error(format!("helper {stream} pipe is unavailable"))
}
