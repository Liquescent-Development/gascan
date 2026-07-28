#![allow(
    dead_code,
    reason = "Task 4 foundations are consumed by the Task 5 daemon supervisor"
)]

use rustix::fd::OwnedFd;
use rustix::fs::{AtFlags, FileType, FlockOperation, Mode, OFlags};
use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, Read as _};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

const DIRECTORY_MODE: u16 = 0o700;
const FILE_MODE: u16 = 0o600;
const SOCKET_NAME: &str = "gascand.sock";
const INSTANCE_NAME: &str = "daemon-instance.json";
const LIFECYCLE_LOCK_NAME: &str = "daemon-lifecycle.lock";
const MAX_INSTANCE_BYTES: u64 = 64 * 1024;
const LIFECYCLE_LOCK_TIMEOUT: Duration = Duration::from_secs(5);
#[cfg(not(target_os = "linux"))]
const PROCESS_INSPECTION_TIMEOUT: Duration = Duration::from_millis(500);

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

    pub(crate) fn lock(&self) -> io::Result<LifecycleLock> {
        self.lock_with_timeout(LIFECYCLE_LOCK_TIMEOUT)
    }

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

pub(crate) trait ProcessInspector {
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

struct OsProcessSignaler;

impl ProcessSignaler for OsProcessSignaler {
    fn signal(&self, pid: u32, signal: rustix::process::Signal) -> io::Result<()> {
        let pid = checked_pid(pid)?;
        rustix::process::kill_process(pid, signal).map_err(errno)
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

#[allow(dead_code)]
#[cfg(target_os = "linux")]
pub(crate) fn signal_attested(
    record: &DaemonInstanceRecord,
    signal: rustix::process::Signal,
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
    let identity = inspector
        .inspect(record.pid, &record.executable)?
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "daemon process exited before signaling",
            )
        })?;
    require_identity_match(record, &identity)?;
    signaler.signal(record.pid, signal)
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

fn read_instance_record_with_hook<F>(
    paths: &DaemonPaths,
    between_identity_and_open: F,
) -> io::Result<Option<DaemonInstanceRecord>>
where
    F: FnOnce() -> io::Result<()>,
{
    let (parent, name) = instance_parent_and_name(paths.instance())?;
    let directory = open_private_directory(parent, paths.expected_uid)?;
    let expected = match file_identity_at(&directory, name, paths.expected_uid) {
        Ok(identity) => identity,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
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

fn validate_record(record: &DaemonInstanceRecord) -> io::Result<()> {
    if record.pid == 0
        || record.owner_token.is_empty()
        || !record.executable.is_absolute()
        || record.start_identity.is_empty()
        || record.instance_token.len() != 64
        || !record
            .instance_token
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        || record.release_version.is_empty()
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
struct FileIdentity {
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
            Err(error) if final_component && error == rustix::io::Errno::NOENT => {
                rustix::fs::mkdirat(&directory, name, Mode::from_raw_mode(DIRECTORY_MODE))
                    .map_err(errno)?;
                directory = rustix::fs::openat(
                    &directory,
                    name,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(errno)?;
                rustix::fs::fchmod(&directory, Mode::from_raw_mode(DIRECTORY_MODE))
                    .map_err(errno)?;
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
fn inspect_process(pid: u32, _expected_executable: &Path) -> io::Result<Option<ProcessIdentity>> {
    if pid == 0 {
        return Ok(None);
    }
    let process = PathBuf::from("/proc").join(pid.to_string());
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
    let executable = match std::fs::read_link(process.join("exe")) {
        Ok(value) => value.canonicalize()?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    Ok(Some(ProcessIdentity {
        pid,
        executable,
        start_identity: format!("linux:{start}"),
    }))
}

#[cfg(not(target_os = "linux"))]
fn inspect_process(pid: u32, expected_executable: &Path) -> io::Result<Option<ProcessIdentity>> {
    inspect_process_with(pid, expected_executable, |field| ps_field(pid, field))
}

#[cfg(not(target_os = "linux"))]
fn inspect_process_with<F>(
    pid: u32,
    expected_executable: &Path,
    mut field: F,
) -> io::Result<Option<ProcessIdentity>>
where
    F: FnMut(&str) -> io::Result<Option<String>>,
{
    if pid == 0 {
        return Ok(None);
    }
    let Some(start_identity) = field("lstart=")? else {
        return Ok(None);
    };
    let Some(command) = field("command=")? else {
        return Ok(None);
    };
    let Some(rechecked_start) = field("lstart=")? else {
        return Ok(None);
    };
    if start_identity != rechecked_start {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "process identity changed during inspection",
        ));
    }
    require_expected_command(&command, expected_executable)?;
    Ok(Some(ProcessIdentity {
        pid,
        executable: expected_executable.to_owned(),
        start_identity,
    }))
}

#[cfg(not(target_os = "linux"))]
fn require_expected_command(command: &str, expected_executable: &Path) -> io::Result<()> {
    let expected = expected_executable.to_str().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "expected daemon executable path is not UTF-8",
        )
    })?;
    let exact_prefix = command.strip_prefix(expected).is_some_and(|suffix| {
        suffix.is_empty() || suffix.chars().next().is_some_and(char::is_whitespace)
    });
    let first_word_matches = command
        .split_whitespace()
        .next()
        .and_then(|candidate| Path::new(candidate).canonicalize().ok())
        .is_some_and(|candidate| candidate == expected_executable);
    if exact_prefix || first_word_matches {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "live process executable does not match daemon attestation",
        ))
    }
}

#[cfg(not(target_os = "linux"))]
fn ps_field(pid: u32, field: &str) -> io::Result<Option<String>> {
    use std::process::Stdio;
    let mut child = std::process::Command::new("/bin/ps")
        .args(["-ww", "-p", &pid.to_string(), "-o", field])
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

fn errno(error: rustix::io::Errno) -> io::Error {
    io::Error::from_raw_os_error(error.raw_os_error())
}

#[cfg(test)]
mod tests {
    use super::{
        DaemonInstanceRecord, DaemonPaths, InstanceTimestamp, OsProcessInspector, ProcessIdentity,
        ProcessInspector, ProcessSignaler, checked_pid, inspect_process_with,
        read_attested_instance, read_instance_record_with_hook, signal_attested_with,
    };
    use std::fs;
    use std::io;
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

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

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn attestation_process_snapshot_rejects_changed_start_identity() -> TestResult {
        let expected = Path::new("/trusted/gascand");
        let mut values = [
            Some("Mon Jul 28 12:00:00 2026".to_owned()),
            Some("/trusted/gascand --flag".to_owned()),
            Some("Mon Jul 28 12:00:01 2026".to_owned()),
        ]
        .into_iter();
        let result = inspect_process_with(7, expected, |_| {
            values
                .next()
                .ok_or_else(|| io::Error::other("unexpected process field request"))
        });
        assert!(result.is_err());
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn attestation_process_snapshot_handles_executable_paths_with_spaces() -> TestResult {
        let expected = Path::new("/trusted/path with spaces/gascand");
        let mut values = [
            Some("Mon Jul 28 12:00:00 2026".to_owned()),
            Some("/trusted/path with spaces/gascand --flag".to_owned()),
            Some("Mon Jul 28 12:00:00 2026".to_owned()),
        ]
        .into_iter();
        let identity = inspect_process_with(7, expected, |_| {
            values
                .next()
                .ok_or_else(|| io::Error::other("unexpected process field request"))
        })?
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
    fn attestation_rejects_pid_outside_platform_range() {
        assert!(checked_pid(u32::MAX).is_err());
    }
}
