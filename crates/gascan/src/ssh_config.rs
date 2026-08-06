use rustix::fd::OwnedFd;
use rustix::fs::{AtFlags, FileType, Mode, OFlags};
use std::cell::Cell;
use std::ffi::OsString;
use std::fs::File;
use std::io::{Read as _, Write as _};
use std::path::{Component, Path, PathBuf};
#[cfg(debug_assertions)]
use std::sync::OnceLock;

pub const INCLUDE_BLOCK_LF: &[u8] = concat!(
    "# >>> gascan managed ssh include >>>\n",
    "Include ~/.config/gascan/ssh/config\n",
    "# <<< gascan managed ssh include <<<\n",
)
.as_bytes();
const INCLUDE_BLOCK_CRLF: &[u8] = concat!(
    "# >>> gascan managed ssh include >>>\r\n",
    "Include ~/.config/gascan/ssh/config\r\n",
    "# <<< gascan managed ssh include <<<\r\n",
)
.as_bytes();
const OFFER_RECEIPT: &str = "include-offer-v1";
#[cfg(debug_assertions)]
static E2E_ACCOUNT_HOME: OnceLock<PathBuf> = OnceLock::new();
const USER_CONFIG: &str = "config";
const MAX_CONFIG_BYTES: u64 = 4 * 1024 * 1024;
const DIRECTORY_MODE: u16 = 0o700;
const FILE_MODE: rustix::fs::RawMode = 0o600;
type ManagedBlock = (usize, usize, &'static [u8]);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IncludeChange {
    Changed,
    Unchanged,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OfferAnswer {
    Installed,
    Declined,
}

#[derive(Debug)]
pub struct SshConfigError {
    kind: SshConfigErrorKind,
    message: &'static str,
    source: Option<std::io::Error>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SshConfigErrorKind {
    Unsafe,
    UpdateFailed,
}

impl SshConfigError {
    fn unsafe_path(message: &'static str) -> Self {
        Self {
            kind: SshConfigErrorKind::Unsafe,
            message,
            source: None,
        }
    }

    fn io(message: &'static str, source: impl Into<std::io::Error>) -> Self {
        Self {
            kind: SshConfigErrorKind::UpdateFailed,
            message,
            source: Some(source.into()),
        }
    }

    fn open_path(message: &'static str, source: rustix::io::Errno) -> Self {
        if matches!(
            source,
            rustix::io::Errno::ACCESS | rustix::io::Errno::LOOP | rustix::io::Errno::NOTDIR
        ) {
            Self::unsafe_path("SSH configuration path type is unsafe")
        } else {
            Self::io(message, source)
        }
    }

    #[must_use]
    pub const fn stable_code(&self) -> &'static str {
        match self.kind {
            SshConfigErrorKind::Unsafe => gascan_proto::error_code::SSH_CONFIG_UNSAFE,
            SshConfigErrorKind::UpdateFailed => gascan_proto::error_code::SSH_CONFIG_UPDATE_FAILED,
        }
    }
}

impl std::fmt::Display for SshConfigError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(source) = &self.source {
            write!(formatter, "{}: {source}", self.message)
        } else {
            formatter.write_str(self.message)
        }
    }
}

impl std::error::Error for SshConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|source| source as &(dyn std::error::Error + 'static))
    }
}

#[derive(Clone, Debug)]
pub struct SshConfig {
    home: PathBuf,
    ssh_directory: PathBuf,
    user_config: PathBuf,
    managed_config: PathBuf,
    expected_uid: u32,
}

impl SshConfig {
    pub fn for_user() -> Result<Self, SshConfigError> {
        #[cfg(debug_assertions)]
        if let Some(home) = E2E_ACCOUNT_HOME.get() {
            return Self::for_environment(None, Some(home));
        }
        let home = gascan_core::account::effective_account_home().map_err(|_| {
            SshConfigError::unsafe_path("effective account home is unavailable or unsafe")
        })?;
        Self::for_environment(None, Some(&home))
    }

    pub fn for_environment(
        xdg_config_home: Option<&Path>,
        home: Option<&Path>,
    ) -> Result<Self, SshConfigError> {
        let home = validated_absolute(
            home.ok_or_else(|| SshConfigError::unsafe_path("HOME is required for SSH setup"))?,
        )?;
        let managed_config = managed_config_path(xdg_config_home, Some(&home))?;
        let ssh_directory = home.join(".ssh");
        Ok(Self {
            user_config: ssh_directory.join(USER_CONFIG),
            ssh_directory,
            home,
            managed_config,
            expected_uid: rustix::process::geteuid().as_raw(),
        })
    }

    #[must_use]
    pub fn managed_config_path(&self) -> &Path {
        &self.managed_config
    }

    #[must_use]
    pub fn ssh_directory_path(&self) -> &Path {
        &self.ssh_directory
    }

    #[must_use]
    pub fn user_config_path(&self) -> &Path {
        &self.user_config
    }

    pub fn contains_include(&self) -> Result<bool, SshConfigError> {
        let Some(directory) = self.open_user_ssh(false)? else {
            return Ok(false);
        };
        let Some((contents, _)) = read_file(&directory, USER_CONFIG)? else {
            return Ok(false);
        };
        Ok(find_block(&contents)?.is_some())
    }

    pub fn install(&self) -> Result<IncludeChange, SshConfigError> {
        let directory = self
            .open_user_ssh(true)?
            .ok_or_else(|| SshConfigError::unsafe_path("cannot create ~/.ssh"))?;
        let (current, identity) = match read_file(&directory, USER_CONFIG)? {
            Some((current, identity)) => (current, Some(identity)),
            None => (Vec::new(), None),
        };
        if find_block(&current)?.is_some() {
            return Ok(IncludeChange::Unchanged);
        }
        reject_partial_markers(&current)?;
        let line_ending = if current.windows(2).any(|pair| pair == b"\r\n") {
            b"\r\n".as_slice()
        } else {
            b"\n".as_slice()
        };
        let block = if line_ending == b"\r\n" {
            INCLUDE_BLOCK_CRLF
        } else {
            INCLUDE_BLOCK_LF
        };
        let mut replacement = Vec::with_capacity(block.len() + current.len());
        replacement.extend_from_slice(block);
        replacement.extend_from_slice(&current);
        atomic_replace(
            &directory,
            USER_CONFIG,
            identity.map(|identity| PreviousFile {
                identity,
                contents: &current,
            }),
            &replacement,
        )?;
        Ok(IncludeChange::Changed)
    }

    pub fn remove(&self) -> Result<IncludeChange, SshConfigError> {
        let Some(directory) = self.open_user_ssh(false)? else {
            return Ok(IncludeChange::Unchanged);
        };
        let Some((current, identity)) = read_file(&directory, USER_CONFIG)? else {
            return Ok(IncludeChange::Unchanged);
        };
        let Some((start, end, _line_ending)) = find_block(&current)? else {
            reject_partial_markers(&current)?;
            return Ok(IncludeChange::Unchanged);
        };
        let mut replacement = Vec::with_capacity(current.len() - (end - start));
        replacement.extend_from_slice(&current[..start]);
        replacement.extend_from_slice(&current[end..]);
        atomic_replace(
            &directory,
            USER_CONFIG,
            Some(PreviousFile {
                identity,
                contents: &current,
            }),
            &replacement,
        )?;
        Ok(IncludeChange::Changed)
    }

    pub fn offer_receipt_exists(&self) -> Result<bool, SshConfigError> {
        let Some(directory) = self.open_managed_ssh(false)? else {
            return Ok(false);
        };
        Ok(read_file(&directory, OFFER_RECEIPT)?.is_some())
    }

    pub fn record_offer_receipt(&self) -> Result<(), SshConfigError> {
        let directory = self
            .open_managed_ssh(true)?
            .ok_or_else(|| SshConfigError::unsafe_path("cannot create managed SSH state"))?;
        let previous = read_file(&directory, OFFER_RECEIPT)?;
        atomic_replace(
            &directory,
            OFFER_RECEIPT,
            previous.as_ref().map(|(contents, identity)| PreviousFile {
                identity: *identity,
                contents,
            }),
            b"answered\n",
        )
    }

    fn open_user_ssh(&self, create: bool) -> Result<Option<SecureDirectory>, SshConfigError> {
        let home = open_directory(&self.home, self.expected_uid)?;
        open_child_directory(&home.fd, ".ssh", self.expected_uid, create)
    }

    fn open_managed_ssh(&self, create: bool) -> Result<Option<SecureDirectory>, SshConfigError> {
        let home = open_directory(&self.home, self.expected_uid)?;
        let Some(config_home) =
            open_child_directory(&home.fd, ".config", self.expected_uid, create)?
        else {
            return Ok(None);
        };
        let Some(gascan) =
            open_child_directory(&config_home.fd, "gascan", self.expected_uid, create)?
        else {
            return Ok(None);
        };
        open_child_directory(&gascan.fd, "ssh", self.expected_uid, create)
    }
}

#[doc(hidden)]
#[cfg(debug_assertions)]
pub fn configure_e2e_account_home(home: &Path) -> Result<(), SshConfigError> {
    let home = validated_absolute(home)?;
    E2E_ACCOUNT_HOME.set(home.clone()).map_err(|existing| {
        if existing == home {
            SshConfigError::unsafe_path("e2e account home was configured more than once")
        } else {
            SshConfigError::unsafe_path("e2e account home authority changed")
        }
    })
}

pub fn managed_config_path(
    _xdg_config_home: Option<&Path>,
    home: Option<&Path>,
) -> Result<PathBuf, SshConfigError> {
    let config_home = validated_absolute(
        home.ok_or_else(|| SshConfigError::unsafe_path("HOME is required for SSH setup"))?,
    )?
    .join(".config");
    if config_home.as_os_str().as_encoded_bytes().contains(&b'$') {
        return Err(SshConfigError::unsafe_path(
            "managed SSH path contains OpenSSH expansion",
        ));
    }
    Ok(config_home.join("gascan/ssh/config"))
}

pub fn first_use_offer(
    config: &SshConfig,
    stdin_is_tty: bool,
    stderr_is_tty: bool,
) -> Result<bool, SshConfigError> {
    if !stdin_is_tty || !stderr_is_tty {
        return Ok(false);
    }
    if config.contains_include()? || config.offer_receipt_exists()? {
        return Ok(false);
    }
    Ok(true)
}

pub fn answer_first_use_offer(
    config: &SshConfig,
    answer: &str,
) -> Result<OfferAnswer, SshConfigError> {
    let answer = if matches!(answer.trim().to_ascii_lowercase().as_str(), "n" | "no") {
        OfferAnswer::Declined
    } else {
        config.install()?;
        OfferAnswer::Installed
    };
    config.record_offer_receipt()?;
    Ok(answer)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileIdentity {
    device: u64,
    inode: u64,
    mode: rustix::fs::RawMode,
}

#[derive(Clone, Copy)]
struct PreviousFile<'a> {
    identity: FileIdentity,
    contents: &'a [u8],
}

impl FileIdentity {
    fn from_stat(stat: &rustix::fs::Stat) -> Self {
        Self {
            device: stat.st_dev as u64,
            inode: stat.st_ino,
            mode: stat.st_mode & 0o7777,
        }
    }
}

struct SecureDirectory {
    fd: OwnedFd,
    expected_uid: u32,
}

fn open_directory(path: &Path, expected_uid: u32) -> Result<SecureDirectory, SshConfigError> {
    let fd = rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| SshConfigError::open_path("open SSH directory", error))?;
    let stat = rustix::fs::fstat(&fd)
        .map_err(|error| SshConfigError::io("inspect SSH directory", error))?;
    validate_directory_stat(&stat, expected_uid)?;
    Ok(SecureDirectory { fd, expected_uid })
}

fn open_child_directory(
    parent: &OwnedFd,
    name: &str,
    expected_uid: u32,
    create: bool,
) -> Result<Option<SecureDirectory>, SshConfigError> {
    let fd = match rustix::fs::openat(
        parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(fd) => fd,
        Err(error) if error == rustix::io::Errno::NOENT && !create => return Ok(None),
        Err(error) if error == rustix::io::Errno::NOENT => {
            rustix::fs::mkdirat(parent, name, Mode::from_raw_mode(DIRECTORY_MODE))
                .map_err(|error| SshConfigError::io("create SSH directory", error))?;
            let fd = rustix::fs::openat(
                parent,
                name,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|error| SshConfigError::open_path("open new SSH directory", error))?;
            rustix::fs::fchmod(&fd, Mode::from_raw_mode(DIRECTORY_MODE))
                .map_err(|error| SshConfigError::io("secure new SSH directory", error))?;
            fd
        }
        Err(error) => return Err(SshConfigError::open_path("open SSH directory", error)),
    };
    let stat = rustix::fs::fstat(&fd)
        .map_err(|error| SshConfigError::io("inspect SSH directory", error))?;
    validate_directory_stat(&stat, expected_uid)?;
    Ok(Some(SecureDirectory { fd, expected_uid }))
}

fn validate_directory_stat(
    stat: &rustix::fs::Stat,
    expected_uid: u32,
) -> Result<(), SshConfigError> {
    if FileType::from_raw_mode(stat.st_mode) != FileType::Directory
        || stat.st_uid != expected_uid
        || stat.st_mode & 0o022 != 0
    {
        return Err(SshConfigError::unsafe_path(
            "SSH directory ownership or permissions are unsafe",
        ));
    }
    Ok(())
}

fn read_file(
    directory: &SecureDirectory,
    name: &str,
) -> Result<Option<(Vec<u8>, FileIdentity)>, SshConfigError> {
    let Some(expected) = file_identity(directory, name)? else {
        return Ok(None);
    };
    let fd = rustix::fs::openat(
        &directory.fd,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| SshConfigError::open_path("open SSH configuration", error))?;
    let stat = rustix::fs::fstat(&fd)
        .map_err(|error| SshConfigError::io("inspect open SSH configuration", error))?;
    validate_file_stat(&stat, directory.expected_uid)?;
    if FileIdentity::from_stat(&stat) != expected {
        return Err(SshConfigError::unsafe_path(
            "SSH configuration changed while opening it",
        ));
    }
    let mut contents = Vec::new();
    File::from(fd)
        .take(MAX_CONFIG_BYTES.saturating_add(1))
        .read_to_end(&mut contents)
        .map_err(|error| SshConfigError::io("read SSH configuration", error))?;
    if contents.len() as u64 > MAX_CONFIG_BYTES {
        return Err(SshConfigError::unsafe_path(
            "SSH configuration is too large",
        ));
    }
    if file_identity(directory, name)? != Some(expected) {
        return Err(SshConfigError::unsafe_path(
            "SSH configuration changed while reading it",
        ));
    }
    Ok(Some((contents, expected)))
}

fn file_identity(
    directory: &SecureDirectory,
    name: &str,
) -> Result<Option<FileIdentity>, SshConfigError> {
    let stat = match rustix::fs::statat(&directory.fd, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => stat,
        Err(error) if error == rustix::io::Errno::NOENT => return Ok(None),
        Err(error) => return Err(SshConfigError::io("inspect SSH configuration", error)),
    };
    validate_file_stat(&stat, directory.expected_uid)?;
    Ok(Some(FileIdentity::from_stat(&stat)))
}

fn validate_file_stat(stat: &rustix::fs::Stat, expected_uid: u32) -> Result<(), SshConfigError> {
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
        || stat.st_uid != expected_uid
        || stat.st_nlink != 1
        || stat.st_mode & 0o022 != 0
    {
        return Err(SshConfigError::unsafe_path(
            "SSH configuration ownership, type, links, or permissions are unsafe",
        ));
    }
    Ok(())
}

fn atomic_replace(
    directory: &SecureDirectory,
    target: &str,
    previous: Option<PreviousFile<'_>>,
    contents: &[u8],
) -> Result<(), SshConfigError> {
    atomic_replace_with_hook(directory, target, previous, contents, || Ok(()))
}

fn atomic_replace_with_hook<F>(
    directory: &SecureDirectory,
    target: &str,
    previous: Option<PreviousFile<'_>>,
    contents: &[u8],
    before_publish: F,
) -> Result<(), SshConfigError>
where
    F: FnOnce() -> Result<(), SshConfigError>,
{
    atomic_replace_with_hooks(
        directory,
        target,
        previous,
        contents,
        before_publish,
        || Ok(()),
    )
}

fn atomic_replace_with_hooks<F, G>(
    directory: &SecureDirectory,
    target: &str,
    previous: Option<PreviousFile<'_>>,
    contents: &[u8],
    before_publish: F,
    before_rollback: G,
) -> Result<(), SshConfigError>
where
    F: FnOnce() -> Result<(), SshConfigError>,
    G: FnOnce() -> Result<(), SshConfigError>,
{
    if contents.len() as u64 > MAX_CONFIG_BYTES {
        return Err(SshConfigError::unsafe_path(
            "SSH configuration is too large",
        ));
    }
    let replacement_mode = previous
        .map(|previous| previous.identity.mode)
        .unwrap_or(FILE_MODE);
    let staging = staging_name()?;
    let cleanup_staging = Cell::new(true);
    let fd = rustix::fs::openat(
        &directory.fd,
        &staging,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(replacement_mode),
    )
    .map_err(|error| SshConfigError::io("create SSH configuration staging file", error))?;
    let result = (|| {
        rustix::fs::fchmod(&fd, Mode::from_raw_mode(replacement_mode))
            .map_err(|error| SshConfigError::io("secure SSH configuration staging file", error))?;
        let stat = rustix::fs::fstat(&fd)
            .map_err(|error| SshConfigError::io("inspect SSH configuration staging file", error))?;
        validate_file_stat(&stat, directory.expected_uid)?;
        let staged_identity = FileIdentity::from_stat(&stat);
        if staged_identity.mode != replacement_mode {
            return Err(SshConfigError::unsafe_path(
                "SSH configuration staging permissions changed unexpectedly",
            ));
        }
        let mut file = File::from(fd);
        file.write_all(contents)
            .map_err(|error| SshConfigError::io("write SSH configuration", error))?;
        file.flush()
            .map_err(|error| SshConfigError::io("flush SSH configuration", error))?;
        file.sync_all()
            .map_err(|error| SshConfigError::io("sync SSH configuration", error))?;
        if file_identity(directory, target)? != previous.map(|previous| previous.identity) {
            return Err(SshConfigError::unsafe_path(
                "SSH configuration changed before replacement",
            ));
        }
        before_publish()?;
        match previous {
            Some(previous) => {
                rustix::fs::renameat_with(
                    &directory.fd,
                    &staging,
                    &directory.fd,
                    target,
                    rustix::fs::RenameFlags::EXCHANGE,
                )
                .map_err(|error| SshConfigError::io("exchange SSH configuration", error))?;
                cleanup_staging.set(false);
                let staging_name = staging.to_string_lossy();
                let observed = read_file(directory, &staging_name);
                let observed_identity = observed
                    .as_ref()
                    .ok()
                    .and_then(|observed| observed.as_ref().map(|(_bytes, identity)| *identity))
                    .or(entry_identity(directory, &staging)?);
                let preserved = observed.is_ok_and(|observed| {
                    observed.is_some_and(|(bytes, identity)| {
                        identity == previous.identity && bytes == previous.contents
                    })
                });
                if !preserved {
                    before_rollback()?;
                    if !entry_has_contents(directory, target, staged_identity, contents) {
                        if let Some(observed_identity) = observed_identity {
                            preserve_staging_for_recovery(
                                directory,
                                target,
                                &staging,
                                observed_identity,
                            )?;
                        }
                        return Err(SshConfigError::unsafe_path(
                            "SSH configuration changed before concurrent-update recovery",
                        ));
                    }
                    rustix::fs::renameat_with(
                        &directory.fd,
                        &staging,
                        &directory.fd,
                        target,
                        rustix::fs::RenameFlags::EXCHANGE,
                    )
                    .map_err(|error| {
                        SshConfigError::io("restore concurrently changed SSH configuration", error)
                    })?;
                    rustix::fs::fsync(&directory.fd).map_err(|error| {
                        SshConfigError::io("sync restored SSH directory", error)
                    })?;
                    if !entry_has_contents(directory, &staging_name, staged_identity, contents) {
                        let restored_staging = entry_identity(directory, &staging)?;
                        if let Some(restored_staging) = restored_staging {
                            preserve_staging_for_recovery(
                                directory,
                                target,
                                &staging,
                                restored_staging,
                            )?;
                        }
                        return Err(SshConfigError::unsafe_path(
                            "SSH configuration changed during concurrent-update recovery",
                        ));
                    }
                    cleanup_staging.set(true);
                    return Err(SshConfigError::unsafe_path(
                        "SSH configuration changed during replacement",
                    ));
                }
                if file_identity(directory, target)? != Some(staged_identity) {
                    return Err(SshConfigError::unsafe_path(
                        "SSH configuration changed after replacement",
                    ));
                }
                rustix::fs::unlinkat(&directory.fd, &staging, AtFlags::empty()).map_err(
                    |error| SshConfigError::io("remove previous SSH configuration", error),
                )?;
            }
            None => rustix::fs::renameat_with(
                &directory.fd,
                &staging,
                &directory.fd,
                target,
                rustix::fs::RenameFlags::NOREPLACE,
            )
            .map_err(|error| SshConfigError::io("install SSH configuration", error))?,
        }
        rustix::fs::fsync(&directory.fd)
            .map_err(|error| SshConfigError::io("sync SSH directory", error))?;
        file_identity(directory, target)?.ok_or_else(|| {
            SshConfigError::unsafe_path("SSH configuration disappeared after replacement")
        })?;
        Ok(())
    })();
    if result.is_err() && cleanup_staging.get() {
        let _ = rustix::fs::unlinkat(&directory.fd, &staging, AtFlags::empty());
    }
    result
}

fn entry_has_contents(
    directory: &SecureDirectory,
    name: &str,
    expected: FileIdentity,
    contents: &[u8],
) -> bool {
    read_file(directory, name).is_ok_and(|observed| {
        observed.is_some_and(|(bytes, identity)| identity == expected && bytes == contents)
    })
}

fn entry_identity(
    directory: &SecureDirectory,
    name: &std::ffi::OsStr,
) -> Result<Option<FileIdentity>, SshConfigError> {
    match rustix::fs::statat(&directory.fd, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => Ok(Some(FileIdentity::from_stat(&stat))),
        Err(error) if error == rustix::io::Errno::NOENT => Ok(None),
        Err(error) => Err(SshConfigError::io("inspect SSH recovery source", error)),
    }
}

fn preserve_staging_for_recovery(
    directory: &SecureDirectory,
    target: &str,
    staging: &std::ffi::OsStr,
    expected: FileIdentity,
) -> Result<(), SshConfigError> {
    if entry_identity(directory, staging)? != Some(expected) {
        return Err(SshConfigError::unsafe_path(
            "SSH recovery source changed before preservation",
        ));
    }
    let recovery = format!(
        ".gascan-recovery-{target}-{:016x}-{:016x}",
        expected.device, expected.inode
    );
    rustix::fs::renameat_with(
        &directory.fd,
        staging,
        &directory.fd,
        &recovery,
        rustix::fs::RenameFlags::NOREPLACE,
    )
    .map_err(|error| SshConfigError::io("preserve concurrent SSH configuration", error))?;
    if entry_identity(directory, std::ffi::OsStr::new(&recovery))? != Some(expected) {
        return Err(SshConfigError::unsafe_path(
            "preserved SSH recovery file changed unexpectedly",
        ));
    }
    rustix::fs::fsync(&directory.fd)
        .map_err(|error| SshConfigError::io("sync SSH recovery file", error))
}

fn staging_name() -> Result<OsString, SshConfigError> {
    let mut random = [0_u8; 12];
    getrandom::fill(&mut random).map_err(|error| {
        SshConfigError::io(
            "create SSH configuration staging name",
            std::io::Error::other(error),
        )
    })?;
    let suffix = gascan_core::hex::lower(&random);
    Ok(OsString::from(format!(".gascan-{suffix}")))
}

fn find_block(contents: &[u8]) -> Result<Option<ManagedBlock>, SshConfigError> {
    let mut found = None;
    for (block, line_ending) in [
        (INCLUDE_BLOCK_LF, b"\n".as_slice()),
        (INCLUDE_BLOCK_CRLF, b"\r\n".as_slice()),
    ] {
        for (start, _) in contents.windows(block.len()).enumerate() {
            let begins_line = start == 0 || contents.get(start.wrapping_sub(1)) == Some(&b'\n');
            if begins_line && &contents[start..start + block.len()] == block {
                if found.is_some() {
                    return Err(SshConfigError::unsafe_path(
                        "SSH configuration contains duplicate Gas Can include blocks",
                    ));
                }
                found = Some((start, start + block.len(), line_ending));
            }
        }
    }
    Ok(found)
}

fn reject_partial_markers(contents: &[u8]) -> Result<(), SshConfigError> {
    const OPEN: &[u8] = b"# >>> gascan managed ssh include >>>";
    const CLOSE: &[u8] = b"# <<< gascan managed ssh include <<<";
    if contents.split(|byte| *byte == b'\n').any(|line| {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        line == OPEN || line == CLOSE
    }) {
        return Err(SshConfigError::unsafe_path(
            "SSH configuration contains a malformed Gas Can include block",
        ));
    }
    Ok(())
}

fn validated_absolute(path: &Path) -> Result<PathBuf, SshConfigError> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::RootDir | Component::Normal(_)))
    {
        return Err(SshConfigError::unsafe_path(
            "SSH configuration path must be absolute and normalized",
        ));
    }
    Ok(path.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{
        INCLUDE_BLOCK_LF, PreviousFile, SshConfig, SshConfigError, USER_CONFIG,
        atomic_replace_with_hook, atomic_replace_with_hooks, read_file, validate_file_stat,
    };
    use std::fs;
    use std::os::unix::fs::PermissionsExt as _;

    #[test]
    fn file_validation_rejects_foreign_ownership() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::NamedTempFile::new()?;
        std::fs::set_permissions(
            temp.path(),
            <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o600),
        )?;
        let fd = rustix::fs::open(
            temp.path(),
            rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::NOFOLLOW,
            rustix::fs::Mode::empty(),
        )?;
        let stat = rustix::fs::fstat(fd)?;
        assert!(matches!(
            validate_file_stat(&stat, stat.st_uid.saturating_add(1)),
            Err(SshConfigError { .. })
        ));
        Ok(())
    }

    #[test]
    fn concurrent_replacement_is_restored_instead_of_being_overwritten()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let home = temp.path().join("home");
        fs::create_dir(&home)?;
        let config = SshConfig::for_environment(None, Some(&home))?;
        fs::create_dir(config.ssh_directory_path())?;
        fs::set_permissions(
            config.ssh_directory_path(),
            fs::Permissions::from_mode(0o700),
        )?;
        let original = b"Host original\n";
        fs::write(config.user_config_path(), original)?;
        fs::set_permissions(config.user_config_path(), fs::Permissions::from_mode(0o600))?;
        let directory = config
            .open_user_ssh(false)?
            .ok_or("test SSH directory missing")?;
        let (current, identity) =
            read_file(&directory, USER_CONFIG)?.ok_or("test config missing")?;
        let replacement = [INCLUDE_BLOCK_LF, current.as_slice()].concat();
        let concurrent = b"Host concurrent-editor\n";
        let concurrent_path = config.ssh_directory_path().join("concurrent");

        let result = atomic_replace_with_hook(
            &directory,
            USER_CONFIG,
            Some(PreviousFile {
                identity,
                contents: &current,
            }),
            &replacement,
            || {
                fs::write(&concurrent_path, concurrent)
                    .map_err(|error| SshConfigError::io("write concurrent replacement", error))?;
                fs::set_permissions(&concurrent_path, fs::Permissions::from_mode(0o600))
                    .map_err(|error| SshConfigError::io("secure concurrent replacement", error))?;
                fs::rename(&concurrent_path, config.user_config_path())
                    .map_err(|error| SshConfigError::io("publish concurrent replacement", error))
            },
        );

        assert!(result.is_err());
        assert_eq!(fs::read(config.user_config_path())?, concurrent);
        Ok(())
    }

    #[test]
    fn second_concurrent_replacement_survives_failed_rollback()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let home = temp.path().join("home");
        fs::create_dir(&home)?;
        let config = SshConfig::for_environment(None, Some(&home))?;
        fs::create_dir(config.ssh_directory_path())?;
        fs::set_permissions(
            config.ssh_directory_path(),
            fs::Permissions::from_mode(0o700),
        )?;
        let original = b"Host original\n";
        fs::write(config.user_config_path(), original)?;
        fs::set_permissions(config.user_config_path(), fs::Permissions::from_mode(0o600))?;
        let directory = config
            .open_user_ssh(false)?
            .ok_or("test SSH directory missing")?;
        let (current, identity) =
            read_file(&directory, USER_CONFIG)?.ok_or("test config missing")?;
        let replacement = [INCLUDE_BLOCK_LF, current.as_slice()].concat();
        let first_editor = b"Host first-editor\n";
        let second_editor = b"Host second-editor\n";
        let first_path = config.ssh_directory_path().join("first-editor");
        let second_path = config.ssh_directory_path().join("second-editor");

        let result = atomic_replace_with_hooks(
            &directory,
            USER_CONFIG,
            Some(PreviousFile {
                identity,
                contents: &current,
            }),
            &replacement,
            || {
                fs::write(&first_path, first_editor)
                    .map_err(|error| SshConfigError::io("write first replacement", error))?;
                fs::set_permissions(&first_path, fs::Permissions::from_mode(0o600))
                    .map_err(|error| SshConfigError::io("secure first replacement", error))?;
                fs::rename(&first_path, config.user_config_path())
                    .map_err(|error| SshConfigError::io("publish first replacement", error))
            },
            || {
                fs::write(&second_path, second_editor)
                    .map_err(|error| SshConfigError::io("write second replacement", error))?;
                fs::set_permissions(&second_path, fs::Permissions::from_mode(0o600))
                    .map_err(|error| SshConfigError::io("secure second replacement", error))?;
                fs::rename(&second_path, config.user_config_path())
                    .map_err(|error| SshConfigError::io("publish second replacement", error))
            },
        );

        assert!(result.is_err());
        assert_eq!(fs::read(config.user_config_path())?, second_editor);
        let recoveries = fs::read_dir(config.ssh_directory_path())?
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".gascan-recovery-config-")
            })
            .collect::<Vec<_>>();
        assert_eq!(recoveries.len(), 1);
        assert_eq!(fs::read(recoveries[0].path())?, first_editor);
        Ok(())
    }

    #[test]
    fn permission_denied_while_opening_a_path_is_unsafe() {
        let error = SshConfigError::open_path("open SSH directory", rustix::io::Errno::ACCESS);
        assert_eq!(
            error.stable_code(),
            gascan_proto::error_code::SSH_CONFIG_UNSAFE
        );
    }
}
