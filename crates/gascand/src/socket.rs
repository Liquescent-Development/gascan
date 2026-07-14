use rustix::process::geteuid;
use std::fs::{self, Metadata, Permissions};
use std::io;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

const DIRECTORY_MODE: u32 = 0o700;
const SOCKET_MODE: u32 = 0o600;

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
        let socket = directory.join("gascand.sock");
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
        ensure_private_directory(&self.directory)
            .map_err(|error| contextual("prepare runtime directory", error))?;
        prepare_socket_path(&self.socket)
            .map_err(|error| contextual("prepare socket path", error))?;
        let listener = UnixListener::bind(&self.socket)
            .map_err(|error| contextual("bind Unix socket", error))?;
        if let Err(error) = fs::set_permissions(&self.socket, Permissions::from_mode(SOCKET_MODE)) {
            let _ = fs::remove_file(&self.socket);
            return Err(error);
        }
        let identity = Identity::from_metadata(&fs::symlink_metadata(&self.socket)?);
        Ok(OwnedSocket {
            listener,
            path: self.socket.clone(),
            identity,
        })
    }
}

fn contextual(action: &str, error: io::Error) -> io::Error {
    io::Error::new(error.kind(), format!("{action}: {error}"))
}

#[derive(Debug)]
pub struct OwnedSocket {
    listener: UnixListener,
    path: PathBuf,
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
        &self.path
    }
}

impl Drop for OwnedSocket {
    fn drop(&mut self) {
        let _ = remove_identity(&self.path, self.identity, "cleanup");
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Identity {
    device: u64,
    inode: u64,
    uid: u32,
}
impl Identity {
    fn from_metadata(metadata: &Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            uid: metadata.uid(),
        }
    }
}

fn ensure_private_directory(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => validate_directory(&metadata),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir(path)?;
            fs::set_permissions(path, Permissions::from_mode(DIRECTORY_MODE))?;
            validate_directory(&fs::symlink_metadata(path)?)
        }
        Err(error) => Err(error),
    }
}

fn validate_directory(metadata: &Metadata) -> io::Result<()> {
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "runtime directory is not a real directory",
        ));
    }
    if metadata.uid() != geteuid().as_raw()
        || metadata.permissions().mode() & 0o777 != DIRECTORY_MODE
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "runtime directory ownership or mode is unsafe",
        ));
    }
    Ok(())
}

fn prepare_socket_path(path: &Path) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if !metadata.file_type().is_socket() || metadata.uid() != geteuid().as_raw() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "socket path is not an owned socket",
        ));
    }
    if UnixStream::connect(path).is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            "daemon socket is live",
        ));
    }
    let before = Identity::from_metadata(&metadata);
    remove_identity(path, before, "stale")
}

fn remove_identity(path: &Path, expected: Identity, purpose: &str) -> io::Result<()> {
    let quarantine =
        path.with_extension(format!("{purpose}-{}-{}", expected.device, expected.inode));
    if fs::symlink_metadata(&quarantine).is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "socket quarantine path already exists",
        ));
    }
    fs::rename(path, &quarantine)?;
    let moved = fs::symlink_metadata(&quarantine)?;
    if moved.file_type().is_socket() && Identity::from_metadata(&moved) == expected {
        fs::remove_file(quarantine)
    } else {
        if fs::symlink_metadata(path).is_err() {
            let _ = fs::rename(&quarantine, path);
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "socket path changed during cleanup",
        ))
    }
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
