use crate::{Store, StoreError};
#[cfg(target_os = "macos")]
use nix::sys::event::{EvFlags, EventFilter, FilterFlag, KEvent, Kqueue};
use rusqlite::{Connection, OpenFlags, backup::Backup};
use rustix::fd::OwnedFd;
use rustix::fs::{AtFlags, FileType, Mode, OFlags};
use rustix::process::geteuid;
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs::File;
use std::io::Write;
#[cfg(target_os = "macos")]
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::FileExt;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;
use thiserror::Error;

const APPLICATION_ID: &str = "dev.gascan";
const CONTROLLER_DIRECTORY: &str = "controller";
const DATABASE_NAME: &str = "state.sqlite3";
const DIRECTORY_MODE: u32 = 0o700;
const FILE_MODE: u32 = 0o600;
const SQLITE_SIDECAR_SUFFIXES: [&str; 3] = ["-wal", "-shm", "-journal"];
const TEMP_PREFIX: &str = ".state.sqlite3.migration-";
const LEGACY_BACKUP_NAME: &str = "state.sqlite3.legacy-backup";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MigrationFault {
    BeforeSnapshotComplete,
    BeforeDurableRename,
    AfterDurableRename,
    DuringLegacyArchive,
}

#[derive(Debug, Error)]
pub enum ControllerStateError {
    #[error("controller state path is invalid: {0}")]
    Invalid(String),
    #[error("controller state path is unsafe: {0}")]
    Unsafe(String),
    #[error(
        "Gascan found conflicting controller databases and will not choose one automatically.\n\nDurable: {durable}\nLegacy: {legacy}\n\nNo data was changed. Back up both files, then select the database to preserve."
    )]
    Conflict { durable: PathBuf, legacy: PathBuf },
    #[error("controller state migration failed: {0}")]
    Migration(String),
    #[error("controller state store could not be opened: {0}")]
    Store(#[from] StoreError),
}

impl ControllerStateError {
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Invalid(_) => "controller_state_invalid",
            Self::Unsafe(_) => "controller_state_unsafe",
            Self::Conflict { .. } => "controller_state_conflict",
            Self::Migration(_) | Self::Store(_) => "controller_state_migration_failed",
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
    open_controller_store_with_optional_fault(paths, None)
}

pub fn open_controller_store_with_fault(
    paths: &ControllerStatePaths,
    fault: MigrationFault,
) -> Result<Store, ControllerStateError> {
    open_controller_store_with_optional_fault(paths, Some(fault))
}

fn open_controller_store_with_optional_fault(
    paths: &ControllerStatePaths,
    fault: Option<MigrationFault>,
) -> Result<Store, ControllerStateError> {
    let controller = open_controller_directory(paths)?;
    cleanup_migration_temps(&controller, paths.expected_uid)?;
    let durable_exists = private_regular_file_exists(
        &controller.descriptor,
        DATABASE_NAME,
        paths.expected_uid,
        "durable controller database",
    )?;
    let legacy = open_legacy_state(paths)?;

    match (durable_exists, legacy) {
        (false, None) => {
            if open_legacy_orphans(paths)?.is_some() {
                return Err(ControllerStateError::Unsafe(
                    "legacy SQLite sidecars exist without either active database".to_owned(),
                ));
            }
            open_controller_store_with_hooks(paths, || Ok(()), || Ok(()))
        }
        (true, None) => {
            if let Some(orphans) = open_legacy_orphans(paths)? {
                archive_legacy_orphans(paths, &controller, &orphans)?;
            }
            open_existing_controller_store(paths)
        }
        (false, Some(legacy)) => migrate_legacy_store(paths, &controller, &legacy, fault),
        (true, Some(legacy)) => resolve_dual_store(paths, &controller, &legacy, fault),
    }
}

struct LegacyState {
    directories: Vec<OwnedFd>,
    database: PrivateDatabase,
    sidecars: BTreeMap<String, PrivateDatabase>,
}

struct LegacyOrphans {
    directories: Vec<OwnedFd>,
    sidecars: BTreeMap<String, PrivateDatabase>,
}

fn open_legacy_state(
    paths: &ControllerStatePaths,
) -> Result<Option<LegacyState>, ControllerStateError> {
    let parent = paths.legacy_database().parent().ok_or_else(|| {
        ControllerStateError::Invalid("legacy database has no parent directory".to_owned())
    })?;
    let Some(directories) = open_existing_directory_chain(parent)? else {
        return Ok(None);
    };
    let directory = directories.last().ok_or_else(|| {
        ControllerStateError::Invalid("legacy database has no parent descriptor".to_owned())
    })?;
    validate_directory(
        directory,
        paths.expected_uid,
        true,
        "legacy runtime directory",
    )?;
    if !private_regular_file_exists(
        directory,
        DATABASE_NAME,
        paths.expected_uid,
        "legacy controller database",
    )? {
        return Ok(None);
    }
    let database = open_named_private_database(
        directory,
        DATABASE_NAME,
        paths.expected_uid,
        false,
        "legacy controller database",
    )?;
    let sidecars = open_private_sidecars(directory, DATABASE_NAME, paths.expected_uid)?;
    Ok(Some(LegacyState {
        directories,
        database,
        sidecars,
    }))
}

fn open_legacy_orphans(
    paths: &ControllerStatePaths,
) -> Result<Option<LegacyOrphans>, ControllerStateError> {
    let parent = paths.legacy_database().parent().ok_or_else(|| {
        ControllerStateError::Invalid("legacy database has no parent directory".to_owned())
    })?;
    let Some(directories) = open_existing_directory_chain(parent)? else {
        return Ok(None);
    };
    let directory = directories.last().ok_or_else(|| {
        ControllerStateError::Invalid("legacy database has no parent descriptor".to_owned())
    })?;
    validate_directory(
        directory,
        paths.expected_uid,
        true,
        "legacy runtime directory",
    )?;
    if entry_exists(directory, DATABASE_NAME)? {
        return Ok(None);
    }
    let sidecars = open_private_sidecars(directory, DATABASE_NAME, paths.expected_uid)?;
    if sidecars.is_empty() {
        Ok(None)
    } else {
        Ok(Some(LegacyOrphans {
            directories,
            sidecars,
        }))
    }
}

fn migrate_legacy_store(
    paths: &ControllerStatePaths,
    controller: &ControllerDirectory,
    legacy: &LegacyState,
    fault: Option<MigrationFault>,
) -> Result<Store, ControllerStateError> {
    let monitor = DatabaseMutationMonitor::new_for_legacy(legacy)?;
    let snapshot_name =
        make_snapshot(paths, controller, &legacy.database, &legacy.sidecars, fault)?;
    monitor.ensure_unchanged().map_err(|error| {
        ControllerStateError::Unsafe(format!(
            "legacy database changed while creating its snapshot: {error}"
        ))
    })?;
    validate_legacy_binding(paths, legacy)?;
    if fault == Some(MigrationFault::BeforeDurableRename) {
        return Err(injected_fault(MigrationFault::BeforeDurableRename));
    }
    if entry_exists(&controller.descriptor, DATABASE_NAME)? {
        return Err(ControllerStateError::Conflict {
            durable: paths.durable_database().to_path_buf(),
            legacy: paths.legacy_database().to_path_buf(),
        });
    }
    rustix::fs::renameat(
        &controller.descriptor,
        snapshot_name.as_str(),
        &controller.descriptor,
        DATABASE_NAME,
    )
    .map_err(|error| migration_fs_error("publishing the durable database", error))?;
    rustix::fs::fsync(&controller.descriptor)
        .map_err(|error| migration_fs_error("syncing the durable controller directory", error))?;
    if fault == Some(MigrationFault::AfterDurableRename) {
        return Err(injected_fault(MigrationFault::AfterDurableRename));
    }
    monitor.ensure_unchanged().map_err(|error| {
        ControllerStateError::Unsafe(format!("legacy database changed before archival: {error}"))
    })?;
    validate_legacy_binding(paths, legacy)?;
    archive_legacy_state(paths, controller, legacy, fault)?;
    cleanup_migration_temps(controller, paths.expected_uid)?;
    open_existing_controller_store(paths)
}

fn resolve_dual_store(
    paths: &ControllerStatePaths,
    controller: &ControllerDirectory,
    legacy: &LegacyState,
    fault: Option<MigrationFault>,
) -> Result<Store, ControllerStateError> {
    resolve_dual_store_with_before_archive(paths, controller, legacy, fault, || Ok(()))
}

fn resolve_dual_store_with_before_archive<F>(
    paths: &ControllerStatePaths,
    controller: &ControllerDirectory,
    legacy: &LegacyState,
    fault: Option<MigrationFault>,
    before_archive: F,
) -> Result<Store, ControllerStateError>
where
    F: FnOnce() -> Result<(), ControllerStateError>,
{
    let durable = open_named_private_database(
        &controller.descriptor,
        DATABASE_NAME,
        paths.expected_uid,
        false,
        "durable controller database",
    )?;
    let durable_sidecars =
        open_private_sidecars(&controller.descriptor, DATABASE_NAME, paths.expected_uid)?;
    let durable_monitor = DatabaseMutationMonitor::new(controller, &durable)?;
    let legacy_monitor = DatabaseMutationMonitor::new_for_legacy(legacy)?;
    let durable_snapshot = make_snapshot(paths, controller, &durable, &durable_sidecars, None)?;
    let legacy_snapshot =
        make_snapshot(paths, controller, &legacy.database, &legacy.sidecars, fault)?;
    durable_monitor.ensure_unchanged()?;
    legacy_monitor.ensure_unchanged()?;
    validate_named_database_binding(paths, controller, &durable, DATABASE_NAME)?;
    validate_legacy_binding(paths, legacy)?;
    let identical = logical_databases_match(
        &controller_path(paths, &durable_snapshot),
        &controller_path(paths, &legacy_snapshot),
    )?;
    cleanup_migration_temps(controller, paths.expected_uid)?;
    if !identical {
        return Err(ControllerStateError::Conflict {
            durable: paths.durable_database().to_path_buf(),
            legacy: paths.legacy_database().to_path_buf(),
        });
    }
    before_archive()?;
    durable_monitor.ensure_unchanged()?;
    validate_named_database_binding(paths, controller, &durable, DATABASE_NAME)?;
    archive_legacy_state_with_guard(paths, controller, legacy, fault, || {
        durable_monitor.ensure_unchanged()?;
        validate_named_database_binding(paths, controller, &durable, DATABASE_NAME)
    })?;
    durable_monitor.ensure_unchanged()?;
    validate_named_database_binding(paths, controller, &durable, DATABASE_NAME)?;
    let store = open_existing_controller_store(paths)?;
    durable_monitor.ensure_unchanged()?;
    validate_named_database_binding(paths, controller, &durable, DATABASE_NAME)?;
    Ok(store)
}

#[cfg(test)]
fn open_controller_store_with_before_dual_archive<F>(
    paths: &ControllerStatePaths,
    before_archive: F,
) -> Result<Store, ControllerStateError>
where
    F: FnOnce() -> Result<(), ControllerStateError>,
{
    let controller = open_controller_directory(paths)?;
    cleanup_migration_temps(&controller, paths.expected_uid)?;
    let legacy = open_legacy_state(paths)?.ok_or_else(|| {
        ControllerStateError::Migration("dual-state test has no legacy database".to_owned())
    })?;
    resolve_dual_store_with_before_archive(paths, &controller, &legacy, None, before_archive)
}

fn make_snapshot(
    paths: &ControllerStatePaths,
    controller: &ControllerDirectory,
    source: &PrivateDatabase,
    sidecars: &BTreeMap<String, PrivateDatabase>,
    fault: Option<MigrationFault>,
) -> Result<String, ControllerStateError> {
    let sequence = next_temp_sequence(&controller.descriptor)?;
    let staged_name = format!("{TEMP_PREFIX}source-{sequence}");
    copy_private_file(
        &source.descriptor,
        &controller.descriptor,
        &staged_name,
        "staging controller database",
    )?;
    for (suffix, sidecar) in sidecars {
        copy_private_file(
            &sidecar.descriptor,
            &controller.descriptor,
            &format!("{staged_name}{suffix}"),
            "staging SQLite sidecar",
        )?;
    }
    rustix::fs::fsync(&controller.descriptor)
        .map_err(|error| migration_fs_error("syncing staged controller state", error))?;
    if fault == Some(MigrationFault::BeforeSnapshotComplete) {
        return Err(injected_fault(MigrationFault::BeforeSnapshotComplete));
    }

    let snapshot_sequence = next_temp_sequence(&controller.descriptor)?;
    let snapshot_name = format!("{TEMP_PREFIX}snapshot-{snapshot_sequence}");
    let snapshot_descriptor =
        create_private_file(&controller.descriptor, &snapshot_name, "migration snapshot")?;
    let snapshot_stat = rustix::fs::fstat(&snapshot_descriptor)
        .map_err(|error| unsafe_error("migration snapshot", error))?;
    let snapshot = PrivateDatabase {
        identity: validate_private_file_stat(
            &snapshot_stat,
            paths.expected_uid,
            "migration snapshot",
        )?,
        descriptor: snapshot_descriptor,
    };
    let staged = open_named_private_database(
        &controller.descriptor,
        &staged_name,
        paths.expected_uid,
        false,
        "staged controller database",
    )?;
    let monitor =
        DatabaseMutationMonitor::new_for_controller_files(controller, &[&staged, &snapshot])?;
    let source_path = controller_path(paths, &staged_name);
    let snapshot_path = controller_path(paths, &snapshot_name);
    let source_connection = Connection::open_with_flags(
        source_path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NOFOLLOW
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| migration_sql_error("opening the staged source", error))?;
    let mut destination_connection = Connection::open_with_flags(
        &snapshot_path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NOFOLLOW
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| migration_sql_error("opening the migration snapshot", error))?;
    let backup = Backup::new(&source_connection, &mut destination_connection)
        .map_err(|error| migration_sql_error("starting the SQLite backup", error))?;
    backup
        .run_to_completion(128, Duration::from_millis(1), None)
        .map_err(|error| migration_sql_error("copying the SQLite snapshot", error))?;
    drop(backup);
    drop(destination_connection);
    drop(source_connection);

    monitor.ensure_unchanged()?;
    validate_named_database_binding(paths, controller, &staged, &staged_name)?;
    validate_named_database_binding(paths, controller, &snapshot, &snapshot_name)?;
    let validated = Store::open_no_follow(&snapshot_path).map_err(ControllerStateError::Store)?;
    validated
        .list_sandboxes()
        .map_err(ControllerStateError::Store)?;
    drop(validated);
    monitor.ensure_unchanged()?;
    rustix::fs::fsync(&snapshot.descriptor)
        .map_err(|error| migration_fs_error("syncing the migration snapshot", error))?;
    validate_named_database_binding(paths, controller, &snapshot, &snapshot_name)?;
    rustix::fs::fsync(&controller.descriptor)
        .map_err(|error| migration_fs_error("syncing the migration snapshot directory", error))?;
    remove_temp_family(&controller.descriptor, &staged_name)?;
    Ok(snapshot_name)
}

fn logical_databases_match(left: &Path, right: &Path) -> Result<bool, ControllerStateError> {
    let connection = Connection::open_with_flags(
        left,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NOFOLLOW
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| migration_sql_error("opening a comparison snapshot", error))?;
    connection
        .execute(
            "ATTACH DATABASE ?1 AS other",
            [right.to_string_lossy().as_ref()],
        )
        .map_err(|error| migration_sql_error("attaching a comparison snapshot", error))?;
    for table in [
        "schema_version",
        "sandboxes",
        "operations",
        "operation_events",
    ] {
        let sql = format!(
            "SELECT EXISTS(SELECT * FROM main.{table} EXCEPT SELECT * FROM other.{table}) \
             OR EXISTS(SELECT * FROM other.{table} EXCEPT SELECT * FROM main.{table})"
        );
        let differs: bool = connection
            .query_row(&sql, [], |row| row.get(0))
            .map_err(|error| migration_sql_error("comparing controller snapshots", error))?;
        if differs {
            return Ok(false);
        }
    }
    Ok(true)
}

fn archive_legacy_state(
    paths: &ControllerStatePaths,
    controller: &ControllerDirectory,
    legacy: &LegacyState,
    fault: Option<MigrationFault>,
) -> Result<(), ControllerStateError> {
    archive_legacy_state_with_guard(paths, controller, legacy, fault, || Ok(()))
}

fn archive_legacy_state_with_guard<F>(
    paths: &ControllerStatePaths,
    controller: &ControllerDirectory,
    legacy: &LegacyState,
    fault: Option<MigrationFault>,
    before_destructive_archive: F,
) -> Result<(), ControllerStateError>
where
    F: FnOnce() -> Result<(), ControllerStateError>,
{
    let monitor = DatabaseMutationMonitor::new_for_legacy(legacy)?;
    let backup_name = collision_free_backup_name(&controller.descriptor, paths.expected_uid)?;
    copy_private_file(
        &legacy.database.descriptor,
        &controller.descriptor,
        &backup_name,
        "archiving the legacy database",
    )?;
    for (suffix, sidecar) in &legacy.sidecars {
        copy_private_file(
            &sidecar.descriptor,
            &controller.descriptor,
            &format!("{backup_name}{suffix}"),
            "archiving a legacy SQLite sidecar",
        )?;
    }
    rustix::fs::fsync(&controller.descriptor)
        .map_err(|error| migration_fs_error("syncing the legacy archive", error))?;
    monitor.ensure_unchanged()?;
    validate_legacy_binding(paths, legacy)?;
    before_destructive_archive()?;
    let runtime = legacy.directories.last().ok_or_else(|| {
        ControllerStateError::Invalid("legacy database has no parent descriptor".to_owned())
    })?;
    rustix::fs::unlinkat(runtime, DATABASE_NAME, AtFlags::empty())
        .map_err(|error| migration_fs_error("removing the archived legacy database", error))?;
    if fault == Some(MigrationFault::DuringLegacyArchive) {
        rustix::fs::fsync(runtime)
            .map_err(|error| migration_fs_error("syncing the legacy runtime directory", error))?;
        return Err(injected_fault(MigrationFault::DuringLegacyArchive));
    }
    for suffix in legacy.sidecars.keys() {
        rustix::fs::unlinkat(
            runtime,
            format!("{DATABASE_NAME}{suffix}"),
            AtFlags::empty(),
        )
        .map_err(|error| migration_fs_error("removing an archived SQLite sidecar", error))?;
    }
    rustix::fs::fsync(runtime)
        .map_err(|error| migration_fs_error("syncing the legacy runtime directory", error))?;
    rustix::fs::fsync(&controller.descriptor)
        .map_err(|error| migration_fs_error("syncing the durable archive directory", error))?;
    Ok(())
}

fn archive_legacy_orphans(
    paths: &ControllerStatePaths,
    controller: &ControllerDirectory,
    orphans: &LegacyOrphans,
) -> Result<(), ControllerStateError> {
    let monitor = DatabaseMutationMonitor::new_for_orphans(orphans)?;
    let backup_name = collision_free_backup_name(&controller.descriptor, paths.expected_uid)?;
    for (suffix, sidecar) in &orphans.sidecars {
        copy_private_file(
            &sidecar.descriptor,
            &controller.descriptor,
            &format!("{backup_name}{suffix}"),
            "archiving an orphaned legacy SQLite sidecar",
        )?;
    }
    rustix::fs::fsync(&controller.descriptor)
        .map_err(|error| migration_fs_error("syncing orphaned legacy sidecars", error))?;
    monitor.ensure_unchanged()?;
    let runtime = orphans.directories.last().ok_or_else(|| {
        ControllerStateError::Invalid("legacy database has no parent descriptor".to_owned())
    })?;
    for (suffix, sidecar) in &orphans.sidecars {
        let name = format!("{DATABASE_NAME}{suffix}");
        let stat = rustix::fs::statat(runtime, &name, AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|error| unsafe_error("orphaned legacy SQLite sidecar", error))?;
        if validate_private_file_stat(&stat, paths.expected_uid, "orphaned legacy SQLite sidecar")?
            != sidecar.identity
        {
            return Err(ControllerStateError::Unsafe(
                "orphaned legacy SQLite sidecar changed during archival".to_owned(),
            ));
        }
        rustix::fs::unlinkat(
            runtime,
            format!("{DATABASE_NAME}{suffix}"),
            AtFlags::empty(),
        )
        .map_err(|error| migration_fs_error("removing an orphaned legacy SQLite sidecar", error))?;
    }
    rustix::fs::fsync(runtime)
        .map_err(|error| migration_fs_error("syncing the legacy runtime directory", error))?;
    Ok(())
}

#[cfg(test)]
fn open_controller_store_with_before_store<F>(
    paths: &ControllerStatePaths,
    before_store: F,
) -> Result<Store, ControllerStateError>
where
    F: FnOnce() -> Result<(), ControllerStateError>,
{
    open_controller_store_with_hooks(paths, before_store, || Ok(()))
}

fn open_controller_store_with_hooks<F, G>(
    paths: &ControllerStatePaths,
    before_store: F,
    after_store_open: G,
) -> Result<Store, ControllerStateError>
where
    F: FnOnce() -> Result<(), ControllerStateError>,
    G: FnOnce() -> Result<(), ControllerStateError>,
{
    let controller_directory = open_controller_directory(paths)?;
    let database = open_private_database(&controller_directory.descriptor, paths.expected_uid)?;
    let mutation_monitor = DatabaseMutationMonitor::new(&controller_directory, &database)?;
    before_store()?;
    let store = match Store::open_no_follow_with_hook(paths.durable_database(), after_store_open) {
        Ok(store) => store,
        Err(error) => {
            mutation_monitor.ensure_unchanged()?;
            validate_database_binding(paths, &controller_directory, &database)?;
            return Err(error);
        }
    };
    mutation_monitor.ensure_unchanged()?;
    validate_database_binding(paths, &controller_directory, &database)?;
    Ok(store)
}

fn open_existing_controller_store(
    paths: &ControllerStatePaths,
) -> Result<Store, ControllerStateError> {
    let controller_directory = open_existing_controller_directory(paths)?;
    let database = open_named_private_database(
        &controller_directory.descriptor,
        DATABASE_NAME,
        paths.expected_uid,
        true,
        "controller database",
    )?;
    let mutation_monitor = DatabaseMutationMonitor::new(&controller_directory, &database)?;
    let store = match Store::open_no_follow(paths.durable_database()) {
        Ok(store) => store,
        Err(error) => {
            mutation_monitor.ensure_unchanged()?;
            validate_database_binding(paths, &controller_directory, &database)?;
            return Err(error.into());
        }
    };
    mutation_monitor.ensure_unchanged()?;
    validate_database_binding(paths, &controller_directory, &database)?;
    Ok(store)
}

fn open_existing_directory_chain(
    path: &Path,
) -> Result<Option<Vec<OwnedFd>>, ControllerStateError> {
    let mut components = path.components();
    if components.next() != Some(Component::RootDir) {
        return Err(ControllerStateError::Invalid(
            "directory path must be absolute".to_owned(),
        ));
    }
    let root = rustix::fs::open(
        "/",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| unsafe_error("root directory", error))?;
    let mut directories = vec![root];
    for component in components {
        let Component::Normal(name) = component else {
            return Err(ControllerStateError::Invalid(
                "directory path contains a non-normal component".to_owned(),
            ));
        };
        let parent = directories.last().ok_or_else(|| {
            ControllerStateError::Invalid("directory traversal lost its parent".to_owned())
        })?;
        let directory = match rustix::fs::openat(
            parent,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(directory) => directory,
            Err(error) if error == rustix::io::Errno::NOENT => return Ok(None),
            Err(error) => return Err(unsafe_error("directory path", error)),
        };
        directories.push(directory);
    }
    Ok(Some(directories))
}

fn private_regular_file_exists(
    directory: &OwnedFd,
    name: &str,
    expected_uid: u32,
    label: &str,
) -> Result<bool, ControllerStateError> {
    let stat = match rustix::fs::statat(directory, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(stat) => stat,
        Err(error) if error == rustix::io::Errno::NOENT => return Ok(false),
        Err(error) => return Err(unsafe_error(label, error)),
    };
    validate_private_file_stat(&stat, expected_uid, label)?;
    Ok(true)
}

fn entry_exists(directory: &OwnedFd, name: &str) -> Result<bool, ControllerStateError> {
    match rustix::fs::statat(directory, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(_) => Ok(true),
        Err(error) if error == rustix::io::Errno::NOENT => Ok(false),
        Err(error) => Err(unsafe_error(name, error)),
    }
}

fn open_named_private_database(
    directory: &OwnedFd,
    name: &str,
    expected_uid: u32,
    writable: bool,
    label: &str,
) -> Result<PrivateDatabase, ControllerStateError> {
    let access = if writable {
        OFlags::RDWR
    } else {
        OFlags::RDONLY
    };
    let descriptor = rustix::fs::openat(
        directory,
        name,
        access | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| unsafe_error(label, error))?;
    let stat = rustix::fs::fstat(&descriptor).map_err(|error| unsafe_error(label, error))?;
    let identity = validate_private_file_stat(&stat, expected_uid, label)?;
    Ok(PrivateDatabase {
        descriptor,
        identity,
    })
}

fn open_private_sidecars(
    directory: &OwnedFd,
    database_name: &str,
    expected_uid: u32,
) -> Result<BTreeMap<String, PrivateDatabase>, ControllerStateError> {
    let mut sidecars = BTreeMap::new();
    for suffix in SQLITE_SIDECAR_SUFFIXES {
        let name = format!("{database_name}{suffix}");
        if private_regular_file_exists(directory, &name, expected_uid, "SQLite sidecar")? {
            sidecars.insert(
                suffix.to_owned(),
                open_named_private_database(
                    directory,
                    &name,
                    expected_uid,
                    false,
                    "SQLite sidecar",
                )?,
            );
        }
    }
    Ok(sidecars)
}

fn validate_private_file_stat(
    stat: &rustix::fs::Stat,
    expected_uid: u32,
    label: &str,
) -> Result<DatabaseIdentity, ControllerStateError> {
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
        || stat.st_uid != expected_uid
        || stat.st_nlink != 1
        || u32::from(Mode::from_raw_mode(stat.st_mode).bits() & 0o7777) != FILE_MODE
    {
        return Err(ControllerStateError::Unsafe(format!(
            "{label} ownership, type, links, or mode is unsafe"
        )));
    }
    Ok(DatabaseIdentity {
        device: stat.st_dev as u64,
        inode: stat.st_ino,
    })
}

fn copy_private_file(
    source: &OwnedFd,
    destination_directory: &OwnedFd,
    destination_name: &str,
    context: &str,
) -> Result<(), ControllerStateError> {
    let destination = create_private_file(destination_directory, destination_name, context)?;
    let source_copy = rustix::io::dup(source)
        .map_err(|error| migration_fs_error("duplicating a migration source descriptor", error))?;
    let source_file = File::from(source_copy);
    let mut destination_file = File::from(destination);
    let mut offset = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = source_file
            .read_at(&mut buffer, offset)
            .map_err(|error| ControllerStateError::Migration(format!("{context}: {error}")))?;
        if read == 0 {
            break;
        }
        destination_file
            .write_all(&buffer[..read])
            .map_err(|error| ControllerStateError::Migration(format!("{context}: {error}")))?;
        offset = offset.checked_add(read as u64).ok_or_else(|| {
            ControllerStateError::Migration(format!("{context}: source file is too large"))
        })?;
    }
    destination_file
        .sync_all()
        .map_err(|error| ControllerStateError::Migration(format!("{context}: {error}")))?;
    Ok(())
}

fn create_private_file(
    directory: &OwnedFd,
    name: &str,
    context: &str,
) -> Result<OwnedFd, ControllerStateError> {
    let file = rustix::fs::openat(
        directory,
        name,
        OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(FILE_MODE as u16),
    )
    .map_err(|error| migration_fs_error(context, error))?;
    rustix::fs::fchmod(&file, Mode::from_raw_mode(FILE_MODE as u16))
        .map_err(|error| migration_fs_error(context, error))?;
    Ok(file)
}

fn next_temp_sequence(directory: &OwnedFd) -> Result<u32, ControllerStateError> {
    for sequence in 0..u32::MAX {
        let source = format!("{TEMP_PREFIX}source-{sequence}");
        let snapshot = format!("{TEMP_PREFIX}snapshot-{sequence}");
        let source_is_free = temp_family_is_free(directory, &source)?;
        let snapshot_is_free = temp_family_is_free(directory, &snapshot)?;
        if source_is_free && snapshot_is_free {
            return Ok(sequence);
        }
    }
    Err(ControllerStateError::Migration(
        "no collision-free migration temporary name is available".to_owned(),
    ))
}

fn temp_family_is_free(directory: &OwnedFd, base: &str) -> Result<bool, ControllerStateError> {
    for name in std::iter::once(base.to_owned()).chain(
        SQLITE_SIDECAR_SUFFIXES
            .iter()
            .map(|suffix| format!("{base}{suffix}")),
    ) {
        if entry_exists(directory, &name)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn cleanup_migration_temps(
    controller: &ControllerDirectory,
    expected_uid: u32,
) -> Result<(), ControllerStateError> {
    cleanup_migration_temps_with_hook(controller, expected_uid, |_| Ok(()))
}

fn cleanup_migration_temps_with_hook<F>(
    controller: &ControllerDirectory,
    expected_uid: u32,
    mut before_unlink: F,
) -> Result<(), ControllerStateError>
where
    F: FnMut(&str) -> Result<(), ControllerStateError>,
{
    let mut directory = rustix::fs::Dir::read_from(&controller.descriptor)
        .map_err(|error| unsafe_error("controller directory", error))?;
    let mut candidates = Vec::new();
    for entry in &mut directory {
        let entry = entry.map_err(|error| unsafe_error("controller directory entry", error))?;
        let name = OsStr::from_bytes(entry.file_name().to_bytes());
        let Some(name) = name.to_str() else {
            continue;
        };
        if exact_migration_temp_name(name) {
            let stat = rustix::fs::statat(&controller.descriptor, name, AtFlags::SYMLINK_NOFOLLOW)
                .map_err(|error| unsafe_error("migration temporary file", error))?;
            validate_private_file_stat(&stat, expected_uid, "migration temporary file")?;
            candidates.push((
                name.to_owned(),
                open_named_private_database(
                    &controller.descriptor,
                    name,
                    expected_uid,
                    false,
                    "migration temporary file",
                )?,
            ));
        }
    }
    for (name, candidate) in candidates {
        before_unlink(&name)?;
        let quarantine = cleanup_quarantine_name(&controller.descriptor, &candidate.identity)?;
        rustix::fs::renameat(
            &controller.descriptor,
            &name,
            &controller.descriptor,
            &quarantine,
        )
        .map_err(|error| unsafe_error("quarantining a migration temporary file", error))?;
        let quarantined_stat = rustix::fs::statat(
            &controller.descriptor,
            &quarantine,
            AtFlags::SYMLINK_NOFOLLOW,
        )
        .map_err(|error| unsafe_error("quarantined migration temporary file", error))?;
        let quarantined_identity = match validate_private_file_stat(
            &quarantined_stat,
            expected_uid,
            "quarantined migration temporary file",
        ) {
            Ok(identity) => identity,
            Err(error) => {
                restore_quarantined_file(&controller.descriptor, &quarantine, &name)?;
                return Err(error);
            }
        };
        if quarantined_identity != candidate.identity {
            restore_quarantined_file(&controller.descriptor, &quarantine, &name)?;
            return Err(ControllerStateError::Unsafe(
                "migration temporary file changed during cleanup".to_owned(),
            ));
        }
        rustix::fs::unlinkat(&controller.descriptor, &quarantine, AtFlags::empty())
            .map_err(|error| unsafe_error("quarantined migration temporary file", error))?;
    }
    rustix::fs::fsync(&controller.descriptor)
        .map_err(|error| migration_fs_error("syncing migration cleanup", error))?;
    Ok(())
}

fn cleanup_quarantine_name(
    directory: &OwnedFd,
    identity: &DatabaseIdentity,
) -> Result<String, ControllerStateError> {
    for sequence in 0..u32::MAX {
        let name = format!(
            ".state.sqlite3.cleanup-quarantine-{:x}-{:x}-{sequence}",
            identity.device, identity.inode
        );
        if !entry_exists(directory, &name)? {
            return Ok(name);
        }
    }
    Err(ControllerStateError::Unsafe(
        "no collision-free cleanup quarantine name is available".to_owned(),
    ))
}

fn restore_quarantined_file(
    directory: &OwnedFd,
    quarantine: &str,
    original: &str,
) -> Result<(), ControllerStateError> {
    rustix::fs::linkat(directory, quarantine, directory, original, AtFlags::empty())
        .map_err(|error| unsafe_error("restoring a substituted migration temporary file", error))?;
    rustix::fs::unlinkat(directory, quarantine, AtFlags::empty())
        .map_err(|error| unsafe_error("restoring a substituted migration temporary file", error))?;
    Ok(())
}

fn exact_migration_temp_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix(TEMP_PREFIX) else {
        return false;
    };
    let rest = rest
        .strip_suffix("-wal")
        .or_else(|| rest.strip_suffix("-shm"))
        .or_else(|| rest.strip_suffix("-journal"))
        .unwrap_or(rest);
    ["source-", "snapshot-"].iter().any(|prefix| {
        rest.strip_prefix(prefix).is_some_and(|number| {
            !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
        })
    })
}

fn remove_temp_family(directory: &OwnedFd, base: &str) -> Result<(), ControllerStateError> {
    for name in std::iter::once(base.to_owned()).chain(
        SQLITE_SIDECAR_SUFFIXES
            .iter()
            .map(|suffix| format!("{base}{suffix}")),
    ) {
        match rustix::fs::unlinkat(directory, &name, AtFlags::empty()) {
            Ok(()) => {}
            Err(error) if error == rustix::io::Errno::NOENT => {}
            Err(error) => return Err(migration_fs_error("removing staged state", error)),
        }
    }
    Ok(())
}

fn collision_free_backup_name(
    directory: &OwnedFd,
    expected_uid: u32,
) -> Result<String, ControllerStateError> {
    for sequence in 0..u32::MAX {
        let name = if sequence == 0 {
            LEGACY_BACKUP_NAME.to_owned()
        } else {
            format!("{LEGACY_BACKUP_NAME}.{sequence}")
        };
        let mut collision = false;
        for candidate in std::iter::once(name.clone()).chain(
            SQLITE_SIDECAR_SUFFIXES
                .iter()
                .map(|suffix| format!("{name}{suffix}")),
        ) {
            if private_regular_file_exists(
                directory,
                &candidate,
                expected_uid,
                "legacy migration backup",
            )? {
                collision = true;
            }
        }
        if !collision {
            return Ok(name);
        }
    }
    Err(ControllerStateError::Migration(
        "no collision-free legacy backup name is available".to_owned(),
    ))
}

fn controller_path(paths: &ControllerStatePaths, name: &str) -> PathBuf {
    paths
        .durable_database()
        .parent()
        .unwrap_or_else(|| Path::new("/"))
        .join(name)
}

fn validate_named_database_binding(
    paths: &ControllerStatePaths,
    directory: &ControllerDirectory,
    database: &PrivateDatabase,
    name: &str,
) -> Result<(), ControllerStateError> {
    let stat = rustix::fs::statat(&directory.descriptor, name, AtFlags::SYMLINK_NOFOLLOW)
        .map_err(|error| unsafe_error("controller database", error))?;
    if validate_private_file_stat(&stat, paths.expected_uid, "controller database")?
        != database.identity
    {
        return Err(ControllerStateError::Unsafe(
            "controller database path changed during migration".to_owned(),
        ));
    }
    let current = open_existing_controller_directory(paths)?;
    let stat = rustix::fs::statat(&current.descriptor, name, AtFlags::SYMLINK_NOFOLLOW)
        .map_err(|error| unsafe_error("controller database", error))?;
    if validate_private_file_stat(&stat, paths.expected_uid, "controller database")?
        != database.identity
    {
        return Err(ControllerStateError::Unsafe(
            "controller directory identity changed during migration".to_owned(),
        ));
    }
    Ok(())
}

fn validate_legacy_binding(
    paths: &ControllerStatePaths,
    legacy: &LegacyState,
) -> Result<(), ControllerStateError> {
    let current =
        open_existing_directory_chain(paths.legacy_database().parent().ok_or_else(|| {
            ControllerStateError::Invalid("legacy database has no parent".to_owned())
        })?)?
        .ok_or_else(|| {
            ControllerStateError::Unsafe(
                "legacy database directory changed during migration".to_owned(),
            )
        })?;
    let current_parent = current.last().ok_or_else(|| {
        ControllerStateError::Unsafe("legacy database directory is unavailable".to_owned())
    })?;
    let stat = rustix::fs::statat(current_parent, DATABASE_NAME, AtFlags::SYMLINK_NOFOLLOW)
        .map_err(|error| unsafe_error("legacy controller database", error))?;
    if validate_private_file_stat(&stat, paths.expected_uid, "legacy controller database")?
        != legacy.database.identity
    {
        return Err(ControllerStateError::Unsafe(
            "legacy controller database identity changed during migration".to_owned(),
        ));
    }
    for (suffix, sidecar) in &legacy.sidecars {
        let name = format!("{DATABASE_NAME}{suffix}");
        let descriptor_stat = rustix::fs::fstat(&sidecar.descriptor)
            .map_err(|error| unsafe_error("legacy SQLite sidecar", error))?;
        if validate_private_file_stat(
            &descriptor_stat,
            paths.expected_uid,
            "legacy SQLite sidecar",
        )? != sidecar.identity
        {
            return Err(ControllerStateError::Unsafe(
                "legacy SQLite sidecar descriptor changed during migration".to_owned(),
            ));
        }
        let path_stat = rustix::fs::statat(current_parent, &name, AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|error| unsafe_error("legacy SQLite sidecar", error))?;
        if validate_private_file_stat(&path_stat, paths.expected_uid, "legacy SQLite sidecar")?
            != sidecar.identity
        {
            return Err(ControllerStateError::Unsafe(
                "legacy SQLite sidecar identity changed during migration".to_owned(),
            ));
        }
    }
    Ok(())
}

fn injected_fault(fault: MigrationFault) -> ControllerStateError {
    ControllerStateError::Migration(format!("injected fault at {fault:?}"))
}

fn migration_fs_error(context: &str, error: rustix::io::Errno) -> ControllerStateError {
    ControllerStateError::Migration(format!("{context}: {error}"))
}

fn migration_sql_error(context: &str, error: rusqlite::Error) -> ControllerStateError {
    ControllerStateError::Migration(format!("{context}: {error}"))
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

struct StateAncestorDirectories {
    home: OwnedFd,
    library: OwnedFd,
    application_support: OwnedFd,
}

struct ControllerDirectory {
    home: OwnedFd,
    library: OwnedFd,
    application_support: OwnedFd,
    application: OwnedFd,
    descriptor: OwnedFd,
}

impl ControllerDirectory {
    fn descriptors(&self) -> [&OwnedFd; 5] {
        [
            &self.home,
            &self.library,
            &self.application_support,
            &self.application,
            &self.descriptor,
        ]
    }
}

fn open_controller_directory(
    paths: &ControllerStatePaths,
) -> Result<ControllerDirectory, ControllerStateError> {
    let ancestors = open_state_ancestor_directories(paths)?;
    let application_directory = ensure_private_child_directory(
        &ancestors.application_support,
        APPLICATION_ID,
        paths.expected_uid,
        "application directory",
    )?;
    let descriptor = ensure_private_child_directory(
        &application_directory,
        CONTROLLER_DIRECTORY,
        paths.expected_uid,
        "controller directory",
    )?;
    Ok(ControllerDirectory {
        home: ancestors.home,
        library: ancestors.library,
        application_support: ancestors.application_support,
        application: application_directory,
        descriptor,
    })
}

fn open_existing_controller_directory(
    paths: &ControllerStatePaths,
) -> Result<ControllerDirectory, ControllerStateError> {
    let ancestors = open_state_ancestor_directories(paths)?;
    let application_directory = open_existing_child_directory(
        &ancestors.application_support,
        OsStr::new(APPLICATION_ID),
        "application directory",
    )?;
    validate_directory(
        &application_directory,
        paths.expected_uid,
        true,
        "application directory",
    )?;
    let controller_directory = open_existing_child_directory(
        &application_directory,
        OsStr::new(CONTROLLER_DIRECTORY),
        "controller directory",
    )?;
    validate_directory(
        &controller_directory,
        paths.expected_uid,
        true,
        "controller directory",
    )?;
    Ok(ControllerDirectory {
        home: ancestors.home,
        library: ancestors.library,
        application_support: ancestors.application_support,
        application: application_directory,
        descriptor: controller_directory,
    })
}

fn open_state_ancestor_directories(
    paths: &ControllerStatePaths,
) -> Result<StateAncestorDirectories, ControllerStateError> {
    let home = paths.durable_database.ancestors().nth(5).ok_or_else(|| {
        ControllerStateError::Invalid("durable database has no account home".to_owned())
    })?;
    let home_directory = open_existing_directory(home, "account home")?;
    validate_directory(&home_directory, paths.expected_uid, false, "account home")?;

    let library = open_existing_child_directory(&home_directory, OsStr::new("Library"), "Library")?;
    validate_directory(&library, paths.expected_uid, false, "Library")?;
    let application_support = open_existing_child_directory(
        &library,
        OsStr::new("Application Support"),
        "Application Support",
    )?;
    validate_directory(
        &application_support,
        paths.expected_uid,
        false,
        "Application Support",
    )?;

    Ok(StateAncestorDirectories {
        home: home_directory,
        library,
        application_support,
    })
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

struct PrivateDatabase {
    descriptor: OwnedFd,
    identity: DatabaseIdentity,
}

#[cfg(target_os = "macos")]
struct DatabaseMutationMonitor {
    queue: Kqueue,
    event_capacity: usize,
}

#[cfg(target_os = "macos")]
impl DatabaseMutationMonitor {
    fn new(
        directory: &ControllerDirectory,
        database: &PrivateDatabase,
    ) -> Result<Self, ControllerStateError> {
        Self::new_for_controller_files(directory, &[database])
    }

    fn new_for_controller_files(
        directory: &ControllerDirectory,
        databases: &[&PrivateDatabase],
    ) -> Result<Self, ControllerStateError> {
        let mut descriptors = directory.descriptors().to_vec();
        descriptors.extend(databases.iter().map(|database| &database.descriptor));
        Self::from_descriptors(&descriptors)
    }

    fn new_for_legacy(legacy: &LegacyState) -> Result<Self, ControllerStateError> {
        let mut descriptors = legacy.directories.last().into_iter().collect::<Vec<_>>();
        descriptors.push(&legacy.database.descriptor);
        descriptors.extend(legacy.sidecars.values().map(|sidecar| &sidecar.descriptor));
        Self::from_descriptors(&descriptors)
    }

    fn new_for_orphans(orphans: &LegacyOrphans) -> Result<Self, ControllerStateError> {
        let mut descriptors = orphans.directories.last().into_iter().collect::<Vec<_>>();
        descriptors.extend(orphans.sidecars.values().map(|sidecar| &sidecar.descriptor));
        Self::from_descriptors(&descriptors)
    }

    fn from_descriptors(descriptors: &[&OwnedFd]) -> Result<Self, ControllerStateError> {
        let queue = Kqueue::new().map_err(|error| {
            ControllerStateError::Unsafe(format!(
                "controller database identity monitor could not be created: {error}"
            ))
        })?;
        let identity_events = FilterFlag::NOTE_DELETE
            | FilterFlag::NOTE_RENAME
            | FilterFlag::NOTE_LINK
            | FilterFlag::NOTE_REVOKE;
        let mut changes = Vec::with_capacity(descriptors.len());
        for descriptor in descriptors {
            changes.push(KEvent::new(
                descriptor.as_raw_fd() as usize,
                EventFilter::EVFILT_VNODE,
                EvFlags::EV_ADD | EvFlags::EV_ENABLE | EvFlags::EV_CLEAR,
                identity_events,
                0,
                0,
            ));
        }
        let event_capacity = changes.len();
        let mut events = [];
        queue
            .kevent(
                &changes,
                &mut events,
                Some(nix::libc::timespec {
                    tv_sec: 0,
                    tv_nsec: 0,
                }),
            )
            .map_err(|error| {
                ControllerStateError::Unsafe(format!(
                    "controller database identity monitor could not be registered: {error}"
                ))
            })?;
        Ok(Self {
            queue,
            event_capacity,
        })
    }

    fn ensure_unchanged(&self) -> Result<(), ControllerStateError> {
        let placeholder = KEvent::new(
            0,
            EventFilter::EVFILT_VNODE,
            EvFlags::empty(),
            FilterFlag::empty(),
            0,
            0,
        );
        let mut events = vec![placeholder; self.event_capacity];
        let count = self
            .queue
            .kevent(
                &[],
                &mut events,
                Some(nix::libc::timespec {
                    tv_sec: 0,
                    tv_nsec: 0,
                }),
            )
            .map_err(|error| {
                ControllerStateError::Unsafe(format!(
                    "controller database identity monitor failed: {error}"
                ))
            })?;
        if count != 0 {
            return Err(ControllerStateError::Unsafe(
                "controller database identity changed while opening the store".to_owned(),
            ));
        }
        Ok(())
    }
}

#[cfg(not(target_os = "macos"))]
struct DatabaseMutationMonitor;

#[cfg(not(target_os = "macos"))]
impl DatabaseMutationMonitor {
    const fn new(
        _directory: &ControllerDirectory,
        _database: &PrivateDatabase,
    ) -> Result<Self, ControllerStateError> {
        Ok(Self)
    }

    const fn new_for_controller_files(
        _directory: &ControllerDirectory,
        _databases: &[&PrivateDatabase],
    ) -> Result<Self, ControllerStateError> {
        Ok(Self)
    }

    const fn new_for_legacy(_legacy: &LegacyState) -> Result<Self, ControllerStateError> {
        Ok(Self)
    }

    const fn new_for_orphans(_orphans: &LegacyOrphans) -> Result<Self, ControllerStateError> {
        Ok(Self)
    }

    const fn ensure_unchanged(&self) -> Result<(), ControllerStateError> {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DatabaseIdentity {
    device: u64,
    inode: u64,
}

fn open_private_database(
    directory: &OwnedFd,
    expected_uid: u32,
) -> Result<PrivateDatabase, ControllerStateError> {
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
    Ok(PrivateDatabase {
        identity: validate_database(&database, expected_uid)?,
        descriptor: database,
    })
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
    let mode = u32::from(Mode::from_raw_mode(stat.st_mode).bits() & 0o7777);
    let mode_is_safe = if private_mode {
        mode == DIRECTORY_MODE
    } else {
        mode & 0o022 == 0
    };
    if !type_is_directory || stat.st_uid != expected_uid || !mode_is_safe {
        return Err(ControllerStateError::Unsafe(format!(
            "{label} ownership, type, or mode is unsafe"
        )));
    }
    Ok(())
}

fn validate_database(
    database: &OwnedFd,
    expected_uid: u32,
) -> Result<DatabaseIdentity, ControllerStateError> {
    let stat =
        rustix::fs::fstat(database).map_err(|error| unsafe_error("controller database", error))?;
    validate_database_stat(&stat, expected_uid)
}

fn validate_database_binding(
    paths: &ControllerStatePaths,
    directory: &ControllerDirectory,
    database: &PrivateDatabase,
) -> Result<(), ControllerStateError> {
    if validate_database(&database.descriptor, paths.expected_uid)? != database.identity {
        return Err(ControllerStateError::Unsafe(
            "controller database descriptor changed while opening the store".to_owned(),
        ));
    }
    let stat = rustix::fs::statat(
        &directory.descriptor,
        DATABASE_NAME,
        AtFlags::SYMLINK_NOFOLLOW,
    )
    .map_err(|error| unsafe_error("controller database", error))?;
    if validate_database_stat(&stat, paths.expected_uid)? != database.identity {
        return Err(ControllerStateError::Unsafe(
            "controller database path changed while opening the store".to_owned(),
        ));
    }
    let current_directory = open_existing_controller_directory(paths)?;
    let stat = rustix::fs::statat(
        &current_directory.descriptor,
        DATABASE_NAME,
        AtFlags::SYMLINK_NOFOLLOW,
    )
    .map_err(|error| unsafe_error("controller database", error))?;
    if validate_database_stat(&stat, paths.expected_uid)? != database.identity {
        return Err(ControllerStateError::Unsafe(
            "controller database path changed while opening the store".to_owned(),
        ));
    }
    Ok(())
}

fn validate_database_stat(
    stat: &rustix::fs::Stat,
    expected_uid: u32,
) -> Result<DatabaseIdentity, ControllerStateError> {
    validate_private_file_stat(stat, expected_uid, "controller database")
}

fn unsafe_error(context: &str, error: rustix::io::Errno) -> ControllerStateError {
    ControllerStateError::Unsafe(format!("{context}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    fn seed_test_store(path: &Path, label: &str) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent)?;
            }
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        }
        let store = Store::open(path)?;
        drop(store);
        let connection = Connection::open(path)?;
        connection.execute(
            "INSERT INTO sandboxes (id, canonical_root, desired_state, actual_state, updated_at_millis) VALUES (?1, ?2, 'running', 'stopped', 7)",
            [
                &format!("{label}-aaaaaaaaaaaa"),
                &format!("/workspace/{label}"),
            ],
        )?;
        drop(connection);
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        Ok(())
    }

    #[test]
    fn dual_state_refuses_durable_replacement_before_archiving_legacy()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = TempDir::new()?;
        let root = temp.path().canonicalize()?;
        let home = root.join("home");
        let library = home.join("Library");
        let application_support = library.join("Application Support");
        for directory in [&home, &library, &application_support] {
            fs::create_dir(directory)?;
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
        }
        let runtime = root.join("runtime");
        fs::create_dir(&runtime)?;
        fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700))?;
        let paths =
            ControllerStatePaths::for_home_and_runtime(&home, &runtime, geteuid().as_raw())?;
        let application = application_support.join(APPLICATION_ID);
        let controller = application.join(CONTROLLER_DIRECTORY);
        fs::create_dir(&application)?;
        fs::set_permissions(&application, fs::Permissions::from_mode(0o700))?;
        fs::create_dir(&controller)?;
        fs::set_permissions(&controller, fs::Permissions::from_mode(0o700))?;
        seed_test_store(paths.durable_database(), "same")?;
        seed_test_store(paths.legacy_database(), "same")?;
        let legacy_before = fs::read(paths.legacy_database())?;
        let replacement = root.join("replacement.sqlite3");
        let replacement_store = Store::open(&replacement)?;
        drop(replacement_store);
        fs::set_permissions(&replacement, fs::Permissions::from_mode(0o600))?;
        let displaced = root.join("displaced-durable.sqlite3");

        let result = open_controller_store_with_before_dual_archive(&paths, || {
            fs::rename(paths.durable_database(), &displaced)
                .map_err(|error| ControllerStateError::Unsafe(error.to_string()))?;
            fs::rename(&replacement, paths.durable_database())
                .map_err(|error| ControllerStateError::Unsafe(error.to_string()))?;
            Ok(())
        });
        assert!(matches!(result, Err(ControllerStateError::Unsafe(_))));
        assert_eq!(fs::read(paths.legacy_database())?, legacy_before);
        Ok(())
    }

    #[test]
    fn abandoned_temp_cleanup_does_not_delete_a_substituted_file()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = TempDir::new()?;
        let root = temp.path().canonicalize()?;
        let home = root.join("home");
        let library = home.join("Library");
        let application_support = library.join("Application Support");
        for directory in [&home, &library, &application_support] {
            fs::create_dir(directory)?;
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
        }
        let paths = ControllerStatePaths::for_home_and_runtime(
            &home,
            &root.join("runtime"),
            geteuid().as_raw(),
        )?;
        let controller = open_controller_directory(&paths)?;
        let name = ".state.sqlite3.migration-source-41";
        let abandoned = controller_path(&paths, name);
        let displaced = root.join("displaced-temp");
        let replacement = root.join("replacement-temp");
        fs::write(&abandoned, b"abandoned")?;
        fs::set_permissions(&abandoned, fs::Permissions::from_mode(0o600))?;
        fs::write(&replacement, b"replacement")?;
        fs::set_permissions(&replacement, fs::Permissions::from_mode(0o600))?;

        let result =
            cleanup_migration_temps_with_hook(&controller, paths.expected_uid, |candidate| {
                if candidate == name {
                    fs::rename(&abandoned, &displaced)
                        .map_err(|error| ControllerStateError::Unsafe(error.to_string()))?;
                    fs::rename(&replacement, &abandoned)
                        .map_err(|error| ControllerStateError::Unsafe(error.to_string()))?;
                }
                Ok(())
            });
        assert!(result.is_err());
        assert_eq!(fs::read(&abandoned)?, b"replacement");
        assert_eq!(fs::read(&displaced)?, b"abandoned");
        Ok(())
    }

    #[test]
    fn rejects_a_database_path_replaced_after_descriptor_validation()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = TempDir::new()?;
        let root = temp.path().canonicalize()?;
        let home = root.join("home");
        let library = home.join("Library");
        let application_support = library.join("Application Support");
        for directory in [&home, &library, &application_support] {
            fs::create_dir(directory)?;
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
        }
        let paths = ControllerStatePaths::for_home_and_runtime(
            &home,
            &root.join("runtime"),
            geteuid().as_raw(),
        )?;
        let replacement = root.join("replacement.sqlite3");
        fs::write(&replacement, b"")?;
        fs::set_permissions(&replacement, fs::Permissions::from_mode(0o600))?;
        let displaced = root.join("displaced.sqlite3");

        let error = match open_controller_store_with_before_store(&paths, || {
            fs::rename(paths.durable_database(), &displaced)
                .map_err(|error| ControllerStateError::Unsafe(error.to_string()))?;
            std::os::unix::fs::symlink(&replacement, paths.durable_database())
                .map_err(|error| ControllerStateError::Unsafe(error.to_string()))?;
            Ok(())
        }) {
            Ok(_) => {
                return Err(std::io::Error::other("replaced database path was accepted").into());
            }
            Err(error) => error,
        };
        assert_eq!(error.code(), "controller_state_unsafe");
        assert_eq!(fs::metadata(&replacement)?.len(), 0);
        Ok(())
    }

    #[test]
    fn rejects_a_controller_path_replaced_after_descriptor_validation()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = TempDir::new()?;
        let root = temp.path().canonicalize()?;
        let home = root.join("home");
        let library = home.join("Library");
        let application_support = library.join("Application Support");
        for directory in [&home, &library, &application_support] {
            fs::create_dir(directory)?;
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
        }
        let paths = ControllerStatePaths::for_home_and_runtime(
            &home,
            &root.join("runtime"),
            geteuid().as_raw(),
        )?;
        let controller = paths
            .durable_database()
            .parent()
            .ok_or_else(|| std::io::Error::other("controller database has no parent"))?
            .to_path_buf();
        let displaced = root.join("displaced-controller");

        let error = match open_controller_store_with_before_store(&paths, || {
            fs::rename(&controller, &displaced)
                .map_err(|error| ControllerStateError::Unsafe(error.to_string()))?;
            fs::create_dir(&controller)
                .map_err(|error| ControllerStateError::Unsafe(error.to_string()))?;
            fs::set_permissions(&controller, fs::Permissions::from_mode(0o700))
                .map_err(|error| ControllerStateError::Unsafe(error.to_string()))?;
            Ok(())
        }) {
            Ok(_) => {
                return Err(std::io::Error::other("replaced controller path was accepted").into());
            }
            Err(error) => error,
        };
        assert_eq!(error.code(), "controller_state_unsafe");
        Ok(())
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn rejects_an_ordinary_database_aba_swap_during_store_open()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = TempDir::new()?;
        let root = temp.path().canonicalize()?;
        let home = root.join("home");
        let library = home.join("Library");
        let application_support = library.join("Application Support");
        for directory in [&home, &library, &application_support] {
            fs::create_dir(directory)?;
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
        }
        let paths = ControllerStatePaths::for_home_and_runtime(
            &home,
            &root.join("runtime"),
            geteuid().as_raw(),
        )?;
        let replacement = root.join("replacement.sqlite3");
        fs::write(&replacement, b"")?;
        fs::set_permissions(&replacement, fs::Permissions::from_mode(0o600))?;
        let displaced = root.join("displaced.sqlite3");
        let opened_replacement = root.join("opened-replacement.sqlite3");

        let error = match open_controller_store_with_hooks(
            &paths,
            || {
                fs::rename(paths.durable_database(), &displaced)
                    .map_err(|error| ControllerStateError::Unsafe(error.to_string()))?;
                fs::rename(&replacement, paths.durable_database())
                    .map_err(|error| ControllerStateError::Unsafe(error.to_string()))?;
                Ok(())
            },
            || {
                fs::rename(paths.durable_database(), &opened_replacement)
                    .map_err(|error| ControllerStateError::Unsafe(error.to_string()))?;
                fs::rename(&displaced, paths.durable_database())
                    .map_err(|error| ControllerStateError::Unsafe(error.to_string()))?;
                Ok(())
            },
        ) {
            Ok(_) => {
                return Err(
                    std::io::Error::other("ordinary database ABA swap was accepted").into(),
                );
            }
            Err(error) => error,
        };
        assert_eq!(error.code(), "controller_state_unsafe");
        Ok(())
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn rejects_an_ancestor_aba_swap_during_store_open() -> Result<(), Box<dyn std::error::Error>> {
        let temp = TempDir::new()?;
        let root = temp.path().canonicalize()?;
        let home = root.join("home");
        let library = home.join("Library");
        let application_support = library.join("Application Support");
        for directory in [&home, &library, &application_support] {
            fs::create_dir(directory)?;
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
        }
        let paths = ControllerStatePaths::for_home_and_runtime(
            &home,
            &root.join("runtime"),
            geteuid().as_raw(),
        )?;
        let application = application_support.join(APPLICATION_ID);
        let displaced = root.join("displaced-application");
        let opened_replacement = root.join("opened-replacement-application");

        let error = match open_controller_store_with_hooks(
            &paths,
            || {
                fs::rename(&application, &displaced)
                    .map_err(|error| ControllerStateError::Unsafe(error.to_string()))?;
                fs::create_dir(&application)
                    .map_err(|error| ControllerStateError::Unsafe(error.to_string()))?;
                fs::set_permissions(&application, fs::Permissions::from_mode(0o700))
                    .map_err(|error| ControllerStateError::Unsafe(error.to_string()))?;
                let controller = application.join(CONTROLLER_DIRECTORY);
                fs::create_dir(&controller)
                    .map_err(|error| ControllerStateError::Unsafe(error.to_string()))?;
                fs::set_permissions(&controller, fs::Permissions::from_mode(0o700))
                    .map_err(|error| ControllerStateError::Unsafe(error.to_string()))?;
                fs::write(controller.join(DATABASE_NAME), b"")
                    .map_err(|error| ControllerStateError::Unsafe(error.to_string()))?;
                fs::set_permissions(
                    controller.join(DATABASE_NAME),
                    fs::Permissions::from_mode(0o600),
                )
                .map_err(|error| ControllerStateError::Unsafe(error.to_string()))?;
                Ok(())
            },
            || {
                fs::rename(&application, &opened_replacement)
                    .map_err(|error| ControllerStateError::Unsafe(error.to_string()))?;
                fs::rename(&displaced, &application)
                    .map_err(|error| ControllerStateError::Unsafe(error.to_string()))?;
                Ok(())
            },
        ) {
            Ok(_) => {
                return Err(std::io::Error::other("ancestor ABA swap was accepted").into());
            }
            Err(error) => error,
        };
        assert_eq!(error.code(), "controller_state_unsafe");
        Ok(())
    }
}
