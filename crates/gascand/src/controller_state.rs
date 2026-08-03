use crate::{Store, StoreError};
use rustix::fd::OwnedFd;
use rustix::fs::{AtFlags, FileType, Mode, OFlags};
use rustix::process::geteuid;
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};
use thiserror::Error;

const APPLICATION_ID: &str = "dev.gascan";
const CONTROLLER_DIRECTORY: &str = "controller";
const DATABASE_NAME: &str = "state.sqlite3";
const DIRECTORY_MODE: u32 = 0o700;
const FILE_MODE: u32 = 0o600;

#[derive(Debug, Error)]
pub enum ControllerStateError {
    #[error("controller state path is invalid: {0}")]
    Invalid(String),
    #[error("controller state path is unsafe: {0}")]
    Unsafe(String),
    #[error("controller state store could not be opened: {0}")]
    Store(#[from] StoreError),
}

impl ControllerStateError {
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Invalid(_) => "controller_state_invalid",
            Self::Unsafe(_) => "controller_state_unsafe",
            Self::Store(_) => "controller_state_migration_failed",
        }
    }
}

#[derive(Debug)]
pub struct ControllerStatePaths {
    durable_database: PathBuf,
    legacy_database: PathBuf,
    expected_uid: u32,
}

impl ControllerStatePaths {
    pub fn for_user(runtime_directory: &Path) -> Result<Self, ControllerStateError> {
        let home = gascan_core::account::effective_account_home().map_err(|error| {
            ControllerStateError::Invalid(format!("effective account home is unavailable: {error}"))
        })?;
        Self::for_home_and_runtime(&home, runtime_directory, geteuid().as_raw())
    }

    pub fn for_home_and_runtime(
        home: &Path,
        runtime_directory: &Path,
        expected_uid: u32,
    ) -> Result<Self, ControllerStateError> {
        validate_absolute_normal_path(home, "account home")?;
        validate_absolute_normal_path(runtime_directory, "runtime directory")?;
        Ok(Self {
            durable_database: home
                .join("Library")
                .join("Application Support")
                .join(APPLICATION_ID)
                .join(CONTROLLER_DIRECTORY)
                .join(DATABASE_NAME),
            legacy_database: runtime_directory.join(DATABASE_NAME),
            expected_uid,
        })
    }

    #[must_use]
    pub fn durable_database(&self) -> &Path {
        &self.durable_database
    }

    #[must_use]
    pub fn legacy_database(&self) -> &Path {
        &self.legacy_database
    }
}

pub fn open_controller_store(paths: &ControllerStatePaths) -> Result<Store, ControllerStateError> {
    let controller_directory = open_controller_directory(paths)?;
    open_private_database(&controller_directory, paths.expected_uid)?;
    Store::open(paths.durable_database()).map_err(ControllerStateError::from)
}

fn validate_absolute_normal_path(path: &Path, label: &str) -> Result<(), ControllerStateError> {
    if path.to_str().is_none() {
        return Err(ControllerStateError::Invalid(format!(
            "{label} is not valid UTF-8"
        )));
    }
    let mut components = path.components();
    if components.next() != Some(Component::RootDir) {
        return Err(ControllerStateError::Invalid(format!(
            "{label} must be absolute"
        )));
    }
    if components.next().is_none()
        || path
            .components()
            .skip(1)
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(ControllerStateError::Invalid(format!(
            "{label} must contain only normal components"
        )));
    }
    Ok(())
}

fn open_controller_directory(
    paths: &ControllerStatePaths,
) -> Result<OwnedFd, ControllerStateError> {
    let home = paths.durable_database.ancestors().nth(5).ok_or_else(|| {
        ControllerStateError::Invalid("durable database has no account home".to_owned())
    })?;
    let mut home_directory = open_existing_directory(home, "account home")?;
    validate_directory(&home_directory, paths.expected_uid, false, "account home")?;

    home_directory =
        open_existing_child_directory(&home_directory, OsStr::new("Library"), "Library")?;
    validate_directory(&home_directory, paths.expected_uid, false, "Library")?;
    home_directory = open_existing_child_directory(
        &home_directory,
        OsStr::new("Application Support"),
        "Application Support",
    )?;
    validate_directory(
        &home_directory,
        paths.expected_uid,
        false,
        "Application Support",
    )?;

    let application_directory = ensure_private_child_directory(
        &home_directory,
        APPLICATION_ID,
        paths.expected_uid,
        "application directory",
    )?;
    ensure_private_child_directory(
        &application_directory,
        CONTROLLER_DIRECTORY,
        paths.expected_uid,
        "controller directory",
    )
}

fn open_existing_directory(path: &Path, label: &str) -> Result<OwnedFd, ControllerStateError> {
    let mut components = path.components();
    if components.next() != Some(Component::RootDir) {
        return Err(ControllerStateError::Invalid(format!(
            "{label} must be absolute"
        )));
    }
    let mut directory = rustix::fs::open(
        "/",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| unsafe_error("root directory", error))?;
    for component in components {
        let Component::Normal(name) = component else {
            return Err(ControllerStateError::Invalid(format!(
                "{label} contains a non-normal component"
            )));
        };
        directory = open_existing_child_directory(&directory, name, label)?;
    }
    Ok(directory)
}

fn open_existing_child_directory(
    parent: &OwnedFd,
    name: &OsStr,
    label: &str,
) -> Result<OwnedFd, ControllerStateError> {
    rustix::fs::openat(
        parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| unsafe_error(label, error))
}

fn ensure_private_child_directory(
    parent: &OwnedFd,
    name: &str,
    expected_uid: u32,
    label: &str,
) -> Result<OwnedFd, ControllerStateError> {
    let exists = match rustix::fs::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(_) => true,
        Err(error) if error == rustix::io::Errno::NOENT => false,
        Err(error) => return Err(unsafe_error(label, error)),
    };
    let (directory, created) = if exists {
        (
            open_existing_child_directory(parent, OsStr::new(name), label)?,
            false,
        )
    } else {
        let directory =
            match rustix::fs::mkdirat(parent, name, Mode::from_raw_mode(DIRECTORY_MODE as u16)) {
                Ok(()) => open_existing_child_directory(parent, OsStr::new(name), label)?,
                Err(error) if error == rustix::io::Errno::EXIST => {
                    return ensure_private_child_directory(parent, name, expected_uid, label);
                }
                Err(error) => return Err(unsafe_error(label, error)),
            };
        (directory, true)
    };
    if created {
        rustix::fs::fchmod(&directory, Mode::from_raw_mode(DIRECTORY_MODE as u16))
            .map_err(|error| unsafe_error(label, error))?;
    }
    validate_directory(&directory, expected_uid, true, label)?;
    Ok(directory)
}

fn open_private_database(
    directory: &OwnedFd,
    expected_uid: u32,
) -> Result<(), ControllerStateError> {
    let created = match rustix::fs::statat(directory, DATABASE_NAME, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(_) => false,
        Err(error) if error == rustix::io::Errno::NOENT => true,
        Err(error) => return Err(unsafe_error("controller database", error)),
    };
    let (database, created) = if created {
        match rustix::fs::openat(
            directory,
            DATABASE_NAME,
            OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(FILE_MODE as u16),
        ) {
            Ok(database) => (database, true),
            Err(error) if error == rustix::io::Errno::EXIST => {
                (open_existing_database(directory)?, false)
            }
            Err(error) => return Err(unsafe_error("controller database", error)),
        }
    } else {
        (open_existing_database(directory)?, false)
    };
    if created {
        rustix::fs::fchmod(&database, Mode::from_raw_mode(FILE_MODE as u16))
            .map_err(|error| unsafe_error("controller database", error))?;
    }
    validate_database(&database, expected_uid)
}

fn open_existing_database(directory: &OwnedFd) -> Result<OwnedFd, ControllerStateError> {
    rustix::fs::openat(
        directory,
        DATABASE_NAME,
        OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| unsafe_error("controller database", error))
}

fn validate_directory(
    directory: &OwnedFd,
    expected_uid: u32,
    private_mode: bool,
    label: &str,
) -> Result<(), ControllerStateError> {
    let stat = rustix::fs::fstat(directory).map_err(|error| unsafe_error(label, error))?;
    let type_is_directory = FileType::from_raw_mode(stat.st_mode) == FileType::Directory;
    let mode_is_safe = !private_mode
        || u32::from(Mode::from_raw_mode(stat.st_mode).bits() & 0o777) == DIRECTORY_MODE;
    if !type_is_directory || stat.st_uid != expected_uid || !mode_is_safe {
        return Err(ControllerStateError::Unsafe(format!(
            "{label} ownership, type, or mode is unsafe"
        )));
    }
    Ok(())
}

fn validate_database(database: &OwnedFd, expected_uid: u32) -> Result<(), ControllerStateError> {
    let stat =
        rustix::fs::fstat(database).map_err(|error| unsafe_error("controller database", error))?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
        || stat.st_uid != expected_uid
        || stat.st_nlink != 1
        || u32::from(Mode::from_raw_mode(stat.st_mode).bits() & 0o777) != FILE_MODE
    {
        return Err(ControllerStateError::Unsafe(
            "controller database ownership, type, links, or mode is unsafe".to_owned(),
        ));
    }
    Ok(())
}

fn unsafe_error(context: &str, error: rustix::io::Errno) -> ControllerStateError {
    ControllerStateError::Unsafe(format!("{context}: {error}"))
}
