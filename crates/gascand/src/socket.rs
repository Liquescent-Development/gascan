use base64::Engine as _;
use rustix::fd::OwnedFd;
use rustix::fs::{AtFlags, FileType, Mode, OFlags};
use rustix::process::geteuid;
use std::ffi::OsStr;
use std::io::{self, Write as _};
#[cfg(target_os = "linux")]
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const DIRECTORY_MODE: u16 = 0o700;
const SOCKET_MODE: u16 = 0o600;
const SOCKET_NAME: &str = "gascand.sock";
const INSTANCE_NAME: &str = "daemon-instance.json";
const LIFECYCLE_LOCK_NAME: &str = "daemon-lifecycle.lock";
static QUARANTINE_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SocketPaths {
    directory: PathBuf,
    socket: PathBuf,
    instance: PathBuf,
    lifecycle_lock: PathBuf,
}

impl SocketPaths {
    pub fn for_user() -> io::Result<Self> {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR");
        Self::for_user_with_uid_and_environment(geteuid().as_raw(), runtime.as_deref())
    }
    pub fn for_user_with_uid_and_environment(
        uid: u32,
        runtime: Option<&OsStr>,
    ) -> io::Result<Self> {
        let directory = runtime.map_or_else(
            || default_runtime_base().join(format!("gascan-{uid}")),
            |root| PathBuf::from(root).join("gascan"),
        );
        Ok(Self::from_runtime_root(directory))
    }
    #[must_use]
    pub fn from_runtime_root(directory: PathBuf) -> Self {
        let socket = directory.join(SOCKET_NAME);
        let instance = directory.join(INSTANCE_NAME);
        let lifecycle_lock = directory.join(LIFECYCLE_LOCK_NAME);
        Self {
            directory,
            socket,
            instance,
            lifecycle_lock,
        }
    }
    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }
    #[must_use]
    pub fn socket(&self) -> &Path {
        &self.socket
    }
    #[must_use]
    pub fn instance(&self) -> &Path {
        &self.instance
    }
    #[must_use]
    pub fn lifecycle_lock(&self) -> &Path {
        &self.lifecycle_lock
    }

    pub fn bind(&self) -> io::Result<OwnedSocket> {
        let directory = open_private_directory(&self.directory)?;
        prepare_socket(&directory)?;
        let (listener, staging, staging_identity) = bind_staging(&directory)?;
        let mut staging_guard = StagingGuard::new(&directory, &staging, staging_identity);
        rustix::fs::chmodat(
            &directory,
            staging.as_str(),
            Mode::from_raw_mode(SOCKET_MODE),
            AtFlags::SYMLINK_NOFOLLOW,
        )
        .map_err(errno)?;
        rustix::fs::renameat_with(
            &directory,
            staging.as_str(),
            &directory,
            SOCKET_NAME,
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .map_err(errno)?;
        staging_guard.disarm();
        drop(staging_guard);
        let identity = identity_at(&directory, SOCKET_NAME)?;
        Ok(OwnedSocket {
            listener,
            directory,
            display_path: self.socket.clone(),
            identity,
        })
    }

    pub fn prepare_directory(&self) -> io::Result<()> {
        open_private_directory(&self.directory).map(drop)
    }
}

#[cfg(target_os = "macos")]
fn default_runtime_base() -> PathBuf {
    PathBuf::from("/private/tmp")
}

#[cfg(not(target_os = "macos"))]
fn default_runtime_base() -> PathBuf {
    PathBuf::from("/tmp")
}

struct StagingGuard<'a> {
    directory: &'a OwnedFd,
    name: &'a str,
    identity: Identity,
    armed: bool,
}
impl<'a> StagingGuard<'a> {
    const fn new(directory: &'a OwnedFd, name: &'a str, identity: Identity) -> Self {
        Self {
            directory,
            name,
            identity,
            armed: true,
        }
    }
    fn disarm(&mut self) {
        self.armed = false;
    }
}
impl Drop for StagingGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            let _ =
                remove_named_identity(self.directory, self.name, self.identity, "rejected-bind");
        }
    }
}

#[derive(Debug)]
pub struct OwnedSocket {
    listener: UnixListener,
    directory: OwnedFd,
    display_path: PathBuf,
    identity: Identity,
}
impl OwnedSocket {
    pub fn try_clone(&self) -> io::Result<UnixListener> {
        self.listener.try_clone()
    }
    pub fn set_nonblocking(&self, value: bool) -> io::Result<()> {
        self.listener.set_nonblocking(value)
    }
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.display_path
    }
}
impl Drop for OwnedSocket {
    fn drop(&mut self) {
        let _ = remove_identity(&self.directory, self.identity, "cleanup");
    }
}

#[derive(Debug)]
pub(crate) struct OwnedInstanceRecord {
    directory: OwnedFd,
    name: std::ffi::OsString,
    identity: Identity,
}

impl Drop for OwnedInstanceRecord {
    fn drop(&mut self) {
        let _ = remove_regular_identity(&self.directory, &self.name, self.identity);
    }
}

pub(crate) fn write_instance_record(
    path: &Path,
    contents: &[u8],
) -> io::Result<OwnedInstanceRecord> {
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
    let directory = open_private_directory(parent)?;
    match rustix::fs::statat(&directory, name, AtFlags::SYMLINK_NOFOLLOW) {
        Err(error) if error == rustix::io::Errno::NOENT => {}
        Ok(stat) => {
            validate_regular_file(&stat)?;
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "daemon instance record already exists",
            ));
        }
        Err(error) => return Err(errno(error)),
    }
    let staging = random_name("instance")?;
    let fd = rustix::fs::openat(
        &directory,
        staging.as_str(),
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(SOCKET_MODE),
    )
    .map_err(errno)?;
    let stat = rustix::fs::fstat(&fd).map_err(errno)?;
    let identity = Identity {
        device: stat.st_dev as u64,
        inode: stat.st_ino,
        uid: stat.st_uid,
    };
    let mut staging_guard = RegularStagingGuard::new(&directory, &staging, identity);
    rustix::fs::fchmod(&fd, Mode::from_raw_mode(SOCKET_MODE)).map_err(errno)?;
    let stat = rustix::fs::fstat(&fd).map_err(errno)?;
    validate_regular_file(&stat)?;
    let mut file = std::fs::File::from(fd);
    file.write_all(contents)?;
    file.sync_all()?;
    rustix::fs::renameat_with(
        &directory,
        staging.as_str(),
        &directory,
        name,
        rustix::fs::RenameFlags::NOREPLACE,
    )
    .map_err(errno)?;
    staging_guard.disarm();
    drop(staging_guard);
    let published = OwnedInstanceRecord {
        directory,
        name: name.to_owned(),
        identity,
    };
    if identity_at(&published.directory, &published.name)? != identity {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "daemon instance record changed while publishing it",
        ));
    }
    Ok(published)
}

struct RegularStagingGuard<'a> {
    directory: &'a OwnedFd,
    name: &'a str,
    identity: Identity,
    armed: bool,
}

impl RegularStagingGuard<'_> {
    fn new<'a>(
        directory: &'a OwnedFd,
        name: &'a str,
        identity: Identity,
    ) -> RegularStagingGuard<'a> {
        RegularStagingGuard {
            directory,
            name,
            identity,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for RegularStagingGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            let _ = remove_regular_identity(self.directory, OsStr::new(self.name), self.identity);
        }
    }
}

fn validate_regular_file(stat: &rustix::fs::Stat) -> io::Result<()> {
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
        || stat.st_uid != geteuid().as_raw()
        || stat.st_nlink != 1
        || Mode::from_raw_mode(stat.st_mode).bits() & 0o777 != SOCKET_MODE
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "daemon instance record ownership, type, links, or mode is unsafe",
        ));
    }
    Ok(())
}

fn remove_regular_identity(
    directory: &OwnedFd,
    source: &OsStr,
    expected: Identity,
) -> io::Result<()> {
    remove_regular_identity_with_hook(directory, source, expected, || Ok(()))
}

fn remove_regular_identity_with_hook<F>(
    directory: &OwnedFd,
    source: &OsStr,
    expected: Identity,
    before_restore: F,
) -> io::Result<()>
where
    F: FnOnce() -> io::Result<()>,
{
    let quarantine = loop {
        let sequence = QUARANTINE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let candidate = format!(
            ".instance-cleanup-{}-{}-{sequence}",
            std::process::id(),
            expected.inode
        );
        match rustix::fs::renameat_with(
            directory,
            source,
            directory,
            candidate.as_str(),
            rustix::fs::RenameFlags::NOREPLACE,
        ) {
            Ok(()) => break candidate,
            Err(error) if error == rustix::io::Errno::EXIST => continue,
            Err(error) if error == rustix::io::Errno::NOENT => return Ok(()),
            Err(error) => return Err(errno(error)),
        }
    };
    let moved = identity_at(directory, &quarantine)?;
    let stat = rustix::fs::statat(directory, quarantine.as_str(), AtFlags::SYMLINK_NOFOLLOW)
        .map_err(errno)?;
    if moved == expected && FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile {
        rustix::fs::unlinkat(directory, quarantine.as_str(), AtFlags::empty()).map_err(errno)
    } else {
        before_restore()?;
        let _ = rustix::fs::renameat_with(
            directory,
            quarantine.as_str(),
            directory,
            source,
            rustix::fs::RenameFlags::NOREPLACE,
        );
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "daemon instance record changed during cleanup",
        ))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Identity {
    device: u64,
    inode: u64,
    uid: u32,
}

fn open_private_directory(path: &Path) -> io::Result<OwnedFd> {
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
    if stat.st_uid != geteuid().as_raw()
        || Mode::from_raw_mode(stat.st_mode).bits() & 0o777 != DIRECTORY_MODE
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "runtime directory ownership or mode is unsafe",
        ));
    }
    Ok(directory)
}

fn prepare_socket(directory: &OwnedFd) -> io::Result<()> {
    if UnixStream::connect(resolved_path(directory, SOCKET_NAME)?).is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            "daemon socket is live",
        ));
    }
    let identity = match identity_at(directory, SOCKET_NAME) {
        Ok(value) => value,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let stat =
        rustix::fs::statat(directory, SOCKET_NAME, AtFlags::SYMLINK_NOFOLLOW).map_err(errno)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::Socket
        || identity.uid != geteuid().as_raw()
    {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "socket path is not an owned socket",
        ));
    }
    if identity_at(directory, SOCKET_NAME)? != identity {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "socket changed during liveness check",
        ));
    }
    remove_identity(directory, identity, "stale")
}

fn remove_identity(directory: &OwnedFd, expected: Identity, purpose: &str) -> io::Result<()> {
    remove_named_identity(directory, SOCKET_NAME, expected, purpose)
}

fn remove_named_identity(
    directory: &OwnedFd,
    source: &str,
    expected: Identity,
    purpose: &str,
) -> io::Result<()> {
    let quarantine = loop {
        let sequence = QUARANTINE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let candidate = format!(
            ".{purpose}-{}-{}-{sequence}",
            std::process::id(),
            expected.inode
        );
        match rustix::fs::renameat_with(
            directory,
            source,
            directory,
            candidate.as_str(),
            rustix::fs::RenameFlags::NOREPLACE,
        ) {
            Ok(()) => break candidate,
            Err(error) if error == rustix::io::Errno::EXIST => continue,
            Err(error) => return Err(errno(error)),
        }
    };
    let moved = identity_at(directory, &quarantine)?;
    let stat = rustix::fs::statat(directory, quarantine.as_str(), AtFlags::SYMLINK_NOFOLLOW)
        .map_err(errno)?;
    if moved == expected && FileType::from_raw_mode(stat.st_mode) == FileType::Socket {
        rustix::fs::unlinkat(directory, quarantine.as_str(), AtFlags::empty()).map_err(errno)
    } else {
        if identity_at(directory, source).is_err() {
            let _ = rustix::fs::renameat(directory, quarantine.as_str(), directory, source);
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "socket changed during cleanup",
        ))
    }
}

fn identity_at<P: rustix::path::Arg>(directory: &OwnedFd, name: P) -> io::Result<Identity> {
    let stat = rustix::fs::statat(directory, name, AtFlags::SYMLINK_NOFOLLOW).map_err(errno)?;
    Ok(Identity {
        device: stat.st_dev as u64,
        inode: stat.st_ino,
        uid: stat.st_uid,
    })
}

fn bind_staging(directory: &OwnedFd) -> io::Result<(UnixListener, String, Identity)> {
    bind_staging_with(directory, |_, _| Ok(()))
}

fn bind_staging_with<F>(
    directory: &OwnedFd,
    mut before_bind: F,
) -> io::Result<(UnixListener, String, Identity)>
where
    F: FnMut(&Path, &str) -> io::Result<()>,
{
    for _ in 0..64 {
        let staging = random_name("bind")?;
        let path = resolved_path(directory, &staging)?;
        before_bind(&path, &staging)?;
        match UnixListener::bind(&path) {
            Ok(listener) => {
                let identity = match identity_at(directory, &staging) {
                    Ok(identity) => identity,
                    Err(_) => {
                        let metadata = std::fs::symlink_metadata(&path)?;
                        if !metadata.file_type().is_socket() || metadata.uid() != geteuid().as_raw()
                        {
                            return Err(io::Error::new(
                                io::ErrorKind::PermissionDenied,
                                "escaped staging identity is invalid",
                            ));
                        }
                        let expected = Identity {
                            device: metadata.dev(),
                            inode: metadata.ino(),
                            uid: metadata.uid(),
                        };
                        drop(listener);
                        cleanup_escaped_staging(&path, &staging, expected)?;
                        continue;
                    }
                };
                let stat =
                    rustix::fs::statat(directory, staging.as_str(), AtFlags::SYMLINK_NOFOLLOW)
                        .map_err(errno)?;
                if identity.uid != geteuid().as_raw()
                    || FileType::from_raw_mode(stat.st_mode) != FileType::Socket
                {
                    drop(listener);
                    remove_named_identity(directory, &staging, identity, "rejected-bind")?;
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "staging socket identity is invalid",
                    ));
                }
                return Ok((listener, staging, identity));
            }
            Err(error)
                if error.kind() == io::ErrorKind::AddrInUse
                    || error.kind() == io::ErrorKind::AlreadyExists =>
            {
                continue;
            }
            Err(error) => return Err(contextual("bind Unix socket staging path", error)),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::WouldBlock,
        "socket directory changed repeatedly during bind",
    ))
}

fn cleanup_escaped_staging(path: &Path, name: &str, expected: Identity) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "staging path has no parent"))?;
    let directory = open_private_directory(parent).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("escaped staging cleanup parent could not be retained: {error}"),
        )
    })?;
    let identity = identity_at(&directory, name).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("escaped staging identity could not be proven: {error}"),
        )
    })?;
    let stat = rustix::fs::statat(&directory, name, AtFlags::SYMLINK_NOFOLLOW).map_err(errno)?;
    if identity != expected
        || identity.uid != geteuid().as_raw()
        || FileType::from_raw_mode(stat.st_mode) != FileType::Socket
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "escaped staging is not the daemon user's socket",
        ));
    }
    remove_named_identity(&directory, name, expected, "escaped-bind")
}

fn random_name(purpose: &str) -> io::Result<String> {
    let mut bytes = [0_u8; 7];
    getrandom::fill(&mut bytes).map_err(io::Error::other)?;
    let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    let _ = purpose;
    Ok(format!(".{token}"))
}

fn resolved_path(directory: &OwnedFd, name: &str) -> io::Result<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        return Ok(PathBuf::from("/proc/self/fd")
            .join(directory.as_raw_fd().to_string())
            .join(name));
    }
    #[cfg(not(target_os = "linux"))]
    {
        let path = rustix::fs::getpath(directory).map_err(errno)?;
        let path = PathBuf::from(path.into_string().map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "socket directory path is not UTF-8",
            )
        })?);
        Ok(path.join(name))
    }
}

fn errno(error: rustix::io::Errno) -> io::Error {
    io::Error::from_raw_os_error(error.raw_os_error())
}
fn contextual(action: &str, error: io::Error) -> io::Error {
    io::Error::new(error.kind(), format!("{action}: {error}"))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeerUid(u32);
impl PeerUid {
    #[must_use]
    pub const fn new(value: u32) -> Self {
        Self(value)
    }
    #[must_use]
    pub fn current() -> Self {
        Self(geteuid().as_raw())
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeerUidMismatch;
pub const fn validate_peer_uid(peer: PeerUid, expected: PeerUid) -> Result<(), PeerUidMismatch> {
    if peer.0 == expected.0 {
        Ok(())
    } else {
        Err(PeerUidMismatch)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        SOCKET_NAME, StagingGuard, bind_staging, bind_staging_with, open_private_directory,
        resolved_path,
    };
    use std::fs;
    use std::os::unix::fs::PermissionsExt as _;
    use std::os::unix::net::UnixListener;

    #[test]
    fn publish_collision_drops_exact_staging_socket() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().canonicalize()?.join("runtime");
        let directory = open_private_directory(&root)?;
        let (listener, staging, identity) = bind_staging(&directory)?;
        let guard = StagingGuard::new(&directory, &staging, identity);
        let collision = UnixListener::bind(resolved_path(&directory, SOCKET_NAME)?)?;
        let result = rustix::fs::renameat_with(
            &directory,
            staging.as_str(),
            &directory,
            SOCKET_NAME,
            rustix::fs::RenameFlags::NOREPLACE,
        );
        assert!(result.is_err());
        drop(guard);
        drop(listener);
        drop(collision);
        assert!(fs::read_dir(root)?.all(|entry| {
            entry.is_ok_and(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
        }));
        Ok(())
    }

    #[test]
    fn instance_record_is_atomic_private_and_identity_guarded()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().canonicalize()?.join("runtime");
        let path = root.join("daemon-instance.json");
        let record = super::write_instance_record(&path, br#"{"pid":7}"#)?;
        let metadata = fs::symlink_metadata(&path)?;
        assert!(metadata.file_type().is_file());
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
        assert_eq!(fs::read(&path)?, br#"{"pid":7}"#);

        fs::remove_file(&path)?;
        fs::write(&path, b"replacement")?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        drop(record);
        assert_eq!(fs::read(path)?, b"replacement");
        Ok(())
    }

    #[test]
    fn instance_record_refuses_unsafe_existing_destination()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().canonicalize()?.join("runtime");
        fs::create_dir(&root)?;
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
        let path = root.join("daemon-instance.json");
        fs::write(&path, b"foreign")?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644))?;
        assert!(super::write_instance_record(&path, b"new").is_err());
        assert_eq!(fs::read(path)?, b"foreign");
        Ok(())
    }

    #[test]
    fn instance_staging_cleanup_preserves_replacement() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().canonicalize()?.join("runtime");
        let directory = super::open_private_directory(&root)?;
        let name = ".staging";
        fs::write(root.join(name), b"original")?;
        fs::set_permissions(root.join(name), fs::Permissions::from_mode(0o600))?;
        let identity = super::identity_at(&directory, name)?;
        let guard = super::RegularStagingGuard::new(&directory, name, identity);
        fs::remove_file(root.join(name))?;
        fs::write(root.join(name), b"replacement")?;
        fs::set_permissions(root.join(name), fs::Permissions::from_mode(0o600))?;
        drop(guard);
        assert_eq!(fs::read(root.join(name))?, b"replacement");
        Ok(())
    }

    #[test]
    fn instance_cleanup_never_clobbers_concurrent_successor()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().canonicalize()?.join("runtime");
        let directory = super::open_private_directory(&root)?;
        let name = "daemon-instance.json";
        fs::write(root.join(name), b"original")?;
        fs::set_permissions(root.join(name), fs::Permissions::from_mode(0o600))?;
        let original = super::identity_at(&directory, name)?;
        fs::remove_file(root.join(name))?;
        fs::write(root.join(name), b"replacement")?;
        fs::set_permissions(root.join(name), fs::Permissions::from_mode(0o600))?;

        let result = super::remove_regular_identity_with_hook(
            &directory,
            std::ffi::OsStr::new(name),
            original,
            || {
                fs::write(root.join(name), b"successor")?;
                fs::set_permissions(root.join(name), fs::Permissions::from_mode(0o600))
            },
        );
        assert!(result.is_err());
        assert_eq!(fs::read(root.join(name))?, b"successor");
        Ok(())
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn darwin_resolve_bind_swap_cleans_escaped_stage_and_retains_foreign_node()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir()?;
        let base = temp.path().canonicalize()?;
        let runtime = base.join("runtime");
        let displaced = base.join("displaced");
        let directory = open_private_directory(&runtime)?;
        let mut attempts = 0_u8;
        let (listener, staging, identity) = bind_staging_with(&directory, |_, _| {
            attempts = attempts.saturating_add(1);
            if attempts == 1 {
                fs::rename(&runtime, &displaced)?;
                fs::create_dir(&runtime)?;
                fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700))?;
                fs::write(runtime.join("foreign"), b"retain")?;
            }
            Ok(())
        })?;
        assert!(attempts >= 2, "escaped first staging bind was not rejected");
        assert_eq!(fs::read(runtime.join("foreign"))?, b"retain");
        assert!(fs::read_dir(&runtime)?.all(|entry| {
            entry.is_ok_and(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
        }));
        let guard = StagingGuard::new(&directory, &staging, identity);
        drop(guard);
        drop(listener);
        assert!(fs::read_dir(displaced)?.all(|entry| {
            entry.is_ok_and(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
        }));
        Ok(())
    }
}
