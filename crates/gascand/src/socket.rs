use rustix::fd::OwnedFd;
use rustix::fs::{AtFlags, FileType, Mode, OFlags};
use rustix::process::geteuid;
use std::io;
#[cfg(target_os = "linux")]
use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const DIRECTORY_MODE: u16 = 0o700;
const SOCKET_MODE: u16 = 0o600;
const SOCKET_NAME: &str = "gascand.sock";
static QUARANTINE_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SocketPaths {
    directory: PathBuf,
    socket: PathBuf,
}

impl SocketPaths {
    pub fn for_user() -> io::Result<Self> {
        let root = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(format!("/tmp/gascan-{}", geteuid().as_raw())));
        Ok(Self::from_runtime_root(root.join("gascan")))
    }
    #[must_use]
    pub fn from_runtime_root(directory: PathBuf) -> Self {
        let socket = directory.join(SOCKET_NAME);
        Self { directory, socket }
    }
    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }
    #[must_use]
    pub fn socket(&self) -> &Path {
        &self.socket
    }

    pub fn bind(&self) -> io::Result<OwnedSocket> {
        let directory = open_private_directory(&self.directory)?;
        prepare_socket(&directory)?;
        let (listener, staging) = bind_staging(&directory)?;
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
    if UnixStream::connect(resolved_path(directory, SOCKET_NAME)?).is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            "daemon socket is live",
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
    let quarantine = loop {
        let sequence = QUARANTINE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let candidate = format!(
            ".{purpose}-{}-{}-{sequence}",
            std::process::id(),
            expected.inode
        );
        match rustix::fs::renameat_with(
            directory,
            SOCKET_NAME,
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
        if identity_at(directory, SOCKET_NAME).is_err() {
            let _ = rustix::fs::renameat(directory, quarantine.as_str(), directory, SOCKET_NAME);
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "socket changed during cleanup",
        ))
    }
}

fn identity_at(directory: &OwnedFd, name: &str) -> io::Result<Identity> {
    let stat = rustix::fs::statat(directory, name, AtFlags::SYMLINK_NOFOLLOW).map_err(errno)?;
    Ok(Identity {
        device: stat.st_dev as u64,
        inode: stat.st_ino,
        uid: stat.st_uid,
    })
}

fn bind_staging(directory: &OwnedFd) -> io::Result<(UnixListener, String)> {
    for _ in 0..64 {
        let sequence = QUARANTINE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let staging = format!(".bind-{}-{sequence}.sock", std::process::id());
        let path = resolved_path(directory, &staging)?;
        match UnixListener::bind(path) {
            Ok(listener) => match identity_at(directory, &staging) {
                Ok(_) => return Ok((listener, staging)),
                Err(_) => drop(listener),
            },
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
