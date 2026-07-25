mod config;
mod identity;

use camino::{Utf8Path, Utf8PathBuf};
use rustix::fd::OwnedFd;
use rustix::fs::{AtFlags, FileType, FlockOperation, Mode, OFlags};
use rustix::process::geteuid;
use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, Read};
use std::net::IpAddr;
#[cfg(target_os = "linux")]
use std::os::fd::AsRawFd;
use std::path::{Component, Path, PathBuf};

pub use config::{publish_openssh_files, readiness_ssh_args};
pub use identity::{HostIdentity, ensure_host_identity};

const DIRECTORY_MODE: u16 = 0o700;
pub(crate) const PRIVATE_MODE: u16 = 0o600;
pub(crate) const PUBLIC_MODE: u16 = 0o644;
const MAX_MANAGED_FILE_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActiveSsh {
    pub host: IpAddr,
    pub port: u16,
    pub alias: String,
    pub host_key_fingerprint: String,
    pub client_key_fingerprint: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManagedSshHost {
    pub active: ActiveSsh,
    pub host_public_key: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SshPaths {
    config_home: Utf8PathBuf,
    gascan_directory: Utf8PathBuf,
    directory: Utf8PathBuf,
    private_key: Utf8PathBuf,
    public_key: Utf8PathBuf,
    known_hosts: Utf8PathBuf,
    config: Utf8PathBuf,
    expected_uid: u32,
}

impl SshPaths {
    pub fn for_user() -> Result<Self, SshError> {
        let xdg = std::env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty());
        let home = std::env::var_os("HOME");
        Self::for_environment(xdg.as_deref(), home.as_deref())
    }

    pub fn for_environment(
        xdg_config_home: Option<&OsStr>,
        home: Option<&OsStr>,
    ) -> Result<Self, SshError> {
        Self::from_environment_with_uid(xdg_config_home, home, geteuid().as_raw())
    }

    fn from_environment_with_uid(
        xdg_config_home: Option<&OsStr>,
        home: Option<&OsStr>,
        expected_uid: u32,
    ) -> Result<Self, SshError> {
        let config_home = if let Some(config_home) = xdg_config_home {
            PathBuf::from(config_home)
        } else {
            PathBuf::from(home.ok_or(SshError::InvalidState(
                "HOME is required when XDG_CONFIG_HOME is unset",
            ))?)
            .join(".config")
        };
        if !config_home.is_absolute() {
            return Err(SshError::InvalidState(
                "SSH configuration home must be absolute",
            ));
        }
        let config_home = utf8_path(config_home)?;
        let gascan_directory = config_home.join("gascan");
        let directory = gascan_directory.join("ssh");
        Ok(Self {
            private_key: directory.join("identity_ed25519"),
            public_key: directory.join("identity_ed25519.pub"),
            known_hosts: directory.join("known_hosts"),
            config: directory.join("config"),
            config_home,
            gascan_directory,
            directory,
            expected_uid,
        })
    }

    #[must_use]
    pub fn gascan_directory(&self) -> &Utf8Path {
        &self.gascan_directory
    }

    #[must_use]
    pub fn directory(&self) -> &Utf8Path {
        &self.directory
    }

    #[must_use]
    pub fn private_key(&self) -> &Utf8Path {
        &self.private_key
    }

    #[must_use]
    pub fn public_key(&self) -> &Utf8Path {
        &self.public_key
    }

    #[must_use]
    pub fn known_hosts(&self) -> &Utf8Path {
        &self.known_hosts
    }

    #[must_use]
    pub fn config(&self) -> &Utf8Path {
        &self.config
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SshError {
    #[error("{0}")]
    InvalidState(&'static str),
    #[error("{action}: {source}")]
    Io {
        action: &'static str,
        #[source]
        source: io::Error,
    },
    #[error("ssh-keygen exceeded its execution bound")]
    KeygenTimeout,
    #[error("ssh-keygen produced invalid or excessive output")]
    KeygenOutput,
    #[error("ssh-keygen rejected the managed key")]
    KeygenRejected,
}

impl SshError {
    pub(crate) fn io(action: &'static str, source: impl Into<io::Error>) -> Self {
        Self::Io {
            action,
            source: source.into(),
        }
    }
}

pub(crate) struct StateDirectory {
    fd: OwnedFd,
    expected_uid: u32,
}

impl StateDirectory {
    pub(crate) fn open(paths: &SshPaths) -> Result<Self, SshError> {
        let config_home = open_config_home(paths.config_home.as_std_path(), paths.expected_uid)?;
        let gascan = open_managed_directory(&config_home, "gascan", paths.expected_uid)?;
        let fd = open_managed_directory(&gascan, "ssh", paths.expected_uid)?;
        rustix::fs::flock(&fd, FlockOperation::LockExclusive)
            .map_err(|error| SshError::io("lock managed SSH directory", error))?;
        Ok(Self {
            fd,
            expected_uid: paths.expected_uid,
        })
    }

    pub(crate) fn metadata(
        &self,
        name: &str,
        required_mode: u16,
    ) -> Result<Option<FileIdentity>, SshError> {
        let stat = match rustix::fs::statat(&self.fd, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) => stat,
            Err(error) if error == rustix::io::Errno::NOENT => return Ok(None),
            Err(error) => return Err(SshError::io("inspect managed SSH file", error)),
        };
        validate_regular_stat(&stat, self.expected_uid, required_mode)?;
        Ok(Some(FileIdentity::from_stat(&stat)))
    }

    pub(crate) fn open_file(
        &self,
        name: &str,
        required_mode: u16,
    ) -> Result<(File, FileIdentity), SshError> {
        let fd = rustix::fs::openat(
            &self.fd,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| SshError::io("open managed SSH file", error))?;
        let stat = rustix::fs::fstat(&fd)
            .map_err(|error| SshError::io("inspect open managed SSH file", error))?;
        validate_regular_stat(&stat, self.expected_uid, required_mode)?;
        Ok((File::from(fd), FileIdentity::from_stat(&stat)))
    }

    pub(crate) fn read_file(
        &self,
        name: &str,
        required_mode: u16,
        maximum: u64,
    ) -> Result<(Vec<u8>, FileIdentity), SshError> {
        let (file, identity) = self.open_file(name, required_mode)?;
        let mut bytes = Vec::new();
        file.take(maximum.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|error| SshError::io("read managed SSH file", error))?;
        if bytes.len() as u64 > maximum {
            return Err(SshError::InvalidState("managed SSH file is too large"));
        }
        Ok((bytes, identity))
    }

    pub(crate) fn create_staging(&self, name: &str, mode: u16) -> Result<File, SshError> {
        let fd = rustix::fs::openat(
            &self.fd,
            name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(mode),
        )
        .map_err(|error| SshError::io("create managed SSH staging file", error))?;
        rustix::fs::fchmod(&fd, Mode::from_raw_mode(mode))
            .map_err(|error| SshError::io("set managed SSH staging mode", error))?;
        let stat = rustix::fs::fstat(&fd)
            .map_err(|error| SshError::io("inspect managed SSH staging file", error))?;
        validate_regular_stat(&stat, self.expected_uid, mode)?;
        Ok(File::from(fd))
    }

    pub(crate) fn harden_staging(&self, name: &str, mode: u16) -> Result<(), SshError> {
        let fd = rustix::fs::openat(
            &self.fd,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| SshError::io("open generated SSH staging file", error))?;
        let stat = rustix::fs::fstat(&fd)
            .map_err(|error| SshError::io("inspect generated SSH staging file", error))?;
        validate_regular_identity_stat(&stat, self.expected_uid)?;
        rustix::fs::fchmod(&fd, Mode::from_raw_mode(mode))
            .map_err(|error| SshError::io("set generated SSH staging mode", error))?;
        let stat = rustix::fs::fstat(&fd)
            .map_err(|error| SshError::io("reinspect generated SSH staging file", error))?;
        validate_regular_stat(&stat, self.expected_uid, mode)
    }

    pub(crate) fn rename_new(&self, source: &str, target: &str) -> Result<(), SshError> {
        rustix::fs::renameat_with(
            &self.fd,
            source,
            &self.fd,
            target,
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .map_err(|error| SshError::io("publish new managed SSH file", error))
    }

    pub(crate) fn rename_replace(&self, source: &str, target: &str) -> Result<(), SshError> {
        rustix::fs::renameat(&self.fd, source, &self.fd, target)
            .map_err(|error| SshError::io("replace managed SSH file", error))
    }

    pub(crate) fn rename_back(&self, source: &str, target: &str) -> Result<(), SshError> {
        rustix::fs::renameat_with(
            &self.fd,
            source,
            &self.fd,
            target,
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .map_err(|error| SshError::io("roll back managed SSH publication", error))
    }

    pub(crate) fn remove(&self, name: &str) {
        let _ = rustix::fs::unlinkat(&self.fd, name, AtFlags::empty());
    }

    pub(crate) fn sync(&self) -> Result<(), SshError> {
        rustix::fs::fsync(&self.fd)
            .map_err(|error| SshError::io("sync managed SSH directory", error))
    }

    pub(crate) fn resolved_path(&self, name: &str) -> Result<PathBuf, SshError> {
        #[cfg(target_os = "linux")]
        {
            Ok(PathBuf::from("/proc")
                .join(std::process::id().to_string())
                .join("fd")
                .join(self.fd.as_raw_fd().to_string())
                .join(name))
        }
        #[cfg(not(target_os = "linux"))]
        {
            let path = rustix::fs::getpath(&self.fd)
                .map_err(|error| SshError::io("resolve managed SSH directory", error))?;
            let path =
                PathBuf::from(path.into_string().map_err(|_| {
                    SshError::InvalidState("managed SSH directory path is not UTF-8")
                })?);
            Ok(path.join(name))
        }
    }

    pub(crate) fn resolved_open_file(&self, file: &File) -> Result<PathBuf, SshError> {
        #[cfg(target_os = "linux")]
        {
            use std::os::fd::AsRawFd as _;
            Ok(PathBuf::from("/proc")
                .join(std::process::id().to_string())
                .join("fd")
                .join(file.as_raw_fd().to_string()))
        }
        #[cfg(not(target_os = "linux"))]
        {
            let path = rustix::fs::getpath(file)
                .map_err(|error| SshError::io("resolve managed SSH file", error))?;
            Ok(PathBuf::from(path.into_string().map_err(|_| {
                SshError::InvalidState("managed SSH file path is not UTF-8")
            })?))
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FileIdentity {
    device: u64,
    inode: u64,
}

impl FileIdentity {
    fn from_stat(stat: &rustix::fs::Stat) -> Self {
        Self {
            device: stat.st_dev as u64,
            inode: stat.st_ino,
        }
    }
}

pub(crate) fn random_staging_name() -> Result<String, SshError> {
    use base64::Engine as _;
    let mut bytes = [0_u8; 12];
    getrandom::fill(&mut bytes).map_err(|error| {
        SshError::io("create managed SSH staging name", io::Error::other(error))
    })?;
    Ok(format!(
        ".{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    ))
}

pub(crate) fn validate_regular_stat(
    stat: &rustix::fs::Stat,
    expected_uid: u32,
    required_mode: u16,
) -> Result<(), SshError> {
    validate_regular_identity_stat(stat, expected_uid)?;
    if stat.st_mode & 0o7777 != required_mode {
        return Err(SshError::InvalidState("managed SSH file mode is unsafe"));
    }
    Ok(())
}

fn validate_regular_identity_stat(
    stat: &rustix::fs::Stat,
    expected_uid: u32,
) -> Result<(), SshError> {
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
        return Err(SshError::InvalidState(
            "managed SSH path is not a regular file",
        ));
    }
    if stat.st_uid != expected_uid {
        return Err(SshError::InvalidState(
            "managed SSH file has foreign ownership",
        ));
    }
    if stat.st_nlink != 1 {
        return Err(SshError::InvalidState(
            "managed SSH file has multiple hard links",
        ));
    }
    Ok(())
}

pub(crate) fn maximum_managed_file_bytes() -> u64 {
    MAX_MANAGED_FILE_BYTES
}

fn open_config_home(path: &Path, expected_uid: u32) -> Result<OwnedFd, SshError> {
    if !path.is_absolute() {
        return Err(SshError::InvalidState(
            "SSH configuration home must be absolute",
        ));
    }
    let mut components = path.components().peekable();
    if components.next() != Some(Component::RootDir) {
        return Err(SshError::InvalidState(
            "SSH configuration home must be absolute",
        ));
    }
    let mut directory = rustix::fs::open(
        "/",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| SshError::io("open filesystem root", error))?;
    let mut created = false;
    while let Some(component) = components.next() {
        let Component::Normal(name) = component else {
            return Err(SshError::InvalidState(
                "SSH configuration path contains a non-normal component",
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
                    .map_err(|error| SshError::io("create SSH configuration home", error))?;
                directory = rustix::fs::openat(
                    &directory,
                    name,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(|error| SshError::io("open new SSH configuration home", error))?;
                rustix::fs::fchmod(&directory, Mode::from_raw_mode(DIRECTORY_MODE))
                    .map_err(|error| SshError::io("secure SSH configuration home", error))?;
                created = true;
            }
            Err(error) => return Err(SshError::io("open SSH configuration ancestor", error)),
        }
        validate_path_ancestor(&directory)?;
    }
    let stat = rustix::fs::fstat(&directory)
        .map_err(|error| SshError::io("inspect SSH configuration home", error))?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::Directory
        || stat.st_uid != expected_uid
        || (!created && stat.st_mode & 0o022 != 0)
        || (created && stat.st_mode & 0o7777 != DIRECTORY_MODE)
    {
        return Err(SshError::InvalidState(
            "SSH configuration home ownership or mode is unsafe",
        ));
    }
    Ok(directory)
}

fn validate_path_ancestor(directory: &OwnedFd) -> Result<(), SshError> {
    let stat = rustix::fs::fstat(directory)
        .map_err(|error| SshError::io("inspect SSH configuration ancestor", error))?;
    let mode = stat.st_mode & 0o7777;
    if FileType::from_raw_mode(stat.st_mode) != FileType::Directory
        || (mode & 0o022 != 0 && mode & 0o1000 == 0)
    {
        return Err(SshError::InvalidState(
            "SSH configuration ancestor mode is unsafe",
        ));
    }
    Ok(())
}

fn open_managed_directory(
    parent: &OwnedFd,
    name: &str,
    expected_uid: u32,
) -> Result<OwnedFd, SshError> {
    let directory = match rustix::fs::openat(
        parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(directory) => directory,
        Err(error) if error == rustix::io::Errno::NOENT => {
            rustix::fs::mkdirat(parent, name, Mode::from_raw_mode(DIRECTORY_MODE))
                .map_err(|error| SshError::io("create managed SSH directory", error))?;
            let directory = rustix::fs::openat(
                parent,
                name,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|error| SshError::io("open new managed SSH directory", error))?;
            rustix::fs::fchmod(&directory, Mode::from_raw_mode(DIRECTORY_MODE))
                .map_err(|error| SshError::io("secure managed SSH directory", error))?;
            directory
        }
        Err(error) => return Err(SshError::io("open managed SSH directory", error)),
    };
    let stat = rustix::fs::fstat(&directory)
        .map_err(|error| SshError::io("inspect managed SSH directory", error))?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::Directory
        || stat.st_uid != expected_uid
        || stat.st_mode & 0o7777 != DIRECTORY_MODE
    {
        return Err(SshError::InvalidState(
            "managed SSH directory ownership or mode is unsafe",
        ));
    }
    Ok(directory)
}

fn utf8_path(path: PathBuf) -> Result<Utf8PathBuf, SshError> {
    Utf8PathBuf::from_path_buf(path)
        .map_err(|_| SshError::InvalidState("managed SSH path is not valid UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::{PUBLIC_MODE, SshError, StateDirectory};
    use rustix::fs::{Mode, OFlags};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn regular_file_validation_rejects_a_foreign_owner() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
        fs::write(temp.path().join("candidate"), b"public")?;
        fs::set_permissions(
            temp.path().join("candidate"),
            fs::Permissions::from_mode(0o644),
        )?;
        let fd = rustix::fs::open(
            temp.path(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        let directory = StateDirectory {
            fd,
            expected_uid: rustix::process::geteuid().as_raw().wrapping_add(1),
        };
        assert!(matches!(
            directory.metadata("candidate", PUBLIC_MODE),
            Err(SshError::InvalidState(
                "managed SSH file has foreign ownership"
            ))
        ));
        Ok(())
    }
}
