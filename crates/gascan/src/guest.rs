use crate::cli::CliError;
use crate::client::Client;
use crate::terminal::RawTerminal;
use gascan_proto::v1;
use std::fmt;
use std::io::IsTerminal as _;
use std::os::fd::AsFd as _;
use std::os::unix::ffi::OsStrExt as _;
use std::path::Path;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio_stream::Stream;

const PIPED_INPUT_FRAME_BYTES: usize = 16 * 1024;
const TERMINAL_INPUT_FRAME_BYTES: usize = 4096;
const MAX_CAPTURE_BYTES: usize = 1024 * 1024;

struct ZeroizingVec(Vec<u8>);

impl Drop for ZeroizingVec {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

pub(crate) struct SensitiveBytes {
    storage: Box<[u8]>,
    len: usize,
    #[cfg(test)]
    drop_observation: Option<(SensitiveDropObserver, SensitiveDropKind)>,
}

impl SensitiveBytes {
    pub(crate) fn zeroed(capacity: usize) -> Self {
        Self {
            storage: vec![0; capacity].into_boxed_slice(),
            len: 0,
            #[cfg(test)]
            drop_observation: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn observe_drop(
        &mut self,
        observer: SensitiveDropObserver,
        kind: SensitiveDropKind,
    ) {
        self.drop_observation = Some((observer, kind));
    }

    pub(crate) fn storage_mut(&mut self) -> &mut [u8] {
        &mut self.storage
    }

    pub(crate) fn storage(&self) -> &[u8] {
        &self.storage
    }

    pub(crate) fn append_bounded(&mut self, bytes: &[u8]) -> bool {
        let retained = bytes.len().min(self.storage.len().saturating_sub(self.len));
        self.storage[self.len..self.len + retained].copy_from_slice(&bytes[..retained]);
        self.len += retained;
        retained < bytes.len()
    }

    pub(crate) fn clear_storage(&mut self) {
        self.storage.fill(0);
    }

    pub(crate) fn trim_one_line_ending(&mut self) {
        if self.len > 0 && self.storage[self.len - 1] == b'\n' {
            self.len -= 1;
            self.storage[self.len] = 0;
            if self.len > 0 && self.storage[self.len - 1] == b'\r' {
                self.len -= 1;
                self.storage[self.len] = 0;
            }
        } else if self.len > 0 && self.storage[self.len - 1] == b'\r' {
            self.len -= 1;
            self.storage[self.len] = 0;
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub(crate) fn expose(&self) -> &[u8] {
        &self.storage[..self.len]
    }
}

impl fmt::Debug for SensitiveBytes {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SensitiveBytes([REDACTED])")
    }
}

impl Drop for SensitiveBytes {
    fn drop(&mut self) {
        self.storage.fill(0);
        #[cfg(test)]
        if let Some((observer, kind)) = &self.drop_observation {
            observer.record(SensitiveDropEvent {
                kind: *kind,
                zeroized: self.storage.iter().all(|byte| *byte == 0),
            });
        }
    }
}

enum SecretStorage {
    Ordinary(ZeroizingVec),
    Sensitive(SensitiveBytes),
}

pub(crate) struct Secret(SecretStorage);

impl Secret {
    pub(crate) fn new(bytes: Vec<u8>) -> Self {
        Self(SecretStorage::Ordinary(ZeroizingVec(bytes)))
    }

    pub(crate) fn from_sensitive(bytes: SensitiveBytes) -> Self {
        Self(SecretStorage::Sensitive(bytes))
    }

    pub(crate) fn expose(&self) -> &[u8] {
        match &self.0 {
            SecretStorage::Ordinary(bytes) => &bytes.0,
            SecretStorage::Sensitive(bytes) => bytes.expose(),
        }
    }

    pub(crate) fn redaction_copy(&self) -> SensitiveBytes {
        let bytes = self.expose();
        let mut copy = SensitiveBytes::zeroed(bytes.len());
        let exceeded = copy.append_bounded(bytes);
        debug_assert!(!exceeded);
        copy
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Secret([REDACTED])")
    }
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SensitiveDropKind {
    StdoutScratch,
    StderrScratch,
    StdoutAccumulation,
    StderrAccumulation,
    RedactionCopy,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SensitiveDropEvent {
    pub(crate) kind: SensitiveDropKind,
    pub(crate) zeroized: bool,
}

#[cfg(test)]
#[derive(Clone, Default)]
pub(crate) struct SensitiveDropObserver(std::sync::Arc<std::sync::Mutex<Vec<SensitiveDropEvent>>>);

#[cfg(test)]
impl SensitiveDropObserver {
    fn record(&self, event: SensitiveDropEvent) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(event);
    }

    pub(crate) fn events(&self) -> Vec<SensitiveDropEvent> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

#[derive(Debug)]
pub(crate) struct GuestCommand {
    pub(crate) argv: Vec<Vec<u8>>,
    pub(crate) environment: Vec<v1::EnvironmentVariable>,
    pub(crate) stdin: Option<Secret>,
}

#[derive(Eq, PartialEq)]
pub(crate) struct GuestOutput {
    pub(crate) code: i32,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
}

#[tonic::async_trait]
pub(crate) trait GuestRunner {
    async fn execute(
        &mut self,
        selector: v1::SandboxSelector,
        command: GuestCommand,
    ) -> Result<GuestOutput, CliError>;

    async fn execute_interactive(
        &mut self,
        selector: v1::SandboxSelector,
        argv: Vec<Vec<u8>>,
    ) -> Result<i32, CliError>;
}

pub(crate) struct ClientGuestRunner<'a> {
    client: &'a mut Client,
}

impl<'a> ClientGuestRunner<'a> {
    pub(crate) fn new(client: &'a mut Client) -> Self {
        Self { client }
    }
}

#[tonic::async_trait]
impl GuestRunner for ClientGuestRunner<'_> {
    async fn execute(
        &mut self,
        selector: v1::SandboxSelector,
        command: GuestCommand,
    ) -> Result<GuestOutput, CliError> {
        let GuestCommand {
            argv,
            environment,
            stdin,
        } = command;
        let token = start_run(self.client, selector, argv, environment, false).await?;
        execute_bounded(self.client, token, stdin).await
    }

    async fn execute_interactive(
        &mut self,
        selector: v1::SandboxSelector,
        argv: Vec<Vec<u8>>,
    ) -> Result<i32, CliError> {
        let stdin_is_tty = std::io::stdin().is_terminal();
        let token = start_run(self.client, selector, argv, allowed_environment(), true).await?;
        attach_to_stdio(self.client, token, true, stdin_is_tty).await
    }
}

async fn start_run(
    client: &mut Client,
    selector: v1::SandboxSelector,
    argv: Vec<Vec<u8>>,
    environment: Vec<v1::EnvironmentVariable>,
    tty: bool,
) -> Result<Vec<u8>, CliError> {
    let mut events = client
        .api
        .run(v1::RunRequest {
            sandbox: Some(selector),
            command: Some(v1::CommandPayload {
                argv,
                environment,
                tty,
            }),
        })
        .await?
        .into_inner();
    first_session_token(&mut events).await
}

pub(crate) async fn first_session_token(
    events: &mut tonic::Streaming<v1::OperationEvent>,
) -> Result<Vec<u8>, CliError> {
    let event = events
        .message()
        .await?
        .ok_or_else(|| CliError::Runtime("daemon returned no session".to_owned()))?;
    if event.session_token.is_empty() {
        return Err(CliError::Runtime(
            "daemon returned an empty session token".to_owned(),
        ));
    }
    Ok(event.session_token)
}

pub(crate) fn attach_frame_error(error: v1::Error) -> CliError {
    CliError::Runtime(format!("{}: {}", error.code, error.message))
}

pub(crate) fn allowed_environment() -> Vec<v1::EnvironmentVariable> {
    gascan_core::policy::filtered_host_environment(std::env::vars())
        .into_iter()
        .map(|(name, value)| v1::EnvironmentVariable { name, value })
        .collect()
}

struct SecretInput {
    token: Vec<u8>,
    stdin: Option<Secret>,
    offset: usize,
    empty_sent: bool,
    close_sent: bool,
}

impl SecretInput {
    fn new(token: Vec<u8>, stdin: Option<Secret>) -> Self {
        Self {
            token,
            stdin,
            offset: 0,
            empty_sent: false,
            close_sent: false,
        }
    }

    fn frame(&self, frame: v1::client_frame::Frame) -> v1::ClientFrame {
        v1::ClientFrame {
            frame: Some(frame),
            session_token: self.token.clone(),
        }
    }
}

impl Stream for SecretInput {
    type Item = v1::ClientFrame;

    fn poll_next(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if this.close_sent {
            return Poll::Ready(None);
        }
        if let Some(stdin) = &this.stdin {
            let stdin_bytes = stdin.expose();
            if stdin_bytes.is_empty() && !this.empty_sent {
                this.empty_sent = true;
                return Poll::Ready(Some(this.frame(v1::client_frame::Frame::Stdin(Vec::new()))));
            }
            if this.offset < stdin_bytes.len() {
                let end = this
                    .offset
                    .saturating_add(PIPED_INPUT_FRAME_BYTES)
                    .min(stdin_bytes.len());
                let bytes = stdin_bytes[this.offset..end].to_vec();
                this.offset = end;
                return Poll::Ready(Some(this.frame(v1::client_frame::Frame::Stdin(bytes))));
            }
            this.stdin = None;
        }
        this.close_sent = true;
        Poll::Ready(Some(
            this.frame(v1::client_frame::Frame::Close(v1::Close {})),
        ))
    }
}

async fn execute_bounded(
    client: &mut Client,
    token: Vec<u8>,
    stdin: Option<Secret>,
) -> Result<GuestOutput, CliError> {
    let input = SecretInput::new(token, stdin);
    let mut attached = client.api.attach(input).await?.into_inner();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    while let Some(frame) = attached.message().await? {
        match frame.frame {
            Some(v1::server_frame::Frame::Stdout(bytes)) => {
                append_bounded("stdout", &mut stdout, bytes)?;
            }
            Some(v1::server_frame::Frame::Stderr(bytes)) => {
                append_bounded("stderr", &mut stderr, bytes)?;
            }
            Some(v1::server_frame::Frame::Exit(exit)) => {
                return Ok(GuestOutput {
                    code: exit.code,
                    stdout,
                    stderr,
                });
            }
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

fn append_bounded(
    stream_name: &'static str,
    destination: &mut Vec<u8>,
    bytes: Vec<u8>,
) -> Result<(), CliError> {
    if bytes.len() > MAX_CAPTURE_BYTES.saturating_sub(destination.len()) {
        return Err(CliError::Runtime(format!(
            "guest {stream_name} exceeded the {MAX_CAPTURE_BYTES}-byte capture limit"
        )));
    }
    destination.extend(bytes);
    Ok(())
}

struct RestoringFd {
    fd: std::os::fd::OwnedFd,
    original_flags: rustix::fs::OFlags,
}

impl std::os::fd::AsFd for RestoringFd {
    fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

impl std::os::fd::AsRawFd for RestoringFd {
    fn as_raw_fd(&self) -> std::os::fd::RawFd {
        self.fd.as_raw_fd()
    }
}

impl Drop for RestoringFd {
    fn drop(&mut self) {
        let _ = rustix::fs::fcntl_setfl(&self.fd, self.original_flags);
    }
}

struct CancellableInput {
    fd: tokio::io::unix::AsyncFd<RestoringFd>,
}

impl CancellableInput {
    fn stdin() -> std::io::Result<Self> {
        Self::from_fd(std::io::stdin())
    }

    fn terminal() -> std::io::Result<Self> {
        let stdin = std::io::stdin();
        let name = rustix::termios::ttyname(stdin.as_fd(), Vec::new())?;
        Self::from_terminal_path(Path::new(std::ffi::OsStr::from_bytes(name.to_bytes())))
    }

    fn from_terminal_path(path: &Path) -> std::io::Result<Self> {
        let flags =
            rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::NONBLOCK | rustix::fs::OFlags::CLOEXEC;
        let fd = rustix::fs::open(path, flags, rustix::fs::Mode::empty())?;
        let original_flags = rustix::fs::fcntl_getfl(&fd)?;
        let fd = RestoringFd { fd, original_flags };
        Ok(Self {
            fd: tokio::io::unix::AsyncFd::new(fd)?,
        })
    }

    fn from_fd(fd: impl std::os::fd::AsFd) -> std::io::Result<Self> {
        let fd = rustix::io::dup(fd)?;
        let original_flags = rustix::fs::fcntl_getfl(&fd)?;
        let fd = RestoringFd { fd, original_flags };
        rustix::fs::fcntl_setfl(&fd, original_flags | rustix::fs::OFlags::NONBLOCK)?;
        Ok(Self {
            fd: tokio::io::unix::AsyncFd::new(fd)?,
        })
    }

    async fn read(&self, bytes: &mut [u8]) -> std::io::Result<usize> {
        loop {
            let mut ready = self.fd.readable().await?;
            match ready.try_io(|fd| {
                rustix::io::read(fd.get_ref(), &mut *bytes).map_err(std::io::Error::from)
            }) {
                Ok(result) => return result,
                Err(_would_block) => {}
            }
        }
    }
}

enum HostInput {
    Cancellable(CancellableInput),
    Fallback(tokio::io::Stdin),
}

impl HostInput {
    fn stdin() -> Self {
        match CancellableInput::stdin() {
            Ok(stdin) => Self::Cancellable(stdin),
            Err(_) => Self::Fallback(tokio::io::stdin()),
        }
    }

    async fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Cancellable(stdin) => stdin.read(bytes).await,
            Self::Fallback(stdin) => {
                use tokio::io::AsyncReadExt as _;
                stdin.read(bytes).await
            }
        }
    }
}

async fn drive_input_until_attach<T>(
    mut producer: Pin<Box<dyn std::future::Future<Output = ()> + Send>>,
    attach: impl std::future::Future<Output = T>,
) -> T {
    tokio::pin!(attach);
    tokio::select! {
        result = &mut attach => result,
        () = producer.as_mut() => attach.await,
    }
}

async fn write_host_output(fd: impl std::os::fd::AsFd, bytes: &[u8]) -> std::io::Result<()> {
    let fd = rustix::io::dup(fd)?;
    let mut offset = 0;
    while offset < bytes.len() {
        match rustix::io::write(&fd, &bytes[offset..]).map_err(std::io::Error::from) {
            Ok(0) => return Err(std::io::ErrorKind::WriteZero.into()),
            Ok(count) => offset += count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(error) => return Err(error),
        }
    }
    if offset == bytes.len() {
        return Ok(());
    }
    let fd = tokio::io::unix::AsyncFd::new(fd)?;
    while offset < bytes.len() {
        let mut writable = fd.writable().await?;
        match writable.try_io(|fd| {
            rustix::io::write(fd.get_ref(), &bytes[offset..]).map_err(std::io::Error::from)
        }) {
            Ok(Ok(0)) => return Err(std::io::ErrorKind::WriteZero.into()),
            Ok(Ok(count)) => offset += count,
            Ok(Err(error)) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Ok(Err(error)) => return Err(error),
            Err(_) => continue,
        }
    }
    Ok(())
}

pub(crate) async fn attach_to_stdio(
    client: &mut Client,
    token: Vec<u8>,
    raw_terminal: bool,
    stdin_is_tty: bool,
) -> Result<i32, CliError> {
    let terminal = if raw_terminal {
        Some(RawTerminal::acquire()?)
    } else {
        None
    };
    let terminal_input = if raw_terminal && stdin_is_tty {
        Some(CancellableInput::terminal()?)
    } else {
        None
    };
    let (input_sender, input_receiver) = tokio::sync::mpsc::channel(16);
    let producer: Option<Pin<Box<dyn std::future::Future<Output = ()> + Send>>> =
        if let Some(stdin) = terminal_input {
            let producer = input_sender.clone();
            let producer_token = token.clone();
            let restore = terminal.as_ref().map(RawTerminal::restore_handle);
            Some(Box::pin(async move {
                forward_terminal_input(stdin, producer, producer_token, restore).await;
            }))
        } else if !stdin_is_tty {
            let producer = input_sender.clone();
            let producer_token = token.clone();
            Some(Box::pin(async move {
                forward_piped_input(producer, producer_token).await;
            }))
        } else {
            input_sender
                .send(v1::ClientFrame {
                    frame: Some(v1::client_frame::Frame::Close(v1::Close {})),
                    session_token: token,
                })
                .await
                .map_err(|_| CliError::Runtime("attach input closed".to_owned()))?;
            None
        };
    drop(input_sender);
    let attach = async {
        let mut attached = client
            .api
            .attach(tokio_stream::wrappers::ReceiverStream::new(input_receiver))
            .await?
            .into_inner();
        while let Some(frame) = attached.message().await? {
            match frame.frame {
                Some(v1::server_frame::Frame::Stdout(bytes)) => {
                    write_host_output(std::io::stdout(), &bytes).await?;
                }
                Some(v1::server_frame::Frame::Stderr(bytes)) => {
                    write_host_output(std::io::stderr(), &bytes).await?;
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
    };
    if let Some(producer) = producer {
        drive_input_until_attach(producer, attach).await
    } else {
        attach.await
    }
}

async fn forward_piped_input(sender: tokio::sync::mpsc::Sender<v1::ClientFrame>, token: Vec<u8>) {
    let mut stdin = HostInput::stdin();
    let mut bytes = vec![0_u8; PIPED_INPUT_FRAME_BYTES];
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
    stdin: CancellableInput,
    sender: tokio::sync::mpsc::Sender<v1::ClientFrame>,
    token: Vec<u8>,
    restore: Option<crate::terminal::TerminalRestore>,
) {
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
    let mut bytes = vec![0_u8; TERMINAL_INPUT_FRAME_BYTES];
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

#[cfg(test)]
mod tests {
    use super::{
        CancellableInput, ClientGuestRunner, GuestCommand, GuestRunner, Secret,
        drive_input_until_attach, write_host_output,
    };
    use crate::client::Client;
    use gascan_proto::v1;
    use gascan_proto::v1::gas_can_server::{GasCan, GasCanServer};
    use hyper_util::rt::TokioIo;
    use std::pin::Pin;
    use std::sync::Arc;
    use tokio::net::{UnixListener, UnixStream};
    use tokio::sync::{Mutex, oneshot};
    use tokio_stream::Stream;
    use tokio_stream::wrappers::UnixListenerStream;
    use tonic::transport::{Channel, Endpoint, Server};
    use tonic::{Code, Request, Response, Status};
    use tower::service_fn;

    const SENTINEL: &str = "gascan-test-secret-7d9f3a";
    const CAPTURE_LIMIT: usize = 1024 * 1024;
    type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;
    type OperationStream =
        Pin<Box<dyn Stream<Item = Result<v1::OperationEvent, Status>> + Send + 'static>>;
    type FrameStream =
        Pin<Box<dyn Stream<Item = Result<v1::ServerFrame, Status>> + Send + 'static>>;

    #[tokio::test]
    async fn host_output_waits_for_nonblocking_capacity_without_losing_bytes() -> TestResult {
        let (reader, writer) = rustix::pipe::pipe()?;
        let flags = rustix::fs::fcntl_getfl(&writer)?;
        rustix::fs::fcntl_setfl(&writer, flags | rustix::fs::OFlags::NONBLOCK)?;
        let fill = vec![b'f'; 4096];
        let mut filled = 0usize;
        loop {
            match rustix::io::write(&writer, &fill) {
                Ok(count) => filled += count,
                Err(rustix::io::Errno::AGAIN) => break,
                Err(error) => return Err(error.into()),
            }
        }
        let expected = vec![b'x'; 8192];
        let task_writer = rustix::io::dup(&writer)?;
        let task_payload = expected.clone();
        let task = tokio::spawn(async move { write_host_output(task_writer, &task_payload).await });
        tokio::task::yield_now().await;
        assert!(!task.is_finished());

        let received = tokio::task::spawn_blocking(move || -> std::io::Result<Vec<u8>> {
            let mut received = vec![0; filled + expected.len()];
            let mut offset = 0;
            while offset < received.len() {
                let count = rustix::io::read(&reader, &mut received[offset..])?;
                if count == 0 {
                    return Err(std::io::ErrorKind::UnexpectedEof.into());
                }
                offset += count;
            }
            Ok(received)
        });
        task.await??;
        let received = received.await??;
        assert!(received[filled..].iter().all(|byte| *byte == b'x'));
        Ok(())
    }

    #[derive(Clone)]
    struct RpcFailure {
        code: Code,
        message: &'static str,
    }

    #[derive(Clone)]
    struct Scenario {
        run_event: Option<v1::OperationEvent>,
        run_rpc_failure: Option<RpcFailure>,
        run_stream_failure: Option<RpcFailure>,
        attach_frames: Vec<v1::ServerFrame>,
        attach_rpc_failure: Option<RpcFailure>,
        attach_stream_failure: Option<RpcFailure>,
        consume_attach_input: bool,
    }

    impl Default for Scenario {
        fn default() -> Self {
            Self {
                run_event: Some(v1::OperationEvent {
                    session_token: b"opaque-session-token".to_vec(),
                    ..Default::default()
                }),
                run_rpc_failure: None,
                run_stream_failure: None,
                attach_frames: vec![exit_frame(0)],
                attach_rpc_failure: None,
                attach_stream_failure: None,
                consume_attach_input: true,
            }
        }
    }

    #[derive(Clone, Default)]
    struct Captures {
        run_requests: Vec<v1::RunRequest>,
        attach_frames: Vec<v1::ClientFrame>,
    }

    #[derive(Clone)]
    struct FakeDaemon {
        scenario: Scenario,
        captures: Arc<Mutex<Captures>>,
    }

    #[tonic::async_trait]
    impl GasCan for FakeDaemon {
        async fn handshake(
            &self,
            _request: Request<v1::HandshakeRequest>,
        ) -> Result<Response<v1::HandshakeResponse>, Status> {
            Err(Status::unimplemented("handshake"))
        }

        async fn status(
            &self,
            _request: Request<v1::StatusRequest>,
        ) -> Result<Response<v1::StatusResponse>, Status> {
            Err(Status::unimplemented("status"))
        }

        async fn list(
            &self,
            _request: Request<v1::ListRequest>,
        ) -> Result<Response<v1::ListResponse>, Status> {
            Err(Status::unimplemented("list"))
        }

        async fn doctor(
            &self,
            _request: Request<v1::DoctorRequest>,
        ) -> Result<Response<v1::DoctorResponse>, Status> {
            Err(Status::unimplemented("doctor"))
        }

        async fn daemon_status(
            &self,
            _request: Request<v1::DaemonStatusRequest>,
        ) -> Result<Response<v1::DaemonStatusResponse>, Status> {
            Err(Status::unimplemented("daemon_status"))
        }

        async fn shutdown_daemon(
            &self,
            _request: Request<v1::ShutdownDaemonRequest>,
        ) -> Result<Response<v1::ShutdownDaemonResponse>, Status> {
            Err(Status::unimplemented("shutdown_daemon"))
        }

        type UpStream = OperationStream;
        async fn up(
            &self,
            _request: Request<v1::UpRequest>,
        ) -> Result<Response<Self::UpStream>, Status> {
            Err(Status::unimplemented("up"))
        }

        type ApplyStream = OperationStream;
        async fn apply(
            &self,
            _request: Request<v1::ApplyRequest>,
        ) -> Result<Response<Self::ApplyStream>, Status> {
            Err(Status::unimplemented("apply"))
        }

        type RunStream = OperationStream;
        async fn run(
            &self,
            request: Request<v1::RunRequest>,
        ) -> Result<Response<Self::RunStream>, Status> {
            self.captures
                .lock()
                .await
                .run_requests
                .push(request.into_inner());
            if let Some(failure) = &self.scenario.run_rpc_failure {
                return Err(Status::new(failure.code, failure.message));
            }
            let mut events = Vec::new();
            if let Some(event) = &self.scenario.run_event {
                events.push(Ok(event.clone()));
            }
            if let Some(failure) = &self.scenario.run_stream_failure {
                events.push(Err(Status::new(failure.code, failure.message)));
            }
            Ok(Response::new(Box::pin(tokio_stream::iter(events))))
        }

        type ShellStream = OperationStream;
        async fn shell(
            &self,
            _request: Request<v1::ShellRequest>,
        ) -> Result<Response<Self::ShellStream>, Status> {
            Err(Status::unimplemented("shell"))
        }

        type DownStream = OperationStream;
        async fn down(
            &self,
            _request: Request<v1::DownRequest>,
        ) -> Result<Response<Self::DownStream>, Status> {
            Err(Status::unimplemented("down"))
        }

        type DestroyStream = OperationStream;
        async fn destroy(
            &self,
            _request: Request<v1::DestroyRequest>,
        ) -> Result<Response<Self::DestroyStream>, Status> {
            Err(Status::unimplemented("destroy"))
        }

        type LogsStream = OperationStream;
        async fn logs(
            &self,
            _request: Request<v1::LogsRequest>,
        ) -> Result<Response<Self::LogsStream>, Status> {
            Err(Status::unimplemented("logs"))
        }

        type AttachStream = FrameStream;
        async fn attach(
            &self,
            request: Request<tonic::Streaming<v1::ClientFrame>>,
        ) -> Result<Response<Self::AttachStream>, Status> {
            if let Some(failure) = &self.scenario.attach_rpc_failure {
                return Err(Status::new(failure.code, failure.message));
            }
            if self.scenario.consume_attach_input {
                let mut input = request.into_inner();
                let mut frames = Vec::new();
                while let Some(frame) = input.message().await? {
                    frames.push(frame);
                }
                self.captures.lock().await.attach_frames.extend(frames);
            }
            let mut frames: Vec<Result<v1::ServerFrame, Status>> = self
                .scenario
                .attach_frames
                .iter()
                .cloned()
                .map(Ok)
                .collect();
            if let Some(failure) = &self.scenario.attach_stream_failure {
                frames.push(Err(Status::new(failure.code, failure.message)));
            }
            Ok(Response::new(Box::pin(tokio_stream::iter(frames))))
        }
    }

    struct Harness {
        client: Client,
        captures: Arc<Mutex<Captures>>,
        shutdown: Option<oneshot::Sender<()>>,
        server: tokio::task::JoinHandle<Result<(), tonic::transport::Error>>,
        _directory: tempfile::TempDir,
    }

    impl Harness {
        async fn start(scenario: Scenario) -> TestResult<Self> {
            let directory = tempfile::tempdir()
                .map_err(|error| std::io::Error::other(format!("tempdir failed: {error}")))?;
            let socket = directory.path().join("fake-gascand.sock");
            let listener = UnixListener::bind(&socket)
                .map_err(|error| std::io::Error::other(format!("bind failed: {error}")))?;
            let captures = Arc::new(Mutex::new(Captures::default()));
            let daemon = FakeDaemon {
                scenario,
                captures: Arc::clone(&captures),
            };
            let (shutdown_sender, shutdown_receiver) = oneshot::channel();
            let server = tokio::spawn(async move {
                Server::builder()
                    .add_service(GasCanServer::new(daemon))
                    .serve_with_incoming_shutdown(UnixListenerStream::new(listener), async move {
                        let _ = shutdown_receiver.await;
                    })
                    .await
            });
            let connector_socket = socket.clone();
            let channel: Channel = Endpoint::from_static("http://[::]:50051")
                .connect_with_connector(service_fn(move |_| {
                    let socket = connector_socket.clone();
                    async move { UnixStream::connect(socket).await.map(TokioIo::new) }
                }))
                .await
                .map_err(|error| std::io::Error::other(format!("connect failed: {error}")))?;
            Ok(Self {
                client: Client {
                    api: v1::gas_can_client::GasCanClient::new(channel),
                },
                captures,
                shutdown: Some(shutdown_sender),
                server,
                _directory: directory,
            })
        }

        async fn finish(mut self) -> TestResult<Captures> {
            let captures = self.captures.lock().await.clone();
            if let Some(shutdown) = self.shutdown.take() {
                let _ = shutdown.send(());
            }
            self.server.await??;
            Ok(captures)
        }
    }

    fn selector() -> v1::SandboxSelector {
        v1::SandboxSelector {
            sandbox_id: "sandbox-under-test".to_owned(),
        }
    }

    fn command(stdin: Option<Secret>) -> GuestCommand {
        GuestCommand {
            argv: vec![
                b"printf".to_vec(),
                vec![0xff, 0x00, b'a'],
                b"two words".to_vec(),
            ],
            environment: vec![v1::EnvironmentVariable {
                name: "GASCAN_TEST_ENV".to_owned(),
                value: "exact value".to_owned(),
            }],
            stdin,
        }
    }

    fn stdout_frame(bytes: Vec<u8>) -> v1::ServerFrame {
        v1::ServerFrame {
            frame: Some(v1::server_frame::Frame::Stdout(bytes)),
        }
    }

    fn stderr_frame(bytes: Vec<u8>) -> v1::ServerFrame {
        v1::ServerFrame {
            frame: Some(v1::server_frame::Frame::Stderr(bytes)),
        }
    }

    fn exit_frame(code: i32) -> v1::ServerFrame {
        v1::ServerFrame {
            frame: Some(v1::server_frame::Frame::Exit(v1::Exit { code, signal: 0 })),
        }
    }

    fn assert_secret_absent(error: &crate::cli::CliError) {
        assert!(!error.to_string().contains(SENTINEL));
        assert!(!format!("{error:?}").contains(SENTINEL));
    }

    #[tokio::test]
    async fn interactive_input_does_not_make_shared_pty_output_nonblocking() -> TestResult {
        use std::os::unix::ffi::OsStrExt as _;
        let pty = rustix_openpty::openpty(None, None)?;
        let name = rustix::termios::ttyname(&pty.user, Vec::new())?;
        let path = std::path::Path::new(std::ffi::OsStr::from_bytes(name.to_bytes()));
        let output = rustix::io::dup(&pty.user)?;
        let before = rustix::fs::fcntl_getfl(&output)?;

        let _input = CancellableInput::from_terminal_path(path)?;

        assert_eq!(rustix::fs::fcntl_getfl(&output)?, before);
        assert!(!before.contains(rustix::fs::OFlags::NONBLOCK));
        Ok(())
    }

    #[tokio::test]
    async fn cancelling_scoped_input_restores_flags_and_leaves_pty_bytes_unclaimed() -> TestResult {
        let pty = rustix_openpty::openpty(None, None)?;
        let mut raw = rustix::termios::tcgetattr(&pty.user)?;
        raw.make_raw();
        rustix::termios::tcsetattr(&pty.user, rustix::termios::OptionalActions::Now, &raw)?;
        let original_flags = rustix::fs::fcntl_getfl(&pty.user)?;
        let input = CancellableInput::from_fd(&pty.user)?;
        let producer: Pin<Box<dyn std::future::Future<Output = ()> + Send>> =
            Box::pin(async move {
                let mut bytes = [0_u8; 1];
                let _ = input.read(&mut bytes).await;
            });
        let read = tokio::spawn(drive_input_until_attach(
            producer,
            std::future::pending::<()>(),
        ));
        tokio::task::yield_now().await;
        read.abort();
        let cancellation = read.await.err().ok_or("input read was not cancelled")?;
        assert!(cancellation.is_cancelled());
        assert_eq!(rustix::fs::fcntl_getfl(&pty.user)?, original_flags);

        assert_eq!(rustix::io::write(&pty.controller, b"x")?, 1);
        let mut bytes = [0_u8; 1];
        assert_eq!(rustix::io::read(&pty.user, &mut bytes)?, 1);
        assert_eq!(bytes, [b'x']);
        Ok(())
    }

    #[tokio::test]
    async fn execute_preserves_exact_argv_environment_and_output_channels() -> TestResult {
        let scenario = Scenario {
            attach_frames: vec![
                stdout_frame(vec![0x00, 0xff, b'o']),
                stderr_frame(vec![b'e', 0x00, 0xfe]),
                exit_frame(0),
            ],
            ..Scenario::default()
        };
        let mut harness = Harness::start(scenario).await?;
        let expected = command(None);
        let expected_argv = expected.argv.clone();
        let expected_environment = expected.environment.clone();
        let output = ClientGuestRunner::new(&mut harness.client)
            .execute(selector(), expected)
            .await?;
        assert_eq!(output.code, 0);
        assert_eq!(output.stdout, vec![0x00, 0xff, b'o']);
        assert_eq!(output.stderr, vec![b'e', 0x00, 0xfe]);

        let captures = harness.finish().await?;
        let request = captures.run_requests.first().ok_or("run request missing")?;
        assert_eq!(request.sandbox, Some(selector()));
        let payload = request.command.as_ref().ok_or("command payload missing")?;
        assert_eq!(payload.argv, expected_argv);
        assert_eq!(payload.environment, expected_environment);
        assert!(!payload.tty);
        assert_eq!(captures.attach_frames.len(), 1);
        assert!(matches!(
            captures.attach_frames[0].frame,
            Some(v1::client_frame::Frame::Close(_))
        ));
        assert_eq!(
            captures.attach_frames[0].session_token,
            b"opaque-session-token"
        );
        Ok(())
    }

    #[tokio::test]
    async fn execute_distinguishes_absent_and_zero_length_stdin() -> TestResult {
        let mut harness = Harness::start(Scenario::default()).await?;
        let output = ClientGuestRunner::new(&mut harness.client)
            .execute(selector(), command(Some(Secret::new(Vec::new()))))
            .await?;
        assert_eq!(output.code, 0);
        let captures = harness.finish().await?;
        assert_eq!(captures.attach_frames.len(), 2);
        assert!(matches!(
            captures.attach_frames[0].frame,
            Some(v1::client_frame::Frame::Stdin(ref bytes)) if bytes.is_empty()
        ));
        assert!(matches!(
            captures.attach_frames[1].frame,
            Some(v1::client_frame::Frame::Close(_))
        ));
        Ok(())
    }

    #[tokio::test]
    async fn execute_streams_large_secret_in_order_and_redacts_debug() -> TestResult {
        assert_eq!(
            format!("{:?}", Secret::new(SENTINEL.as_bytes().to_vec())),
            "Secret([REDACTED])"
        );
        let mut secret = SENTINEL.as_bytes().to_vec();
        secret.extend((0_u8..=255).cycle().take(40 * 1024));
        let expected = secret.clone();
        let guest_command = command(Some(Secret::new(secret)));
        let debug = format!("{guest_command:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains(SENTINEL));

        let mut harness = Harness::start(Scenario::default()).await?;
        ClientGuestRunner::new(&mut harness.client)
            .execute(selector(), guest_command)
            .await?;
        let captures = harness.finish().await?;
        assert!(captures.attach_frames.len() > 2);
        let token = b"opaque-session-token";
        assert!(
            captures
                .attach_frames
                .iter()
                .all(|frame| frame.session_token == token)
        );
        assert!(
            captures.attach_frames[..captures.attach_frames.len() - 1]
                .iter()
                .all(|frame| matches!(
                    frame.frame.as_ref(),
                    Some(v1::client_frame::Frame::Stdin(_))
                ))
        );
        let received: Vec<u8> = captures
            .attach_frames
            .iter()
            .filter_map(|frame| match &frame.frame {
                Some(v1::client_frame::Frame::Stdin(bytes)) => Some(bytes.as_slice()),
                _ => None,
            })
            .flatten()
            .copied()
            .collect();
        assert_eq!(received, expected);
        assert!(matches!(
            captures
                .attach_frames
                .last()
                .and_then(|frame| frame.frame.as_ref()),
            Some(v1::client_frame::Frame::Close(_))
        ));
        Ok(())
    }

    #[tokio::test]
    async fn execute_returns_nonzero_exit_as_output() -> TestResult {
        let scenario = Scenario {
            attach_frames: vec![stderr_frame(b"failed normally".to_vec()), exit_frame(23)],
            ..Scenario::default()
        };
        let mut harness = Harness::start(scenario).await?;
        let output = ClientGuestRunner::new(&mut harness.client)
            .execute(selector(), command(None))
            .await?;
        assert_eq!(output.code, 23);
        assert_eq!(output.stderr, b"failed normally");
        harness.finish().await?;
        Ok(())
    }

    #[tokio::test]
    async fn execute_reports_server_frame_error_without_secret() -> TestResult {
        let scenario = Scenario {
            attach_frames: vec![v1::ServerFrame {
                frame: Some(v1::server_frame::Frame::Error(v1::Error {
                    code: "guest_failed".to_owned(),
                    message: "guest command failed".to_owned(),
                    details: Vec::new(),
                })),
            }],
            ..Scenario::default()
        };
        let mut harness = Harness::start(scenario).await?;
        let error = ClientGuestRunner::new(&mut harness.client)
            .execute(
                selector(),
                command(Some(Secret::new(SENTINEL.as_bytes().to_vec()))),
            )
            .await
            .err()
            .ok_or("server frame error unexpectedly succeeded")?;
        assert_eq!(error.to_string(), "guest_failed: guest command failed");
        assert_secret_absent(&error);
        harness.finish().await?;
        Ok(())
    }

    #[tokio::test]
    async fn execute_rejects_missing_and_empty_session_tokens() -> TestResult {
        for (run_event, expected_error) in [
            (None, "daemon returned no session"),
            (
                Some(v1::OperationEvent {
                    session_token: Vec::new(),
                    ..Default::default()
                }),
                "daemon returned an empty session token",
            ),
        ] {
            let scenario = Scenario {
                run_event,
                ..Scenario::default()
            };
            let mut harness = Harness::start(scenario).await?;
            let error = ClientGuestRunner::new(&mut harness.client)
                .execute(
                    selector(),
                    command(Some(Secret::new(SENTINEL.as_bytes().to_vec()))),
                )
                .await
                .err()
                .ok_or("missing token unexpectedly succeeded")?;
            assert_eq!(error.to_string(), expected_error);
            assert_secret_absent(&error);
            harness.finish().await?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn execute_rejects_attach_without_exit_status() -> TestResult {
        let scenario = Scenario {
            attach_frames: vec![stdout_frame(b"partial".to_vec())],
            ..Scenario::default()
        };
        let mut harness = Harness::start(scenario).await?;
        let error = ClientGuestRunner::new(&mut harness.client)
            .execute(
                selector(),
                command(Some(Secret::new(SENTINEL.as_bytes().to_vec()))),
            )
            .await
            .err()
            .ok_or("missing exit unexpectedly succeeded")?;
        assert_eq!(error.to_string(), "attach ended without exit status");
        assert_secret_absent(&error);
        harness.finish().await?;
        Ok(())
    }

    #[tokio::test]
    async fn execute_bounds_stdout_and_stderr_independently() -> TestResult {
        let scenario = Scenario {
            attach_frames: vec![
                stdout_frame(vec![b'o'; CAPTURE_LIMIT]),
                stderr_frame(vec![b'e'; CAPTURE_LIMIT]),
                exit_frame(0),
            ],
            ..Scenario::default()
        };
        let mut harness = Harness::start(scenario).await?;
        let output = ClientGuestRunner::new(&mut harness.client)
            .execute(selector(), command(None))
            .await?;
        assert_eq!(output.stdout.len(), CAPTURE_LIMIT);
        assert_eq!(output.stderr.len(), CAPTURE_LIMIT);
        harness.finish().await?;

        for (stream_name, frames) in [
            (
                "stdout",
                vec![
                    stdout_frame(vec![b'o'; CAPTURE_LIMIT]),
                    stdout_frame(vec![b'o']),
                    exit_frame(0),
                ],
            ),
            (
                "stderr",
                vec![
                    stderr_frame(vec![b'e'; CAPTURE_LIMIT]),
                    stderr_frame(vec![b'e']),
                    exit_frame(0),
                ],
            ),
        ] {
            let scenario = Scenario {
                attach_frames: frames,
                ..Scenario::default()
            };
            let mut harness = Harness::start(scenario).await?;
            let error = ClientGuestRunner::new(&mut harness.client)
                .execute(
                    selector(),
                    command(Some(Secret::new(SENTINEL.as_bytes().to_vec()))),
                )
                .await
                .err()
                .ok_or("oversized output unexpectedly succeeded")?;
            assert!(error.to_string().contains(stream_name));
            assert_secret_absent(&error);
            harness.finish().await?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn execute_redacts_secret_across_rpc_and_stream_failures() -> TestResult {
        let failures = [
            Scenario {
                run_rpc_failure: Some(RpcFailure {
                    code: Code::Unavailable,
                    message: "run unavailable",
                }),
                ..Scenario::default()
            },
            Scenario {
                run_stream_failure: Some(RpcFailure {
                    code: Code::Internal,
                    message: "run stream failed",
                }),
                run_event: None,
                ..Scenario::default()
            },
            Scenario {
                attach_rpc_failure: Some(RpcFailure {
                    code: Code::Unavailable,
                    message: "attach unavailable",
                }),
                ..Scenario::default()
            },
            Scenario {
                attach_frames: Vec::new(),
                attach_stream_failure: Some(RpcFailure {
                    code: Code::Internal,
                    message: "attach stream failed",
                }),
                ..Scenario::default()
            },
        ];
        for scenario in failures {
            let mut harness = Harness::start(scenario).await?;
            let error = ClientGuestRunner::new(&mut harness.client)
                .execute(
                    selector(),
                    command(Some(Secret::new(SENTINEL.as_bytes().to_vec()))),
                )
                .await
                .err()
                .ok_or("RPC failure unexpectedly succeeded")?;
            assert_secret_absent(&error);
            harness.finish().await?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn execute_interactive_delegates_exact_argv_to_tty_run() -> TestResult {
        let scenario = Scenario {
            attach_frames: vec![exit_frame(17)],
            consume_attach_input: false,
            ..Scenario::default()
        };
        let mut harness = Harness::start(scenario).await?;
        let argv = vec![b"sh".to_vec(), b"-lc".to_vec(), vec![0xff, b'x']];
        let code = ClientGuestRunner::new(&mut harness.client)
            .execute_interactive(selector(), argv.clone())
            .await?;
        assert_eq!(code, 17);
        let captures = harness.finish().await?;
        let request = captures.run_requests.first().ok_or("run request missing")?;
        let payload = request.command.as_ref().ok_or("command payload missing")?;
        assert_eq!(payload.argv, argv);
        assert!(payload.tty);
        Ok(())
    }
}
