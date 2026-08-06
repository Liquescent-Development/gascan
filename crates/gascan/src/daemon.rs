#![allow(
    dead_code,
    reason = "Task 5 management entry points are consumed by the Task 6 CLI commands"
)]

use rustix::fd::OwnedFd;
use rustix::fs::{AtFlags, FileType, FlockOperation, Mode, OFlags};
use serde::{Deserialize, Serialize};
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, Read as _};
use std::os::unix::fs::FileExt as _;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

const DIRECTORY_MODE: u16 = 0o700;
const FILE_MODE: u16 = 0o600;
const INSTANCE_TOMBSTONE_MODE: u16 = 0o200;
const SOCKET_NAME: &str = "gascand.sock";
const INSTANCE_NAME: &str = "daemon-instance.json";
const LIFECYCLE_LOCK_NAME: &str = "daemon-lifecycle.lock";
const STARTUP_DIAGNOSTIC_NAME: &str = "daemon-startup-error.json";
const MAX_INSTANCE_BYTES: u64 = 64 * 1024;
const LIFECYCLE_LOCK_TIMEOUT: Duration = Duration::from_secs(60);
const ENDPOINT_CHANGED_DURING_PROBE: &str =
    "daemon endpoint pathname changed during the successful probe";
#[cfg(target_os = "macos")]
const PROCESS_INSPECTION_TIMEOUT: Duration = Duration::from_millis(500);
const STARTUP_DIAGNOSTIC_PREFIX: &str = "GASCAN_CONTROLLER_STARTUP_ERROR ";
const MAX_STARTUP_DIAGNOSTIC_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DaemonPaths {
    directory: PathBuf,
    socket: PathBuf,
    instance: PathBuf,
    lifecycle_lock: PathBuf,
    expected_uid: u32,
}

impl DaemonPaths {
    pub(crate) fn for_user() -> io::Result<Self> {
        let uid = rustix::process::geteuid().as_raw();
        let runtime = std::env::var_os("XDG_RUNTIME_DIR");
        let directory = runtime.map_or_else(
            || default_runtime_base().join(format!("gascan-{uid}")),
            |root| PathBuf::from(root).join("gascan"),
        );
        let mut paths = Self::from_runtime_root_with_uid(directory, uid);
        if let Some(instance) = std::env::var_os("GASCAN_DAEMON_INSTANCE_PATH") {
            paths.instance = instance.into();
        }
        Ok(paths)
    }

    pub(crate) fn from_runtime_root(directory: PathBuf) -> Self {
        Self::from_runtime_root_with_uid(directory, rustix::process::geteuid().as_raw())
    }

    fn from_runtime_root_with_uid(directory: PathBuf, expected_uid: u32) -> Self {
        Self {
            socket: directory.join(SOCKET_NAME),
            instance: directory.join(INSTANCE_NAME),
            lifecycle_lock: directory.join(LIFECYCLE_LOCK_NAME),
            directory,
            expected_uid,
        }
    }

    pub(crate) fn directory(&self) -> &Path {
        &self.directory
    }

    pub(crate) fn socket(&self) -> &Path {
        &self.socket
    }

    pub(crate) fn instance(&self) -> &Path {
        &self.instance
    }

    pub(crate) fn lifecycle_lock(&self) -> &Path {
        &self.lifecycle_lock
    }

    pub(crate) fn prepare_directory(&self) -> io::Result<()> {
        open_private_directory(&self.directory, self.expected_uid).map(drop)
    }

    #[cfg(test)]
    pub(crate) fn lock(&self) -> io::Result<LifecycleLock> {
        self.lock_with_timeout(LIFECYCLE_LOCK_TIMEOUT)
    }

    #[cfg(test)]
    fn lock_with_timeout(&self, timeout: Duration) -> io::Result<LifecycleLock> {
        let directory = open_private_directory(&self.directory, self.expected_uid)?;
        let fd = open_lock(&directory, self.expected_uid)?;
        let deadline = Instant::now() + timeout;
        loop {
            match rustix::fs::flock(&fd, FlockOperation::NonBlockingLockExclusive) {
                Ok(()) => break,
                Err(error)
                    if error == rustix::io::Errno::AGAIN
                        || error == rustix::io::Errno::WOULDBLOCK =>
                {
                    if Instant::now() >= deadline {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "timed out acquiring daemon lifecycle lock",
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => return Err(errno(error)),
            }
        }
        validate_open_file(
            &directory,
            OsStr::new(LIFECYCLE_LOCK_NAME),
            &fd,
            self.expected_uid,
        )?;
        Ok(LifecycleLock { _fd: fd })
    }

    async fn lock_async(&self) -> io::Result<LifecycleLock> {
        self.lock_async_with_timeout(LIFECYCLE_LOCK_TIMEOUT).await
    }

    async fn lock_async_with_timeout(&self, timeout: Duration) -> io::Result<LifecycleLock> {
        let directory = open_private_directory(&self.directory, self.expected_uid)?;
        let fd = open_lock(&directory, self.expected_uid)?;
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            match rustix::fs::flock(&fd, FlockOperation::NonBlockingLockExclusive) {
                Ok(()) => break,
                Err(error)
                    if error == rustix::io::Errno::AGAIN
                        || error == rustix::io::Errno::WOULDBLOCK =>
                {
                    if tokio::time::Instant::now() >= deadline {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "timed out acquiring daemon lifecycle lock",
                        ));
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                Err(error) => return Err(errno(error)),
            }
        }
        validate_open_file(
            &directory,
            OsStr::new(LIFECYCLE_LOCK_NAME),
            &fd,
            self.expected_uid,
        )?;
        Ok(LifecycleLock { _fd: fd })
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn default_runtime_base() -> PathBuf {
    PathBuf::from("/private/tmp")
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn default_runtime_base() -> PathBuf {
    PathBuf::from("/tmp")
}

#[derive(Debug)]
pub(crate) struct LifecycleLock {
    _fd: OwnedFd,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct InstanceTimestamp {
    pub(crate) seconds: i64,
    pub(crate) nanos: i32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct DaemonInstanceRecord {
    pub(crate) pid: u32,
    pub(crate) owner_token: String,
    pub(crate) executable: PathBuf,
    pub(crate) start_identity: String,
    pub(crate) instance_token: String,
    pub(crate) release_version: String,
    pub(crate) started_at: InstanceTimestamp,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProcessIdentity {
    pub(crate) pid: u32,
    pub(crate) executable: PathBuf,
    pub(crate) start_identity: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DaemonIdentity {
    pub(crate) pid: u32,
    pub(crate) executable: PathBuf,
    pub(crate) start_identity: String,
    pub(crate) instance_token: String,
    pub(crate) release_version: Option<String>,
    pub(crate) started_at: Option<InstanceTimestamp>,
}

impl From<&DaemonInstanceRecord> for DaemonIdentity {
    fn from(record: &DaemonInstanceRecord) -> Self {
        Self {
            pid: record.pid,
            executable: record.executable.clone(),
            start_identity: record.start_identity.clone(),
            instance_token: record.instance_token.clone(),
            release_version: Some(record.release_version.clone()),
            started_at: Some(record.started_at.clone()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DaemonState {
    Stopped,
    Current,
    Outdated,
    Unhealthy,
    Unreachable,
    Unsafe,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DaemonStatus {
    pub(crate) state: DaemonState,
    pub(crate) identity: Option<DaemonIdentity>,
    pub(crate) legacy: bool,
    pub(crate) detail: Option<String>,
}

impl DaemonStatus {
    fn new(state: DaemonState) -> Self {
        Self {
            state,
            identity: None,
            legacy: false,
            detail: None,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct EndpointSession<C> {
    pub(crate) connection: C,
    pub(crate) identity: DaemonIdentity,
    pub(crate) compatible_api: bool,
    pub(crate) safe_transport: bool,
    pub(crate) healthy: bool,
}

#[derive(Clone, Debug)]
pub(crate) enum EndpointProbe<C> {
    AbsentOrInert,
    Unresponsive(String),
    Connected(EndpointSession<C>),
    Unsafe(String),
}

#[tonic::async_trait]
pub(crate) trait DaemonEndpoint: Send + Sync {
    type Connection: Send;

    async fn probe(
        &self,
        paths: &DaemonPaths,
        expected_path: EndpointPathState,
    ) -> Result<EndpointProbe<Self::Connection>, crate::client::ClientError>;

    async fn graceful_shutdown(
        &self,
        connection: &mut Self::Connection,
        instance_token: &str,
    ) -> Result<(), crate::client::ClientError>;
}

#[derive(Debug)]
pub(crate) enum SupervisorError {
    Client(crate::client::ClientError),
    Io(io::Error),
    Outdated {
        running_version: Option<String>,
        installed_version: String,
    },
    InvalidState {
        state: DaemonState,
        detail: Option<String>,
    },
    Readiness {
        state: DaemonState,
        detail: Option<String>,
    },
    ControllerStartup {
        code: String,
        message: String,
    },
    GracefulTimeout {
        identity: Box<DaemonIdentity>,
    },
    IdentityChanged {
        detail: String,
    },
    ExitTimeout {
        identity: Box<DaemonIdentity>,
        forced: bool,
    },
    TombstoneBusy {
        detail: String,
    },
    TombstoneChanged {
        detail: String,
    },
}

impl std::fmt::Display for SupervisorError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Client(error) => error.fmt(formatter),
            Self::Io(error) => write!(formatter, "daemon supervisor I/O error: {error}"),
            Self::Outdated {
                running_version,
                installed_version,
            } => write!(
                formatter,
                "running daemon version {} does not match installed version {installed_version}",
                running_version.as_deref().unwrap_or("legacy")
            ),
            Self::InvalidState { state, detail } => write!(
                formatter,
                "daemon state {state:?} cannot be changed safely{}",
                detail
                    .as_deref()
                    .map_or_else(String::new, |detail| format!(": {detail}"))
            ),
            Self::Readiness { state, detail } => write!(
                formatter,
                "started daemon did not become healthy and current (state {state:?}){}",
                detail
                    .as_deref()
                    .map_or_else(String::new, |detail| format!(": {detail}"))
            ),
            Self::ControllerStartup { code, message } => {
                write!(formatter, "{code}: {message}")
            }
            Self::GracefulTimeout { identity } => write!(
                formatter,
                "daemon {} did not exit after graceful shutdown; retry with --force to interrupt active work",
                identity.pid
            ),
            Self::IdentityChanged { detail } => {
                write!(
                    formatter,
                    "daemon identity changed during shutdown: {detail}"
                )
            }
            Self::ExitTimeout { identity, forced } => write!(
                formatter,
                "daemon {} did not exit after {} shutdown",
                identity.pid,
                if *forced { "forced" } else { "graceful" }
            ),
            Self::TombstoneBusy { detail } => {
                write!(formatter, "daemon publication is still active: {detail}")
            }
            Self::TombstoneChanged { detail } => {
                write!(formatter, "daemon publication residue changed: {detail}")
            }
        }
    }
}

impl SupervisorError {
    pub(crate) const fn suggestion(&self) -> Option<&'static str> {
        match self {
            Self::GracefulTimeout { .. } => Some("--force"),
            Self::Client(_)
            | Self::Io(_)
            | Self::Outdated { .. }
            | Self::InvalidState { .. }
            | Self::Readiness { .. }
            | Self::ControllerStartup { .. }
            | Self::IdentityChanged { .. }
            | Self::ExitTimeout { .. }
            | Self::TombstoneBusy { .. }
            | Self::TombstoneChanged { .. } => None,
        }
    }
}

impl std::error::Error for SupervisorError {}

impl From<crate::client::ClientError> for SupervisorError {
    fn from(error: crate::client::ClientError) -> Self {
        Self::Client(error)
    }
}

impl From<io::Error> for SupervisorError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Debug)]
pub(crate) struct Inspection<C> {
    status: DaemonStatus,
    session: Option<EndpointSession<C>>,
    record: Option<DaemonInstanceRecord>,
    interrupted_tombstone: Option<InterruptedTombstone>,
    published_record: Option<InterruptedTombstone>,
}

impl<C> Inspection<C> {
    pub(crate) const fn status(&self) -> &DaemonStatus {
        &self.status
    }
}

#[derive(Debug)]
struct InterruptedTombstone {
    directory: OwnedFd,
    name: OsString,
    file: File,
    identity: FileIdentity,
    expected_uid: u32,
    size: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DaemonLaunch {
    pub(crate) executable: PathBuf,
    pub(crate) current_dir: PathBuf,
    pub(crate) instance_path: PathBuf,
    pub(crate) owner_token: String,
    pub(crate) stderr_path: Option<PathBuf>,
    pub(crate) startup_diagnostic_path: PathBuf,
}

pub(crate) trait DaemonSpawner: Send + Sync {
    fn spawn(&self, launch: &DaemonLaunch) -> io::Result<DaemonStartupMonitor>;
}

#[derive(Debug, Default)]
pub(crate) struct DaemonStartupMonitor {
    source: Option<StartupDiagnosticSource>,
}

impl DaemonStartupMonitor {
    pub(crate) fn from_file(file: File, owner_token: String) -> Self {
        Self {
            source: Some(StartupDiagnosticSource { file, owner_token }),
        }
    }

    fn controller_error(&self) -> io::Result<Option<SupervisorError>> {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

        let Some(source) = &self.source else {
            return Ok(None);
        };
        let metadata = source.file.metadata()?;
        if !metadata.file_type().is_file()
            || metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.permissions().mode() & 0o777 != u32::from(FILE_MODE)
            || metadata.nlink() != 0
            || metadata.len() > MAX_STARTUP_DIAGNOSTIC_BYTES as u64
        {
            return Ok(None);
        }
        let size = usize::try_from(metadata.len())
            .map_err(|_| io::Error::other("daemon startup diagnostic size overflow"))?;
        let mut bytes = vec![0_u8; size];
        let mut offset = 0;
        while offset < bytes.len() {
            let read = source.file.read_at(&mut bytes[offset..], offset as u64)?;
            if read == 0 {
                return Ok(None);
            }
            offset += read;
        }
        let Ok(captured) = std::str::from_utf8(&bytes) else {
            return Ok(None);
        };
        for line in captured.lines() {
            let Some(payload) = line.strip_prefix(STARTUP_DIAGNOSTIC_PREFIX) else {
                continue;
            };
            let Ok(diagnostic) = serde_json::from_str::<ControllerStartupDiagnostic>(payload)
            else {
                continue;
            };
            if !matches!(
                diagnostic.code.as_str(),
                "controller_state_conflict"
                    | "controller_state_unsafe"
                    | "controller_state_invalid"
                    | "controller_state_migration_failed"
            ) || diagnostic.message.trim().is_empty()
                || diagnostic.owner_token != source.owner_token
            {
                continue;
            }
            return Ok(Some(SupervisorError::ControllerStartup {
                code: diagnostic.code,
                message: diagnostic.message,
            }));
        }
        Ok(None)
    }
}

#[derive(Debug)]
struct StartupDiagnosticSource {
    file: File,
    owner_token: String,
}

#[derive(Debug, Deserialize)]
struct ControllerStartupDiagnostic {
    code: String,
    message: String,
    owner_token: String,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct SupervisorTimeouts {
    pub(crate) readiness: Duration,
    pub(crate) shutdown: Duration,
    pub(crate) poll: Duration,
}

impl Default for SupervisorTimeouts {
    fn default() -> Self {
        Self {
            readiness: Duration::from_secs(15),
            shutdown: Duration::from_secs(15),
            poll: Duration::from_millis(25),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DaemonTransition {
    None,
    Started,
    Stopped,
    Restarted,
    Recovered,
}

#[tonic::async_trait]
pub(crate) trait DaemonLifecycleObserver: Send {
    async fn transition_started(&mut self, transition: DaemonTransition);
}

#[derive(Default)]
pub(crate) struct NoopDaemonLifecycleObserver;

#[tonic::async_trait]
impl DaemonLifecycleObserver for NoopDaemonLifecycleObserver {
    async fn transition_started(&mut self, _transition: DaemonTransition) {}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StopMode {
    Automatic,
    Explicit { force: bool },
}

impl StopMode {
    const fn allows_force(self) -> bool {
        matches!(self, Self::Explicit { force: true })
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ShutdownPolicy {
    mode: StopMode,
    timeouts: SupervisorTimeouts,
}

impl ShutdownPolicy {
    pub(crate) const fn new(mode: StopMode, timeouts: SupervisorTimeouts) -> Self {
        Self { mode, timeouts }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LifecycleOutcome {
    pub(crate) status: DaemonStatus,
    pub(crate) transition: DaemonTransition,
    pub(crate) forced: bool,
}

#[derive(Debug)]
pub(crate) struct ConnectedDaemon<C> {
    pub(crate) connection: C,
    pub(crate) identity: DaemonIdentity,
}

#[derive(Debug)]
pub(crate) struct ConnectionOutcome<C> {
    pub(crate) daemon: ConnectedDaemon<C>,
    pub(crate) transition: DaemonTransition,
}

pub(crate) trait ProcessInspector: Clone + Send + Sync + 'static {
    fn inspect(&self, pid: u32, expected_executable: &Path) -> io::Result<Option<ProcessIdentity>>;
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct OsProcessInspector;

impl ProcessInspector for OsProcessInspector {
    fn inspect(&self, pid: u32, expected_executable: &Path) -> io::Result<Option<ProcessIdentity>> {
        inspect_process(pid, expected_executable)
    }
}

pub(crate) trait ProcessSignaler {
    fn signal(&self, pid: u32, signal: rustix::process::Signal) -> io::Result<()>;
}

pub(crate) trait AttestedProcessSignaler: Clone + Send + Sync + 'static {
    fn signal_attested(
        &self,
        identity: &DaemonIdentity,
        signal: rustix::process::Signal,
    ) -> io::Result<()>;

    fn signal_attested_until(
        &self,
        identity: &DaemonIdentity,
        signal: rustix::process::Signal,
        deadline: Instant,
    ) -> io::Result<()> {
        require_deadline(deadline, "daemon signaling deadline elapsed")?;
        self.signal_attested(identity, signal)
    }
}

struct OsProcessSignaler;

impl ProcessSignaler for OsProcessSignaler {
    fn signal(&self, pid: u32, signal: rustix::process::Signal) -> io::Result<()> {
        let pid = checked_pid(pid)?;
        rustix::process::kill_process(pid, signal).map_err(errno)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct OsAttestedProcessSignaler;

impl AttestedProcessSignaler for OsAttestedProcessSignaler {
    fn signal_attested(
        &self,
        identity: &DaemonIdentity,
        signal: rustix::process::Signal,
    ) -> io::Result<()> {
        signal_attested(&signaling_record(identity), signal)
    }

    fn signal_attested_until(
        &self,
        identity: &DaemonIdentity,
        signal: rustix::process::Signal,
        deadline: Instant,
    ) -> io::Result<()> {
        signal_attested_until(&signaling_record(identity), signal, deadline)
    }
}

fn signaling_record(identity: &DaemonIdentity) -> DaemonInstanceRecord {
    DaemonInstanceRecord {
        pid: identity.pid,
        owner_token: "endpoint-attested".to_owned(),
        executable: identity.executable.clone(),
        start_identity: identity.start_identity.clone(),
        instance_token: identity.instance_token.clone(),
        release_version: identity
            .release_version
            .clone()
            .unwrap_or_else(|| "legacy".to_owned()),
        started_at: identity.started_at.clone().unwrap_or(InstanceTimestamp {
            seconds: 1,
            nanos: 0,
        }),
    }
}

#[allow(dead_code)]
#[cfg(not(target_os = "linux"))]
pub(crate) fn signal_attested(
    record: &DaemonInstanceRecord,
    signal: rustix::process::Signal,
) -> io::Result<()> {
    signal_attested_with(record, &OsProcessInspector, &OsProcessSignaler, signal)
}

#[cfg(not(target_os = "linux"))]
fn signal_attested_until(
    record: &DaemonInstanceRecord,
    signal: rustix::process::Signal,
    deadline: Instant,
) -> io::Result<()> {
    signal_attested_with_deadline(
        record,
        &OsProcessInspector,
        &OsProcessSignaler,
        signal,
        Some(deadline),
    )
}

#[allow(dead_code)]
#[cfg(target_os = "linux")]
pub(crate) fn signal_attested(
    record: &DaemonInstanceRecord,
    signal: rustix::process::Signal,
) -> io::Result<()> {
    signal_attested_linux(record, signal, None)
}

#[cfg(target_os = "linux")]
fn signal_attested_until(
    record: &DaemonInstanceRecord,
    signal: rustix::process::Signal,
    deadline: Instant,
) -> io::Result<()> {
    signal_attested_linux(record, signal, Some(deadline))
}

#[cfg(target_os = "linux")]
fn signal_attested_linux(
    record: &DaemonInstanceRecord,
    signal: rustix::process::Signal,
    deadline: Option<Instant>,
) -> io::Result<()> {
    let pid = checked_pid(record.pid)?;
    let pidfd =
        rustix::process::pidfd_open(pid, rustix::process::PidfdFlags::empty()).map_err(errno)?;
    let identity = OsProcessInspector
        .inspect(record.pid, &record.executable)?
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "daemon process exited before signaling",
            )
        })?;
    require_identity_match(record, &identity)?;
    if let Some(deadline) = deadline {
        require_deadline(deadline, "daemon signaling deadline elapsed")?;
    }
    rustix::process::pidfd_send_signal(&pidfd, signal).map_err(errno)
}

fn checked_pid(pid: u32) -> io::Result<rustix::process::Pid> {
    let raw = i32::try_from(pid).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "daemon process id exceeds the platform range",
        )
    })?;
    rustix::process::Pid::from_raw(raw)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "daemon process id is zero"))
}

fn signal_attested_with<P: ProcessInspector, S: ProcessSignaler>(
    record: &DaemonInstanceRecord,
    inspector: &P,
    signaler: &S,
    signal: rustix::process::Signal,
) -> io::Result<()> {
    signal_attested_with_deadline(record, inspector, signaler, signal, None)
}

fn signal_attested_with_deadline<P: ProcessInspector, S: ProcessSignaler>(
    record: &DaemonInstanceRecord,
    inspector: &P,
    signaler: &S,
    signal: rustix::process::Signal,
    deadline: Option<Instant>,
) -> io::Result<()> {
    let identity = inspector
        .inspect(record.pid, &record.executable)?
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "daemon process exited before signaling",
            )
        })?;
    require_identity_match(record, &identity)?;
    if let Some(deadline) = deadline {
        require_deadline(deadline, "daemon signaling deadline elapsed")?;
    }
    signaler.signal(record.pid, signal)
}

fn require_deadline(deadline: Instant, detail: &str) -> io::Result<()> {
    if Instant::now() >= deadline {
        Err(io::Error::new(io::ErrorKind::TimedOut, detail))
    } else {
        Ok(())
    }
}

async fn inspect_process_supervised<P: ProcessInspector>(
    inspector: &P,
    pid: u32,
    expected_executable: &Path,
) -> io::Result<Option<ProcessIdentity>> {
    let inspector = inspector.clone();
    let expected_executable = expected_executable.to_owned();
    tokio::task::spawn_blocking(move || inspector.inspect(pid, &expected_executable))
        .await
        .map_err(|error| io::Error::other(format!("process inspection task failed: {error}")))?
}

pub(crate) fn read_attested_instance<P: ProcessInspector>(
    paths: &DaemonPaths,
    inspector: &P,
) -> io::Result<Option<DaemonInstanceRecord>> {
    let Some(record) = read_instance_record(paths)? else {
        return Ok(None);
    };
    let identity = inspector
        .inspect(record.pid, &record.executable)?
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "daemon instance process is not live",
            )
        })?;
    require_identity_match(&record, &identity)?;
    Ok(Some(record))
}

async fn probe_authenticated<E: DaemonEndpoint>(
    paths: &DaemonPaths,
    endpoint: &E,
) -> Result<EndpointProbe<E::Connection>, crate::client::ClientError> {
    let before = match inspect_endpoint_path(paths) {
        Ok(state) => state,
        Err(error) => return Ok(EndpointProbe::Unsafe(error.to_string())),
    };
    let probe = endpoint.probe(paths, before).await?;
    let after = match inspect_endpoint_path(paths) {
        Ok(state) => state,
        Err(error) => return Ok(EndpointProbe::Unsafe(error.to_string())),
    };
    match (before, after) {
        (EndpointPathState::Absent, EndpointPathState::Absent) => Ok(probe),
        (EndpointPathState::SafeSocket(before), EndpointPathState::SafeSocket(after))
            if before == after =>
        {
            Ok(probe)
        }
        _ => Ok(EndpointProbe::Unsafe(
            ENDPOINT_CHANGED_DURING_PROBE.to_owned(),
        )),
    }
}

pub(crate) async fn inspect_with<E, P>(
    paths: &DaemonPaths,
    expected_executable: &Path,
    endpoint: &E,
    inspector: &P,
) -> Result<Inspection<E::Connection>, SupervisorError>
where
    E: DaemonEndpoint,
    P: ProcessInspector,
{
    let record = read_instance_record_for_inspection(paths);
    let interrupted_tombstone = if record.is_err() {
        match open_interrupted_tombstone(paths) {
            Ok(tombstone) => tombstone,
            Err(error) => {
                let _ = probe_authenticated(paths, endpoint).await?;
                return Ok(Inspection {
                    status: DaemonStatus {
                        state: DaemonState::Unsafe,
                        identity: None,
                        legacy: false,
                        detail: Some(error.to_string()),
                    },
                    session: None,
                    record: None,
                    interrupted_tombstone: None,
                    published_record: None,
                });
            }
        }
    } else {
        None
    };
    let published_record = match record.as_ref() {
        Ok(Some(record)) => open_published_record(paths, record).map(Some),
        Ok(None) | Err(_) => Ok(None),
    };
    let probe = probe_authenticated(paths, endpoint).await?;
    if let Some(tombstone) = interrupted_tombstone {
        return Ok(match probe {
            EndpointProbe::AbsentOrInert => Inspection {
                status: DaemonStatus {
                    state: DaemonState::Unsafe,
                    identity: None,
                    legacy: false,
                    detail: Some("daemon record publication was interrupted".to_owned()),
                },
                session: None,
                record: None,
                interrupted_tombstone: Some(tombstone),
                published_record: None,
            },
            EndpointProbe::Unresponsive(detail) => Inspection {
                status: DaemonStatus {
                    state: DaemonState::Unsafe,
                    identity: None,
                    legacy: false,
                    detail: Some(format!(
                        "daemon record publication was interrupted beside an unresponsive endpoint: {detail}"
                    )),
                },
                session: None,
                record: None,
                interrupted_tombstone: None,
                published_record: None,
            },
            EndpointProbe::Connected(session) => Inspection {
                status: DaemonStatus {
                    state: DaemonState::Unsafe,
                    identity: Some(session.identity.clone()),
                    legacy: session.identity.release_version.is_none(),
                    detail: Some(
                        "a daemon endpoint is live beside interrupted publication state".to_owned(),
                    ),
                },
                session: Some(session),
                record: None,
                interrupted_tombstone: Some(tombstone),
                published_record: None,
            },
            EndpointProbe::Unsafe(detail) => Inspection {
                status: DaemonStatus {
                    state: DaemonState::Unsafe,
                    identity: None,
                    legacy: false,
                    detail: Some(detail),
                },
                session: None,
                record: None,
                interrupted_tombstone: None,
                published_record: None,
            },
        });
    }
    let record = match record {
        Ok(record) => record,
        Err(error) => {
            return Ok(Inspection {
                status: DaemonStatus {
                    state: DaemonState::Unsafe,
                    identity: None,
                    legacy: false,
                    detail: Some(error.to_string()),
                },
                session: None,
                record: None,
                interrupted_tombstone: None,
                published_record: None,
            });
        }
    };
    let published_record = match published_record {
        Ok(published_record) => published_record,
        Err(error) => {
            return Ok(match probe {
                EndpointProbe::AbsentOrInert | EndpointProbe::Unresponsive(_) => Inspection {
                    status: DaemonStatus {
                        state: DaemonState::Unsafe,
                        identity: record.as_ref().map(DaemonIdentity::from),
                        legacy: false,
                        detail: Some(error.to_string()),
                    },
                    session: None,
                    record,
                    interrupted_tombstone: None,
                    published_record: None,
                },
                EndpointProbe::Connected(session) => Inspection {
                    status: DaemonStatus {
                        state: DaemonState::Unsafe,
                        identity: Some(session.identity.clone()),
                        legacy: session.identity.release_version.is_none(),
                        detail: Some(error.to_string()),
                    },
                    session: Some(session),
                    record,
                    interrupted_tombstone: None,
                    published_record: None,
                },
                EndpointProbe::Unsafe(detail) => Inspection {
                    status: DaemonStatus {
                        state: DaemonState::Unsafe,
                        identity: record.as_ref().map(DaemonIdentity::from),
                        legacy: false,
                        detail: Some(format!("{detail}: {error}")),
                    },
                    session: None,
                    record,
                    interrupted_tombstone: None,
                    published_record: None,
                },
            });
        }
    };

    let mut inspection = match probe {
        EndpointProbe::Unsafe(detail) => Inspection {
            status: DaemonStatus {
                state: DaemonState::Unsafe,
                identity: record.as_ref().map(DaemonIdentity::from),
                legacy: false,
                detail: Some(detail),
            },
            session: None,
            record,
            interrupted_tombstone: None,
            published_record: None,
        },
        EndpointProbe::AbsentOrInert => classify_unreachable(paths, record, inspector).await?,
        EndpointProbe::Unresponsive(detail) => {
            classify_unresponsive(paths, record, inspector, detail).await?
        }
        EndpointProbe::Connected(session) => {
            classify_connected(expected_executable, record, session, inspector).await?
        }
    };
    inspection.published_record = published_record;
    Ok(inspection)
}

pub(crate) async fn start_with<E, P, S>(
    paths: &DaemonPaths,
    expected_executable: &Path,
    endpoint: &E,
    inspector: &P,
    spawner: &S,
    timeouts: SupervisorTimeouts,
) -> Result<LifecycleOutcome, SupervisorError>
where
    E: DaemonEndpoint,
    P: ProcessInspector,
    S: DaemonSpawner,
{
    let _lock = paths.lock_async().await?;
    let inspected = inspect_with(paths, expected_executable, endpoint, inspector).await?;
    let (inspected, started) = ensure_started_locked(
        paths,
        expected_executable,
        endpoint,
        inspector,
        spawner,
        timeouts,
        inspected,
    )
    .await?;
    Ok(LifecycleOutcome {
        status: inspected.status,
        transition: if started {
            DaemonTransition::Started
        } else {
            DaemonTransition::None
        },
        forced: false,
    })
}

async fn ensure_started_locked<E, P, S>(
    paths: &DaemonPaths,
    expected_executable: &Path,
    endpoint: &E,
    inspector: &P,
    spawner: &S,
    timeouts: SupervisorTimeouts,
    mut inspected: Inspection<E::Connection>,
) -> Result<(Inspection<E::Connection>, bool), SupervisorError>
where
    E: DaemonEndpoint,
    P: ProcessInspector,
    S: DaemonSpawner,
{
    if inspected.status.state == DaemonState::Unsafe && inspected.session.is_none() {
        if let Some(tombstone) = inspected.interrupted_tombstone.take() {
            recover_interrupted_tombstone(paths, endpoint, tombstone, timeouts).await?;
            inspected = Inspection {
                status: DaemonStatus::new(DaemonState::Stopped),
                session: None,
                record: None,
                interrupted_tombstone: None,
                published_record: None,
            };
        }
    }
    match inspected.status.state {
        DaemonState::Current => return Ok((inspected, false)),
        DaemonState::Stopped => {
            if let Some(record) = inspected.record.take() {
                let published_record = inspected.published_record.take().ok_or_else(|| {
                    SupervisorError::TombstoneChanged {
                        detail: "stale daemon record is not bound to a validated descriptor"
                            .to_owned(),
                    }
                })?;
                recover_stale_published_record(
                    paths,
                    endpoint,
                    inspector,
                    published_record,
                    &record,
                    timeouts,
                )
                .await?;
            }
        }
        DaemonState::Outdated => {
            return Err(SupervisorError::Outdated {
                running_version: inspected
                    .status
                    .identity
                    .as_ref()
                    .and_then(|identity| identity.release_version.clone()),
                installed_version: env!("CARGO_PKG_VERSION").to_owned(),
            });
        }
        state => {
            return Err(SupervisorError::InvalidState {
                state,
                detail: inspected.status.detail,
            });
        }
    }
    let launch = daemon_launch(paths, expected_executable)?;
    let startup = spawner.spawn(&launch)?;
    let expected_owner_token = launch.owner_token;
    let deadline = tokio::time::Instant::now() + timeouts.readiness;
    loop {
        let inspected = match tokio::time::timeout_at(
            deadline,
            inspect_with(paths, expected_executable, endpoint, inspector),
        )
        .await
        {
            Ok(inspected) => inspected?,
            Err(_) => {
                if let Some(error) = startup.controller_error()? {
                    return Err(error);
                }
                return Err(SupervisorError::Readiness {
                    state: DaemonState::Unreachable,
                    detail: Some(
                        "daemon readiness deadline elapsed during state inspection".to_owned(),
                    ),
                });
            }
        };
        // Written as a nested `if` rather than a let-chain: let-chains are
        // unstable before Rust 1.88 and `rust-toolchain.toml` pins 1.85.0, which
        // `Cargo.toml`'s `rust-version = "1.85"` also declares. The short-circuit
        // is preserved — `controller_error()` is only called for the two states.
        if matches!(
            inspected.status.state,
            DaemonState::Stopped | DaemonState::Unreachable
        ) {
            if let Some(error) = startup.controller_error()? {
                return Err(error);
            }
        }
        match inspected.status.state {
            DaemonState::Current
                if inspected
                    .record
                    .as_ref()
                    .is_some_and(|record| record.owner_token == expected_owner_token) =>
            {
                return Ok((inspected, true));
            }
            DaemonState::Current
                if inspected.record.is_none() && tokio::time::Instant::now() < deadline =>
            {
                tokio::time::sleep_until(std::cmp::min(
                    deadline,
                    tokio::time::Instant::now() + timeouts.poll,
                ))
                .await;
            }
            DaemonState::Current => {
                return Err(SupervisorError::Readiness {
                    state: DaemonState::Unhealthy,
                    detail: Some(
                        "started daemon record does not carry this launch's fresh owner token"
                            .to_owned(),
                    ),
                });
            }
            DaemonState::Stopped | DaemonState::Unreachable
                if tokio::time::Instant::now() < deadline =>
            {
                tokio::time::sleep_until(std::cmp::min(
                    deadline,
                    tokio::time::Instant::now() + timeouts.poll,
                ))
                .await;
            }
            DaemonState::Unsafe
                if inspected.session.is_some()
                    && inspected.interrupted_tombstone.is_some()
                    && tokio::time::Instant::now() < deadline =>
            {
                tokio::time::sleep_until(std::cmp::min(
                    deadline,
                    tokio::time::Instant::now() + timeouts.poll,
                ))
                .await;
            }
            DaemonState::Unsafe
                if inspected.status.detail.as_deref() == Some(ENDPOINT_CHANGED_DURING_PROBE)
                    && tokio::time::Instant::now() < deadline =>
            {
                tokio::time::sleep_until(std::cmp::min(
                    deadline,
                    tokio::time::Instant::now() + timeouts.poll,
                ))
                .await;
            }
            state => {
                return Err(SupervisorError::Readiness {
                    state,
                    detail: inspected.status.detail,
                });
            }
        }
    }
}

async fn recover_interrupted_tombstone<E: DaemonEndpoint>(
    paths: &DaemonPaths,
    endpoint: &E,
    tombstone: InterruptedTombstone,
    timeouts: SupervisorTimeouts,
) -> Result<(), SupervisorError> {
    for probe_index in 0..2 {
        if probe_index > 0 {
            tokio::time::sleep(timeouts.poll).await;
        }
        validate_held_interrupted_tombstone(&tombstone)?;
        prove_endpoint_absent_or_inert(paths, endpoint, "interrupted publication").await?;
        validate_held_interrupted_tombstone(&tombstone)?;
    }

    retire_held_record(&tombstone)
}

async fn recover_stale_published_record<E, P>(
    paths: &DaemonPaths,
    endpoint: &E,
    inspector: &P,
    published_record: InterruptedTombstone,
    record: &DaemonInstanceRecord,
    timeouts: SupervisorTimeouts,
) -> Result<(), SupervisorError>
where
    E: DaemonEndpoint,
    P: ProcessInspector,
{
    for probe_index in 0..2 {
        if probe_index > 0 {
            tokio::time::sleep(timeouts.poll).await;
        }
        validate_held_published_record(&published_record, record)?;
        match inspect_process_supervised(inspector, record.pid, &record.executable).await {
            Ok(None) => {}
            Ok(Some(_)) => {
                return Err(SupervisorError::TombstoneBusy {
                    detail: "the recorded process appeared during stale-record recovery".to_owned(),
                });
            }
            Err(error) => {
                return Err(SupervisorError::TombstoneBusy {
                    detail: format!(
                        "the recorded process could not be proven absent during recovery: {error}"
                    ),
                });
            }
        }
        prove_endpoint_absent_or_inert(paths, endpoint, "stale-record recovery").await?;
        validate_held_published_record(&published_record, record)?;
    }

    retire_held_record(&published_record)
}

async fn prove_endpoint_absent_or_inert<E: DaemonEndpoint>(
    paths: &DaemonPaths,
    endpoint: &E,
    context: &str,
) -> Result<(), SupervisorError> {
    let before = inspect_endpoint_path(paths).map_err(|error| SupervisorError::TombstoneBusy {
        detail: format!("{context} found an unsafe endpoint path: {error}"),
    })?;
    let probe = endpoint.probe(paths, before).await?;
    let after = inspect_endpoint_path(paths).map_err(|error| SupervisorError::TombstoneBusy {
        detail: format!("{context} found an unsafe endpoint path after probing: {error}"),
    })?;

    let stable_or_removed = match (before, after) {
        (EndpointPathState::Absent, EndpointPathState::Absent)
        | (EndpointPathState::SafeSocket(_), EndpointPathState::Absent) => true,
        (EndpointPathState::SafeSocket(before), EndpointPathState::SafeSocket(after)) => {
            before == after
        }
        (EndpointPathState::Absent, EndpointPathState::SafeSocket(_)) => false,
    };
    if !stable_or_removed {
        return Err(SupervisorError::TombstoneBusy {
            detail: format!("{context} endpoint pathname changed while proving absence"),
        });
    }

    match probe {
        EndpointProbe::AbsentOrInert => Ok(()),
        EndpointProbe::Unresponsive(detail) => Err(SupervisorError::TombstoneBusy {
            detail: format!("{context} found a live but unresponsive endpoint: {detail}"),
        }),
        EndpointProbe::Connected(_) => Err(SupervisorError::TombstoneBusy {
            detail: format!("{context} found a connected daemon endpoint"),
        }),
        EndpointProbe::Unsafe(detail) => Err(SupervisorError::TombstoneBusy { detail }),
    }
}

fn retire_held_record(record: &InterruptedTombstone) -> Result<(), SupervisorError> {
    rustix::fs::fchmod(&record.file, Mode::from_raw_mode(INSTANCE_TOMBSTONE_MODE))
        .map_err(errno)?;
    rustix::fs::ftruncate(&record.file, 0).map_err(errno)?;
    record.file.sync_all()?;
    validate_retired_tombstone(record)?;
    Ok(())
}

fn validate_held_interrupted_tombstone(
    tombstone: &InterruptedTombstone,
) -> Result<(), SupervisorError> {
    let stat = rustix::fs::fstat(&tombstone.file).map_err(errno)?;
    let identity = FileIdentity {
        device: stat.st_dev as u64,
        inode: stat.st_ino,
    };
    if !is_interrupted_tombstone(&stat, tombstone.expected_uid)
        || identity != tombstone.identity
        || stat.st_size as u64 != tombstone.size
    {
        return Err(SupervisorError::TombstoneChanged {
            detail: "held descriptor no longer identifies the validated residue".to_owned(),
        });
    }
    let path = rustix::fs::statat(
        &tombstone.directory,
        tombstone.name.as_os_str(),
        AtFlags::SYMLINK_NOFOLLOW,
    )
    .map_err(|error| SupervisorError::TombstoneChanged {
        detail: errno(error).to_string(),
    })?;
    let path_identity = FileIdentity {
        device: path.st_dev as u64,
        inode: path.st_ino,
    };
    if !is_interrupted_tombstone(&path, tombstone.expected_uid)
        || path_identity != tombstone.identity
        || path.st_size as u64 != tombstone.size
    {
        return Err(SupervisorError::TombstoneChanged {
            detail: "pathname no longer names the held validated residue".to_owned(),
        });
    }
    Ok(())
}

fn validate_held_published_record(
    published_record: &InterruptedTombstone,
    expected_record: &DaemonInstanceRecord,
) -> Result<(), SupervisorError> {
    let stat = rustix::fs::fstat(&published_record.file).map_err(errno)?;
    let identity = FileIdentity {
        device: stat.st_dev as u64,
        inode: stat.st_ino,
    };
    if validate_file_stat(&stat, published_record.expected_uid).is_err()
        || identity != published_record.identity
        || stat.st_size as u64 != published_record.size
    {
        return Err(SupervisorError::TombstoneChanged {
            detail: "held descriptor no longer identifies the validated daemon record".to_owned(),
        });
    }
    let path = rustix::fs::statat(
        &published_record.directory,
        published_record.name.as_os_str(),
        AtFlags::SYMLINK_NOFOLLOW,
    )
    .map_err(|error| SupervisorError::TombstoneChanged {
        detail: errno(error).to_string(),
    })?;
    let path_identity = FileIdentity {
        device: path.st_dev as u64,
        inode: path.st_ino,
    };
    if validate_file_stat(&path, published_record.expected_uid).is_err()
        || path_identity != published_record.identity
        || path.st_size as u64 != published_record.size
    {
        return Err(SupervisorError::TombstoneChanged {
            detail: "pathname no longer names the held validated daemon record".to_owned(),
        });
    }
    let actual_record = read_record_from_held_file(&published_record.file, published_record.size)
        .map_err(|error| SupervisorError::TombstoneChanged {
        detail: format!("held daemon record changed while recovering it: {error}"),
    })?;
    if &actual_record != expected_record {
        return Err(SupervisorError::TombstoneChanged {
            detail: "held daemon record contents changed while recovering it".to_owned(),
        });
    }
    Ok(())
}

fn validate_retired_tombstone(tombstone: &InterruptedTombstone) -> Result<(), SupervisorError> {
    let stat = rustix::fs::fstat(&tombstone.file).map_err(errno)?;
    let identity = FileIdentity {
        device: stat.st_dev as u64,
        inode: stat.st_ino,
    };
    if !is_instance_tombstone(&stat, tombstone.expected_uid) || identity != tombstone.identity {
        return Err(SupervisorError::TombstoneChanged {
            detail: "held descriptor did not reach the exact empty tombstone state".to_owned(),
        });
    }
    let path = rustix::fs::statat(
        &tombstone.directory,
        tombstone.name.as_os_str(),
        AtFlags::SYMLINK_NOFOLLOW,
    )
    .map_err(|error| SupervisorError::TombstoneChanged {
        detail: errno(error).to_string(),
    })?;
    let path_identity = FileIdentity {
        device: path.st_dev as u64,
        inode: path.st_ino,
    };
    if !is_instance_tombstone(&path, tombstone.expected_uid) || path_identity != tombstone.identity
    {
        return Err(SupervisorError::TombstoneChanged {
            detail: "pathname does not name the retired held descriptor".to_owned(),
        });
    }
    Ok(())
}

pub(crate) async fn stop_with<E, P, S>(
    paths: &DaemonPaths,
    expected_executable: &Path,
    endpoint: &E,
    inspector: &P,
    signaler: &S,
    mode: StopMode,
    timeouts: SupervisorTimeouts,
) -> Result<LifecycleOutcome, SupervisorError>
where
    E: DaemonEndpoint,
    P: ProcessInspector,
    S: AttestedProcessSignaler,
{
    let _lock = paths.lock_async().await?;
    let inspected = inspect_with(paths, expected_executable, endpoint, inspector).await?;
    stop_inspected_locked(
        paths,
        expected_executable,
        endpoint,
        inspector,
        signaler,
        ShutdownPolicy::new(mode, timeouts),
        inspected,
    )
    .await
}

async fn stop_inspected_locked<E, P, S>(
    paths: &DaemonPaths,
    expected_executable: &Path,
    endpoint: &E,
    inspector: &P,
    signaler: &S,
    policy: ShutdownPolicy,
    mut inspected: Inspection<E::Connection>,
) -> Result<LifecycleOutcome, SupervisorError>
where
    E: DaemonEndpoint,
    P: ProcessInspector,
    S: AttestedProcessSignaler,
{
    if inspected.status.state == DaemonState::Stopped {
        return Ok(LifecycleOutcome {
            status: inspected.status,
            transition: DaemonTransition::None,
            forced: false,
        });
    }
    if !matches!(
        inspected.status.state,
        DaemonState::Current | DaemonState::Outdated
    ) {
        return Err(SupervisorError::InvalidState {
            state: inspected.status.state,
            detail: inspected.status.detail,
        });
    }
    let identity =
        inspected
            .status
            .identity
            .clone()
            .ok_or_else(|| SupervisorError::IdentityChanged {
                detail: "running state omitted daemon identity".to_owned(),
            })?;
    let graceful_deadline = tokio::time::Instant::now() + policy.timeouts.shutdown;

    if inspected.status.legacy {
        let endpoint_path_before =
            inspect_endpoint_path(paths).map_err(|error| SupervisorError::IdentityChanged {
                detail: format!("legacy endpoint path was unsafe before re-attestation: {error}"),
            })?;
        if !matches!(endpoint_path_before, EndpointPathState::SafeSocket(_)) {
            return Err(SupervisorError::IdentityChanged {
                detail: "legacy endpoint path disappeared before re-attestation".to_owned(),
            });
        }
        let second = tokio::time::timeout_at(
            graceful_deadline,
            endpoint.probe(paths, endpoint_path_before),
        )
        .await
        .map_err(|_| SupervisorError::IdentityChanged {
            detail: "legacy endpoint re-attestation timed out".to_owned(),
        })??;
        let endpoint_path_after =
            inspect_endpoint_path(paths).map_err(|error| SupervisorError::IdentityChanged {
                detail: format!("legacy endpoint path changed during re-attestation: {error}"),
            })?;
        if endpoint_path_after != endpoint_path_before {
            return Err(SupervisorError::IdentityChanged {
                detail: "legacy endpoint pathname changed during re-attestation".to_owned(),
            });
        }
        let EndpointProbe::Connected(second) = second else {
            return Err(SupervisorError::IdentityChanged {
                detail: "legacy endpoint disappeared before signaling".to_owned(),
            });
        };
        if second.identity != identity
            || !second.safe_transport
            || !second.healthy
            || inspected
                .record
                .as_ref()
                .is_some_and(|record| !record_matches_endpoint(record, &second.identity))
        {
            return Err(SupervisorError::IdentityChanged {
                detail: "legacy endpoint attestations were not identical".to_owned(),
            });
        }
        signal_identity(
            signaler,
            &identity,
            rustix::process::Signal::TERM,
            graceful_deadline,
        )
        .await?;
    } else {
        let session =
            inspected
                .session
                .as_mut()
                .ok_or_else(|| SupervisorError::IdentityChanged {
                    detail: "running daemon lost its connected session".to_owned(),
                })?;
        if let Ok(Err(error)) = tokio::time::timeout_at(
            graceful_deadline,
            endpoint.graceful_shutdown(&mut session.connection, &identity.instance_token),
        )
        .await
        {
            if !policy.mode.allows_force() || graceful_error_forbids_force(&error) {
                return Err(error.into());
            }
        }
    }

    if wait_for_exit_until(
        &identity,
        inspector,
        graceful_deadline,
        policy.timeouts.poll,
    )
    .await?
    {
        let stopped = confirm_stopped(
            paths,
            expected_executable,
            endpoint,
            inspector,
            graceful_deadline,
            "graceful shutdown",
        )
        .await?;
        return Ok(LifecycleOutcome {
            status: stopped,
            transition: DaemonTransition::Stopped,
            forced: false,
        });
    }
    if !policy.mode.allows_force() {
        return Err(SupervisorError::GracefulTimeout {
            identity: Box::new(identity),
        });
    }

    let force_deadline = tokio::time::Instant::now() + policy.timeouts.shutdown;
    signal_identity(
        signaler,
        &identity,
        rustix::process::Signal::KILL,
        force_deadline,
    )
    .await?;
    if !wait_for_exit_until(&identity, inspector, force_deadline, policy.timeouts.poll).await? {
        return Err(SupervisorError::ExitTimeout {
            identity: Box::new(identity),
            forced: true,
        });
    }
    let stopped = confirm_stopped(
        paths,
        expected_executable,
        endpoint,
        inspector,
        force_deadline,
        "forced shutdown",
    )
    .await?;
    Ok(LifecycleOutcome {
        status: stopped,
        transition: DaemonTransition::Stopped,
        forced: true,
    })
}

fn graceful_error_forbids_force(error: &crate::client::ClientError) -> bool {
    match error {
        crate::client::ClientError::Rpc(status) => matches!(
            status.code(),
            tonic::Code::PermissionDenied | tonic::Code::Unauthenticated
        ),
        crate::client::ClientError::Api(_) => true,
        crate::client::ClientError::Io(_) | crate::client::ClientError::Transport(_) => false,
    }
}

pub(crate) async fn restart_with<E, P, Spawn, Signal>(
    paths: &DaemonPaths,
    expected_executable: &Path,
    endpoint: &E,
    inspector: &P,
    spawner: &Spawn,
    signaler: &Signal,
    policy: ShutdownPolicy,
) -> Result<LifecycleOutcome, SupervisorError>
where
    E: DaemonEndpoint,
    P: ProcessInspector,
    Spawn: DaemonSpawner,
    Signal: AttestedProcessSignaler,
{
    let _lock = paths.lock_async().await?;
    let inspected = inspect_with(paths, expected_executable, endpoint, inspector).await?;
    let recoverable_tombstone =
        inspected.interrupted_tombstone.is_some() && inspected.session.is_none();
    let forced = if inspected.status.state == DaemonState::Stopped || recoverable_tombstone {
        false
    } else {
        stop_inspected_locked(
            paths,
            expected_executable,
            endpoint,
            inspector,
            signaler,
            policy,
            inspected,
        )
        .await?
        .forced
    };
    let stopped = inspect_with(paths, expected_executable, endpoint, inspector).await?;
    let (current, _) = ensure_started_locked(
        paths,
        expected_executable,
        endpoint,
        inspector,
        spawner,
        policy.timeouts,
        stopped,
    )
    .await?;
    Ok(LifecycleOutcome {
        status: current.status,
        transition: DaemonTransition::Restarted,
        forced,
    })
}

pub(crate) async fn connect_current_or_recover_with<E, P, Spawn, Signal>(
    paths: &DaemonPaths,
    expected_executable: &Path,
    endpoint: &E,
    inspector: &P,
    spawner: &Spawn,
    signaler: &Signal,
    timeouts: SupervisorTimeouts,
) -> Result<ConnectionOutcome<E::Connection>, SupervisorError>
where
    E: DaemonEndpoint,
    P: ProcessInspector,
    Spawn: DaemonSpawner,
    Signal: AttestedProcessSignaler,
{
    let mut observer = NoopDaemonLifecycleObserver;
    connect_current_or_recover_with_observer(
        paths,
        expected_executable,
        endpoint,
        inspector,
        spawner,
        signaler,
        timeouts,
        &mut observer,
    )
    .await
}

#[allow(
    clippy::too_many_arguments,
    reason = "the observer extends the existing testable supervisor boundary without hiding its lifecycle dependencies"
)]
pub(crate) async fn connect_current_or_recover_with_observer<E, P, Spawn, Signal, Observer>(
    paths: &DaemonPaths,
    expected_executable: &Path,
    endpoint: &E,
    inspector: &P,
    spawner: &Spawn,
    signaler: &Signal,
    timeouts: SupervisorTimeouts,
    observer: &mut Observer,
) -> Result<ConnectionOutcome<E::Connection>, SupervisorError>
where
    E: DaemonEndpoint,
    P: ProcessInspector,
    Spawn: DaemonSpawner,
    Signal: AttestedProcessSignaler,
    Observer: DaemonLifecycleObserver,
{
    let initial = inspect_with(paths, expected_executable, endpoint, inspector).await?;
    if initial.status.state == DaemonState::Current {
        return connected_outcome(initial, DaemonTransition::None);
    }

    let _lock = paths.lock_async().await?;
    let inspected = inspect_with(paths, expected_executable, endpoint, inspector).await?;
    let (current, transition) = match inspected.status.state {
        DaemonState::Current => (inspected, DaemonTransition::None),
        DaemonState::Stopped => {
            let (current, _) = ensure_started_locked(
                paths,
                expected_executable,
                endpoint,
                inspector,
                spawner,
                timeouts,
                inspected,
            )
            .await?;
            (current, DaemonTransition::Started)
        }
        DaemonState::Unsafe
            if inspected.interrupted_tombstone.is_some() && inspected.session.is_none() =>
        {
            let (current, _) = ensure_started_locked(
                paths,
                expected_executable,
                endpoint,
                inspector,
                spawner,
                timeouts,
                inspected,
            )
            .await?;
            (current, DaemonTransition::Started)
        }
        DaemonState::Outdated => {
            observer
                .transition_started(DaemonTransition::Recovered)
                .await;
            stop_inspected_locked(
                paths,
                expected_executable,
                endpoint,
                inspector,
                signaler,
                ShutdownPolicy::new(StopMode::Automatic, timeouts),
                inspected,
            )
            .await?;
            let stopped = inspect_with(paths, expected_executable, endpoint, inspector).await?;
            let (current, _) = ensure_started_locked(
                paths,
                expected_executable,
                endpoint,
                inspector,
                spawner,
                timeouts,
                stopped,
            )
            .await?;
            (current, DaemonTransition::Recovered)
        }
        state => {
            return Err(SupervisorError::InvalidState {
                state,
                detail: inspected.status.detail,
            });
        }
    };
    connected_outcome(current, transition)
}

pub(crate) async fn inspect() -> Result<DaemonStatus, SupervisorError> {
    let (paths, executable) = supervisor_context()?;
    inspect_with(
        &paths,
        &executable,
        &crate::client::TonicEndpoint,
        &OsProcessInspector,
    )
    .await
    .map(|inspected| inspected.status)
}

pub(crate) async fn start() -> Result<LifecycleOutcome, SupervisorError> {
    let (paths, executable) = supervisor_context()?;
    start_with(
        &paths,
        &executable,
        &crate::client::TonicEndpoint,
        &OsProcessInspector,
        &crate::client::TokioDaemonSpawner,
        SupervisorTimeouts::default(),
    )
    .await
}

pub(crate) async fn stop(force: bool) -> Result<LifecycleOutcome, SupervisorError> {
    let (paths, executable) = supervisor_context()?;
    stop_with(
        &paths,
        &executable,
        &crate::client::TonicEndpoint,
        &OsProcessInspector,
        &OsAttestedProcessSignaler,
        StopMode::Explicit { force },
        SupervisorTimeouts::default(),
    )
    .await
}

pub(crate) async fn restart(force: bool) -> Result<LifecycleOutcome, SupervisorError> {
    let (paths, executable) = supervisor_context()?;
    restart_with(
        &paths,
        &executable,
        &crate::client::TonicEndpoint,
        &OsProcessInspector,
        &crate::client::TokioDaemonSpawner,
        &OsAttestedProcessSignaler,
        ShutdownPolicy::new(StopMode::Explicit { force }, SupervisorTimeouts::default()),
    )
    .await
}

pub(crate) async fn connect_current_or_recover()
-> Result<ConnectionOutcome<crate::client::Client>, SupervisorError> {
    let (paths, executable) = supervisor_context()?;
    connect_current_or_recover_with(
        &paths,
        &executable,
        &crate::client::TonicEndpoint,
        &OsProcessInspector,
        &crate::client::TokioDaemonSpawner,
        &OsAttestedProcessSignaler,
        SupervisorTimeouts::default(),
    )
    .await
}

pub(crate) async fn connect_current_or_recover_observing<Observer>(
    observer: &mut Observer,
) -> Result<ConnectionOutcome<crate::client::Client>, SupervisorError>
where
    Observer: DaemonLifecycleObserver,
{
    let (paths, executable) = supervisor_context()?;
    connect_current_or_recover_with_observer(
        &paths,
        &executable,
        &crate::client::TonicEndpoint,
        &OsProcessInspector,
        &crate::client::TokioDaemonSpawner,
        &OsAttestedProcessSignaler,
        SupervisorTimeouts::default(),
        observer,
    )
    .await
}

fn supervisor_context() -> Result<(DaemonPaths, PathBuf), SupervisorError> {
    let paths = DaemonPaths::for_user()?;
    let executable = crate::client::daemon_path()?;
    if !executable.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "daemon executable path must be absolute",
        )
        .into());
    }
    let executable = match executable.canonicalize() {
        Ok(executable) => executable,
        Err(error) if error.kind() == io::ErrorKind::NotFound => executable,
        Err(error) => return Err(error.into()),
    };
    Ok((paths, executable))
}

fn connected_outcome<C>(
    mut inspected: Inspection<C>,
    transition: DaemonTransition,
) -> Result<ConnectionOutcome<C>, SupervisorError> {
    if inspected.status.state != DaemonState::Current {
        return Err(SupervisorError::Readiness {
            state: inspected.status.state,
            detail: inspected.status.detail,
        });
    }
    let identity = inspected
        .status
        .identity
        .ok_or_else(|| SupervisorError::IdentityChanged {
            detail: "current daemon status omitted identity".to_owned(),
        })?;
    let session = inspected
        .session
        .take()
        .ok_or_else(|| SupervisorError::IdentityChanged {
            detail: "current daemon status omitted its validated connection".to_owned(),
        })?;
    if session.identity != identity {
        return Err(SupervisorError::IdentityChanged {
            detail: "validated connection identity changed before use".to_owned(),
        });
    }
    Ok(ConnectionOutcome {
        daemon: ConnectedDaemon {
            connection: session.connection,
            identity,
        },
        transition,
    })
}

async fn signal_identity<S: AttestedProcessSignaler>(
    signaler: &S,
    identity: &DaemonIdentity,
    signal: rustix::process::Signal,
    deadline: tokio::time::Instant,
) -> Result<(), SupervisorError> {
    if tokio::time::Instant::now() >= deadline {
        return Ok(());
    }
    let signaler = signaler.clone();
    let identity = identity.clone();
    let task = tokio::task::spawn_blocking(move || {
        signaler.signal_attested_until(&identity, signal, deadline.into_std())
    });
    let result = match tokio::time::timeout_at(deadline, task).await {
        Ok(result) => result.map_err(|error| SupervisorError::IdentityChanged {
            detail: format!("attested signaling task failed: {error}"),
        })?,
        Err(_) => return Ok(()),
    };
    match result {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::TimedOut => Ok(()),
        Err(error) => Err(SupervisorError::IdentityChanged {
            detail: error.to_string(),
        }),
    }
}

async fn wait_for_exit_until<P: ProcessInspector>(
    identity: &DaemonIdentity,
    inspector: &P,
    deadline: tokio::time::Instant,
    poll: Duration,
) -> Result<bool, SupervisorError> {
    loop {
        if tokio::time::Instant::now() >= deadline {
            return Ok(false);
        }
        let inspection = match tokio::time::timeout_at(
            deadline,
            inspect_process_supervised(inspector, identity.pid, &identity.executable),
        )
        .await
        {
            Ok(inspection) => inspection,
            Err(_) => return Ok(false),
        };
        match inspection {
            Ok(None) => return Ok(true),
            Ok(Some(process)) => {
                require_endpoint_process_match(identity, &process).map_err(|error| {
                    SupervisorError::IdentityChanged {
                        detail: error.to_string(),
                    }
                })?;
            }
            Err(error) => {
                return Err(SupervisorError::IdentityChanged {
                    detail: error.to_string(),
                });
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(false);
        }
        tokio::time::sleep_until(std::cmp::min(deadline, tokio::time::Instant::now() + poll)).await;
    }
}

async fn confirm_stopped<E, P>(
    paths: &DaemonPaths,
    expected_executable: &Path,
    endpoint: &E,
    inspector: &P,
    deadline: tokio::time::Instant,
    context: &str,
) -> Result<DaemonStatus, SupervisorError>
where
    E: DaemonEndpoint,
    P: ProcessInspector,
{
    let inspected = tokio::time::timeout_at(
        deadline,
        inspect_with(paths, expected_executable, endpoint, inspector),
    )
    .await
    .map_err(|_| SupervisorError::IdentityChanged {
        detail: format!("{context} stopped-state confirmation timed out"),
    })??;
    if inspected.status.state != DaemonState::Stopped {
        return Err(SupervisorError::InvalidState {
            state: inspected.status.state,
            detail: inspected.status.detail,
        });
    }
    Ok(inspected.status)
}

fn daemon_launch(
    paths: &DaemonPaths,
    expected_executable: &Path,
) -> Result<DaemonLaunch, SupervisorError> {
    if !expected_executable.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "daemon executable path must be absolute",
        )
        .into());
    }
    paths.prepare_directory()?;
    let instance_path = std::env::var_os("GASCAN_DAEMON_INSTANCE_PATH")
        .map_or_else(|| paths.instance().to_owned(), PathBuf::from);
    if !instance_path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "daemon instance path must be absolute",
        )
        .into());
    }
    let owner_token = match std::env::var_os("GASCAN_DAEMON_OWNER_TOKEN") {
        Some(value) => value
            .into_string()
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "daemon owner token must be valid UTF-8",
                )
            })?
            .to_owned(),
        None => random_token()?,
    };
    if owner_token.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "daemon owner token must not be empty",
        )
        .into());
    }
    Ok(DaemonLaunch {
        executable: expected_executable.to_owned(),
        current_dir: paths.directory().to_owned(),
        instance_path,
        owner_token,
        stderr_path: std::env::var_os("GASCAN_DAEMON_STDERR_PATH").map(PathBuf::from),
        startup_diagnostic_path: paths.directory().join(STARTUP_DIAGNOSTIC_NAME),
    })
}

fn random_token() -> io::Result<String> {
    let mut random = [0_u8; 32];
    getrandom::fill(&mut random).map_err(io::Error::other)?;
    Ok(gascan_core::hex::lower(&random))
}

async fn classify_connected<C, P: ProcessInspector>(
    expected_executable: &Path,
    record: Option<DaemonInstanceRecord>,
    session: EndpointSession<C>,
    inspector: &P,
) -> Result<Inspection<C>, SupervisorError> {
    let identity = &session.identity;
    let legacy = identity.release_version.is_none();
    let unhealthy = |detail: String,
                     record: Option<DaemonInstanceRecord>,
                     session: EndpointSession<C>| Inspection {
        status: DaemonStatus {
            state: DaemonState::Unhealthy,
            identity: Some(session.identity.clone()),
            legacy,
            detail: Some(detail),
        },
        session: Some(session),
        record,
        interrupted_tombstone: None,
        published_record: None,
    };

    if let Err(error) = validate_endpoint_identity(identity) {
        return Ok(unhealthy(error.to_string(), record, session));
    }
    if !session.safe_transport {
        return Ok(unhealthy(
            "daemon endpoint transport security is unsafe".to_owned(),
            record,
            session,
        ));
    }
    if identity.executable != expected_executable {
        return Ok(unhealthy(
            "daemon endpoint is not the trusted installed executable".to_owned(),
            record,
            session,
        ));
    }
    if let Some(published) = &record {
        if !record_matches_endpoint(published, identity) {
            return Ok(unhealthy(
                "daemon endpoint identity contradicts its protected record".to_owned(),
                record,
                session,
            ));
        }
    }

    let process =
        match inspect_process_supervised(inspector, identity.pid, expected_executable).await {
            Ok(Some(process)) => process,
            Ok(None) => {
                return Ok(unhealthy(
                    "daemon endpoint process is not live".to_owned(),
                    record,
                    session,
                ));
            }
            Err(error) => return Ok(unhealthy(error.to_string(), record, session)),
        };
    if let Err(error) = require_endpoint_process_match(identity, &process) {
        return Ok(unhealthy(error.to_string(), record, session));
    }
    if !session.healthy {
        return Ok(unhealthy(
            "daemon endpoint reported unhealthy state".to_owned(),
            record,
            session,
        ));
    }

    let state = match identity.release_version.as_deref() {
        None => DaemonState::Outdated,
        Some(version) if version != env!("CARGO_PKG_VERSION") => DaemonState::Outdated,
        Some(_) if session.compatible_api => DaemonState::Current,
        Some(_) => {
            return Ok(unhealthy(
                "current daemon release rejected the installed API".to_owned(),
                record,
                session,
            ));
        }
    };
    Ok(Inspection {
        status: DaemonStatus {
            state,
            identity: Some(identity.clone()),
            legacy,
            detail: None,
        },
        session: Some(session),
        record,
        interrupted_tombstone: None,
        published_record: None,
    })
}

async fn classify_unreachable<C, P: ProcessInspector>(
    paths: &DaemonPaths,
    record: Option<DaemonInstanceRecord>,
    inspector: &P,
) -> Result<Inspection<C>, SupervisorError> {
    if let Err(error) = validate_inert_endpoint(paths) {
        return Ok(Inspection {
            status: DaemonStatus {
                state: DaemonState::Unsafe,
                identity: record.as_ref().map(DaemonIdentity::from),
                legacy: false,
                detail: Some(error.to_string()),
            },
            session: None,
            record,
            interrupted_tombstone: None,
            published_record: None,
        });
    }
    let Some(record) = record else {
        return Ok(Inspection {
            status: DaemonStatus::new(DaemonState::Stopped),
            session: None,
            record: None,
            interrupted_tombstone: None,
            published_record: None,
        });
    };
    let identity = DaemonIdentity::from(&record);
    match inspect_process_supervised(inspector, record.pid, &record.executable).await {
        Ok(None) => Ok(Inspection {
            status: DaemonStatus::new(DaemonState::Stopped),
            session: None,
            record: Some(record),
            interrupted_tombstone: None,
            published_record: None,
        }),
        Ok(Some(process)) => match require_identity_match(&record, &process) {
            Ok(()) => Ok(Inspection {
                status: DaemonStatus {
                    state: DaemonState::Unreachable,
                    identity: Some(identity),
                    legacy: false,
                    detail: Some(
                        "protected daemon identity is live but its endpoint is unreachable"
                            .to_owned(),
                    ),
                },
                session: None,
                record: Some(record),
                interrupted_tombstone: None,
                published_record: None,
            }),
            Err(error) => Ok(Inspection {
                status: DaemonStatus {
                    state: DaemonState::Unsafe,
                    identity: Some(identity),
                    legacy: false,
                    detail: Some(error.to_string()),
                },
                session: None,
                record: Some(record),
                interrupted_tombstone: None,
                published_record: None,
            }),
        },
        Err(error) => Ok(Inspection {
            status: DaemonStatus {
                state: DaemonState::Unsafe,
                identity: Some(identity),
                legacy: false,
                detail: Some(error.to_string()),
            },
            session: None,
            record: Some(record),
            interrupted_tombstone: None,
            published_record: None,
        }),
    }
}

async fn classify_unresponsive<C, P: ProcessInspector>(
    paths: &DaemonPaths,
    record: Option<DaemonInstanceRecord>,
    inspector: &P,
    detail: String,
) -> Result<Inspection<C>, SupervisorError> {
    match inspect_endpoint_path(paths) {
        Ok(EndpointPathState::Absent) if record.is_none() => {
            classify_unreachable(paths, record, inspector).await
        }
        Ok(EndpointPathState::Absent | EndpointPathState::SafeSocket(_)) => Ok(Inspection {
            status: DaemonStatus {
                state: DaemonState::Unreachable,
                identity: record.as_ref().map(DaemonIdentity::from),
                legacy: false,
                detail: Some(detail),
            },
            session: None,
            record,
            interrupted_tombstone: None,
            published_record: None,
        }),
        Err(error) => Ok(Inspection {
            status: DaemonStatus {
                state: DaemonState::Unsafe,
                identity: record.as_ref().map(DaemonIdentity::from),
                legacy: false,
                detail: Some(error.to_string()),
            },
            session: None,
            record,
            interrupted_tombstone: None,
            published_record: None,
        }),
    }
}

fn validate_endpoint_identity(identity: &DaemonIdentity) -> io::Result<()> {
    if let Some(release_version) = &identity.release_version {
        semver::Version::parse(release_version).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("daemon endpoint release version is not valid SemVer: {error}"),
            )
        })?;
    }
    if identity.pid == 0
        || !identity.executable.is_absolute()
        || identity.start_identity.is_empty()
        || identity.instance_token.len() != 64
        || !identity
            .instance_token
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        || identity.started_at.as_ref().is_some_and(|timestamp| {
            timestamp.seconds <= 0 || !(0..1_000_000_000).contains(&timestamp.nanos)
        })
        || (identity.release_version.is_some() != identity.started_at.is_some())
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "daemon endpoint identity fields are invalid",
        ));
    }
    Ok(())
}

fn record_matches_endpoint(record: &DaemonInstanceRecord, identity: &DaemonIdentity) -> bool {
    identity.pid == record.pid
        && identity.executable == record.executable
        && identity.start_identity == record.start_identity
        && identity.instance_token == record.instance_token
        && identity.release_version.as_deref() == Some(record.release_version.as_str())
        && identity.started_at.as_ref() == Some(&record.started_at)
}

fn require_endpoint_process_match(
    identity: &DaemonIdentity,
    process: &ProcessIdentity,
) -> io::Result<()> {
    if process.pid != identity.pid
        || process.executable != identity.executable
        || process.start_identity != identity.start_identity
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "daemon endpoint process identity is contradictory",
        ));
    }
    Ok(())
}

fn require_identity_match(
    record: &DaemonInstanceRecord,
    identity: &ProcessIdentity,
) -> io::Result<()> {
    if record.pid == 0
        || identity.pid != record.pid
        || identity.start_identity != record.start_identity
        || identity.executable != record.executable
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "daemon process identity does not match its protected record",
        ));
    }
    Ok(())
}

fn read_instance_record(paths: &DaemonPaths) -> io::Result<Option<DaemonInstanceRecord>> {
    read_instance_record_with_hook(paths, || Ok(()))
}

fn read_instance_record_for_inspection(
    paths: &DaemonPaths,
) -> io::Result<Option<DaemonInstanceRecord>> {
    match read_instance_record_with_hook_and_directory_mode(paths, || Ok(()), false) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        result => result,
    }
}

fn open_published_record(
    paths: &DaemonPaths,
    expected_record: &DaemonInstanceRecord,
) -> io::Result<InterruptedTombstone> {
    let (parent, name) = instance_parent_and_name(paths.instance())?;
    let directory = open_private_directory_with_mode(parent, paths.expected_uid, false)?;
    let fd = rustix::fs::openat(
        &directory,
        name,
        OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(errno)?;
    let identity = validate_open_file(&directory, name, &fd, paths.expected_uid)?;
    let stat = rustix::fs::fstat(&fd).map_err(errno)?;
    let size = stat.st_size as u64;
    if size > MAX_INSTANCE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "daemon instance record is too large",
        ));
    }
    let file = File::from(fd);
    let actual_record = read_record_from_held_file(&file, size)?;
    if &actual_record != expected_record {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "daemon instance record changed while binding its descriptor",
        ));
    }
    let rechecked =
        rustix::fs::statat(&directory, name, AtFlags::SYMLINK_NOFOLLOW).map_err(errno)?;
    validate_file_stat(&rechecked, paths.expected_uid)?;
    if (FileIdentity {
        device: rechecked.st_dev as u64,
        inode: rechecked.st_ino,
    }) != identity
        || rechecked.st_size as u64 != size
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "daemon instance path changed while binding its descriptor",
        ));
    }
    Ok(InterruptedTombstone {
        directory,
        name: name.to_owned(),
        file,
        identity,
        expected_uid: paths.expected_uid,
        size,
    })
}

fn read_record_from_held_file(file: &File, size: u64) -> io::Result<DaemonInstanceRecord> {
    let size = usize::try_from(size).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "daemon instance record does not fit in memory",
        )
    })?;
    let mut bytes = vec![0_u8; size];
    let mut offset = 0;
    while offset < bytes.len() {
        let read = file.read_at(&mut bytes[offset..], offset as u64)?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "daemon instance record was truncated while reading it",
            ));
        }
        offset += read;
    }
    let record: DaemonInstanceRecord = serde_json::from_slice(&bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    validate_record(&record)?;
    Ok(record)
}

fn open_interrupted_tombstone(paths: &DaemonPaths) -> io::Result<Option<InterruptedTombstone>> {
    let (parent, name) = instance_parent_and_name(paths.instance())?;
    let directory = match open_private_directory_with_mode(parent, paths.expected_uid, false) {
        Ok(directory) => directory,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let initial = match rustix::fs::statat(&directory, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => stat,
        Err(error) if error == rustix::io::Errno::NOENT => return Ok(None),
        Err(error) => return Err(errno(error)),
    };
    if !is_interrupted_tombstone(&initial, paths.expected_uid) {
        return Ok(None);
    }
    if initial.st_size as u64 > MAX_INSTANCE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "interrupted daemon instance record is too large",
        ));
    }
    let identity = FileIdentity {
        device: initial.st_dev as u64,
        inode: initial.st_ino,
    };
    let fd = rustix::fs::openat(
        &directory,
        name,
        OFlags::WRONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(errno)?;
    let opened = rustix::fs::fstat(&fd).map_err(errno)?;
    if !is_interrupted_tombstone(&opened, paths.expected_uid)
        || (FileIdentity {
            device: opened.st_dev as u64,
            inode: opened.st_ino,
        }) != identity
        || opened.st_size != initial.st_size
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "interrupted daemon instance descriptor changed while opening it",
        ));
    }
    let rechecked =
        rustix::fs::statat(&directory, name, AtFlags::SYMLINK_NOFOLLOW).map_err(errno)?;
    if !is_interrupted_tombstone(&rechecked, paths.expected_uid)
        || (FileIdentity {
            device: rechecked.st_dev as u64,
            inode: rechecked.st_ino,
        }) != identity
        || rechecked.st_size != initial.st_size
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "interrupted daemon instance path changed while opening it",
        ));
    }
    Ok(Some(InterruptedTombstone {
        directory,
        name: name.to_owned(),
        file: File::from(fd),
        identity,
        expected_uid: paths.expected_uid,
        size: initial.st_size as u64,
    }))
}

fn is_interrupted_tombstone(stat: &rustix::fs::Stat, expected_uid: u32) -> bool {
    FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile
        && stat.st_uid == expected_uid
        && stat.st_nlink == 1
        && Mode::from_raw_mode(stat.st_mode).bits() & 0o777 == INSTANCE_TOMBSTONE_MODE
        && stat.st_size > 0
}

fn read_instance_record_with_hook<F>(
    paths: &DaemonPaths,
    between_identity_and_open: F,
) -> io::Result<Option<DaemonInstanceRecord>>
where
    F: FnOnce() -> io::Result<()>,
{
    read_instance_record_with_hook_and_directory_mode(paths, between_identity_and_open, true)
}

fn read_instance_record_with_hook_and_directory_mode<F>(
    paths: &DaemonPaths,
    between_identity_and_open: F,
    create_directory: bool,
) -> io::Result<Option<DaemonInstanceRecord>>
where
    F: FnOnce() -> io::Result<()>,
{
    let (parent, name) = instance_parent_and_name(paths.instance())?;
    let directory = open_private_directory_with_mode(parent, paths.expected_uid, create_directory)?;
    let initial_stat = match rustix::fs::statat(&directory, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => stat,
        Err(error) if error == rustix::io::Errno::NOENT => return Ok(None),
        Err(error) => return Err(errno(error)),
    };
    if is_instance_tombstone(&initial_stat, paths.expected_uid) {
        validate_instance_tombstone(&directory, name, &initial_stat, paths.expected_uid)?;
        return Ok(None);
    }
    let expected = file_identity_at(&directory, name, paths.expected_uid)?;
    between_identity_and_open()?;
    let fd = rustix::fs::openat(
        &directory,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(errno)?;
    let actual = validate_open_file(&directory, name, &fd, paths.expected_uid)?;
    if actual != expected {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "daemon instance record changed while opening it",
        ));
    }
    let mut bytes = Vec::new();
    File::from(fd)
        .take(MAX_INSTANCE_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_INSTANCE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "daemon instance record is too large",
        ));
    }
    if file_identity_at(&directory, name, paths.expected_uid)? != expected {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "daemon instance record changed while reading it",
        ));
    }
    let record: DaemonInstanceRecord = serde_json::from_slice(&bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    validate_record(&record)?;
    Ok(Some(record))
}

fn is_instance_tombstone(stat: &rustix::fs::Stat, expected_uid: u32) -> bool {
    FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile
        && stat.st_uid == expected_uid
        && stat.st_nlink == 1
        && Mode::from_raw_mode(stat.st_mode).bits() & 0o777 == INSTANCE_TOMBSTONE_MODE
        && stat.st_size == 0
}

fn validate_instance_tombstone(
    directory: &OwnedFd,
    name: &OsStr,
    initial_stat: &rustix::fs::Stat,
    expected_uid: u32,
) -> io::Result<()> {
    let expected = FileIdentity {
        device: initial_stat.st_dev as u64,
        inode: initial_stat.st_ino,
    };
    let fd = rustix::fs::openat(
        directory,
        name,
        OFlags::WRONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(errno)?;
    let opened = rustix::fs::fstat(&fd).map_err(errno)?;
    if !is_instance_tombstone(&opened, expected_uid)
        || (FileIdentity {
            device: opened.st_dev as u64,
            inode: opened.st_ino,
        }) != expected
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "daemon instance tombstone changed while opening it",
        ));
    }
    let rechecked =
        rustix::fs::statat(directory, name, AtFlags::SYMLINK_NOFOLLOW).map_err(errno)?;
    if !is_instance_tombstone(&rechecked, expected_uid)
        || (FileIdentity {
            device: rechecked.st_dev as u64,
            inode: rechecked.st_ino,
        }) != expected
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "daemon instance tombstone changed while validating it",
        ));
    }
    Ok(())
}

fn validate_record(record: &DaemonInstanceRecord) -> io::Result<()> {
    semver::Version::parse(&record.release_version).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("daemon record release version is not valid SemVer: {error}"),
        )
    })?;
    if record.pid == 0
        || record.owner_token.is_empty()
        || !record.executable.is_absolute()
        || record.start_identity.is_empty()
        || record.instance_token.len() != 64
        || !record
            .instance_token
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        || record.started_at.seconds <= 0
        || !(0..1_000_000_000).contains(&record.started_at.nanos)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "daemon instance record fields are invalid",
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EndpointPathState {
    Absent,
    SafeSocket(FileIdentity),
}

fn inspect_endpoint_path(paths: &DaemonPaths) -> io::Result<EndpointPathState> {
    let directory =
        match open_private_directory_with_mode(paths.directory(), paths.expected_uid, false) {
            Ok(directory) => directory,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(EndpointPathState::Absent);
            }
            Err(error) => return Err(error),
        };
    let stat = match rustix::fs::statat(
        &directory,
        OsStr::new(SOCKET_NAME),
        AtFlags::SYMLINK_NOFOLLOW,
    ) {
        Ok(stat) => stat,
        Err(error) if error == rustix::io::Errno::NOENT => {
            return Ok(EndpointPathState::Absent);
        }
        Err(error) => return Err(errno(error)),
    };
    if FileType::from_raw_mode(stat.st_mode) != FileType::Socket
        || stat.st_uid != paths.expected_uid
        || stat.st_nlink != 1
        || Mode::from_raw_mode(stat.st_mode).bits() & 0o777 != FILE_MODE
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "daemon endpoint ownership, type, links, or mode is unsafe",
        ));
    }
    Ok(EndpointPathState::SafeSocket(FileIdentity {
        device: stat.st_dev as u64,
        inode: stat.st_ino,
    }))
}

pub(crate) fn validate_endpoint_path_identity(
    paths: &DaemonPaths,
    expected: FileIdentity,
) -> io::Result<()> {
    match inspect_endpoint_path(paths)? {
        EndpointPathState::SafeSocket(observed) if observed == expected => Ok(()),
        EndpointPathState::Absent | EndpointPathState::SafeSocket(_) => Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "daemon endpoint pathname changed before transport authentication completed",
        )),
    }
}

fn validate_inert_endpoint(paths: &DaemonPaths) -> io::Result<()> {
    inspect_endpoint_path(paths).map(drop)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FileIdentity {
    device: u64,
    inode: u64,
}

fn instance_parent_and_name(path: &Path) -> io::Result<(&Path, &OsStr)> {
    if !path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "daemon instance path must be absolute",
        ));
    }
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "daemon instance path has no parent",
        )
    })?;
    let name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "daemon instance path has no file name",
        )
    })?;
    Ok((parent, name))
}

fn open_private_directory(path: &Path, expected_uid: u32) -> io::Result<OwnedFd> {
    open_private_directory_with_mode(path, expected_uid, true)
}

fn open_private_directory_with_mode(
    path: &Path,
    expected_uid: u32,
    create_final: bool,
) -> io::Result<OwnedFd> {
    open_private_directory_with_mode_and_create_hook(path, expected_uid, create_final, || Ok(()))
}

#[cfg(test)]
fn open_private_directory_with_create_hook<F>(
    path: &Path,
    expected_uid: u32,
    before_create: F,
) -> io::Result<OwnedFd>
where
    F: FnOnce() -> io::Result<()>,
{
    open_private_directory_with_mode_and_create_hook(path, expected_uid, true, before_create)
}

fn open_private_directory_with_mode_and_create_hook<F>(
    path: &Path,
    expected_uid: u32,
    create_final: bool,
    before_create: F,
) -> io::Result<OwnedFd>
where
    F: FnOnce() -> io::Result<()>,
{
    if !path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "runtime directory must be absolute",
        ));
    }
    let mut components = path.components().peekable();
    if components.next() != Some(Component::RootDir) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "runtime directory must be absolute",
        ));
    }
    let mut directory = rustix::fs::open(
        "/",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(errno)?;
    let mut before_create = Some(before_create);
    while let Some(component) = components.next() {
        let Component::Normal(name) = component else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "runtime directory contains a non-normal component",
            ));
        };
        let final_component = components.peek().is_none();
        match rustix::fs::openat(
            &directory,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(next) => directory = next,
            Err(error) if create_final && final_component && error == rustix::io::Errno::NOENT => {
                if let Some(before_create) = before_create.take() {
                    before_create()?;
                }
                let created = match rustix::fs::mkdirat(
                    &directory,
                    name,
                    Mode::from_raw_mode(DIRECTORY_MODE),
                ) {
                    Ok(()) => true,
                    Err(error) if error == rustix::io::Errno::EXIST => false,
                    Err(error) => return Err(errno(error)),
                };
                let next = rustix::fs::openat(
                    &directory,
                    name,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(errno)?;
                if created {
                    rustix::fs::fchmod(&next, Mode::from_raw_mode(DIRECTORY_MODE))
                        .map_err(errno)?;
                }
                directory = next;
            }
            Err(error) => return Err(errno(error)),
        }
    }
    let stat = rustix::fs::fstat(&directory).map_err(errno)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::Directory
        || stat.st_uid != expected_uid
        || Mode::from_raw_mode(stat.st_mode).bits() & 0o777 != DIRECTORY_MODE
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "runtime directory ownership or mode is unsafe",
        ));
    }
    Ok(directory)
}

fn open_lock(directory: &OwnedFd, expected_uid: u32) -> io::Result<OwnedFd> {
    match rustix::fs::openat(
        directory,
        LIFECYCLE_LOCK_NAME,
        OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(FILE_MODE),
    ) {
        Ok(fd) => {
            rustix::fs::fchmod(&fd, Mode::from_raw_mode(FILE_MODE)).map_err(errno)?;
            Ok(fd)
        }
        Err(error) if error == rustix::io::Errno::EXIST => rustix::fs::openat(
            directory,
            LIFECYCLE_LOCK_NAME,
            OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(errno),
        Err(error) => Err(errno(error)),
    }
    .and_then(|fd| {
        validate_open_file(
            directory,
            OsStr::new(LIFECYCLE_LOCK_NAME),
            &fd,
            expected_uid,
        )?;
        Ok(fd)
    })
}

fn validate_open_file(
    directory: &OwnedFd,
    name: &OsStr,
    fd: &OwnedFd,
    expected_uid: u32,
) -> io::Result<FileIdentity> {
    let stat = rustix::fs::fstat(fd).map_err(errno)?;
    validate_file_stat(&stat, expected_uid)?;
    let identity = FileIdentity {
        device: stat.st_dev as u64,
        inode: stat.st_ino,
    };
    if file_identity_at(directory, name, expected_uid)? != identity {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "protected runtime file changed while opening it",
        ));
    }
    Ok(identity)
}

fn file_identity_at(
    directory: &OwnedFd,
    name: &OsStr,
    expected_uid: u32,
) -> io::Result<FileIdentity> {
    let stat = rustix::fs::statat(directory, name, AtFlags::SYMLINK_NOFOLLOW).map_err(errno)?;
    validate_file_stat(&stat, expected_uid)?;
    Ok(FileIdentity {
        device: stat.st_dev as u64,
        inode: stat.st_ino,
    })
}

fn validate_file_stat(stat: &rustix::fs::Stat, expected_uid: u32) -> io::Result<()> {
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
        || stat.st_uid != expected_uid
        || stat.st_nlink != 1
        || Mode::from_raw_mode(stat.st_mode).bits() & 0o777 != FILE_MODE
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "protected runtime file ownership, type, links, or mode is unsafe",
        ));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn inspect_process(pid: u32, expected_executable: &Path) -> io::Result<Option<ProcessIdentity>> {
    if pid == 0 {
        return Ok(None);
    }
    let process = PathBuf::from("/proc").join(pid.to_string());
    let Some(start_identity) = linux_start_identity(&process)? else {
        return Ok(None);
    };
    let executable = match std::fs::read_link(process.join("exe")) {
        Ok(value) => value.canonicalize()?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let Some(rechecked_start) = linux_start_identity(&process)? else {
        return Ok(None);
    };
    coherent_process_identity(
        pid,
        expected_executable,
        start_identity,
        executable,
        rechecked_start,
    )
    .map(Some)
}

#[cfg(target_os = "linux")]
fn linux_start_identity(process: &Path) -> io::Result<Option<String>> {
    let stat = match std::fs::read_to_string(process.join("stat")) {
        Ok(value) => value,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let remainder = stat
        .rsplit_once(") ")
        .map(|(_, value)| value)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "malformed process stat identity",
            )
        })?;
    let start = remainder.split_whitespace().nth(19).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "process stat lacks start identity",
        )
    })?;
    Ok(Some(format!("linux:{start}")))
}

fn coherent_process_identity(
    pid: u32,
    expected_executable: &Path,
    start_identity: String,
    executable: PathBuf,
    rechecked_start: String,
) -> io::Result<ProcessIdentity> {
    if start_identity != rechecked_start {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "process identity changed during inspection",
        ));
    }
    if executable != expected_executable {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "live process executable does not match daemon attestation",
        ));
    }
    Ok(ProcessIdentity {
        pid,
        executable,
        start_identity,
    })
}

#[cfg(target_os = "macos")]
fn inspect_process(pid: u32, expected_executable: &Path) -> io::Result<Option<ProcessIdentity>> {
    inspect_process_with(
        pid,
        expected_executable,
        || ps_field(pid, "lstart="),
        || lsof_executable(pid),
    )
}

#[cfg(target_os = "macos")]
fn inspect_process_with<S, E>(
    pid: u32,
    expected_executable: &Path,
    mut start: S,
    mut executable: E,
) -> io::Result<Option<ProcessIdentity>>
where
    S: FnMut() -> io::Result<Option<String>>,
    E: FnMut() -> io::Result<Option<PathBuf>>,
{
    if pid == 0 {
        return Ok(None);
    }
    let Some(start_identity) = start()? else {
        return Ok(None);
    };
    let Some(executable) = executable()? else {
        return Ok(None);
    };
    let Some(rechecked_start) = start()? else {
        return Ok(None);
    };
    coherent_process_identity(
        pid,
        expected_executable,
        start_identity,
        executable,
        rechecked_start,
    )
    .map(Some)
}

#[cfg(target_os = "macos")]
fn lsof_executable(pid: u32) -> io::Result<Option<PathBuf>> {
    use std::process::Stdio;
    let mut child = std::process::Command::new("/usr/sbin/lsof")
        .args(["-a", "-p", &pid.to_string(), "-d", "txt", "-F0fn"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let deadline = Instant::now() + PROCESS_INSPECTION_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait()? {
            let output = child.wait_with_output()?;
            if !status.success() {
                return Ok(None);
            }
            return parse_lsof_executable(&output.stdout, pid);
        }
        if Instant::now() >= deadline {
            child.kill()?;
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "process executable inspection timed out",
            ));
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(target_os = "macos")]
fn parse_lsof_executable(output: &[u8], pid: u32) -> io::Result<Option<PathBuf>> {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt as _;

    if output
        .iter()
        .all(|byte| *byte == 0 || byte.is_ascii_whitespace())
    {
        return Ok(None);
    }
    let mut matched_process = false;
    let mut awaiting_text_name = false;
    for raw_field in output.split(|byte| *byte == 0) {
        let field = raw_field.strip_prefix(b"\n").unwrap_or(raw_field);
        let field = field.strip_prefix(b"\r").unwrap_or(field);
        if let Some(value) = field.strip_prefix(b"p") {
            let observed = std::str::from_utf8(value)
                .ok()
                .and_then(|value| value.parse::<u32>().ok());
            if observed != Some(pid) {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "process executable inspection returned a different process",
                ));
            }
            matched_process = true;
            awaiting_text_name = false;
        } else if field == b"ftxt" && matched_process {
            awaiting_text_name = true;
        } else if awaiting_text_name {
            let Some(path) = field.strip_prefix(b"n") else {
                continue;
            };
            let path = PathBuf::from(OsString::from_vec(path.to_vec()));
            if !path.is_absolute() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "process executable inspection returned a relative path",
                ));
            }
            return Ok(Some(path));
        }
    }
    if matched_process {
        Ok(None)
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "process executable inspection omitted the process record",
        ))
    }
}

#[cfg(target_os = "macos")]
fn ps_field(pid: u32, field: &str) -> io::Result<Option<String>> {
    use std::process::Stdio;
    let mut child = std::process::Command::new("/bin/ps")
        .args(["-ww", "-p", &pid.to_string(), "-o", field])
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .env("TZ", "UTC")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let deadline = Instant::now() + PROCESS_INSPECTION_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait()? {
            let output = child.wait_with_output()?;
            if !status.success() {
                return Ok(None);
            }
            let value = String::from_utf8(output.stdout)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            let value = value.trim().to_owned();
            return if value.is_empty() {
                Ok(None)
            } else {
                Ok(Some(value))
            };
        }
        if Instant::now() >= deadline {
            child.kill()?;
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "process inspection timed out",
            ));
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn inspect_process(_pid: u32, _expected_executable: &Path) -> io::Result<Option<ProcessIdentity>> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "daemon process attestation is supported only on Linux and macOS",
    ))
}

fn errno(error: rustix::io::Errno) -> io::Error {
    io::Error::from_raw_os_error(error.raw_os_error())
}

#[cfg(test)]
mod tests {
    use super::{
        AttestedProcessSignaler, ConnectionOutcome, DaemonEndpoint, DaemonIdentity,
        DaemonInstanceRecord, DaemonLaunch, DaemonLifecycleObserver, DaemonPaths, DaemonSpawner,
        DaemonStartupMonitor, DaemonState, DaemonTransition, EndpointPathState, EndpointProbe,
        EndpointSession, InstanceTimestamp, MAX_STARTUP_DIAGNOSTIC_BYTES, OsProcessInspector,
        ProcessIdentity, ProcessInspector, ProcessSignaler, ShutdownPolicy, StopMode,
        SupervisorError, SupervisorTimeouts, checked_pid, coherent_process_identity,
        connect_current_or_recover_with, connect_current_or_recover_with_observer, inspect_with,
        read_attested_instance, read_instance_record_with_hook, restart_with, signal_attested_with,
        signal_attested_with_deadline, signal_identity, start_with, stop_with, wait_for_exit_until,
    };
    #[cfg(target_os = "macos")]
    use super::{inspect_process_with, parse_lsof_executable};
    use std::collections::VecDeque;
    use std::fs;
    use std::io;
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::Duration;
    use tokio::sync::Notify;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn root(temp: &tempfile::TempDir) -> io::Result<PathBuf> {
        temp.path().canonicalize()
    }

    fn record(executable: &Path) -> DaemonInstanceRecord {
        DaemonInstanceRecord {
            pid: std::process::id(),
            owner_token: "owner-token".to_owned(),
            executable: executable.to_owned(),
            start_identity: "start-identity".to_owned(),
            instance_token: "11".repeat(32),
            release_version: env!("CARGO_PKG_VERSION").to_owned(),
            started_at: InstanceTimestamp {
                seconds: 1_785_263_800,
                nanos: 123_000_000,
            },
        }
    }

    fn write_record(paths: &DaemonPaths, record: &DaemonInstanceRecord) -> TestResult {
        paths.prepare_directory()?;
        fs::write(paths.instance(), serde_json::to_vec(record)?)?;
        fs::set_permissions(paths.instance(), fs::Permissions::from_mode(0o600))?;
        Ok(())
    }

    #[test]
    fn startup_diagnostics_accept_only_known_controller_codes() -> TestResult {
        use std::io::Write as _;

        let temp = tempfile::tempdir()?;
        let diagnostic = temp.path().join("startup.json");
        fs::write(
            &diagnostic,
            b"noise\nGASCAN_CONTROLLER_STARTUP_ERROR {\"code\":\"arbitrary_code\",\"message\":\"forged\",\"owner_token\":\"owner\"}\nGASCAN_CONTROLLER_STARTUP_ERROR {\"code\":\"controller_state_unsafe\",\"message\":\"wrong launch\",\"owner_token\":\"other\"}\n",
        )?;
        fs::set_permissions(&diagnostic, fs::Permissions::from_mode(0o600))?;
        let mut writer = std::fs::OpenOptions::new()
            .read(true)
            .append(true)
            .open(&diagnostic)?;
        let monitor = DaemonStartupMonitor::from_file(writer.try_clone()?, "owner".to_owned());
        fs::remove_file(&diagnostic)?;
        assert!(monitor.controller_error()?.is_none());

        writer.write_all(
            b"GASCAN_CONTROLLER_STARTUP_ERROR {\"code\":\"controller_state_unsafe\",\"message\":\"application directory mode is unsafe\",\"owner_token\":\"owner\"}\n",
        )?;
        let error = monitor
            .controller_error()?
            .ok_or("controller diagnostic missing")?;
        assert!(matches!(
            error,
            SupervisorError::ControllerStartup { code, message }
                if code == "controller_state_unsafe"
                    && message == "application directory mode is unsafe"
        ));

        writer.set_len(u64::try_from(MAX_STARTUP_DIAGNOSTIC_BYTES + 1)?)?;
        assert!(monitor.controller_error()?.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn inherited_startup_diagnostic_survives_path_replacement() -> TestResult {
        use std::os::unix::fs::PermissionsExt as _;

        let temp = tempfile::tempdir()?;
        let root = temp.path().canonicalize()?;
        let runtime = root.join("runtime");
        fs::create_dir(&runtime)?;
        fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700))?;
        let startup_path = runtime.join("daemon-startup-error.json");
        let script = root.join("fixture-gascand");
        fs::write(
            &script,
            "#!/bin/sh\nreplacement_path=\"$PWD/daemon-startup-error.json\"\nif [ -e \"$replacement_path\" ]; then path_state=present; else path_state=missing; fi\nprintf 'replacement-path' > \"$replacement_path\"\nprintf '%s\\n' 'GASCAN_CONTROLLER_STARTUP_ERROR {\"code\":\"controller_state_unsafe\",\"message\":\"trusted inherited descriptor\",\"owner_token\":\"test-owner\"}' >> \"/dev/fd/$GASCAN_CONTROLLER_STARTUP_FD\"\nprintf '%s\\n%s\\n' \"$path_state\" \"$GASCAN_CONTROLLER_STARTUP_FD\" >&2\n",
        )?;
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700))?;
        let stderr_path = root.join("daemon.stderr");
        let launch = DaemonLaunch {
            executable: script,
            current_dir: runtime.clone(),
            instance_path: runtime.join("daemon-instance.json"),
            owner_token: "test-owner".to_owned(),
            stderr_path: Some(stderr_path.clone()),
            startup_diagnostic_path: startup_path.clone(),
        };

        let monitor = DaemonSpawner::spawn(&crate::client::TokioDaemonSpawner, &launch)?;
        let deadline =
            tokio::time::Instant::now() + crate::client::FIXTURE_DAEMON_DIAGNOSTIC_DEADLINE;
        let error = loop {
            if let Some(error) = monitor.controller_error()? {
                break error;
            }
            if tokio::time::Instant::now() >= deadline {
                let stderr = fs::read_to_string(&stderr_path).unwrap_or_default();
                let replacement = fs::read(&startup_path).unwrap_or_default();
                return Err(format!(
                    "fixture daemon did not write its inherited diagnostic within {:?}: stderr={stderr:?}, replacement={replacement:?}",
                    crate::client::FIXTURE_DAEMON_DIAGNOSTIC_DEADLINE
                )
                .into());
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        assert!(matches!(
            error,
            SupervisorError::ControllerStartup { ref code, ref message }
                if code == "controller_state_unsafe"
                    && message == "trusted inherited descriptor"
        ));
        assert_eq!(fs::read(&startup_path)?, b"replacement-path");
        let stderr = fs::read_to_string(stderr_path)?;
        let mut lines = stderr.lines();
        assert_eq!(lines.next(), Some("missing"));
        assert!(
            lines
                .next()
                .and_then(|fd| fd.parse::<i32>().ok())
                .is_some_and(|fd| fd >= 3)
        );
        Ok(())
    }

    #[test]
    fn runtime_paths_create_private_directory_and_owned_lock() -> TestResult {
        let temp = tempfile::tempdir()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        let lock = paths.lock()?;
        let directory = fs::symlink_metadata(paths.directory())?;
        let lock_metadata = fs::symlink_metadata(paths.lifecycle_lock())?;
        assert_eq!(directory.permissions().mode() & 0o777, 0o700);
        assert_eq!(directory.uid(), rustix::process::geteuid().as_raw());
        assert_eq!(lock_metadata.permissions().mode() & 0o777, 0o600);
        assert_eq!(lock_metadata.uid(), rustix::process::geteuid().as_raw());
        assert!(lock_metadata.file_type().is_file());
        assert!(!lock_metadata.file_type().is_symlink());
        drop(lock);
        Ok(())
    }

    #[test]
    fn runtime_concurrent_directory_creation_reopens_and_validates_the_winner() -> TestResult {
        let temp = tempfile::tempdir()?;
        let runtime = root(&temp)?.join("runtime");
        let uid = rustix::process::geteuid().as_raw();
        let directory = super::open_private_directory_with_create_hook(&runtime, uid, || {
            fs::create_dir(&runtime)?;
            fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700))
        })?;
        drop(directory);

        let unsafe_runtime = root(&temp)?.join("unsafe-runtime");
        let result = super::open_private_directory_with_create_hook(&unsafe_runtime, uid, || {
            fs::create_dir(&unsafe_runtime)?;
            fs::set_permissions(&unsafe_runtime, fs::Permissions::from_mode(0o755))
        });
        assert!(result.is_err(), "an unsafe mkdir winner was repaired");
        Ok(())
    }

    #[test]
    fn runtime_lock_serializes_and_contender_rechecks_after_acquiring() -> TestResult {
        let temp = tempfile::tempdir()?;
        let paths = Arc::new(DaemonPaths::from_runtime_root(root(&temp)?.join("runtime")));
        let first = paths.lock()?;
        let state_changed = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (observed_tx, observed_rx) = std::sync::mpsc::channel();
        let contender_paths = Arc::clone(&paths);
        let contender_state = Arc::clone(&state_changed);
        let contender = std::thread::spawn(move || -> io::Result<()> {
            ready_tx
                .send(())
                .map_err(|_| io::Error::other("lock readiness receiver closed"))?;
            let _second = contender_paths.lock()?;
            observed_tx
                .send(contender_state.load(Ordering::Acquire))
                .map_err(|_| io::Error::other("lock observation receiver closed"))?;
            Ok(())
        });
        ready_rx.recv_timeout(Duration::from_secs(1))?;
        assert!(
            observed_rx.recv_timeout(Duration::from_millis(50)).is_err(),
            "the second contender acquired while the first held the lifecycle lock"
        );
        state_changed.store(true, Ordering::Release);
        drop(first);
        assert!(observed_rx.recv_timeout(Duration::from_secs(1))?);
        contender
            .join()
            .map_err(|_| io::Error::other("lock contender panicked"))??;
        Ok(())
    }

    #[test]
    fn runtime_paths_fail_closed_for_unsafe_directory_or_lock() -> TestResult {
        let temp = tempfile::tempdir()?;
        let base = root(&temp)?;

        let relative = DaemonPaths::from_runtime_root(PathBuf::from("relative"));
        assert!(relative.prepare_directory().is_err());

        let permissive_root = base.join("permissive");
        fs::create_dir(&permissive_root)?;
        fs::set_permissions(&permissive_root, fs::Permissions::from_mode(0o755))?;
        assert!(
            DaemonPaths::from_runtime_root(permissive_root)
                .prepare_directory()
                .is_err()
        );

        let target = base.join("target");
        fs::create_dir(&target)?;
        fs::set_permissions(&target, fs::Permissions::from_mode(0o700))?;
        let linked = base.join("linked");
        std::os::unix::fs::symlink(&target, &linked)?;
        assert!(
            DaemonPaths::from_runtime_root(linked)
                .prepare_directory()
                .is_err()
        );

        let wrong_owner = DaemonPaths::from_runtime_root_with_uid(
            base.join("wrong-owner"),
            rustix::process::geteuid().as_raw().saturating_add(1),
        );
        assert!(wrong_owner.prepare_directory().is_err());

        let lock_paths = DaemonPaths::from_runtime_root(base.join("unsafe-lock"));
        lock_paths.prepare_directory()?;
        fs::write(lock_paths.lifecycle_lock(), b"")?;
        fs::set_permissions(
            lock_paths.lifecycle_lock(),
            fs::Permissions::from_mode(0o644),
        )?;
        assert!(lock_paths.lock().is_err());

        fs::remove_file(lock_paths.lifecycle_lock())?;
        std::os::unix::fs::symlink(base.join("elsewhere"), lock_paths.lifecycle_lock())?;
        assert!(lock_paths.lock().is_err());
        Ok(())
    }

    #[test]
    fn runtime_lock_acquisition_is_bounded() -> TestResult {
        let temp = tempfile::tempdir()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        let _first = paths.lock()?;
        let error = match paths.lock_with_timeout(Duration::from_millis(20)) {
            Ok(_) => return Err("contended lifecycle lock unexpectedly succeeded".into()),
            Err(error) => error,
        };
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        Ok(())
    }

    #[test]
    fn runtime_default_lock_budget_outlasts_the_longest_default_transition() {
        let timeouts = SupervisorTimeouts::default();
        let forced_restart = timeouts.shutdown * 2 + timeouts.readiness;

        assert!(
            super::LIFECYCLE_LOCK_TIMEOUT > forced_restart,
            "a contender must be able to wait for force shutdown plus replacement readiness"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn runtime_async_lock_wait_does_not_starve_the_executor() -> TestResult {
        let temp = tempfile::tempdir()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        let first = paths.lock()?;

        let release = async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            drop(first);
        };
        let contender = paths.lock_async_with_timeout(Duration::from_millis(100));
        let ((), contender) = tokio::join!(release, contender);

        drop(contender?);
        Ok(())
    }

    #[derive(Clone)]
    struct FakeInspector {
        identity: Option<ProcessIdentity>,
    }

    impl ProcessInspector for FakeInspector {
        fn inspect(
            &self,
            _pid: u32,
            _expected_executable: &Path,
        ) -> io::Result<Option<ProcessIdentity>> {
            Ok(self.identity.clone())
        }
    }

    #[derive(Clone)]
    struct StallingInspector {
        identity: Option<ProcessIdentity>,
        delay: Duration,
        timer_progressed: Arc<AtomicBool>,
    }

    impl ProcessInspector for StallingInspector {
        fn inspect(
            &self,
            _pid: u32,
            _expected_executable: &Path,
        ) -> io::Result<Option<ProcessIdentity>> {
            std::thread::sleep(self.delay);
            assert!(
                self.timer_progressed.load(Ordering::Acquire),
                "a synchronous process inspection blocked current-thread timer/lock progress"
            );
            Ok(self.identity.clone())
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn process_inspection_does_not_block_current_thread_timer_or_lock_progress() -> TestResult
    {
        let temp = tempfile::tempdir()?;
        let executable = std::env::current_exe()?.canonicalize()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        let independent_paths =
            DaemonPaths::from_runtime_root(root(&temp)?.join("independent-runtime"));
        let expected = record(&executable);
        write_record(&paths, &expected)?;
        let timer_progressed = Arc::new(AtomicBool::new(false));
        let inspector = StallingInspector {
            identity: Some(process_for(&endpoint_identity(&expected))),
            delay: Duration::from_millis(100),
            timer_progressed: Arc::clone(&timer_progressed),
        };
        let endpoint = FakeEndpoint::new(EndpointProbe::AbsentOrInert);

        let inspection = inspect_with(&paths, &executable, &endpoint, &inspector);
        let runtime_progress = async {
            tokio::time::sleep(Duration::from_millis(10)).await;
            let lock = independent_paths
                .lock_async_with_timeout(Duration::from_millis(50))
                .await?;
            timer_progressed.store(true, Ordering::Release);
            drop(lock);
            Ok::<(), io::Error>(())
        };
        let (inspection, runtime_progress) = tokio::join!(inspection, runtime_progress);

        runtime_progress?;
        assert_eq!(inspection?.status.state, DaemonState::Unreachable);
        Ok(())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn wait_for_exit_does_not_overrun_a_stalled_inspection_deadline() -> TestResult {
        let executable = std::env::current_exe()?.canonicalize()?;
        let expected = record(&executable);
        let identity = endpoint_identity(&expected);
        let inspector = StallingInspector {
            identity: Some(process_for(&identity)),
            delay: Duration::from_millis(200),
            timer_progressed: Arc::new(AtomicBool::new(true)),
        };
        let started = std::time::Instant::now();

        let exited = wait_for_exit_until(
            &identity,
            &inspector,
            tokio::time::Instant::now() + Duration::from_millis(20),
            Duration::from_millis(1),
        )
        .await?;

        assert!(!exited);
        assert!(
            started.elapsed() < Duration::from_millis(100),
            "wait_for_exit overran its deadline by waiting for a stalled inspector"
        );
        Ok(())
    }

    fn publish_fake_socket_for_connected_probe(
        path: &Path,
        probe: &EndpointProbe<()>,
    ) -> io::Result<()> {
        if !matches!(probe, EndpointProbe::Connected(_)) {
            return Ok(());
        }
        match fs::symlink_metadata(path) {
            Ok(_) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("fake endpoint socket has no parent"))?;
        fs::create_dir_all(parent)?;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        let listener = std::os::unix::net::UnixListener::bind(path)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        drop(listener);
        Ok(())
    }

    #[derive(Clone)]
    struct FakeEndpoint {
        probe: EndpointProbe<()>,
        probes: Arc<AtomicUsize>,
    }

    impl FakeEndpoint {
        fn new(probe: EndpointProbe<()>) -> Self {
            Self {
                probe,
                probes: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    #[tonic::async_trait]
    impl DaemonEndpoint for FakeEndpoint {
        type Connection = ();

        async fn probe(
            &self,
            paths: &DaemonPaths,
            _expected_path: EndpointPathState,
        ) -> Result<EndpointProbe<Self::Connection>, crate::client::ClientError> {
            self.probes.fetch_add(1, AtomicOrdering::AcqRel);
            publish_fake_socket_for_connected_probe(paths.socket(), &self.probe)
                .map_err(crate::client::ClientError::Io)?;
            Ok(self.probe.clone())
        }

        async fn graceful_shutdown(
            &self,
            _connection: &mut Self::Connection,
            _instance_token: &str,
        ) -> Result<(), crate::client::ClientError> {
            Err(crate::client::ClientError::Api(
                "unexpected_shutdown".to_owned(),
            ))
        }
    }

    #[derive(Clone)]
    struct SocketReplacingEndpoint {
        paths: DaemonPaths,
        identity: DaemonIdentity,
        replaced: Arc<AtomicBool>,
        shutdowns: Arc<AtomicUsize>,
    }

    #[tonic::async_trait]
    impl DaemonEndpoint for SocketReplacingEndpoint {
        type Connection = ();

        async fn probe(
            &self,
            _paths: &DaemonPaths,
            _expected_path: EndpointPathState,
        ) -> Result<EndpointProbe<Self::Connection>, crate::client::ClientError> {
            if !self.replaced.swap(true, Ordering::AcqRel) {
                let original = self.paths.directory().join("original.sock");
                fs::rename(self.paths.socket(), original)
                    .map_err(crate::client::ClientError::Io)?;
                let replacement = std::os::unix::net::UnixListener::bind(self.paths.socket())
                    .map_err(crate::client::ClientError::Io)?;
                fs::set_permissions(self.paths.socket(), fs::Permissions::from_mode(0o600))
                    .map_err(crate::client::ClientError::Io)?;
                drop(replacement);
            }
            Ok(connected(self.identity.clone()))
        }

        async fn graceful_shutdown(
            &self,
            _connection: &mut Self::Connection,
            _instance_token: &str,
        ) -> Result<(), crate::client::ClientError> {
            self.shutdowns.fetch_add(1, AtomicOrdering::AcqRel);
            Ok(())
        }
    }

    fn endpoint_identity(record: &DaemonInstanceRecord) -> DaemonIdentity {
        DaemonIdentity {
            pid: record.pid,
            executable: record.executable.clone(),
            start_identity: record.start_identity.clone(),
            instance_token: record.instance_token.clone(),
            release_version: Some(record.release_version.clone()),
            started_at: Some(record.started_at.clone()),
        }
    }

    fn connected(identity: DaemonIdentity) -> EndpointProbe<()> {
        EndpointProbe::Connected(EndpointSession {
            connection: (),
            identity,
            compatible_api: true,
            safe_transport: true,
            healthy: true,
        })
    }

    fn matching_inspector(record: &DaemonInstanceRecord) -> FakeInspector {
        FakeInspector {
            identity: Some(ProcessIdentity {
                pid: record.pid,
                executable: record.executable.clone(),
                start_identity: record.start_identity.clone(),
            }),
        }
    }

    #[tokio::test]
    async fn classification_stopped_does_not_create_or_remove_state() -> TestResult {
        let temp = tempfile::tempdir()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        let executable = std::env::current_exe()?.canonicalize()?;
        let endpoint = FakeEndpoint::new(EndpointProbe::AbsentOrInert);

        let inspected = inspect_with(
            &paths,
            &executable,
            &endpoint,
            &FakeInspector { identity: None },
        )
        .await?;

        assert_eq!(inspected.status().state, DaemonState::Stopped);
        assert_eq!(endpoint.probes.load(AtomicOrdering::Acquire), 1);
        assert!(!paths.directory().exists());
        Ok(())
    }

    #[tokio::test]
    async fn classification_running_current_requires_matching_endpoint_record_and_process()
    -> TestResult {
        let temp = tempfile::tempdir()?;
        let executable = std::env::current_exe()?.canonicalize()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        let expected = record(&executable);
        write_record(&paths, &expected)?;
        let _listener = bind_safe_test_socket(&paths)?;
        let endpoint = FakeEndpoint::new(connected(endpoint_identity(&expected)));

        let inspected = inspect_with(
            &paths,
            &executable,
            &endpoint,
            &matching_inspector(&expected),
        )
        .await?;

        assert_eq!(inspected.status().state, DaemonState::Current);
        assert!(!inspected.status().legacy);
        assert_eq!(
            inspected.status().identity.as_ref(),
            Some(&endpoint_identity(&expected))
        );
        Ok(())
    }

    #[tokio::test]
    async fn classification_rejects_a_responding_endpoint_reached_through_a_symlink() -> TestResult
    {
        let temp = tempfile::tempdir()?;
        let root = root(&temp)?;
        let paths = DaemonPaths::from_runtime_root(root.join("runtime"));
        let executable = std::env::current_exe()?.canonicalize()?;
        let expected = record(&executable);
        let foreign_directory = root.join("foreign");
        fs::create_dir(&foreign_directory)?;
        let foreign_socket = foreign_directory.join("foreign.sock");
        let _foreign = std::os::unix::net::UnixListener::bind(&foreign_socket)?;
        paths.prepare_directory()?;
        std::os::unix::fs::symlink(&foreign_socket, paths.socket())?;
        let endpoint = FakeEndpoint::new(connected(endpoint_identity(&expected)));

        let inspected = inspect_with(
            &paths,
            &executable,
            &endpoint,
            &matching_inspector(&expected),
        )
        .await?;

        assert_eq!(inspected.status().state, DaemonState::Unsafe);
        assert!(
            inspected.session.is_none(),
            "an unauthenticated symlink endpoint retained a usable session"
        );
        assert_eq!(
            endpoint.probes.load(AtomicOrdering::Acquire),
            0,
            "the foreign endpoint received a probe before pathname authentication"
        );
        Ok(())
    }

    #[tokio::test]
    async fn stop_rejects_a_safe_socket_replaced_during_probe_without_rpc_or_signal() -> TestResult
    {
        let temp = tempfile::tempdir()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        let executable = std::env::current_exe()?.canonicalize()?;
        let expected = record(&executable);
        write_record(&paths, &expected)?;
        let _original = bind_safe_test_socket(&paths)?;
        let endpoint = SocketReplacingEndpoint {
            paths: paths.clone(),
            identity: endpoint_identity(&expected),
            replaced: Arc::new(AtomicBool::new(false)),
            shutdowns: Arc::new(AtomicUsize::new(0)),
        };
        let signaler = NeverSignaler::default();

        let result = stop_with(
            &paths,
            &executable,
            &endpoint,
            &matching_inspector(&expected),
            &signaler,
            StopMode::Explicit { force: true },
            test_timeouts(),
        )
        .await;

        assert!(matches!(
            result,
            Err(SupervisorError::InvalidState {
                state: DaemonState::Unsafe,
                ..
            })
        ));
        assert_eq!(
            endpoint.shutdowns.load(AtomicOrdering::Acquire),
            0,
            "a replacement endpoint received the shutdown RPC"
        );
        assert_eq!(
            signaler.signals.load(AtomicOrdering::Acquire),
            0,
            "a process was signaled after endpoint replacement"
        );
        Ok(())
    }

    #[tokio::test]
    async fn tonic_probe_sends_no_protocol_bytes_after_the_socket_precheck_is_invalidated()
    -> TestResult {
        use std::io::Read as _;

        let temp = tempfile::tempdir()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        let original = bind_safe_test_socket(&paths)?;
        let before = super::inspect_endpoint_path(&paths)?;
        let retired = paths.directory().join("retired.sock");
        fs::rename(paths.socket(), &retired)?;
        let foreign = std::os::unix::net::UnixListener::bind(paths.socket())?;
        fs::set_permissions(paths.socket(), fs::Permissions::from_mode(0o600))?;
        foreign.set_nonblocking(true)?;
        let reader = std::thread::spawn(move || -> io::Result<Vec<u8>> {
            let deadline = std::time::Instant::now() + Duration::from_secs(1);
            let (mut stream, _) = loop {
                match foreign.accept() {
                    Ok(accepted) => break accepted,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        if std::time::Instant::now() >= deadline {
                            return Ok(Vec::new());
                        }
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => return Err(error),
                }
            };
            stream.set_nonblocking(true)?;
            let mut bytes = vec![0_u8; 64];
            let read_deadline = std::time::Instant::now() + Duration::from_secs(1);
            let read = loop {
                match stream.read(&mut bytes) {
                    Ok(read) => break read,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        if std::time::Instant::now() >= read_deadline {
                            break 0;
                        }
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => return Err(error),
                }
            };
            bytes.truncate(read);
            Ok(bytes)
        });

        let _probe = crate::client::TonicEndpoint.probe(&paths, before).await?;
        let observed = reader
            .join()
            .map_err(|_| io::Error::other("foreign endpoint reader panicked"))??;
        assert!(
            observed.is_empty(),
            "a socket substituted after the precheck received protocol bytes: {observed:?}"
        );
        drop(original);
        assert_ne!(before, super::inspect_endpoint_path(&paths)?);
        Ok(())
    }

    #[tokio::test]
    async fn classification_running_outdated_uses_exact_release_comparison() -> TestResult {
        let temp = tempfile::tempdir()?;
        let executable = std::env::current_exe()?.canonicalize()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        let mut expected = record(&executable);
        expected.release_version = "0.1.10".to_owned();
        write_record(&paths, &expected)?;
        let _listener = bind_safe_test_socket(&paths)?;
        let endpoint = FakeEndpoint::new(connected(endpoint_identity(&expected)));

        let inspected = inspect_with(
            &paths,
            &executable,
            &endpoint,
            &matching_inspector(&expected),
        )
        .await?;

        assert_eq!(inspected.status().state, DaemonState::Outdated);
        assert!(!inspected.status().legacy);
        Ok(())
    }

    #[tokio::test]
    async fn classification_malformed_endpoint_release_is_unhealthy_not_outdated() -> TestResult {
        let temp = tempfile::tempdir()?;
        let executable = std::env::current_exe()?.canonicalize()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        let mut identity = endpoint_identity(&record(&executable));
        identity.release_version = Some("definitely not semver".to_owned());
        let inspector = FakeInspector {
            identity: Some(process_for(&identity)),
        };
        let _listener = bind_safe_test_socket(&paths)?;
        let endpoint = FakeEndpoint::new(connected(identity));

        let inspected = inspect_with(&paths, &executable, &endpoint, &inspector).await?;

        assert_eq!(inspected.status().state, DaemonState::Unhealthy);
        assert!(
            inspected
                .status()
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("release version"))
        );
        Ok(())
    }

    #[tokio::test]
    async fn classification_malformed_record_release_is_unsafe_not_outdated() -> TestResult {
        let temp = tempfile::tempdir()?;
        let executable = std::env::current_exe()?.canonicalize()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        let mut expected = record(&executable);
        expected.release_version = "not@semver".to_owned();
        write_record(&paths, &expected)?;
        let _listener = bind_safe_test_socket(&paths)?;
        let endpoint = FakeEndpoint::new(connected(endpoint_identity(&expected)));

        let inspected = inspect_with(
            &paths,
            &executable,
            &endpoint,
            &matching_inspector(&expected),
        )
        .await?;

        assert_eq!(inspected.status().state, DaemonState::Unsafe);
        assert!(
            inspected
                .status()
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("release version"))
        );
        Ok(())
    }

    #[tokio::test]
    async fn classification_protected_record_cannot_bypass_the_installed_executable() -> TestResult
    {
        let temp = tempfile::tempdir()?;
        let root = root(&temp)?;
        let installed = root.join("installed/gascand");
        let unrelated = root.join("unrelated/gascand");
        fs::create_dir_all(installed.parent().ok_or("installed parent missing")?)?;
        fs::create_dir_all(unrelated.parent().ok_or("unrelated parent missing")?)?;
        fs::write(&installed, b"installed")?;
        fs::write(&unrelated, b"unrelated")?;
        let paths = DaemonPaths::from_runtime_root(root.join("runtime"));
        let expected = record(&unrelated);
        write_record(&paths, &expected)?;
        let _listener = bind_safe_test_socket(&paths)?;
        let endpoint = FakeEndpoint::new(connected(endpoint_identity(&expected)));

        let inspected = inspect_with(
            &paths,
            &installed,
            &endpoint,
            &matching_inspector(&expected),
        )
        .await?;

        assert_eq!(inspected.status().state, DaemonState::Unhealthy);
        assert!(
            inspected
                .status()
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("installed executable"))
        );
        Ok(())
    }

    #[tokio::test]
    async fn classification_running_legacy_has_no_release_field() -> TestResult {
        let temp = tempfile::tempdir()?;
        let executable = std::env::current_exe()?.canonicalize()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        let identity = DaemonIdentity {
            pid: std::process::id(),
            executable: executable.clone(),
            start_identity: "legacy-start".to_owned(),
            instance_token: "22".repeat(32),
            release_version: None,
            started_at: None,
        };
        let inspector = FakeInspector {
            identity: Some(ProcessIdentity {
                pid: identity.pid,
                executable: executable.clone(),
                start_identity: identity.start_identity.clone(),
            }),
        };
        let _listener = bind_safe_test_socket(&paths)?;
        let endpoint = FakeEndpoint::new(connected(identity));

        let inspected = inspect_with(&paths, &executable, &endpoint, &inspector).await?;

        assert_eq!(inspected.status().state, DaemonState::Outdated);
        assert!(inspected.status().legacy);
        Ok(())
    }

    #[tokio::test]
    async fn classification_running_unhealthy_rejects_contradictory_identity() -> TestResult {
        let temp = tempfile::tempdir()?;
        let executable = std::env::current_exe()?.canonicalize()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        let expected = record(&executable);
        write_record(&paths, &expected)?;
        let _listener = bind_safe_test_socket(&paths)?;
        let mut contradictory = endpoint_identity(&expected);
        contradictory.instance_token = "33".repeat(32);
        let endpoint = FakeEndpoint::new(connected(contradictory));

        let inspected = inspect_with(
            &paths,
            &executable,
            &endpoint,
            &matching_inspector(&expected),
        )
        .await?;

        assert_eq!(inspected.status().state, DaemonState::Unhealthy);
        assert!(paths.instance().exists());
        Ok(())
    }

    #[tokio::test]
    async fn classification_unreachable_retains_valid_live_record_identity() -> TestResult {
        let temp = tempfile::tempdir()?;
        let executable = std::env::current_exe()?.canonicalize()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        let expected = record(&executable);
        write_record(&paths, &expected)?;
        let endpoint = FakeEndpoint::new(EndpointProbe::AbsentOrInert);

        let inspected = inspect_with(
            &paths,
            &executable,
            &endpoint,
            &matching_inspector(&expected),
        )
        .await?;

        assert_eq!(inspected.status().state, DaemonState::Unreachable);
        assert_eq!(
            inspected.status().identity.as_ref(),
            Some(&endpoint_identity(&expected))
        );
        assert!(paths.instance().exists());
        Ok(())
    }

    #[tokio::test]
    async fn classification_unsafe_record_fails_closed_without_deleting_it() -> TestResult {
        let temp = tempfile::tempdir()?;
        let executable = std::env::current_exe()?.canonicalize()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        paths.prepare_directory()?;
        fs::write(paths.instance(), b"{")?;
        fs::set_permissions(paths.instance(), fs::Permissions::from_mode(0o600))?;
        let endpoint = FakeEndpoint::new(EndpointProbe::AbsentOrInert);

        let inspected = inspect_with(
            &paths,
            &executable,
            &endpoint,
            &FakeInspector { identity: None },
        )
        .await?;

        assert_eq!(inspected.status().state, DaemonState::Unsafe);
        assert_eq!(fs::read(paths.instance())?, b"{");
        assert_eq!(endpoint.probes.load(AtomicOrdering::Acquire), 1);
        Ok(())
    }

    #[tokio::test]
    async fn classification_unsafe_inert_endpoint_with_stale_record_fails_closed() -> TestResult {
        let temp = tempfile::tempdir()?;
        let executable = std::env::current_exe()?.canonicalize()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        let expected = record(&executable);
        write_record(&paths, &expected)?;
        fs::write(paths.socket(), b"not a socket")?;
        fs::set_permissions(paths.socket(), fs::Permissions::from_mode(0o600))?;
        let endpoint = FakeEndpoint::new(EndpointProbe::AbsentOrInert);

        let inspected = inspect_with(
            &paths,
            &executable,
            &endpoint,
            &FakeInspector { identity: None },
        )
        .await?;

        assert_eq!(inspected.status().state, DaemonState::Unsafe);
        assert_eq!(
            inspected.status().detail.as_deref(),
            Some("daemon endpoint ownership, type, links, or mode is unsafe")
        );
        assert!(paths.instance().exists());
        assert!(paths.socket().exists());
        Ok(())
    }

    #[derive(Clone)]
    struct MutableEndpoint {
        probe: Arc<Mutex<EndpointProbe<()>>>,
        probes: Arc<AtomicUsize>,
    }

    impl MutableEndpoint {
        fn new(probe: EndpointProbe<()>) -> Self {
            Self {
                probe: Arc::new(Mutex::new(probe)),
                probes: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn set(&self, probe: EndpointProbe<()>) -> io::Result<()> {
            *self
                .probe
                .lock()
                .map_err(|_| io::Error::other("fake endpoint state was poisoned"))? = probe;
            Ok(())
        }
    }

    #[tonic::async_trait]
    impl DaemonEndpoint for MutableEndpoint {
        type Connection = ();

        async fn probe(
            &self,
            paths: &DaemonPaths,
            _expected_path: EndpointPathState,
        ) -> Result<EndpointProbe<Self::Connection>, crate::client::ClientError> {
            self.probes.fetch_add(1, AtomicOrdering::AcqRel);
            let probe = self
                .probe
                .lock()
                .map_err(|_| {
                    crate::client::ClientError::Io(io::Error::other(
                        "fake endpoint state was poisoned",
                    ))
                })
                .map(|probe| probe.clone())?;
            publish_fake_socket_for_connected_probe(paths.socket(), &probe)
                .map_err(crate::client::ClientError::Io)?;
            Ok(probe)
        }

        async fn graceful_shutdown(
            &self,
            _connection: &mut Self::Connection,
            _instance_token: &str,
        ) -> Result<(), crate::client::ClientError> {
            Err(crate::client::ClientError::Api(
                "unexpected_shutdown".to_owned(),
            ))
        }
    }

    #[derive(Default)]
    struct PublicationProbeGate {
        connected_probe_observed: Mutex<bool>,
        changed: Condvar,
    }

    impl PublicationProbeGate {
        fn observe_connected_probe(&self) -> io::Result<()> {
            *self
                .connected_probe_observed
                .lock()
                .map_err(|_| io::Error::other("publication probe gate was poisoned"))? = true;
            self.changed.notify_all();
            Ok(())
        }

        fn wait_for_connected_probe(&self) -> io::Result<()> {
            let observed = self
                .connected_probe_observed
                .lock()
                .map_err(|_| io::Error::other("publication probe gate was poisoned"))?;
            let (observed, timeout) = self
                .changed
                .wait_timeout_while(observed, Duration::from_secs(1), |observed| !*observed)
                .map_err(|_| io::Error::other("publication probe gate was poisoned"))?;
            if !*observed && timeout.timed_out() {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "connected endpoint probe was not observed",
                ));
            }
            Ok(())
        }
    }

    #[derive(Clone)]
    struct PublicationEndpoint {
        endpoint: MutableEndpoint,
        gate: Arc<PublicationProbeGate>,
    }

    #[tonic::async_trait]
    impl DaemonEndpoint for PublicationEndpoint {
        type Connection = ();

        async fn probe(
            &self,
            paths: &DaemonPaths,
            expected_path: EndpointPathState,
        ) -> Result<EndpointProbe<Self::Connection>, crate::client::ClientError> {
            let probe = self.endpoint.probe(paths, expected_path).await?;
            if matches!(&probe, EndpointProbe::Connected(_)) {
                self.gate
                    .observe_connected_probe()
                    .map_err(crate::client::ClientError::Io)?;
            }
            Ok(probe)
        }

        async fn graceful_shutdown(
            &self,
            connection: &mut Self::Connection,
            instance_token: &str,
        ) -> Result<(), crate::client::ClientError> {
            self.endpoint
                .graceful_shutdown(connection, instance_token)
                .await
        }
    }

    #[derive(Clone)]
    struct MutableInspector {
        identity: Arc<Mutex<Option<ProcessIdentity>>>,
        inspections: Arc<AtomicUsize>,
        absent_inspections: Arc<AtomicUsize>,
        stall_on_absent: Arc<Mutex<Option<(usize, Duration)>>>,
    }

    impl MutableInspector {
        fn new(identity: Option<ProcessIdentity>) -> Self {
            Self {
                identity: Arc::new(Mutex::new(identity)),
                inspections: Arc::new(AtomicUsize::new(0)),
                absent_inspections: Arc::new(AtomicUsize::new(0)),
                stall_on_absent: Arc::new(Mutex::new(None)),
            }
        }

        fn set(&self, identity: Option<ProcessIdentity>) -> io::Result<()> {
            *self
                .identity
                .lock()
                .map_err(|_| io::Error::other("fake process state was poisoned"))? = identity;
            Ok(())
        }

        fn stall_on_absent_inspection(&self, inspection: usize, delay: Duration) -> io::Result<()> {
            *self
                .stall_on_absent
                .lock()
                .map_err(|_| io::Error::other("fake absent-inspection stall was poisoned"))? =
                Some((inspection, delay));
            Ok(())
        }
    }

    impl ProcessInspector for MutableInspector {
        fn inspect(
            &self,
            _pid: u32,
            _expected_executable: &Path,
        ) -> io::Result<Option<ProcessIdentity>> {
            self.inspections.fetch_add(1, AtomicOrdering::AcqRel);
            let identity = self
                .identity
                .lock()
                .map_err(|_| io::Error::other("fake process state was poisoned"))
                .map(|identity| identity.clone())?;
            if identity.is_none() {
                let absent = self.absent_inspections.fetch_add(1, AtomicOrdering::AcqRel) + 1;
                let stall = *self
                    .stall_on_absent
                    .lock()
                    .map_err(|_| io::Error::other("fake absent-inspection stall was poisoned"))?;
                if stall.is_some_and(|(inspection, _)| inspection == absent) {
                    std::thread::sleep(stall.map_or(Duration::ZERO, |(_, delay)| delay));
                }
            }
            Ok(identity)
        }
    }

    #[derive(Default)]
    struct SpawnGate {
        entered: AtomicBool,
        released: Mutex<bool>,
        changed: Condvar,
    }

    impl SpawnGate {
        fn wait(&self) -> io::Result<()> {
            self.entered.store(true, Ordering::Release);
            let mut released = self
                .released
                .lock()
                .map_err(|_| io::Error::other("spawn gate was poisoned"))?;
            while !*released {
                released = self
                    .changed
                    .wait(released)
                    .map_err(|_| io::Error::other("spawn gate was poisoned"))?;
            }
            Ok(())
        }

        fn release(&self) -> io::Result<()> {
            *self
                .released
                .lock()
                .map_err(|_| io::Error::other("spawn gate was poisoned"))? = true;
            self.changed.notify_all();
            Ok(())
        }
    }

    #[derive(Clone)]
    struct FakeSpawner {
        endpoint: MutableEndpoint,
        inspector: MutableInspector,
        launches: Arc<Mutex<Vec<DaemonLaunch>>>,
        release_version: String,
        endpoint_token: Option<String>,
        owner_token: Option<String>,
        gate: Option<Arc<SpawnGate>>,
    }

    impl FakeSpawner {
        fn current(endpoint: MutableEndpoint, inspector: MutableInspector) -> Self {
            Self {
                endpoint,
                inspector,
                launches: Arc::new(Mutex::new(Vec::new())),
                release_version: env!("CARGO_PKG_VERSION").to_owned(),
                endpoint_token: None,
                owner_token: None,
                gate: None,
            }
        }

        fn launches(&self) -> io::Result<Vec<DaemonLaunch>> {
            self.launches
                .lock()
                .map_err(|_| io::Error::other("fake launches were poisoned"))
                .map(|launches| launches.clone())
        }
    }

    impl DaemonSpawner for FakeSpawner {
        fn spawn(&self, launch: &DaemonLaunch) -> io::Result<DaemonStartupMonitor> {
            self.launches
                .lock()
                .map_err(|_| io::Error::other("fake launches were poisoned"))?
                .push(launch.clone());
            let published = DaemonInstanceRecord {
                pid: std::process::id(),
                owner_token: self
                    .owner_token
                    .clone()
                    .unwrap_or_else(|| launch.owner_token.clone()),
                executable: launch.executable.clone(),
                start_identity: "spawned-start".to_owned(),
                instance_token: "44".repeat(32),
                release_version: self.release_version.clone(),
                started_at: InstanceTimestamp {
                    seconds: 1_785_263_900,
                    nanos: 456_000_000,
                },
            };
            fs::write(&launch.instance_path, serde_json::to_vec(&published)?)?;
            fs::set_permissions(&launch.instance_path, fs::Permissions::from_mode(0o600))?;
            self.inspector.set(Some(ProcessIdentity {
                pid: published.pid,
                executable: published.executable.clone(),
                start_identity: published.start_identity.clone(),
            }))?;
            let mut identity = endpoint_identity(&published);
            if let Some(token) = &self.endpoint_token {
                identity.instance_token = token.clone();
            }
            self.endpoint.set(connected(identity))?;
            if let Some(gate) = &self.gate {
                gate.wait()?;
            }
            Ok(DaemonStartupMonitor::default())
        }
    }

    #[derive(Clone)]
    struct DelayedPublicationSpawner {
        endpoint: MutableEndpoint,
        inspector: MutableInspector,
        gate: Arc<PublicationProbeGate>,
        publisher: Arc<Mutex<Option<std::thread::JoinHandle<io::Result<()>>>>>,
        initial_tombstone: bool,
    }

    impl DelayedPublicationSpawner {
        fn finish(&self) -> io::Result<()> {
            let publisher = self
                .publisher
                .lock()
                .map_err(|_| io::Error::other("delayed publisher was poisoned"))?
                .take()
                .ok_or_else(|| io::Error::other("delayed publisher was not started"))?;
            publisher
                .join()
                .map_err(|_| io::Error::other("delayed publisher panicked"))?
        }
    }

    impl DaemonSpawner for DelayedPublicationSpawner {
        fn spawn(&self, launch: &DaemonLaunch) -> io::Result<DaemonStartupMonitor> {
            let published = DaemonInstanceRecord {
                pid: std::process::id(),
                owner_token: launch.owner_token.clone(),
                executable: launch.executable.clone(),
                start_identity: "delayed-publication-start".to_owned(),
                instance_token: "45".repeat(32),
                release_version: env!("CARGO_PKG_VERSION").to_owned(),
                started_at: InstanceTimestamp {
                    seconds: 1_785_263_901,
                    nanos: 456_000_000,
                },
            };
            if self.initial_tombstone {
                fs::write(&launch.instance_path, b"publication-in-progress")?;
                fs::set_permissions(&launch.instance_path, fs::Permissions::from_mode(0o200))?;
            } else {
                let socket = launch.current_dir.join(super::SOCKET_NAME);
                let listener = std::os::unix::net::UnixListener::bind(&socket)?;
                fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))?;
                drop(listener);
            }
            self.inspector.set(Some(ProcessIdentity {
                pid: published.pid,
                executable: published.executable.clone(),
                start_identity: published.start_identity.clone(),
            }))?;
            self.endpoint
                .set(connected(endpoint_identity(&published)))?;

            let instance_path = launch.instance_path.clone();
            let gate = self.gate.clone();
            let publisher = std::thread::spawn(move || -> io::Result<()> {
                gate.wait_for_connected_probe()?;
                fs::write(&instance_path, serde_json::to_vec(&published)?)?;
                fs::set_permissions(&instance_path, fs::Permissions::from_mode(0o600))
            });
            *self
                .publisher
                .lock()
                .map_err(|_| io::Error::other("delayed publisher was poisoned"))? = Some(publisher);
            Ok(DaemonStartupMonitor::default())
        }
    }

    fn test_timeouts() -> SupervisorTimeouts {
        SupervisorTimeouts {
            readiness: Duration::from_millis(100),
            shutdown: Duration::from_millis(25),
            poll: Duration::from_millis(1),
        }
    }

    #[tokio::test]
    async fn start_stopped_spawns_exactly_one_daemon() -> TestResult {
        let temp = tempfile::tempdir()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        let executable = std::env::current_exe()?.canonicalize()?;
        let endpoint = MutableEndpoint::new(EndpointProbe::AbsentOrInert);
        let inspector = MutableInspector::new(None);
        let spawner = FakeSpawner::current(endpoint.clone(), inspector.clone());

        let outcome = start_with(
            &paths,
            &executable,
            &endpoint,
            &inspector,
            &spawner,
            test_timeouts(),
        )
        .await?;

        assert_eq!(outcome.transition, DaemonTransition::Started);
        assert_eq!(outcome.status.state, DaemonState::Current);
        assert_eq!(spawner.launches()?.len(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn start_readiness_rejects_a_record_with_the_wrong_launch_owner_token() -> TestResult {
        let temp = tempfile::tempdir()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        let executable = std::env::current_exe()?.canonicalize()?;
        let endpoint = MutableEndpoint::new(EndpointProbe::AbsentOrInert);
        let inspector = MutableInspector::new(None);
        let mut configured = FakeSpawner::current(endpoint.clone(), inspector.clone());
        configured.owner_token = Some("not-this-launch".to_owned());

        let result = start_with(
            &paths,
            &executable,
            &endpoint,
            &inspector,
            &configured,
            test_timeouts(),
        )
        .await;

        assert!(
            matches!(
                &result,
                Err(SupervisorError::Readiness {
                    state: DaemonState::Unhealthy,
                    ..
                })
            ),
            "unexpected readiness result: {result:?}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn start_readiness_waits_for_its_own_connected_publication_to_finish() -> TestResult {
        let temp = tempfile::tempdir()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        let executable = std::env::current_exe()?.canonicalize()?;
        let endpoint = MutableEndpoint::new(EndpointProbe::AbsentOrInert);
        let inspector = MutableInspector::new(None);
        let gate = Arc::new(PublicationProbeGate::default());
        let publication_endpoint = PublicationEndpoint {
            endpoint: endpoint.clone(),
            gate: gate.clone(),
        };
        let spawner = DelayedPublicationSpawner {
            endpoint,
            inspector: inspector.clone(),
            gate,
            publisher: Arc::new(Mutex::new(None)),
            initial_tombstone: true,
        };

        let outcome = start_with(
            &paths,
            &executable,
            &publication_endpoint,
            &inspector,
            &spawner,
            SupervisorTimeouts {
                readiness: Duration::from_millis(200),
                shutdown: Duration::from_millis(25),
                poll: Duration::from_millis(1),
            },
        )
        .await;
        spawner.finish()?;
        let outcome = outcome?;

        assert_eq!(outcome.status.state, DaemonState::Current);
        assert_eq!(outcome.transition, DaemonTransition::Started);
        Ok(())
    }

    #[tokio::test]
    async fn start_readiness_waits_when_the_endpoint_precedes_its_record() -> TestResult {
        let temp = tempfile::tempdir()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        let executable = std::env::current_exe()?.canonicalize()?;
        let endpoint = MutableEndpoint::new(EndpointProbe::AbsentOrInert);
        let inspector = MutableInspector::new(None);
        let gate = Arc::new(PublicationProbeGate::default());
        let publication_endpoint = PublicationEndpoint {
            endpoint: endpoint.clone(),
            gate: gate.clone(),
        };
        let spawner = DelayedPublicationSpawner {
            endpoint,
            inspector: inspector.clone(),
            gate,
            publisher: Arc::new(Mutex::new(None)),
            initial_tombstone: false,
        };

        let outcome = start_with(
            &paths,
            &executable,
            &publication_endpoint,
            &inspector,
            &spawner,
            SupervisorTimeouts {
                readiness: Duration::from_millis(200),
                shutdown: Duration::from_millis(25),
                poll: Duration::from_millis(1),
            },
        )
        .await;
        spawner.finish()?;
        let outcome = outcome?;

        assert_eq!(outcome.status.state, DaemonState::Current);
        assert_eq!(outcome.transition, DaemonTransition::Started);
        Ok(())
    }

    #[tokio::test]
    async fn start_current_is_a_successful_noop() -> TestResult {
        let temp = tempfile::tempdir()?;
        let executable = std::env::current_exe()?.canonicalize()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        let expected = record(&executable);
        write_record(&paths, &expected)?;
        let _listener = bind_safe_test_socket(&paths)?;
        let endpoint = MutableEndpoint::new(connected(endpoint_identity(&expected)));
        let inspector = MutableInspector::new(matching_inspector(&expected).identity);
        let spawner = FakeSpawner::current(endpoint.clone(), inspector.clone());

        let outcome = start_with(
            &paths,
            &executable,
            &endpoint,
            &inspector,
            &spawner,
            test_timeouts(),
        )
        .await?;

        assert_eq!(outcome.transition, DaemonTransition::None);
        assert_eq!(outcome.status.state, DaemonState::Current);
        assert!(spawner.launches()?.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn start_outdated_is_not_accepted_as_current() -> TestResult {
        let temp = tempfile::tempdir()?;
        let executable = std::env::current_exe()?.canonicalize()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        let mut expected = record(&executable);
        expected.release_version = "0.1.10".to_owned();
        write_record(&paths, &expected)?;
        let _listener = bind_safe_test_socket(&paths)?;
        let endpoint = MutableEndpoint::new(connected(endpoint_identity(&expected)));
        let inspector = MutableInspector::new(matching_inspector(&expected).identity);
        let spawner = FakeSpawner::current(endpoint.clone(), inspector.clone());

        let result = start_with(
            &paths,
            &executable,
            &endpoint,
            &inspector,
            &spawner,
            test_timeouts(),
        )
        .await;

        assert!(matches!(result, Err(SupervisorError::Outdated { .. })));
        assert!(spawner.launches()?.is_empty());
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn start_concurrent_callers_converge_after_lock_reinspection() -> TestResult {
        let temp = tempfile::tempdir()?;
        let paths = Arc::new(DaemonPaths::from_runtime_root(root(&temp)?.join("runtime")));
        let executable = Arc::new(std::env::current_exe()?.canonicalize()?);
        let endpoint = Arc::new(MutableEndpoint::new(EndpointProbe::AbsentOrInert));
        let inspector = Arc::new(MutableInspector::new(None));
        let gate = Arc::new(SpawnGate::default());
        let mut configured =
            FakeSpawner::current(endpoint.as_ref().clone(), inspector.as_ref().clone());
        configured.gate = Some(Arc::clone(&gate));
        let spawner = Arc::new(configured);

        let first = {
            let paths = Arc::clone(&paths);
            let executable = Arc::clone(&executable);
            let endpoint = Arc::clone(&endpoint);
            let inspector = Arc::clone(&inspector);
            let spawner = Arc::clone(&spawner);
            tokio::spawn(async move {
                start_with(
                    &paths,
                    &executable,
                    endpoint.as_ref(),
                    inspector.as_ref(),
                    spawner.as_ref(),
                    test_timeouts(),
                )
                .await
            })
        };
        tokio::time::timeout(Duration::from_secs(1), async {
            while !gate.entered.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        let second = {
            let paths = Arc::clone(&paths);
            let executable = Arc::clone(&executable);
            let endpoint = Arc::clone(&endpoint);
            let inspector = Arc::clone(&inspector);
            let spawner = Arc::clone(&spawner);
            tokio::spawn(async move {
                start_with(
                    &paths,
                    &executable,
                    endpoint.as_ref(),
                    inspector.as_ref(),
                    spawner.as_ref(),
                    test_timeouts(),
                )
                .await
            })
        };
        tokio::time::sleep(Duration::from_millis(10)).await;
        gate.release()?;
        let first = tokio::time::timeout(Duration::from_secs(2), first).await???;
        let second = tokio::time::timeout(Duration::from_secs(2), second).await???;

        assert_eq!(spawner.launches()?.len(), 1);
        assert_eq!(first.status.state, DaemonState::Current);
        assert_eq!(second.status.state, DaemonState::Current);
        assert_eq!(
            [first.transition, second.transition]
                .into_iter()
                .filter(|transition| *transition == DaemonTransition::Started)
                .count(),
            1
        );
        Ok(())
    }

    #[tokio::test]
    async fn start_spawn_uses_the_protected_runtime_directory_as_cwd() -> TestResult {
        let temp = tempfile::tempdir()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        let executable = std::env::current_exe()?.canonicalize()?;
        let endpoint = MutableEndpoint::new(EndpointProbe::AbsentOrInert);
        let inspector = MutableInspector::new(None);
        let spawner = FakeSpawner::current(endpoint.clone(), inspector.clone());

        start_with(
            &paths,
            &executable,
            &endpoint,
            &inspector,
            &spawner,
            test_timeouts(),
        )
        .await?;

        let launches = spawner.launches()?;
        assert_eq!(launches[0].current_dir, paths.directory());
        assert_eq!(
            fs::symlink_metadata(paths.directory())?
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        Ok(())
    }

    #[tokio::test]
    async fn start_readiness_rejects_an_outdated_spawned_release() -> TestResult {
        let temp = tempfile::tempdir()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        let executable = std::env::current_exe()?.canonicalize()?;
        let endpoint = MutableEndpoint::new(EndpointProbe::AbsentOrInert);
        let inspector = MutableInspector::new(None);
        let mut configured = FakeSpawner::current(endpoint.clone(), inspector.clone());
        configured.release_version = "0.1.10".to_owned();

        let result = start_with(
            &paths,
            &executable,
            &endpoint,
            &inspector,
            &configured,
            test_timeouts(),
        )
        .await;

        assert!(matches!(
            result,
            Err(SupervisorError::Readiness {
                state: DaemonState::Outdated,
                ..
            })
        ));
        Ok(())
    }

    #[tokio::test]
    async fn start_readiness_rejects_identity_that_does_not_match_the_record() -> TestResult {
        let temp = tempfile::tempdir()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        let executable = std::env::current_exe()?.canonicalize()?;
        let endpoint = MutableEndpoint::new(EndpointProbe::AbsentOrInert);
        let inspector = MutableInspector::new(None);
        let mut configured = FakeSpawner::current(endpoint.clone(), inspector.clone());
        configured.endpoint_token = Some("55".repeat(32));

        let result = start_with(
            &paths,
            &executable,
            &endpoint,
            &inspector,
            &configured,
            test_timeouts(),
        )
        .await;

        assert!(matches!(
            result,
            Err(SupervisorError::Readiness {
                state: DaemonState::Unhealthy,
                ..
            })
        ));
        Ok(())
    }

    #[derive(Clone)]
    struct TombstoneOnlySpawner {
        inner: FakeSpawner,
    }

    impl DaemonSpawner for TombstoneOnlySpawner {
        fn spawn(&self, launch: &DaemonLaunch) -> io::Result<DaemonStartupMonitor> {
            match fs::symlink_metadata(&launch.instance_path) {
                Ok(metadata)
                    if metadata.file_type().is_file()
                        && metadata.permissions().mode() & 0o777 == 0o200
                        && metadata.len() == 0 => {}
                Ok(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "publisher requires an exact empty write-only tombstone",
                    ));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            self.inner.spawn(launch)
        }
    }

    #[tokio::test]
    async fn start_retires_a_stale_valid_record_through_its_held_descriptor() -> TestResult {
        let temp = tempfile::tempdir()?;
        let executable = std::env::current_exe()?.canonicalize()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        let stale = record(&executable);
        write_record(&paths, &stale)?;
        let endpoint = MutableEndpoint::new(EndpointProbe::AbsentOrInert);
        let inspector = MutableInspector::new(None);
        let inner = FakeSpawner::current(endpoint.clone(), inspector.clone());
        let spawner = TombstoneOnlySpawner {
            inner: inner.clone(),
        };

        let outcome = start_with(
            &paths,
            &executable,
            &endpoint,
            &inspector,
            &spawner,
            test_timeouts(),
        )
        .await?;

        assert_eq!(outcome.transition, DaemonTransition::Started);
        assert_eq!(outcome.status.state, DaemonState::Current);
        assert_eq!(inner.launches()?.len(), 1);
        Ok(())
    }

    #[derive(Default)]
    struct CountingNoopSpawner(AtomicUsize);

    impl DaemonSpawner for CountingNoopSpawner {
        fn spawn(&self, _launch: &DaemonLaunch) -> io::Result<DaemonStartupMonitor> {
            self.0.fetch_add(1, AtomicOrdering::AcqRel);
            Ok(DaemonStartupMonitor::default())
        }
    }

    #[derive(Clone)]
    struct ReplacingEndpoint {
        instance_path: PathBuf,
        replacement: Vec<u8>,
        replace_on_probe: usize,
        probes: Arc<AtomicUsize>,
    }

    #[tonic::async_trait]
    impl DaemonEndpoint for ReplacingEndpoint {
        type Connection = ();

        async fn probe(
            &self,
            _paths: &DaemonPaths,
            _expected_path: EndpointPathState,
        ) -> Result<EndpointProbe<Self::Connection>, crate::client::ClientError> {
            let probe = self.probes.fetch_add(1, AtomicOrdering::AcqRel) + 1;
            if probe == self.replace_on_probe {
                fs::remove_file(&self.instance_path)?;
                fs::write(&self.instance_path, &self.replacement)?;
                fs::set_permissions(&self.instance_path, fs::Permissions::from_mode(0o200))?;
            }
            Ok(EndpointProbe::AbsentOrInert)
        }

        async fn graceful_shutdown(
            &self,
            _connection: &mut Self::Connection,
            _instance_token: &str,
        ) -> Result<(), crate::client::ClientError> {
            Err(crate::client::ClientError::Api(
                "unexpected_shutdown".to_owned(),
            ))
        }
    }

    #[derive(Clone)]
    enum UnsafeEndpointReplacement {
        RegularFile,
        Symlink(PathBuf),
    }

    #[derive(Clone)]
    struct ReplacingSocketEndpoint {
        socket_path: PathBuf,
        replacement: UnsafeEndpointReplacement,
        replace_on_probe: usize,
        probes: Arc<AtomicUsize>,
    }

    #[tonic::async_trait]
    impl DaemonEndpoint for ReplacingSocketEndpoint {
        type Connection = ();

        async fn probe(
            &self,
            _paths: &DaemonPaths,
            _expected_path: EndpointPathState,
        ) -> Result<EndpointProbe<Self::Connection>, crate::client::ClientError> {
            let probe = self.probes.fetch_add(1, AtomicOrdering::AcqRel) + 1;
            if probe == self.replace_on_probe {
                match &self.replacement {
                    UnsafeEndpointReplacement::RegularFile => {
                        fs::write(&self.socket_path, b"foreign-endpoint")?;
                        fs::set_permissions(&self.socket_path, fs::Permissions::from_mode(0o600))?;
                    }
                    UnsafeEndpointReplacement::Symlink(target) => {
                        std::os::unix::fs::symlink(target, &self.socket_path)?;
                    }
                }
            }
            Ok(EndpointProbe::AbsentOrInert)
        }

        async fn graceful_shutdown(
            &self,
            _connection: &mut Self::Connection,
            _instance_token: &str,
        ) -> Result<(), crate::client::ClientError> {
            Err(crate::client::ClientError::Api(
                "unexpected_shutdown".to_owned(),
            ))
        }
    }

    #[tokio::test]
    async fn tombstone_recovery_refuses_a_non_socket_replacement_between_probes() -> TestResult {
        let temp = tempfile::tempdir()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        let executable = std::env::current_exe()?.canonicalize()?;
        paths.prepare_directory()?;
        fs::write(paths.instance(), b"partial-publication")?;
        fs::set_permissions(paths.instance(), fs::Permissions::from_mode(0o200))?;
        let endpoint = ReplacingSocketEndpoint {
            socket_path: paths.socket().to_owned(),
            replacement: UnsafeEndpointReplacement::RegularFile,
            replace_on_probe: 2,
            probes: Arc::new(AtomicUsize::new(0)),
        };
        let spawner = CountingNoopSpawner::default();

        let result = start_with(
            &paths,
            &executable,
            &endpoint,
            &FakeInspector { identity: None },
            &spawner,
            test_timeouts(),
        )
        .await;

        assert!(matches!(result, Err(SupervisorError::TombstoneBusy { .. })));
        fs::set_permissions(paths.instance(), fs::Permissions::from_mode(0o600))?;
        assert_eq!(fs::read(paths.instance())?, b"partial-publication");
        assert_eq!(fs::read(paths.socket())?, b"foreign-endpoint");
        assert_eq!(spawner.0.load(AtomicOrdering::Acquire), 0);
        Ok(())
    }

    #[tokio::test]
    async fn stale_record_recovery_refuses_a_symlink_replacement_between_probes() -> TestResult {
        let temp = tempfile::tempdir()?;
        let base = root(&temp)?;
        let paths = DaemonPaths::from_runtime_root(base.join("runtime"));
        let executable = std::env::current_exe()?.canonicalize()?;
        let stale = record(&executable);
        let stale_bytes = serde_json::to_vec(&stale)?;
        write_record(&paths, &stale)?;
        let target = base.join("foreign-endpoint");
        fs::write(&target, b"retain")?;
        let endpoint = ReplacingSocketEndpoint {
            socket_path: paths.socket().to_owned(),
            replacement: UnsafeEndpointReplacement::Symlink(target.clone()),
            replace_on_probe: 2,
            probes: Arc::new(AtomicUsize::new(0)),
        };
        let spawner = CountingNoopSpawner::default();

        let result = start_with(
            &paths,
            &executable,
            &endpoint,
            &FakeInspector { identity: None },
            &spawner,
            test_timeouts(),
        )
        .await;

        assert!(matches!(result, Err(SupervisorError::TombstoneBusy { .. })));
        fs::set_permissions(paths.instance(), fs::Permissions::from_mode(0o600))?;
        assert_eq!(fs::read(paths.instance())?, stale_bytes);
        assert!(
            fs::symlink_metadata(paths.socket())?
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read(target)?, b"retain");
        assert_eq!(spawner.0.load(AtomicOrdering::Acquire), 0);
        Ok(())
    }

    #[tokio::test]
    async fn stale_record_recovery_refuses_unresponsive_transport_after_path_absence() -> TestResult
    {
        let temp = tempfile::tempdir()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        let executable = std::env::current_exe()?.canonicalize()?;
        let stale = record(&executable);
        let stale_bytes = serde_json::to_vec(&stale)?;
        write_record(&paths, &stale)?;
        let endpoint = FakeEndpoint::new(EndpointProbe::Unresponsive(
            "accepted transport did not complete its handshake".to_owned(),
        ));
        let spawner = CountingNoopSpawner::default();

        let result = start_with(
            &paths,
            &executable,
            &endpoint,
            &FakeInspector { identity: None },
            &spawner,
            test_timeouts(),
        )
        .await;

        assert!(matches!(
            result,
            Err(SupervisorError::InvalidState {
                state: DaemonState::Unreachable,
                ..
            })
        ));
        assert_eq!(fs::read(paths.instance())?, stale_bytes);
        assert_eq!(spawner.0.load(AtomicOrdering::Acquire), 0);
        Ok(())
    }

    #[tokio::test]
    async fn tombstone_recovery_refuses_an_accepted_stalled_listener_timeout() -> TestResult {
        let temp = tempfile::tempdir()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        let executable = std::env::current_exe()?.canonicalize()?;
        paths.prepare_directory()?;
        fs::write(paths.instance(), b"partial-publication")?;
        fs::set_permissions(paths.instance(), fs::Permissions::from_mode(0o200))?;
        let listener = std::os::unix::net::UnixListener::bind(paths.socket())?;
        fs::set_permissions(paths.socket(), fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let accepted = Arc::new(AtomicUsize::new(0));
        let holder = {
            let stop = Arc::clone(&stop);
            let accepted = Arc::clone(&accepted);
            let socket = paths.socket().to_owned();
            std::thread::spawn(move || -> io::Result<()> {
                let mut held = Vec::new();
                while !stop.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            if accepted.load(AtomicOrdering::Acquire) == 0 {
                                fs::remove_file(&socket)?;
                            }
                            held.push(stream);
                            accepted.fetch_add(1, AtomicOrdering::AcqRel);
                        }
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(1));
                        }
                        Err(error) => return Err(error),
                    }
                }
                Ok(())
            })
        };
        let spawner = CountingNoopSpawner::default();

        let result = start_with(
            &paths,
            &executable,
            &crate::client::TonicEndpoint,
            &FakeInspector { identity: None },
            &spawner,
            test_timeouts(),
        )
        .await;

        stop.store(true, Ordering::Release);
        holder
            .join()
            .map_err(|_| io::Error::other("stalled listener holder panicked"))??;
        assert!(
            matches!(
                &result,
                Err(SupervisorError::InvalidState {
                    state: DaemonState::Unsafe,
                    ..
                })
            ),
            "unexpected stalled-listener result: {result:?}"
        );
        assert!(accepted.load(AtomicOrdering::Acquire) > 0);
        fs::set_permissions(paths.instance(), fs::Permissions::from_mode(0o600))?;
        assert_eq!(fs::read(paths.instance())?, b"partial-publication");
        assert_eq!(spawner.0.load(AtomicOrdering::Acquire), 0);
        Ok(())
    }

    #[tokio::test]
    async fn endpoint_probe_distinguishes_an_orphaned_socket_as_inert() -> TestResult {
        let temp = tempfile::tempdir()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        paths.prepare_directory()?;
        let listener = std::os::unix::net::UnixListener::bind(paths.socket())?;
        fs::set_permissions(paths.socket(), fs::Permissions::from_mode(0o600))?;
        drop(listener);

        let before = super::inspect_endpoint_path(&paths)?;
        let probe = crate::client::TonicEndpoint.probe(&paths, before).await?;

        assert!(matches!(probe, EndpointProbe::AbsentOrInert));
        Ok(())
    }

    #[tokio::test]
    async fn tombstone_recovery_retires_held_residue_after_bounded_endpoint_reinspection()
    -> TestResult {
        let temp = tempfile::tempdir()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        let executable = std::env::current_exe()?.canonicalize()?;
        paths.prepare_directory()?;
        fs::write(paths.instance(), b"partial-publication")?;
        fs::set_permissions(paths.instance(), fs::Permissions::from_mode(0o200))?;
        let endpoint = MutableEndpoint::new(EndpointProbe::AbsentOrInert);
        let inspector = MutableInspector::new(None);
        let spawner = FakeSpawner::current(endpoint.clone(), inspector.clone());

        let outcome = start_with(
            &paths,
            &executable,
            &endpoint,
            &inspector,
            &spawner,
            test_timeouts(),
        )
        .await?;

        assert_eq!(outcome.transition, DaemonTransition::Started);
        assert_eq!(outcome.status.state, DaemonState::Current);
        assert_eq!(spawner.launches()?.len(), 1);
        assert!(endpoint.probes.load(AtomicOrdering::Acquire) >= 4);
        Ok(())
    }

    #[tokio::test]
    async fn tombstone_recovery_refuses_when_a_daemon_appears_during_reinspection() -> TestResult {
        let temp = tempfile::tempdir()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        let executable = std::env::current_exe()?.canonicalize()?;
        paths.prepare_directory()?;
        fs::write(paths.instance(), b"partial-publication")?;
        fs::set_permissions(paths.instance(), fs::Permissions::from_mode(0o200))?;
        let identity = legacy_identity(&executable);
        let inspector = MutableInspector::new(Some(process_for(&identity)));
        let endpoint = StopEndpoint::with_probes(
            vec![
                EndpointProbe::AbsentOrInert,
                EndpointProbe::AbsentOrInert,
                connected(identity),
            ],
            inspector.clone(),
        )?;
        let spawner = CountingNoopSpawner::default();

        let result = start_with(
            &paths,
            &executable,
            &endpoint,
            &inspector,
            &spawner,
            test_timeouts(),
        )
        .await;

        assert!(matches!(result, Err(SupervisorError::TombstoneBusy { .. })));
        fs::set_permissions(paths.instance(), fs::Permissions::from_mode(0o600))?;
        assert_eq!(fs::read(paths.instance())?, b"partial-publication");
        assert_eq!(spawner.0.load(AtomicOrdering::Acquire), 0);
        Ok(())
    }

    #[tokio::test]
    async fn tombstone_recovery_never_mutates_a_pathname_replacement() -> TestResult {
        let temp = tempfile::tempdir()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        let executable = std::env::current_exe()?.canonicalize()?;
        paths.prepare_directory()?;
        fs::write(paths.instance(), b"partial-publication")?;
        fs::set_permissions(paths.instance(), fs::Permissions::from_mode(0o200))?;
        let replacement = b"replacement-owned-elsewhere".to_vec();
        let endpoint = ReplacingEndpoint {
            instance_path: paths.instance().to_owned(),
            replacement: replacement.clone(),
            replace_on_probe: 2,
            probes: Arc::new(AtomicUsize::new(0)),
        };
        let spawner = CountingNoopSpawner::default();

        let result = start_with(
            &paths,
            &executable,
            &endpoint,
            &FakeInspector { identity: None },
            &spawner,
            test_timeouts(),
        )
        .await;

        assert!(matches!(
            result,
            Err(SupervisorError::TombstoneChanged { .. })
        ));
        fs::set_permissions(paths.instance(), fs::Permissions::from_mode(0o600))?;
        assert_eq!(fs::read(paths.instance())?, replacement);
        assert_eq!(spawner.0.load(AtomicOrdering::Acquire), 0);
        Ok(())
    }

    #[derive(Clone, Copy)]
    enum ShutdownFailure {
        Transport,
        Internal,
        PermissionDenied,
    }

    #[derive(Clone)]
    struct StopEndpoint {
        state: Arc<Mutex<EndpointProbe<()>>>,
        queued: Arc<Mutex<VecDeque<EndpointProbe<()>>>>,
        inspector: MutableInspector,
        shutdown_tokens: Arc<Mutex<Vec<String>>>,
        retire_path: Arc<Mutex<Option<PathBuf>>>,
        exit_on_shutdown: bool,
        stall_shutdown: bool,
        shutdown_failure: Option<ShutdownFailure>,
        stall_second_probe: bool,
        probes: Arc<AtomicUsize>,
    }

    impl StopEndpoint {
        fn new(
            state: EndpointProbe<()>,
            inspector: MutableInspector,
            exit_on_shutdown: bool,
        ) -> Self {
            Self {
                state: Arc::new(Mutex::new(state)),
                queued: Arc::new(Mutex::new(VecDeque::new())),
                inspector,
                shutdown_tokens: Arc::new(Mutex::new(Vec::new())),
                retire_path: Arc::new(Mutex::new(None)),
                exit_on_shutdown,
                stall_shutdown: false,
                shutdown_failure: None,
                stall_second_probe: false,
                probes: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn with_probes(
            probes: Vec<EndpointProbe<()>>,
            inspector: MutableInspector,
        ) -> io::Result<Self> {
            let fallback = probes
                .last()
                .cloned()
                .ok_or_else(|| io::Error::other("fake endpoint needs a probe"))?;
            Ok(Self {
                state: Arc::new(Mutex::new(fallback)),
                queued: Arc::new(Mutex::new(VecDeque::from(probes))),
                inspector,
                shutdown_tokens: Arc::new(Mutex::new(Vec::new())),
                retire_path: Arc::new(Mutex::new(None)),
                exit_on_shutdown: false,
                stall_shutdown: false,
                shutdown_failure: None,
                stall_second_probe: false,
                probes: Arc::new(AtomicUsize::new(0)),
            })
        }

        fn with_stalled_shutdown(mut self) -> Self {
            self.stall_shutdown = true;
            self
        }

        fn with_shutdown_failure(mut self, failure: ShutdownFailure) -> Self {
            self.shutdown_failure = Some(failure);
            self
        }

        fn with_stalled_second_probe(mut self) -> Self {
            self.stall_second_probe = true;
            self
        }

        fn set(&self, state: EndpointProbe<()>) -> io::Result<()> {
            *self
                .state
                .lock()
                .map_err(|_| io::Error::other("stop endpoint state was poisoned"))? = state;
            Ok(())
        }

        fn shutdown_tokens(&self) -> io::Result<Vec<String>> {
            self.shutdown_tokens
                .lock()
                .map_err(|_| io::Error::other("shutdown tokens were poisoned"))
                .map(|tokens| tokens.clone())
        }

        fn retire_on_exit(&self, path: &Path) -> io::Result<()> {
            *self
                .retire_path
                .lock()
                .map_err(|_| io::Error::other("retirement path was poisoned"))? =
                Some(path.to_owned());
            Ok(())
        }

        fn simulate_exit(&self) -> io::Result<()> {
            self.inspector.set(None)?;
            self.set(EndpointProbe::AbsentOrInert)?;
            let path = self
                .retire_path
                .lock()
                .map_err(|_| io::Error::other("retirement path was poisoned"))?
                .clone();
            if let Some(path) = path {
                fs::set_permissions(&path, fs::Permissions::from_mode(0o200))?;
                fs::write(path, b"")?;
            }
            Ok(())
        }
    }

    #[tonic::async_trait]
    impl DaemonEndpoint for StopEndpoint {
        type Connection = ();

        async fn probe(
            &self,
            paths: &DaemonPaths,
            _expected_path: EndpointPathState,
        ) -> Result<EndpointProbe<Self::Connection>, crate::client::ClientError> {
            let probe = self.probes.fetch_add(1, AtomicOrdering::AcqRel) + 1;
            if self.stall_second_probe && probe == 2 {
                return std::future::pending().await;
            }
            let state = if let Some(probe) = self
                .queued
                .lock()
                .map_err(|_| {
                    crate::client::ClientError::Io(io::Error::other(
                        "stop endpoint probes were poisoned",
                    ))
                })?
                .pop_front()
            {
                probe
            } else {
                self.state
                    .lock()
                    .map_err(|_| {
                        crate::client::ClientError::Io(io::Error::other(
                            "stop endpoint state was poisoned",
                        ))
                    })?
                    .clone()
            };
            publish_fake_socket_for_connected_probe(paths.socket(), &state)
                .map_err(crate::client::ClientError::Io)?;
            Ok(state)
        }

        async fn graceful_shutdown(
            &self,
            _connection: &mut Self::Connection,
            instance_token: &str,
        ) -> Result<(), crate::client::ClientError> {
            self.shutdown_tokens
                .lock()
                .map_err(|_| {
                    crate::client::ClientError::Io(io::Error::other(
                        "shutdown tokens were poisoned",
                    ))
                })?
                .push(instance_token.to_owned());
            if self.stall_shutdown {
                return std::future::pending().await;
            }
            if let Some(failure) = self.shutdown_failure {
                return Err(match failure {
                    ShutdownFailure::Transport => crate::client::ClientError::Io(io::Error::new(
                        io::ErrorKind::ConnectionReset,
                        "connection reset",
                    )),
                    ShutdownFailure::Internal => crate::client::ClientError::Rpc(Box::new(
                        tonic::Status::internal("daemon shutdown internal failure"),
                    )),
                    ShutdownFailure::PermissionDenied => crate::client::ClientError::Rpc(Box::new(
                        tonic::Status::permission_denied("daemon instance token does not match"),
                    )),
                });
            }
            if self.exit_on_shutdown {
                self.simulate_exit()?;
            }
            Ok(())
        }
    }

    #[derive(Clone)]
    struct RecordingAttestedSignaler {
        inspector: MutableInspector,
        endpoint: StopEndpoint,
        signals: Arc<Mutex<Vec<rustix::process::Signal>>>,
        verified_at_signal: Arc<Mutex<Vec<usize>>>,
        exit_on_signal: bool,
    }

    struct RecordingRawSignaler {
        signals: Arc<Mutex<Vec<rustix::process::Signal>>>,
    }

    impl ProcessSignaler for RecordingRawSignaler {
        fn signal(&self, _pid: u32, signal: rustix::process::Signal) -> io::Result<()> {
            self.signals
                .lock()
                .map_err(|_| io::Error::other("recorded signals were poisoned"))?
                .push(signal);
            Ok(())
        }
    }

    impl RecordingAttestedSignaler {
        fn new(inspector: MutableInspector, endpoint: StopEndpoint, exit_on_signal: bool) -> Self {
            Self {
                inspector,
                endpoint,
                signals: Arc::new(Mutex::new(Vec::new())),
                verified_at_signal: Arc::new(Mutex::new(Vec::new())),
                exit_on_signal,
            }
        }

        fn signals(&self) -> io::Result<Vec<rustix::process::Signal>> {
            self.signals
                .lock()
                .map_err(|_| io::Error::other("recorded signals were poisoned"))
                .map(|signals| signals.clone())
        }

        fn verified_at_signal(&self) -> io::Result<Vec<usize>> {
            self.verified_at_signal
                .lock()
                .map_err(|_| io::Error::other("verification counts were poisoned"))
                .map(|counts| counts.clone())
        }
    }

    impl AttestedProcessSignaler for RecordingAttestedSignaler {
        fn signal_attested(
            &self,
            identity: &DaemonIdentity,
            signal: rustix::process::Signal,
        ) -> io::Result<()> {
            let record = DaemonInstanceRecord {
                pid: identity.pid,
                owner_token: "endpoint-attested".to_owned(),
                executable: identity.executable.clone(),
                start_identity: identity.start_identity.clone(),
                instance_token: identity.instance_token.clone(),
                release_version: identity
                    .release_version
                    .clone()
                    .unwrap_or_else(|| "legacy".to_owned()),
                started_at: identity.started_at.clone().unwrap_or(InstanceTimestamp {
                    seconds: 1,
                    nanos: 0,
                }),
            };
            signal_attested_with(
                &record,
                &self.inspector,
                &RecordingRawSignaler {
                    signals: Arc::clone(&self.signals),
                },
                signal,
            )?;
            self.verified_at_signal
                .lock()
                .map_err(|_| io::Error::other("verification counts were poisoned"))?
                .push(self.inspector.inspections.load(AtomicOrdering::Acquire));
            if self.exit_on_signal {
                self.endpoint.simulate_exit()?;
            }
            Ok(())
        }
    }

    #[derive(Clone)]
    struct StallingAttestedSignaler {
        delay: Duration,
        timer_progressed: Arc<AtomicBool>,
    }

    impl AttestedProcessSignaler for StallingAttestedSignaler {
        fn signal_attested(
            &self,
            _identity: &DaemonIdentity,
            _signal: rustix::process::Signal,
        ) -> io::Result<()> {
            std::thread::sleep(self.delay);
            assert!(
                self.timer_progressed.load(Ordering::Acquire),
                "synchronous attested signaling blocked the current-thread Tokio executor"
            );
            Ok(())
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn attested_signaling_does_not_block_current_thread_timer_progress() -> TestResult {
        let executable = std::env::current_exe()?.canonicalize()?;
        let identity = endpoint_identity(&record(&executable));
        let timer_progressed = Arc::new(AtomicBool::new(false));
        let signaler = StallingAttestedSignaler {
            delay: Duration::from_millis(100),
            timer_progressed: Arc::clone(&timer_progressed),
        };

        let signal = async {
            signal_identity(
                &signaler,
                &identity,
                rustix::process::Signal::TERM,
                tokio::time::Instant::now() + Duration::from_secs(1),
            )
            .await
        };
        let timer = async {
            tokio::time::sleep(Duration::from_millis(10)).await;
            timer_progressed.store(true, Ordering::Release);
        };
        let (signal, ()) = tokio::join!(signal, timer);

        signal?;
        Ok(())
    }

    fn legacy_identity(executable: &Path) -> DaemonIdentity {
        DaemonIdentity {
            pid: std::process::id(),
            executable: executable.to_owned(),
            start_identity: "legacy-start".to_owned(),
            instance_token: "66".repeat(32),
            release_version: None,
            started_at: None,
        }
    }

    fn process_for(identity: &DaemonIdentity) -> ProcessIdentity {
        ProcessIdentity {
            pid: identity.pid,
            executable: identity.executable.clone(),
            start_identity: identity.start_identity.clone(),
        }
    }

    fn bind_safe_test_socket(paths: &DaemonPaths) -> io::Result<std::os::unix::net::UnixListener> {
        use std::os::unix::fs::PermissionsExt as _;
        paths.prepare_directory()?;
        let listener = std::os::unix::net::UnixListener::bind(paths.socket())?;
        fs::set_permissions(paths.socket(), fs::Permissions::from_mode(0o600))?;
        Ok(listener)
    }

    #[tokio::test]
    async fn stop_stopped_is_a_successful_noop() -> TestResult {
        let temp = tempfile::tempdir()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        let executable = std::env::current_exe()?.canonicalize()?;
        let inspector = MutableInspector::new(None);
        let endpoint = StopEndpoint::new(EndpointProbe::AbsentOrInert, inspector.clone(), false);
        let signaler = RecordingAttestedSignaler::new(inspector.clone(), endpoint.clone(), true);

        let outcome = stop_with(
            &paths,
            &executable,
            &endpoint,
            &inspector,
            &signaler,
            StopMode::Explicit { force: false },
            test_timeouts(),
        )
        .await?;

        assert_eq!(outcome.transition, DaemonTransition::None);
        assert_eq!(outcome.status.state, DaemonState::Stopped);
        assert!(endpoint.shutdown_tokens()?.is_empty());
        assert!(signaler.signals()?.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn stop_current_uses_authenticated_shutdown_rpc() -> TestResult {
        let temp = tempfile::tempdir()?;
        let executable = std::env::current_exe()?.canonicalize()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        let expected = record(&executable);
        write_record(&paths, &expected)?;
        let _listener = bind_safe_test_socket(&paths)?;
        let inspector = MutableInspector::new(Some(process_for(&endpoint_identity(&expected))));
        let endpoint = StopEndpoint::new(
            connected(endpoint_identity(&expected)),
            inspector.clone(),
            true,
        );
        let signaler = RecordingAttestedSignaler::new(inspector.clone(), endpoint.clone(), true);

        let outcome = stop_with(
            &paths,
            &executable,
            &endpoint,
            &inspector,
            &signaler,
            StopMode::Explicit { force: false },
            test_timeouts(),
        )
        .await?;

        assert_eq!(outcome.transition, DaemonTransition::Stopped);
        assert_eq!(endpoint.shutdown_tokens()?, vec![expected.instance_token]);
        assert!(signaler.signals()?.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn stop_graceful_timeout_is_typed_and_suggests_force() -> TestResult {
        let temp = tempfile::tempdir()?;
        let executable = std::env::current_exe()?.canonicalize()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        let expected = record(&executable);
        write_record(&paths, &expected)?;
        let _listener = bind_safe_test_socket(&paths)?;
        let inspector = MutableInspector::new(Some(process_for(&endpoint_identity(&expected))));
        let endpoint = StopEndpoint::new(
            connected(endpoint_identity(&expected)),
            inspector.clone(),
            false,
        );
        let signaler = RecordingAttestedSignaler::new(inspector.clone(), endpoint.clone(), true);

        let error = stop_with(
            &paths,
            &executable,
            &endpoint,
            &inspector,
            &signaler,
            StopMode::Explicit { force: false },
            test_timeouts(),
        )
        .await
        .err()
        .ok_or("graceful timeout unexpectedly succeeded")?;

        assert!(matches!(error, SupervisorError::GracefulTimeout { .. }));
        assert_eq!(error.suggestion(), Some("--force"));
        assert!(signaler.signals()?.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn stop_bounds_a_stalled_shutdown_rpc_without_automatic_force() -> TestResult {
        let temp = tempfile::tempdir()?;
        let executable = std::env::current_exe()?.canonicalize()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        let expected = record(&executable);
        write_record(&paths, &expected)?;
        let _listener = bind_safe_test_socket(&paths)?;
        let inspector = MutableInspector::new(Some(process_for(&endpoint_identity(&expected))));
        let endpoint = StopEndpoint::new(
            connected(endpoint_identity(&expected)),
            inspector.clone(),
            false,
        )
        .with_stalled_shutdown();
        let signaler = RecordingAttestedSignaler::new(inspector.clone(), endpoint.clone(), true);

        let result = tokio::time::timeout(
            Duration::from_millis(250),
            stop_with(
                &paths,
                &executable,
                &endpoint,
                &inspector,
                &signaler,
                StopMode::Automatic,
                test_timeouts(),
            ),
        )
        .await
        .map_err(|_| "stalled shutdown RPC escaped the supervisor timeout")?;

        assert!(matches!(
            result,
            Err(SupervisorError::GracefulTimeout { .. })
        ));
        assert_eq!(endpoint.shutdown_tokens()?, vec![expected.instance_token]);
        assert!(signaler.signals()?.is_empty());
        Ok(())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn graceful_stop_confirmation_cannot_overrun_its_absolute_deadline() -> TestResult {
        let temp = tempfile::tempdir()?;
        let executable = std::env::current_exe()?.canonicalize()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        let expected = record(&executable);
        write_record(&paths, &expected)?;
        let _listener = bind_safe_test_socket(&paths)?;
        let inspector = MutableInspector::new(Some(process_for(&endpoint_identity(&expected))));
        inspector.stall_on_absent_inspection(2, Duration::from_millis(200))?;
        let endpoint = StopEndpoint::new(
            connected(endpoint_identity(&expected)),
            inspector.clone(),
            true,
        );
        let signaler = RecordingAttestedSignaler::new(inspector.clone(), endpoint.clone(), true);
        let started = std::time::Instant::now();

        let result = stop_with(
            &paths,
            &executable,
            &endpoint,
            &inspector,
            &signaler,
            StopMode::Explicit { force: false },
            SupervisorTimeouts {
                readiness: Duration::from_millis(100),
                shutdown: Duration::from_millis(40),
                poll: Duration::from_millis(1),
            },
        )
        .await;

        assert!(matches!(
            result,
            Err(SupervisorError::IdentityChanged { .. })
        ));
        assert!(
            started.elapsed() < Duration::from_millis(120),
            "graceful stopped-state confirmation overran its deadline"
        );
        assert!(signaler.signals()?.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn stop_legacy_double_attests_then_verifies_process_before_sigterm() -> TestResult {
        let temp = tempfile::tempdir()?;
        let executable = std::env::current_exe()?.canonicalize()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        let _listener = bind_safe_test_socket(&paths)?;
        let identity = legacy_identity(&executable);
        let inspector = MutableInspector::new(Some(process_for(&identity)));
        let endpoint = StopEndpoint::with_probes(
            vec![connected(identity.clone()), connected(identity.clone())],
            inspector.clone(),
        )?;
        let signaler = RecordingAttestedSignaler::new(inspector.clone(), endpoint.clone(), true);

        let outcome = stop_with(
            &paths,
            &executable,
            &endpoint,
            &inspector,
            &signaler,
            StopMode::Automatic,
            test_timeouts(),
        )
        .await?;

        assert_eq!(outcome.transition, DaemonTransition::Stopped);
        assert!(endpoint.probes.load(AtomicOrdering::Acquire) >= 3);
        assert_eq!(signaler.signals()?, vec![rustix::process::Signal::TERM]);
        assert!(
            signaler
                .verified_at_signal()?
                .first()
                .copied()
                .unwrap_or_default()
                >= 2
        );
        Ok(())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn stop_legacy_second_attestation_timeout_never_force_signals() -> TestResult {
        let temp = tempfile::tempdir()?;
        let executable = std::env::current_exe()?.canonicalize()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        let _listener = bind_safe_test_socket(&paths)?;
        let identity = legacy_identity(&executable);
        let inspector = MutableInspector::new(Some(process_for(&identity)));
        let endpoint = StopEndpoint::new(connected(identity), inspector.clone(), false)
            .with_stalled_second_probe();
        let signaler = RecordingAttestedSignaler::new(inspector.clone(), endpoint.clone(), true);

        let result = stop_with(
            &paths,
            &executable,
            &endpoint,
            &inspector,
            &signaler,
            StopMode::Explicit { force: true },
            test_timeouts(),
        )
        .await;

        let detail = match result {
            Err(SupervisorError::IdentityChanged { detail }) => detail,
            other => return Err(format!("unexpected legacy stop result: {other:?}").into()),
        };
        assert!(
            detail.contains("legacy endpoint re-attestation timed out"),
            "stalled second attestation failed for the wrong reason: {detail}"
        );
        assert!(
            endpoint.probes.load(AtomicOrdering::Acquire) >= 2,
            "stalled second attestation was never attempted"
        );
        assert!(signaler.signals()?.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn stop_legacy_changed_endpoint_identity_never_signals() -> TestResult {
        let executable = std::env::current_exe()?.canonicalize()?;
        for (name, changed) in [
            ("token", {
                let mut identity = legacy_identity(&executable);
                identity.instance_token = "77".repeat(32);
                identity
            }),
            ("pid", {
                let mut identity = legacy_identity(&executable);
                identity.pid = identity.pid.saturating_add(1);
                identity
            }),
            ("executable", {
                let mut identity = legacy_identity(&executable);
                identity.executable = PathBuf::from("/different/gascand");
                identity
            }),
            ("start", {
                let mut identity = legacy_identity(&executable);
                identity.start_identity = "different-start".to_owned();
                identity
            }),
        ] {
            let temp = tempfile::tempdir()?;
            let paths =
                DaemonPaths::from_runtime_root(root(&temp)?.join(format!("runtime-{name}")));
            let _listener = bind_safe_test_socket(&paths)?;
            let original = legacy_identity(&executable);
            let inspector = MutableInspector::new(Some(process_for(&original)));
            let endpoint = StopEndpoint::with_probes(
                vec![connected(original), connected(changed)],
                inspector.clone(),
            )?;
            let signaler =
                RecordingAttestedSignaler::new(inspector.clone(), endpoint.clone(), true);

            let result = stop_with(
                &paths,
                &executable,
                &endpoint,
                &inspector,
                &signaler,
                StopMode::Automatic,
                test_timeouts(),
            )
            .await;

            let detail = match result {
                Err(SupervisorError::IdentityChanged { detail }) => detail,
                other => {
                    return Err(format!(
                        "changed {name} identity returned an unexpected result: {other:?}"
                    )
                    .into());
                }
            };
            assert!(
                detail.contains("legacy endpoint attestations were not identical"),
                "changed {name} identity failed for the wrong reason: {detail}"
            );
            assert!(
                endpoint.probes.load(AtomicOrdering::Acquire) >= 2,
                "changed {name} second attestation was never attempted"
            );
            assert!(
                signaler.signals()?.is_empty(),
                "changed {name} identity was signaled"
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn stop_automatic_mode_never_escalates_to_force() -> TestResult {
        let temp = tempfile::tempdir()?;
        let executable = std::env::current_exe()?.canonicalize()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        let expected = record(&executable);
        write_record(&paths, &expected)?;
        let _listener = bind_safe_test_socket(&paths)?;
        let inspector = MutableInspector::new(Some(process_for(&endpoint_identity(&expected))));
        let endpoint = StopEndpoint::new(
            connected(endpoint_identity(&expected)),
            inspector.clone(),
            false,
        );
        let signaler = RecordingAttestedSignaler::new(inspector.clone(), endpoint.clone(), true);

        let result = stop_with(
            &paths,
            &executable,
            &endpoint,
            &inspector,
            &signaler,
            StopMode::Automatic,
            test_timeouts(),
        )
        .await;

        assert!(matches!(
            result,
            Err(SupervisorError::GracefulTimeout { .. })
        ));
        assert!(!signaler.signals()?.contains(&rustix::process::Signal::KILL));
        Ok(())
    }

    #[tokio::test]
    async fn stop_explicit_force_revalidates_identity_and_confirms_exit() -> TestResult {
        let temp = tempfile::tempdir()?;
        let executable = std::env::current_exe()?.canonicalize()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        let expected = record(&executable);
        write_record(&paths, &expected)?;
        let _listener = bind_safe_test_socket(&paths)?;
        let identity = endpoint_identity(&expected);
        let inspector = MutableInspector::new(Some(process_for(&identity)));
        let endpoint = StopEndpoint::new(connected(identity), inspector.clone(), false);
        let signaler = RecordingAttestedSignaler::new(inspector.clone(), endpoint.clone(), true);

        let outcome = stop_with(
            &paths,
            &executable,
            &endpoint,
            &inspector,
            &signaler,
            StopMode::Explicit { force: true },
            test_timeouts(),
        )
        .await?;

        assert!(outcome.forced);
        assert_eq!(outcome.status.state, DaemonState::Stopped);
        assert_eq!(signaler.signals()?, vec![rustix::process::Signal::KILL]);
        assert!(
            signaler
                .verified_at_signal()?
                .first()
                .copied()
                .unwrap_or_default()
                >= 2
        );
        assert!(
            inspector
                .inspect(expected.pid, &expected.executable)?
                .is_none()
        );
        Ok(())
    }

    #[tokio::test]
    async fn stop_explicit_force_survives_attested_transport_and_internal_rpc_errors() -> TestResult
    {
        for (name, failure) in [
            ("transport", ShutdownFailure::Transport),
            ("internal", ShutdownFailure::Internal),
        ] {
            let temp = tempfile::tempdir()?;
            let executable = std::env::current_exe()?.canonicalize()?;
            let paths =
                DaemonPaths::from_runtime_root(root(&temp)?.join(format!("runtime-{name}")));
            let expected = record(&executable);
            write_record(&paths, &expected)?;
            let _listener = bind_safe_test_socket(&paths)?;
            let identity = endpoint_identity(&expected);
            let inspector = MutableInspector::new(Some(process_for(&identity)));
            let endpoint = StopEndpoint::new(connected(identity), inspector.clone(), false)
                .with_shutdown_failure(failure);
            let signaler =
                RecordingAttestedSignaler::new(inspector.clone(), endpoint.clone(), true);

            let outcome = stop_with(
                &paths,
                &executable,
                &endpoint,
                &inspector,
                &signaler,
                StopMode::Explicit { force: true },
                test_timeouts(),
            )
            .await?;

            assert!(outcome.forced, "{name} failure did not reach force");
            assert_eq!(
                signaler.signals()?,
                vec![rustix::process::Signal::KILL],
                "{name} failure did not use the attested force path"
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn stop_explicit_force_never_bypasses_shutdown_token_authentication() -> TestResult {
        let temp = tempfile::tempdir()?;
        let executable = std::env::current_exe()?.canonicalize()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        let expected = record(&executable);
        write_record(&paths, &expected)?;
        let _listener = bind_safe_test_socket(&paths)?;
        let identity = endpoint_identity(&expected);
        let inspector = MutableInspector::new(Some(process_for(&identity)));
        let endpoint = StopEndpoint::new(connected(identity), inspector.clone(), false)
            .with_shutdown_failure(ShutdownFailure::PermissionDenied);
        let signaler = RecordingAttestedSignaler::new(inspector.clone(), endpoint.clone(), true);

        let result = stop_with(
            &paths,
            &executable,
            &endpoint,
            &inspector,
            &signaler,
            StopMode::Explicit { force: true },
            test_timeouts(),
        )
        .await;

        assert!(matches!(
            result,
            Err(SupervisorError::Client(crate::client::ClientError::Rpc(status)))
                if status.code() == tonic::Code::PermissionDenied
        ));
        assert!(
            signaler.signals()?.is_empty(),
            "token authentication failure reached an OS signal"
        );
        Ok(())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn forced_stop_confirmation_cannot_overrun_its_absolute_deadline() -> TestResult {
        let temp = tempfile::tempdir()?;
        let executable = std::env::current_exe()?.canonicalize()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        let expected = record(&executable);
        write_record(&paths, &expected)?;
        let _listener = bind_safe_test_socket(&paths)?;
        let identity = endpoint_identity(&expected);
        let inspector = MutableInspector::new(Some(process_for(&identity)));
        inspector.stall_on_absent_inspection(2, Duration::from_millis(200))?;
        let endpoint = StopEndpoint::new(connected(identity), inspector.clone(), false);
        let signaler = RecordingAttestedSignaler::new(inspector.clone(), endpoint.clone(), true);
        let started = std::time::Instant::now();

        let result = stop_with(
            &paths,
            &executable,
            &endpoint,
            &inspector,
            &signaler,
            StopMode::Explicit { force: true },
            SupervisorTimeouts {
                readiness: Duration::from_millis(100),
                shutdown: Duration::from_millis(40),
                poll: Duration::from_millis(1),
            },
        )
        .await;

        assert!(matches!(
            result,
            Err(SupervisorError::IdentityChanged { .. })
        ));
        assert!(
            started.elapsed() < Duration::from_millis(120),
            "forced stopped-state confirmation overran its deadline"
        );
        assert_eq!(signaler.signals()?, vec![rustix::process::Signal::KILL]);
        Ok(())
    }

    #[derive(Clone, Default)]
    struct NeverSignaler {
        signals: Arc<AtomicUsize>,
    }

    impl AttestedProcessSignaler for NeverSignaler {
        fn signal_attested(
            &self,
            _identity: &DaemonIdentity,
            _signal: rustix::process::Signal,
        ) -> io::Result<()> {
            self.signals.fetch_add(1, AtomicOrdering::AcqRel);
            Err(io::Error::other("unexpected signal"))
        }
    }

    #[derive(Clone)]
    struct StopSpawner {
        endpoint: StopEndpoint,
        inspector: MutableInspector,
        launches: Arc<Mutex<Vec<DaemonLaunch>>>,
    }

    impl StopSpawner {
        fn new(endpoint: StopEndpoint, inspector: MutableInspector) -> Self {
            Self {
                endpoint,
                inspector,
                launches: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn launch_count(&self) -> io::Result<usize> {
            self.launches
                .lock()
                .map_err(|_| io::Error::other("stop launches were poisoned"))
                .map(|launches| launches.len())
        }
    }

    impl DaemonSpawner for StopSpawner {
        fn spawn(&self, launch: &DaemonLaunch) -> io::Result<DaemonStartupMonitor> {
            self.launches
                .lock()
                .map_err(|_| io::Error::other("stop launches were poisoned"))?
                .push(launch.clone());
            let published = DaemonInstanceRecord {
                pid: std::process::id(),
                owner_token: launch.owner_token.clone(),
                executable: launch.executable.clone(),
                start_identity: "replacement-start".to_owned(),
                instance_token: "88".repeat(32),
                release_version: env!("CARGO_PKG_VERSION").to_owned(),
                started_at: InstanceTimestamp {
                    seconds: 1_785_264_000,
                    nanos: 789_000_000,
                },
            };
            fs::write(&launch.instance_path, serde_json::to_vec(&published)?)?;
            fs::set_permissions(&launch.instance_path, fs::Permissions::from_mode(0o600))?;
            self.inspector
                .set(Some(process_for(&endpoint_identity(&published))))?;
            self.endpoint
                .set(connected(endpoint_identity(&published)))?;
            Ok(DaemonStartupMonitor::default())
        }
    }

    struct GatedRecoveryObserver {
        started: Arc<Notify>,
        release: Arc<Notify>,
        transitions: Arc<Mutex<Vec<DaemonTransition>>>,
    }

    #[tonic::async_trait]
    impl DaemonLifecycleObserver for GatedRecoveryObserver {
        async fn transition_started(&mut self, transition: DaemonTransition) {
            if let Ok(mut transitions) = self.transitions.lock() {
                transitions.push(transition);
            }
            self.started.notify_waiters();
            self.release.notified().await;
        }
    }

    #[tokio::test]
    async fn restart_stopped_starts_the_installed_daemon() -> TestResult {
        let temp = tempfile::tempdir()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        let executable = std::env::current_exe()?.canonicalize()?;
        let inspector = MutableInspector::new(None);
        let endpoint = StopEndpoint::new(EndpointProbe::AbsentOrInert, inspector.clone(), false);
        let spawner = StopSpawner::new(endpoint.clone(), inspector.clone());

        let outcome = restart_with(
            &paths,
            &executable,
            &endpoint,
            &inspector,
            &spawner,
            &NeverSignaler::default(),
            ShutdownPolicy::new(StopMode::Explicit { force: false }, test_timeouts()),
        )
        .await?;

        assert_eq!(outcome.transition, DaemonTransition::Restarted);
        assert_eq!(outcome.status.state, DaemonState::Current);
        assert_eq!(spawner.launch_count()?, 1);
        Ok(())
    }

    #[tokio::test]
    async fn restart_current_gracefully_stops_then_starts_once() -> TestResult {
        let temp = tempfile::tempdir()?;
        let executable = std::env::current_exe()?.canonicalize()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        let expected = record(&executable);
        write_record(&paths, &expected)?;
        let _listener = bind_safe_test_socket(&paths)?;
        let inspector = MutableInspector::new(Some(process_for(&endpoint_identity(&expected))));
        let endpoint = StopEndpoint::new(
            connected(endpoint_identity(&expected)),
            inspector.clone(),
            true,
        );
        endpoint.retire_on_exit(paths.instance())?;
        let spawner = StopSpawner::new(endpoint.clone(), inspector.clone());

        let outcome = restart_with(
            &paths,
            &executable,
            &endpoint,
            &inspector,
            &spawner,
            &NeverSignaler::default(),
            ShutdownPolicy::new(StopMode::Explicit { force: false }, test_timeouts()),
        )
        .await?;

        assert_eq!(outcome.transition, DaemonTransition::Restarted);
        assert_eq!(outcome.status.state, DaemonState::Current);
        assert_eq!(endpoint.shutdown_tokens()?, vec![expected.instance_token]);
        assert_eq!(spawner.launch_count()?, 1);
        Ok(())
    }

    #[tokio::test]
    async fn connect_current_returns_the_validated_connection_without_mutation() -> TestResult {
        let temp = tempfile::tempdir()?;
        let executable = std::env::current_exe()?.canonicalize()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        let expected = record(&executable);
        write_record(&paths, &expected)?;
        let inspector = MutableInspector::new(Some(process_for(&endpoint_identity(&expected))));
        let endpoint = MutableEndpoint::new(connected(endpoint_identity(&expected)));
        let spawner = FakeSpawner::current(endpoint.clone(), inspector.clone());
        let signaler = NeverSignaler::default();

        let outcome: ConnectionOutcome<()> = connect_current_or_recover_with(
            &paths,
            &executable,
            &endpoint,
            &inspector,
            &spawner,
            &signaler,
            test_timeouts(),
        )
        .await?;

        assert_eq!(outcome.transition, DaemonTransition::None);
        assert_eq!(outcome.daemon.identity, endpoint_identity(&expected));
        assert!(spawner.launches()?.is_empty());
        assert_eq!(signaler.signals.load(AtomicOrdering::Acquire), 0);
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn connect_waits_for_gated_publication_then_converges_on_current() -> TestResult {
        let temp = tempfile::tempdir()?;
        let paths = Arc::new(DaemonPaths::from_runtime_root(root(&temp)?.join("runtime")));
        let executable = Arc::new(std::env::current_exe()?.canonicalize()?);
        let endpoint = Arc::new(MutableEndpoint::new(EndpointProbe::Unsafe(
            "daemon publication is in progress".to_owned(),
        )));
        let inspector = Arc::new(MutableInspector::new(None));
        let spawner = Arc::new(FakeSpawner::current(
            endpoint.as_ref().clone(),
            inspector.as_ref().clone(),
        ));
        let signaler = Arc::new(NeverSignaler::default());
        let lifecycle = paths.lock()?;
        let contender = {
            let paths = Arc::clone(&paths);
            let executable = Arc::clone(&executable);
            let endpoint = Arc::clone(&endpoint);
            let inspector = Arc::clone(&inspector);
            let spawner = Arc::clone(&spawner);
            let signaler = Arc::clone(&signaler);
            tokio::spawn(async move {
                connect_current_or_recover_with(
                    &paths,
                    &executable,
                    endpoint.as_ref(),
                    inspector.as_ref(),
                    spawner.as_ref(),
                    signaler.as_ref(),
                    test_timeouts(),
                )
                .await
            })
        };

        tokio::time::timeout(Duration::from_secs(1), async {
            while endpoint.probes.load(AtomicOrdering::Acquire) < 1 {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        assert!(
            !contender.is_finished(),
            "a transient unsafe publication was rejected before lifecycle serialization"
        );

        let expected = record(&executable);
        write_record(&paths, &expected)?;
        let _listener = bind_safe_test_socket(&paths)?;
        inspector.set(matching_inspector(&expected).identity)?;
        endpoint.set(connected(endpoint_identity(&expected)))?;
        drop(lifecycle);

        let outcome = tokio::time::timeout(Duration::from_secs(1), contender).await???;
        assert_eq!(outcome.transition, DaemonTransition::None);
        assert_eq!(outcome.daemon.identity, endpoint_identity(&expected));
        assert!(spawner.launches()?.is_empty());
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn connect_waits_for_shutdown_contender_then_converges_on_current() -> TestResult {
        let temp = tempfile::tempdir()?;
        let paths = Arc::new(DaemonPaths::from_runtime_root(root(&temp)?.join("runtime")));
        let executable = Arc::new(std::env::current_exe()?.canonicalize()?);
        let expected = record(&executable);
        write_record(&paths, &expected)?;
        let _listener = bind_safe_test_socket(&paths)?;
        let endpoint = Arc::new(MutableEndpoint::new(EndpointProbe::Unresponsive(
            "daemon is closing its listener".to_owned(),
        )));
        let inspector = Arc::new(MutableInspector::new(
            matching_inspector(&expected).identity,
        ));
        let spawner = Arc::new(FakeSpawner::current(
            endpoint.as_ref().clone(),
            inspector.as_ref().clone(),
        ));
        let signaler = Arc::new(NeverSignaler::default());
        let lifecycle = paths.lock()?;
        let contender = {
            let paths = Arc::clone(&paths);
            let executable = Arc::clone(&executable);
            let endpoint = Arc::clone(&endpoint);
            let inspector = Arc::clone(&inspector);
            let spawner = Arc::clone(&spawner);
            let signaler = Arc::clone(&signaler);
            tokio::spawn(async move {
                connect_current_or_recover_with(
                    &paths,
                    &executable,
                    endpoint.as_ref(),
                    inspector.as_ref(),
                    spawner.as_ref(),
                    signaler.as_ref(),
                    test_timeouts(),
                )
                .await
            })
        };

        tokio::time::timeout(Duration::from_secs(1), async {
            while endpoint.probes.load(AtomicOrdering::Acquire) < 1 {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        assert!(
            !contender.is_finished(),
            "a transient unreachable shutdown state was rejected before lifecycle serialization"
        );

        endpoint.set(connected(endpoint_identity(&expected)))?;
        drop(lifecycle);

        let outcome = tokio::time::timeout(Duration::from_secs(1), contender).await???;
        assert_eq!(outcome.transition, DaemonTransition::None);
        assert_eq!(outcome.daemon.identity, endpoint_identity(&expected));
        assert!(spawner.launches()?.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn connect_stopped_starts_and_returns_a_current_connection() -> TestResult {
        let temp = tempfile::tempdir()?;
        let executable = std::env::current_exe()?.canonicalize()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        let inspector = MutableInspector::new(None);
        let endpoint = MutableEndpoint::new(EndpointProbe::AbsentOrInert);
        let spawner = FakeSpawner::current(endpoint.clone(), inspector.clone());

        let outcome = connect_current_or_recover_with(
            &paths,
            &executable,
            &endpoint,
            &inspector,
            &spawner,
            &NeverSignaler::default(),
            test_timeouts(),
        )
        .await?;

        assert_eq!(outcome.transition, DaemonTransition::Started);
        assert_eq!(
            outcome.daemon.identity.release_version.as_deref(),
            Some(env!("CARGO_PKG_VERSION"))
        );
        assert_eq!(spawner.launches()?.len(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn connect_outdated_gracefully_recovers_without_force() -> TestResult {
        let temp = tempfile::tempdir()?;
        let executable = std::env::current_exe()?.canonicalize()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        let mut expected = record(&executable);
        expected.release_version = "0.1.10".to_owned();
        write_record(&paths, &expected)?;
        let inspector = MutableInspector::new(Some(process_for(&endpoint_identity(&expected))));
        let endpoint = StopEndpoint::new(
            connected(endpoint_identity(&expected)),
            inspector.clone(),
            true,
        );
        endpoint.retire_on_exit(paths.instance())?;
        let spawner = StopSpawner::new(endpoint.clone(), inspector.clone());
        let signaler = RecordingAttestedSignaler::new(inspector.clone(), endpoint.clone(), true);

        let outcome = connect_current_or_recover_with(
            &paths,
            &executable,
            &endpoint,
            &inspector,
            &spawner,
            &signaler,
            test_timeouts(),
        )
        .await?;

        assert_eq!(outcome.transition, DaemonTransition::Recovered);
        assert_eq!(
            outcome.daemon.identity.release_version.as_deref(),
            Some(env!("CARGO_PKG_VERSION"))
        );
        assert_eq!(spawner.launch_count()?, 1);
        assert!(!signaler.signals()?.contains(&rustix::process::Signal::KILL));
        Ok(())
    }

    #[tokio::test]
    async fn recovery_observer_starts_before_gated_outdated_shutdown() -> TestResult {
        let temp = tempfile::tempdir()?;
        let executable = std::env::current_exe()?.canonicalize()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        let mut expected = record(&executable);
        expected.release_version = "0.1.10".to_owned();
        write_record(&paths, &expected)?;
        let inspector = MutableInspector::new(Some(process_for(&endpoint_identity(&expected))));
        let endpoint = StopEndpoint::new(
            connected(endpoint_identity(&expected)),
            inspector.clone(),
            true,
        );
        endpoint.retire_on_exit(paths.instance())?;
        let spawner = StopSpawner::new(endpoint.clone(), inspector.clone());
        let signaler = RecordingAttestedSignaler::new(inspector.clone(), endpoint.clone(), true);
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let transitions = Arc::new(Mutex::new(Vec::new()));
        let started_wait = started.notified();
        let task_paths = paths.clone();
        let task_executable = executable.clone();
        let task_endpoint = endpoint.clone();
        let task_inspector = inspector.clone();
        let task_spawner = spawner.clone();
        let task_signaler = signaler.clone();
        let mut observer = GatedRecoveryObserver {
            started: started.clone(),
            release: release.clone(),
            transitions: transitions.clone(),
        };

        let task = tokio::spawn(async move {
            connect_current_or_recover_with_observer(
                &task_paths,
                &task_executable,
                &task_endpoint,
                &task_inspector,
                &task_spawner,
                &task_signaler,
                test_timeouts(),
                &mut observer,
            )
            .await
        });

        started_wait.await;
        assert_eq!(endpoint.shutdown_tokens()?, Vec::<String>::new());
        assert_eq!(spawner.launch_count()?, 0);
        assert_eq!(
            *transitions
                .lock()
                .map_err(|_| io::Error::other("recovery transitions were poisoned"))?,
            vec![DaemonTransition::Recovered]
        );

        release.notify_one();
        let outcome = task.await??;
        assert_eq!(outcome.transition, DaemonTransition::Recovered);
        Ok(())
    }

    #[tokio::test]
    async fn connect_outdated_timeout_never_force_kills_or_spawns() -> TestResult {
        let temp = tempfile::tempdir()?;
        let executable = std::env::current_exe()?.canonicalize()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        let mut expected = record(&executable);
        expected.release_version = "0.1.10".to_owned();
        write_record(&paths, &expected)?;
        let inspector = MutableInspector::new(Some(process_for(&endpoint_identity(&expected))));
        let endpoint = StopEndpoint::new(
            connected(endpoint_identity(&expected)),
            inspector.clone(),
            false,
        );
        let spawner = StopSpawner::new(endpoint.clone(), inspector.clone());
        let signaler = RecordingAttestedSignaler::new(inspector.clone(), endpoint.clone(), true);

        let result = connect_current_or_recover_with(
            &paths,
            &executable,
            &endpoint,
            &inspector,
            &spawner,
            &signaler,
            test_timeouts(),
        )
        .await;

        assert!(matches!(
            result,
            Err(SupervisorError::GracefulTimeout { .. })
        ));
        assert_eq!(spawner.launch_count()?, 0);
        assert!(!signaler.signals()?.contains(&rustix::process::Signal::KILL));
        Ok(())
    }

    #[test]
    fn attestation_accepts_valid_current_record_with_live_identity() -> TestResult {
        let temp = tempfile::tempdir()?;
        let executable = std::env::current_exe()?.canonicalize()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        let expected = record(&executable);
        write_record(&paths, &expected)?;
        let inspector = FakeInspector {
            identity: Some(ProcessIdentity {
                pid: expected.pid,
                executable,
                start_identity: expected.start_identity.clone(),
            }),
        };
        assert_eq!(read_attested_instance(&paths, &inspector)?, Some(expected));
        Ok(())
    }

    #[test]
    fn attestation_rejects_unsafe_or_malformed_records_and_reports_absence() -> TestResult {
        let temp = tempfile::tempdir()?;
        let executable = std::env::current_exe()?.canonicalize()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        paths.prepare_directory()?;
        let inspector = FakeInspector { identity: None };
        assert_eq!(read_attested_instance(&paths, &inspector)?, None);

        let cases: &[(&str, &[u8], u32)] =
            &[("malformed", b"{", 0o600), ("permissive", b"{}", 0o644)];
        for (name, contents, mode) in cases {
            let case = DaemonPaths::from_runtime_root(root(&temp)?.join(name));
            case.prepare_directory()?;
            fs::write(case.instance(), contents)?;
            fs::set_permissions(case.instance(), fs::Permissions::from_mode(*mode))?;
            assert!(read_attested_instance(&case, &inspector).is_err());
        }

        let wrong_uid_path = DaemonPaths::from_runtime_root(root(&temp)?.join("wrong-uid-file"));
        write_record(&wrong_uid_path, &record(&executable))?;
        let fd = rustix::fs::open(
            wrong_uid_path.instance(),
            rustix::fs::OFlags::RDONLY,
            rustix::fs::Mode::empty(),
        )?;
        let stat = rustix::fs::fstat(fd)?;
        assert!(
            super::validate_file_stat(
                &stat,
                rustix::process::geteuid().as_raw().saturating_add(1),
            )
            .is_err()
        );

        let symlink = DaemonPaths::from_runtime_root(root(&temp)?.join("symlink"));
        symlink.prepare_directory()?;
        let target = root(&temp)?.join("record-target");
        fs::write(&target, serde_json::to_vec(&record(&executable))?)?;
        std::os::unix::fs::symlink(target, symlink.instance())?;
        assert!(read_attested_instance(&symlink, &inspector).is_err());

        let non_regular = DaemonPaths::from_runtime_root(root(&temp)?.join("non-regular"));
        non_regular.prepare_directory()?;
        fs::create_dir(non_regular.instance())?;
        assert!(read_attested_instance(&non_regular, &inspector).is_err());

        let wrong_owner = DaemonPaths::from_runtime_root_with_uid(
            root(&temp)?.join("wrong-owner-record"),
            rustix::process::geteuid().as_raw().saturating_add(1),
        );
        fs::create_dir(wrong_owner.directory())?;
        fs::set_permissions(wrong_owner.directory(), fs::Permissions::from_mode(0o700))?;
        fs::write(
            wrong_owner.instance(),
            serde_json::to_vec(&record(&executable))?,
        )?;
        fs::set_permissions(wrong_owner.instance(), fs::Permissions::from_mode(0o600))?;
        assert!(read_attested_instance(&wrong_owner, &inspector).is_err());
        Ok(())
    }

    #[test]
    fn attestation_treats_exact_empty_write_only_tombstone_as_absent() -> TestResult {
        let temp = tempfile::tempdir()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        paths.prepare_directory()?;
        fs::write(paths.instance(), b"")?;
        fs::set_permissions(paths.instance(), fs::Permissions::from_mode(0o200))?;
        assert_eq!(
            read_attested_instance(&paths, &FakeInspector { identity: None })?,
            None
        );
        Ok(())
    }

    #[test]
    fn attestation_rejects_nonempty_write_only_publication_residue() -> TestResult {
        let temp = tempfile::tempdir()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        paths.prepare_directory()?;
        fs::write(paths.instance(), b"partial")?;
        fs::set_permissions(paths.instance(), fs::Permissions::from_mode(0o200))?;
        assert!(
            read_attested_instance(&paths, &FakeInspector { identity: None }).is_err(),
            "nonempty unpublished bytes were treated as an absent record"
        );
        Ok(())
    }

    #[test]
    fn attestation_rejects_changed_record_identity_between_reads() -> TestResult {
        let temp = tempfile::tempdir()?;
        let executable = std::env::current_exe()?.canonicalize()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        let expected = record(&executable);
        write_record(&paths, &expected)?;
        let replacement = serde_json::to_vec(&expected)?;
        let result = read_instance_record_with_hook(&paths, || {
            fs::remove_file(paths.instance())?;
            fs::write(paths.instance(), &replacement)?;
            fs::set_permissions(paths.instance(), fs::Permissions::from_mode(0o600))
        });
        assert!(result.is_err());
        Ok(())
    }

    #[test]
    fn attestation_rejects_pid_reuse_and_executable_mismatch() -> TestResult {
        let temp = tempfile::tempdir()?;
        let executable = std::env::current_exe()?.canonicalize()?;
        let paths = DaemonPaths::from_runtime_root(root(&temp)?.join("runtime"));
        let expected = record(&executable);
        write_record(&paths, &expected)?;

        for identity in [
            ProcessIdentity {
                pid: expected.pid,
                executable: executable.clone(),
                start_identity: "reused-pid".to_owned(),
            },
            ProcessIdentity {
                pid: expected.pid,
                executable: root(&temp)?.join("different-executable"),
                start_identity: expected.start_identity.clone(),
            },
        ] {
            assert!(
                read_attested_instance(
                    &paths,
                    &FakeInspector {
                        identity: Some(identity)
                    }
                )
                .is_err()
            );
        }
        Ok(())
    }

    #[test]
    fn attestation_rejects_semantically_invalid_records() -> TestResult {
        let temp = tempfile::tempdir()?;
        let executable = std::env::current_exe()?.canonicalize()?;
        let valid = record(&executable);
        let inspector = FakeInspector {
            identity: Some(ProcessIdentity {
                pid: valid.pid,
                executable: executable.clone(),
                start_identity: valid.start_identity.clone(),
            }),
        };
        let mut cases = Vec::new();
        let mut empty_owner = valid.clone();
        empty_owner.owner_token.clear();
        cases.push(("empty-owner", empty_owner));
        let mut relative_executable = valid.clone();
        relative_executable.executable = PathBuf::from("gascand");
        cases.push(("relative-executable", relative_executable));
        let mut empty_start = valid.clone();
        empty_start.start_identity.clear();
        cases.push(("empty-start", empty_start));
        let mut bad_instance_token = valid.clone();
        bad_instance_token.instance_token = "not-a-256-bit-token".to_owned();
        cases.push(("bad-instance-token", bad_instance_token));
        let mut empty_release = valid.clone();
        empty_release.release_version.clear();
        cases.push(("empty-release", empty_release));
        let mut bad_timestamp = valid;
        bad_timestamp.started_at.nanos = 1_000_000_000;
        cases.push(("bad-timestamp", bad_timestamp));

        for (name, invalid) in cases {
            let paths = DaemonPaths::from_runtime_root(root(&temp)?.join(name));
            write_record(&paths, &invalid)?;
            assert!(
                read_attested_instance(&paths, &inspector).is_err(),
                "accepted semantically invalid record: {name}"
            );
        }
        Ok(())
    }

    #[test]
    fn attestation_live_process_inspector_reports_exact_current_identity() -> TestResult {
        let executable = std::env::current_exe()?.canonicalize()?;
        let identity = OsProcessInspector
            .inspect(std::process::id(), &executable)?
            .ok_or("current process was not live")?;
        assert_eq!(identity.pid, std::process::id());
        assert_eq!(identity.executable, executable);
        assert!(!identity.start_identity.is_empty());
        Ok(())
    }

    #[test]
    fn attestation_process_snapshot_rejects_changed_start_identity() -> TestResult {
        let result = coherent_process_identity(
            7,
            Path::new("/trusted/gascand"),
            "start:one".to_owned(),
            PathBuf::from("/trusted/gascand"),
            "start:two".to_owned(),
        );
        assert!(result.is_err());
        Ok(())
    }

    #[test]
    fn attestation_process_snapshot_rejects_different_executable() -> TestResult {
        let result = coherent_process_identity(
            7,
            Path::new("/trusted/gascand"),
            "start:one".to_owned(),
            PathBuf::from("/attacker/gascand"),
            "start:one".to_owned(),
        );
        assert!(result.is_err());
        Ok(())
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn attestation_macos_snapshot_uses_observed_executable_not_argv() -> TestResult {
        let expected = Path::new("/trusted/gascand");
        let mut starts = [
            Some("Mon Jul 28 12:00:00 2026".to_owned()),
            Some("Mon Jul 28 12:00:00 2026".to_owned()),
        ]
        .into_iter();
        let result = inspect_process_with(
            7,
            expected,
            || {
                starts
                    .next()
                    .ok_or_else(|| io::Error::other("unexpected start identity request"))
            },
            || Ok(Some(PathBuf::from("/attacker/gascand"))),
        );
        assert!(result.is_err());
        Ok(())
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn attestation_macos_parser_selects_first_kernel_text_vnode() -> TestResult {
        let output = b"p7\0\nftxt\0n/trusted/path with spaces/gascand\0\nftxt\0n/usr/lib/dyld\0\n";
        assert_eq!(
            parse_lsof_executable(output, 7)?,
            Some(PathBuf::from("/trusted/path with spaces/gascand"))
        );
        Ok(())
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn attestation_macos_parser_rejects_wrong_process_record() -> TestResult {
        let output = b"p8\0\nftxt\0n/trusted/gascand\0\n";
        assert!(parse_lsof_executable(output, 7).is_err());
        Ok(())
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn attestation_macos_parser_treats_empty_exit_race_as_absent() -> TestResult {
        assert_eq!(parse_lsof_executable(b"", 7)?, None);
        Ok(())
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn attestation_process_snapshot_handles_executable_paths_with_spaces() -> TestResult {
        let expected = Path::new("/trusted/path with spaces/gascand");
        let mut starts = [
            Some("Mon Jul 28 12:00:00 2026".to_owned()),
            Some("Mon Jul 28 12:00:00 2026".to_owned()),
        ]
        .into_iter();
        let identity = inspect_process_with(
            7,
            expected,
            || {
                starts
                    .next()
                    .ok_or_else(|| io::Error::other("unexpected start identity request"))
            },
            || Ok(Some(expected.to_owned())),
        )?
        .ok_or("snapshot was unexpectedly absent")?;
        assert_eq!(identity.executable, expected);
        Ok(())
    }

    #[derive(Default)]
    struct CountingSignaler(std::sync::atomic::AtomicUsize);

    impl ProcessSignaler for CountingSignaler {
        fn signal(&self, _pid: u32, _signal: rustix::process::Signal) -> io::Result<()> {
            self.0.fetch_add(1, Ordering::AcqRel);
            Ok(())
        }
    }

    #[test]
    fn attestation_signal_rechecks_identity_immediately_before_signaling() -> TestResult {
        let executable = std::env::current_exe()?.canonicalize()?;
        let expected = record(&executable);
        let reused = FakeInspector {
            identity: Some(ProcessIdentity {
                pid: expected.pid,
                executable,
                start_identity: "reused-before-signal".to_owned(),
            }),
        };
        let signaler = CountingSignaler::default();
        assert!(
            signal_attested_with(&expected, &reused, &signaler, rustix::process::Signal::TERM,)
                .is_err()
        );
        assert_eq!(signaler.0.load(Ordering::Acquire), 0);
        Ok(())
    }

    #[test]
    fn attestation_signal_rechecks_deadline_immediately_before_signaling() -> TestResult {
        let executable = std::env::current_exe()?.canonicalize()?;
        let expected = record(&executable);
        let inspector = StallingInspector {
            identity: Some(process_for(&endpoint_identity(&expected))),
            delay: Duration::from_millis(30),
            timer_progressed: Arc::new(AtomicBool::new(true)),
        };
        let signaler = CountingSignaler::default();

        let error = match signal_attested_with_deadline(
            &expected,
            &inspector,
            &signaler,
            rustix::process::Signal::TERM,
            Some(std::time::Instant::now() + Duration::from_millis(5)),
        ) {
            Err(error) => error,
            Ok(()) => return Err("an expired transition deadline allowed signaling".into()),
        };

        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert_eq!(signaler.0.load(Ordering::Acquire), 0);
        Ok(())
    }

    #[test]
    fn attestation_rejects_pid_outside_platform_range() {
        assert!(checked_pid(u32::MAX).is_err());
    }
}
