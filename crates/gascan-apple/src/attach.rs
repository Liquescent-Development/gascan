use std::{
    io::{Read, Write},
    process::Stdio,
};

use gascan_core::runtime::RuntimeError;
use nix::{
    sys::signal::{Signal, kill},
    unistd::Pid,
};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::Command,
    sync::{mpsc, oneshot},
};

const OPERATION: &str = "container exec";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachInput {
    Stdin(Vec<u8>),
    Resize { rows: u16, cols: u16 },
    Signal(i32),
    Close,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachOutput {
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
    Exit(i32),
}

#[derive(Clone, Debug)]
pub struct AppleAttach {
    program: String,
}

impl Default for AppleAttach {
    fn default() -> Self {
        Self::new("container")
    }
}

impl AppleAttach {
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
        }
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
        let mut args = vec!["exec".to_owned(), "-i".to_owned()];
        if tty {
            args.push("-t".to_owned());
        }
        args.push(container.as_ref().to_owned());
        args.extend(argv.into_iter().map(|arg| arg.as_ref().to_owned()));
        if args.len() == if tty { 4 } else { 3 } {
            return Err(RuntimeError::CommandIo {
                operation: OPERATION.to_owned(),
                message: "guest argv must not be empty".to_owned(),
            });
        }

        if tty {
            self.exec_tty(args)
        } else {
            self.exec_piped(args).await
        }
    }

    async fn exec_piped(&self, args: Vec<String>) -> Result<AttachSession, RuntimeError> {
        let mut child = Command::new(&self.program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(io_error)?;
        let pid = child.id().ok_or_else(|| RuntimeError::CommandIo {
            operation: OPERATION.to_owned(),
            message: "spawned Apple CLI has no process id".to_owned(),
        })?;
        let stdin = child.stdin.take().ok_or_else(missing_stdin)?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| missing_stream("stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| missing_stream("stderr"))?;
        let (input_tx, input_rx) = mpsc::channel(16);
        let (output_tx, output_rx) = mpsc::channel(32);

        tokio::spawn(piped_input(input_rx, stdin, pid));
        let stdout_task = tokio::spawn(pump(stdout, output_tx.clone(), StreamKind::Stdout));
        let stderr_task = tokio::spawn(pump(stderr, output_tx.clone(), StreamKind::Stderr));
        tokio::spawn(async move {
            let result = child.wait().await.map_err(io_error).and_then(exact_status);
            let _ = stdout_task.await;
            let _ = stderr_task.await;
            let _ = output_tx.send(result.map(AttachOutput::Exit)).await;
        });
        Ok(AttachSession::new(
            input_tx,
            output_rx,
            false,
            SessionGuard::pid(pid),
        ))
    }

    fn exec_tty(&self, args: Vec<String>) -> Result<AttachSession, RuntimeError> {
        let pair = native_pty_system()
            .openpty(PtySize::default())
            .map_err(pty_error)?;
        let mut command = CommandBuilder::new(&self.program);
        command.args(args);
        let mut child = pair.slave.spawn_command(command).map_err(pty_error)?;
        let pid = child.process_id().ok_or_else(|| RuntimeError::CommandIo {
            operation: OPERATION.to_owned(),
            message: "PTY Apple CLI child has no process id".to_owned(),
        })?;
        let killer = child.clone_killer();
        let session_killer = child.clone_killer();
        let reader = pair.master.try_clone_reader().map_err(pty_error)?;
        let writer = pair.master.take_writer().map_err(pty_error)?;
        drop(pair.slave);
        let (input_tx, input_rx) = mpsc::channel(16);
        let (output_tx, output_rx) = mpsc::channel(32);

        tokio::task::spawn_blocking(move || tty_input(input_rx, writer, pair.master, killer, pid));
        let reader_tx = output_tx.clone();
        tokio::task::spawn_blocking(move || pump_tty(reader, reader_tx));
        tokio::task::spawn_blocking(move || {
            let result = child.wait().map_err(io_error).and_then(|status| {
                i32::try_from(status.exit_code()).map_err(|error| RuntimeError::InvalidOutput {
                    operation: OPERATION.to_owned(),
                    message: format!("exit code is outside i32: {error}"),
                })
            });
            let _ = output_tx.blocking_send(result.map(AttachOutput::Exit));
        });
        Ok(AttachSession::new(
            input_tx,
            output_rx,
            true,
            SessionGuard::pty(session_killer),
        ))
    }
}

pub struct AttachSession {
    input: mpsc::Sender<InputRequest>,
    output: mpsc::Receiver<Result<AttachOutput, RuntimeError>>,
    exit: Option<i32>,
    tty: bool,
    guard: Option<SessionGuard>,
}

impl AttachSession {
    fn new(
        input: mpsc::Sender<InputRequest>,
        output: mpsc::Receiver<Result<AttachOutput, RuntimeError>>,
        tty: bool,
        guard: SessionGuard,
    ) -> Self {
        Self {
            input,
            output,
            exit: None,
            tty,
            guard: Some(guard),
        }
    }

    pub async fn send(&self, input: AttachInput) -> Result<(), RuntimeError> {
        if let AttachInput::Signal(number) = &input {
            validated_signal(*number)?;
        }
        if matches!(input, AttachInput::Resize { .. }) && !self.tty {
            return Err(RuntimeError::UnsupportedCapability {
                capability: "resize requires a TTY attachment".to_owned(),
            });
        }
        let (reply, response) = oneshot::channel();
        self.input
            .send(InputRequest { input, reply })
            .await
            .map_err(|_| RuntimeError::CommandIo {
                operation: OPERATION.to_owned(),
                message: "attachment input is closed".to_owned(),
            })?;
        response.await.map_err(|_| RuntimeError::CommandIo {
            operation: OPERATION.to_owned(),
            message: "attachment input closed without acknowledging the operation".to_owned(),
        })?
    }

    pub async fn recv(&mut self) -> Result<Option<AttachOutput>, RuntimeError> {
        match self.output.recv().await {
            Some(Ok(AttachOutput::Exit(code))) => {
                self.exit = Some(code);
                if let Some(guard) = self.guard.as_mut() {
                    guard.disarm();
                }
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
        while let Some(message) = self.recv().await? {
            match message {
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
        while let Some(message) = self.recv().await? {
            if let AttachOutput::Exit(code) = message {
                return Ok(code);
            }
        }
        Err(RuntimeError::CommandIo {
            operation: OPERATION.to_owned(),
            message: "attachment closed without an exit event".to_owned(),
        })
    }
}

enum StreamKind {
    Stdout,
    Stderr,
}

struct InputRequest {
    input: AttachInput,
    reply: oneshot::Sender<Result<(), RuntimeError>>,
}

enum SessionTarget {
    Pid(u32),
    Pty(Box<dyn portable_pty::ChildKiller + Send + Sync>),
}

struct SessionGuard {
    target: SessionTarget,
    armed: bool,
}

impl SessionGuard {
    fn pid(pid: u32) -> Self {
        Self {
            target: SessionTarget::Pid(pid),
            armed: true,
        }
    }

    fn pty(killer: Box<dyn portable_pty::ChildKiller + Send + Sync>) -> Self {
        Self {
            target: SessionTarget::Pty(killer),
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        match &mut self.target {
            SessionTarget::Pid(pid) => {
                let _ = kill_owned(*pid, Signal::SIGKILL);
            }
            SessionTarget::Pty(killer) => {
                let _ = killer.kill();
            }
        }
    }
}

async fn pump<R: AsyncRead + Unpin>(
    mut reader: R,
    tx: mpsc::Sender<Result<AttachOutput, RuntimeError>>,
    kind: StreamKind,
) {
    let mut buffer = vec![0; 8192];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) => break,
            Ok(count) => {
                let bytes = buffer[..count].to_vec();
                let output = match kind {
                    StreamKind::Stdout => AttachOutput::Stdout(bytes),
                    StreamKind::Stderr => AttachOutput::Stderr(bytes),
                };
                if tx.send(Ok(output)).await.is_err() {
                    break;
                }
            }
            Err(error) => {
                let _ = tx.send(Err(io_error(error))).await;
                break;
            }
        }
    }
}

async fn piped_input(
    mut rx: mpsc::Receiver<InputRequest>,
    mut stdin: tokio::process::ChildStdin,
    pid: u32,
) {
    let mut closed = false;
    while let Some(request) = rx.recv().await {
        let (result, close) = match request.input {
            AttachInput::Stdin(bytes) => (stdin.write_all(&bytes).await.map_err(io_error), false),
            AttachInput::Signal(signal) => (send_signal(pid, signal), false),
            AttachInput::Close => (Ok(()), true),
            AttachInput::Resize { .. } => (
                Err(RuntimeError::UnsupportedCapability {
                    capability: "resize requires a TTY attachment".to_owned(),
                }),
                false,
            ),
        };
        let failed = result.is_err();
        let _ = request.reply.send(result);
        if close {
            closed = true;
            break;
        }
        if failed {
            break;
        }
    }
    if !closed {
        let _ = kill_owned(pid, Signal::SIGKILL);
    }
}

fn tty_input(
    mut rx: mpsc::Receiver<InputRequest>,
    mut writer: Box<dyn Write + Send>,
    master: Box<dyn portable_pty::MasterPty + Send>,
    mut killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,
    pid: u32,
) {
    let mut closed = false;
    while let Some(request) = rx.blocking_recv() {
        let close = matches!(&request.input, AttachInput::Close);
        let result = match request.input {
            AttachInput::Stdin(bytes) => writer.write_all(&bytes).map_err(io_error),
            AttachInput::Resize { rows, cols } => master
                .resize(PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .map_err(pty_error),
            AttachInput::Signal(signal) => send_signal(pid, signal),
            AttachInput::Close => Ok(()),
        };
        let failed = result.is_err();
        let _ = request.reply.send(result);
        if close {
            closed = true;
            break;
        }
        if failed {
            break;
        }
    }
    if !closed {
        let _ = killer.kill();
    }
}

fn pump_tty(
    mut reader: Box<dyn Read + Send>,
    tx: mpsc::Sender<Result<AttachOutput, RuntimeError>>,
) {
    let mut buffer = vec![0; 8192];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => {
                if tx
                    .blocking_send(Ok(AttachOutput::Stdout(buffer[..count].to_vec())))
                    .is_err()
                {
                    break;
                }
            }
            Err(error) => {
                let _ = tx.blocking_send(Err(io_error(error)));
                break;
            }
        }
    }
}

fn send_signal(pid: u32, number: i32) -> Result<(), RuntimeError> {
    let signal = validated_signal(number)?;
    kill_owned(pid, signal)
}

fn validated_signal(number: i32) -> Result<Signal, RuntimeError> {
    let signal = Signal::try_from(number).map_err(|_| unsupported_signal(number))?;
    if signal != Signal::SIGINT && signal != Signal::SIGTERM {
        return Err(unsupported_signal(number));
    }
    Ok(signal)
}

fn kill_owned(pid: u32, signal: Signal) -> Result<(), RuntimeError> {
    let pid = i32::try_from(pid).map_err(|error| RuntimeError::InvalidOutput {
        operation: OPERATION.to_owned(),
        message: format!("process id is outside i32: {error}"),
    })?;
    kill(Pid::from_raw(pid), signal).map_err(|error| RuntimeError::CommandIo {
        operation: OPERATION.to_owned(),
        message: format!("failed to signal Apple CLI child: {error}"),
    })
}

fn exact_status(status: std::process::ExitStatus) -> Result<i32, RuntimeError> {
    status.code().ok_or_else(|| RuntimeError::CommandFailed {
        operation: OPERATION.to_owned(),
        exit_code: None,
        stderr: "Apple CLI terminated by signal".to_owned(),
    })
}
fn io_error(error: std::io::Error) -> RuntimeError {
    RuntimeError::CommandIo {
        operation: OPERATION.to_owned(),
        message: error.to_string(),
    }
}
fn pty_error(error: anyhow::Error) -> RuntimeError {
    RuntimeError::CommandIo {
        operation: OPERATION.to_owned(),
        message: error.to_string(),
    }
}
fn missing_stdin() -> RuntimeError {
    missing_stream("stdin")
}
fn missing_stream(stream: &str) -> RuntimeError {
    RuntimeError::CommandIo {
        operation: OPERATION.to_owned(),
        message: format!("piped {stream} is unavailable"),
    }
}
fn unsupported_signal(number: i32) -> RuntimeError {
    RuntimeError::UnsupportedCapability {
        capability: format!("attachment signal {number}; only SIGINT and SIGTERM are supported"),
    }
}
