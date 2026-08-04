use crate::{Store, StoreError};
#[cfg(target_os = "macos")]
use nix::sys::event::{EvFlags, EventFilter, FilterFlag, KEvent, Kqueue};
use rusqlite::{Connection, OpenFlags, backup::Backup};
use rustix::fd::OwnedFd;
use rustix::fs::{AtFlags, FileType, Mode, OFlags};
use rustix::process::geteuid;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs::File;
use std::io::{Read, Write};
#[cfg(target_os = "macos")]
use std::os::fd::{AsRawFd, RawFd};
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
const ARCHIVE_QUARANTINE_PREFIX: &str = ".state.sqlite3.archive-quarantine-";
const ARCHIVE_MARKER_HEADER: &str = "GASCAN_LEGACY_ARCHIVE_V1";
const ARCHIVE_PREPARED_MARKER: &str = "prepared";
const ARCHIVE_COMMITTED_MARKER: &str = "committed";
const ARCHIVE_RESTORED_MARKER: &str = "restored";
const ORPHAN_QUARANTINE_PREFIX: &str = ".state.sqlite3.orphan-quarantine-";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MigrationFault {
    BeforeSnapshotComplete,
    BeforeDurableRename,
    AfterDurableRename,
    DuringLegacyArchive,
    AfterLegacyMoveBeforeValidation,
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
    recover_legacy_archive_transactions(paths)?;
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
    migrate_legacy_store_with_after_stage_hook(paths, controller, legacy, fault, |_| Ok(()))
}

fn migrate_legacy_store_with_after_stage_hook<F>(
    paths: &ControllerStatePaths,
    controller: &ControllerDirectory,
    legacy: &LegacyState,
    fault: Option<MigrationFault>,
    after_stage: F,
) -> Result<Store, ControllerStateError>
where
    F: FnOnce(&str) -> Result<(), ControllerStateError>,
{
    migrate_legacy_store_with_snapshot_hooks(paths, controller, legacy, fault, after_stage, |_| {
        Ok(())
    })
}

fn migrate_legacy_store_with_snapshot_hooks<F, G>(
    paths: &ControllerStatePaths,
    controller: &ControllerDirectory,
    legacy: &LegacyState,
    fault: Option<MigrationFault>,
    after_stage: F,
    after_consumption: G,
) -> Result<Store, ControllerStateError>
where
    F: FnOnce(&str) -> Result<(), ControllerStateError>,
    G: FnOnce(&str) -> Result<(), ControllerStateError>,
{
    let monitor = DatabaseMutationMonitor::new_for_legacy(legacy)?;
    let snapshot_name = make_snapshot_with_hooks(
        paths,
        controller,
        &legacy.database,
        &legacy.sidecars,
        fault,
        after_stage,
        after_consumption,
    )?;
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
    let transaction = archive_legacy_state(paths, controller, legacy, fault)?;
    if let Err(error) = cleanup_migration_temps(controller, paths.expected_uid) {
        transaction.restore()?;
        return Err(error);
    }
    let store = match open_existing_controller_store(paths) {
        Ok(store) => store,
        Err(error) => {
            transaction.restore()?;
            return Err(error);
        }
    };
    transaction.commit()?;
    Ok(store)
}

fn resolve_dual_store(
    paths: &ControllerStatePaths,
    controller: &ControllerDirectory,
    legacy: &LegacyState,
    fault: Option<MigrationFault>,
) -> Result<Store, ControllerStateError> {
    resolve_dual_store_with_hooks(
        paths,
        controller,
        legacy,
        fault,
        || Ok(()),
        || Ok(()),
        || Ok(()),
    )
}

fn resolve_dual_store_with_hooks<F, G, H>(
    paths: &ControllerStatePaths,
    controller: &ControllerDirectory,
    legacy: &LegacyState,
    fault: Option<MigrationFault>,
    before_archive: F,
    final_unlink_window: G,
    after_archive: H,
) -> Result<Store, ControllerStateError>
where
    F: FnOnce() -> Result<(), ControllerStateError>,
    G: FnOnce() -> Result<(), ControllerStateError>,
    H: FnOnce() -> Result<(), ControllerStateError>,
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
    let durable_monitor = DatabaseMutationMonitor::new_for_controller_family_identity(
        controller,
        &durable,
        &durable_sidecars,
    )?;
    let legacy_monitor = DatabaseMutationMonitor::new_for_legacy(legacy)?;
    let durable_snapshot = make_snapshot(paths, controller, &durable, &durable_sidecars, None)?;
    let legacy_snapshot =
        make_snapshot(paths, controller, &legacy.database, &legacy.sidecars, fault)?;
    durable_monitor.ensure_unchanged()?;
    legacy_monitor.ensure_unchanged()?;
    validate_database_family_binding(
        paths,
        controller,
        &durable,
        &durable_sidecars,
        DATABASE_NAME,
    )?;
    validate_legacy_binding(paths, legacy)?;
    let identical = logical_databases_match(
        &controller_path(paths, &durable_snapshot),
        &controller_path(paths, &legacy_snapshot),
    )?;
    cleanup_migration_temps(controller, paths.expected_uid)?;
    let durable_monitor = DatabaseMutationMonitor::new_for_controller_family(
        controller,
        &durable,
        &durable_sidecars,
    )?;
    validate_database_family_binding(
        paths,
        controller,
        &durable,
        &durable_sidecars,
        DATABASE_NAME,
    )?;
    if !identical {
        return Err(ControllerStateError::Conflict {
            durable: paths.durable_database().to_path_buf(),
            legacy: paths.legacy_database().to_path_buf(),
        });
    }
    before_archive()?;
    durable_monitor.ensure_unchanged()?;
    validate_database_family_binding(
        paths,
        controller,
        &durable,
        &durable_sidecars,
        DATABASE_NAME,
    )?;
    let transaction = archive_legacy_state_with_guard(
        paths,
        controller,
        legacy,
        fault,
        final_unlink_window,
        || {
            let monitor = DatabaseMutationMonitor::new_for_controller_family(
                controller,
                &durable,
                &durable_sidecars,
            )?;
            validate_database_family_binding(
                paths,
                controller,
                &durable,
                &durable_sidecars,
                DATABASE_NAME,
            )?;
            Ok(monitor)
        },
        |monitor| {
            monitor.ensure_unchanged()?;
            validate_database_family_binding(
                paths,
                controller,
                &durable,
                &durable_sidecars,
                DATABASE_NAME,
            )
        },
    )?;
    if let Err(error) = after_archive() {
        transaction.restore()?;
        return Err(error);
    }
    if let Err(error) = validate_database_family_binding(
        paths,
        controller,
        &durable,
        &durable_sidecars,
        DATABASE_NAME,
    ) {
        transaction.restore()?;
        return Err(error);
    }
    let store = match open_existing_controller_store(paths) {
        Ok(store) => store,
        Err(error) => {
            transaction.restore()?;
            return Err(error);
        }
    };
    transaction.commit()?;
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
    resolve_dual_store_with_hooks(
        paths,
        &controller,
        &legacy,
        None,
        before_archive,
        || Ok(()),
        || Ok(()),
    )
}

#[cfg(test)]
fn open_controller_store_with_final_dual_unlink_hook<F>(
    paths: &ControllerStatePaths,
    final_unlink_window: F,
) -> Result<Store, ControllerStateError>
where
    F: FnOnce() -> Result<(), ControllerStateError>,
{
    let controller = open_controller_directory(paths)?;
    cleanup_migration_temps(&controller, paths.expected_uid)?;
    let legacy = open_legacy_state(paths)?.ok_or_else(|| {
        ControllerStateError::Migration("dual-state test has no legacy database".to_owned())
    })?;
    resolve_dual_store_with_hooks(
        paths,
        &controller,
        &legacy,
        None,
        || Ok(()),
        final_unlink_window,
        || Ok(()),
    )
}

#[cfg(test)]
fn open_controller_store_with_after_dual_archive_hook<F>(
    paths: &ControllerStatePaths,
    after_archive: F,
) -> Result<Store, ControllerStateError>
where
    F: FnOnce() -> Result<(), ControllerStateError>,
{
    let controller = open_controller_directory(paths)?;
    cleanup_migration_temps(&controller, paths.expected_uid)?;
    let legacy = open_legacy_state(paths)?.ok_or_else(|| {
        ControllerStateError::Migration("dual-state test has no legacy database".to_owned())
    })?;
    resolve_dual_store_with_hooks(
        paths,
        &controller,
        &legacy,
        None,
        || Ok(()),
        || Ok(()),
        after_archive,
    )
}

fn make_snapshot(
    paths: &ControllerStatePaths,
    controller: &ControllerDirectory,
    source: &PrivateDatabase,
    sidecars: &BTreeMap<String, PrivateDatabase>,
    fault: Option<MigrationFault>,
) -> Result<String, ControllerStateError> {
    make_snapshot_with_after_stage_hook(paths, controller, source, sidecars, fault, |_| Ok(()))
}

fn make_snapshot_with_after_stage_hook<F>(
    paths: &ControllerStatePaths,
    controller: &ControllerDirectory,
    source: &PrivateDatabase,
    sidecars: &BTreeMap<String, PrivateDatabase>,
    fault: Option<MigrationFault>,
    after_stage: F,
) -> Result<String, ControllerStateError>
where
    F: FnOnce(&str) -> Result<(), ControllerStateError>,
{
    make_snapshot_with_hooks(
        paths,
        controller,
        source,
        sidecars,
        fault,
        after_stage,
        |_| Ok(()),
    )
}

fn make_snapshot_with_hooks<F, G>(
    paths: &ControllerStatePaths,
    controller: &ControllerDirectory,
    source: &PrivateDatabase,
    sidecars: &BTreeMap<String, PrivateDatabase>,
    fault: Option<MigrationFault>,
    after_stage: F,
    after_consumption: G,
) -> Result<String, ControllerStateError>
where
    F: FnOnce(&str) -> Result<(), ControllerStateError>,
    G: FnOnce(&str) -> Result<(), ControllerStateError>,
{
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
    let staged = open_named_private_database(
        &controller.descriptor,
        &staged_name,
        paths.expected_uid,
        false,
        "staged controller database",
    )?;
    let staged_sidecars =
        open_private_sidecars(&controller.descriptor, &staged_name, paths.expected_uid)?;
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
    let staged_monitor =
        DatabaseMutationMonitor::new_for_snapshot_input(controller, &staged, &staged_sidecars)?;
    after_stage(&staged_name)?;
    staged_monitor.ensure_unchanged()?;
    validate_database_family_binding(paths, controller, &staged, &staged_sidecars, &staged_name)?;
    let monitor = DatabaseMutationMonitor::new_for_controller_files(controller, &[&snapshot])?;
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

    after_consumption(&staged_name)?;
    ensure_snapshot_monitor_safe(
        &staged_monitor,
        controller,
        &staged,
        &staged_sidecars,
        &staged_name,
        paths,
        false,
    )?;
    drop(destination_connection);
    drop(source_connection);
    ensure_snapshot_monitor_safe(
        &staged_monitor,
        controller,
        &staged,
        &staged_sidecars,
        &staged_name,
        paths,
        true,
    )?;
    monitor.ensure_unchanged()?;
    validate_named_database_binding(paths, controller, &staged, &staged_name)?;
    validate_consumed_database_family_binding(
        paths,
        controller,
        &staged,
        &staged_sidecars,
        &staged_name,
    )?;
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
) -> Result<LegacyArchiveTransaction, ControllerStateError> {
    archive_legacy_state_with_guard(
        paths,
        controller,
        legacy,
        fault,
        || Ok(()),
        || Ok(()),
        |_| Ok(()),
    )
}

fn archive_legacy_state_with_guard<F, G, H, T>(
    paths: &ControllerStatePaths,
    controller: &ControllerDirectory,
    legacy: &LegacyState,
    fault: Option<MigrationFault>,
    final_unlink_window: F,
    arm_durable_guard: G,
    validate_durable_guard: H,
) -> Result<LegacyArchiveTransaction, ControllerStateError>
where
    F: FnOnce() -> Result<(), ControllerStateError>,
    G: FnOnce() -> Result<T, ControllerStateError>,
    H: FnOnce(&T) -> Result<(), ControllerStateError>,
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
    let durable_guard = arm_durable_guard()?;
    final_unlink_window()?;
    let runtime = legacy.directories.last().ok_or_else(|| {
        ControllerStateError::Invalid("legacy database has no parent descriptor".to_owned())
    })?;
    // Keep the same-filesystem originals in a private namespace until the
    // durable family has been revalidated. This makes the active legacy names
    // recoverable without copying if the destructive decision becomes stale.
    let (_quarantine_name, quarantine) = create_legacy_archive_quarantine(runtime)?;
    let identities = legacy_archive_identities(legacy);
    write_archive_prepared_marker(&quarantine, &identities)?;
    rustix::fs::fsync(runtime)
        .map_err(|error| migration_fs_error("syncing prepared archive transaction", error))?;
    let names = std::iter::once(DATABASE_NAME.to_owned())
        .chain(
            legacy
                .sidecars
                .keys()
                .map(|suffix| format!("{DATABASE_NAME}{suffix}")),
        )
        .collect::<Vec<_>>();
    let mut moved = Vec::with_capacity(names.len());
    for name in &names {
        if let Err(error) = rustix::fs::renameat(runtime, name, &quarantine, name) {
            restore_legacy_archive_quarantine(runtime, &quarantine, &moved)?;
            return Err(migration_fs_error(
                "quarantining archived legacy state",
                error,
            ));
        }
        moved.push(name.clone());
        if name == DATABASE_NAME && fault == Some(MigrationFault::DuringLegacyArchive) {
            rustix::fs::fsync(&legacy.database.descriptor)
                .map_err(|error| migration_fs_error("syncing interrupted legacy archive", error))?;
            rustix::fs::fsync(&quarantine).map_err(|error| {
                migration_fs_error("syncing interrupted archive quarantine", error)
            })?;
            rustix::fs::fsync(runtime).map_err(|error| {
                migration_fs_error("syncing the legacy runtime directory", error)
            })?;
            return Err(injected_fault(MigrationFault::DuringLegacyArchive));
        }
    }
    rustix::fs::fsync(&legacy.database.descriptor)
        .map_err(|error| migration_fs_error("syncing quarantined legacy database", error))?;
    for sidecar in legacy.sidecars.values() {
        rustix::fs::fsync(&sidecar.descriptor)
            .map_err(|error| migration_fs_error("syncing quarantined legacy sidecar", error))?;
    }
    rustix::fs::fsync(&quarantine)
        .map_err(|error| migration_fs_error("syncing legacy archive quarantine", error))?;
    rustix::fs::fsync(runtime)
        .map_err(|error| migration_fs_error("syncing the legacy runtime directory", error))?;
    if fault == Some(MigrationFault::AfterLegacyMoveBeforeValidation) {
        return Err(injected_fault(
            MigrationFault::AfterLegacyMoveBeforeValidation,
        ));
    }
    let transaction = LegacyArchiveTransaction {
        runtime: rustix::io::dup(runtime)
            .map_err(|error| unsafe_error("retaining legacy runtime directory", error))?,
        quarantine,
        identities,
        moved,
        expected_uid: paths.expected_uid,
    };
    if let Err(error) = validate_durable_guard(&durable_guard) {
        transaction.restore()?;
        return Err(error);
    }
    rustix::fs::fsync(&controller.descriptor)
        .map_err(|error| migration_fs_error("syncing the durable archive directory", error))?;
    Ok(transaction)
}

fn create_legacy_archive_quarantine(
    directory: &OwnedFd,
) -> Result<(String, OwnedFd), ControllerStateError> {
    create_private_quarantine(
        directory,
        ARCHIVE_QUARANTINE_PREFIX,
        "legacy archive quarantine",
    )
}

fn create_private_quarantine(
    directory: &OwnedFd,
    prefix: &str,
    label: &str,
) -> Result<(String, OwnedFd), ControllerStateError> {
    for _ in 0..128 {
        let mut random = [0_u8; 16];
        getrandom::fill(&mut random).map_err(|error| {
            ControllerStateError::Unsafe(format!(
                "generating a legacy archive quarantine name: {error}"
            ))
        })?;
        let token = random
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let name = format!("{prefix}{token}");
        match rustix::fs::mkdirat(directory, &name, Mode::from_raw_mode(DIRECTORY_MODE as u16)) {
            Ok(()) => {
                let quarantine =
                    open_existing_child_directory(directory, OsStr::new(&name), label)?;
                return Ok((name, quarantine));
            }
            Err(error) if error == rustix::io::Errno::EXIST => continue,
            Err(error) => {
                return Err(unsafe_error(&format!("creating a {label}"), error));
            }
        }
    }
    Err(ControllerStateError::Unsafe(format!(
        "no collision-free {label} name is available"
    )))
}

fn legacy_archive_identities(legacy: &LegacyState) -> BTreeMap<String, DatabaseIdentity> {
    std::iter::once((DATABASE_NAME.to_owned(), legacy.database.identity))
        .chain(
            legacy
                .sidecars
                .iter()
                .map(|(suffix, sidecar)| (format!("{DATABASE_NAME}{suffix}"), sidecar.identity)),
        )
        .collect()
}

fn write_archive_prepared_marker(
    quarantine: &OwnedFd,
    identities: &BTreeMap<String, DatabaseIdentity>,
) -> Result<(), ControllerStateError> {
    let descriptor = create_private_file(
        quarantine,
        ARCHIVE_PREPARED_MARKER,
        "legacy archive transaction marker",
    )?;
    let mut marker = File::from(descriptor);
    writeln!(marker, "{ARCHIVE_MARKER_HEADER}")
        .map_err(|error| ControllerStateError::Migration(error.to_string()))?;
    for (name, identity) in identities {
        writeln!(marker, "{name}\t{}\t{}", identity.device, identity.inode)
            .map_err(|error| ControllerStateError::Migration(error.to_string()))?;
    }
    marker
        .sync_all()
        .map_err(|error| ControllerStateError::Migration(error.to_string()))?;
    rustix::fs::fsync(quarantine)
        .map_err(|error| migration_fs_error("syncing the archive transaction marker", error))?;
    Ok(())
}

struct LegacyArchiveTransaction {
    runtime: OwnedFd,
    quarantine: OwnedFd,
    identities: BTreeMap<String, DatabaseIdentity>,
    moved: Vec<String>,
    expected_uid: u32,
}

impl LegacyArchiveTransaction {
    fn restore(self) -> Result<(), ControllerStateError> {
        validate_live_archive_transaction_layout(&self)?;
        validate_archive_files(
            &self.quarantine,
            &self.identities,
            self.moved.iter().map(String::as_str),
            self.expected_uid,
        )?;
        restore_legacy_archive_quarantine(&self.runtime, &self.quarantine, &self.moved)?;
        rustix::fs::fsync(&self.quarantine)
            .map_err(|error| migration_fs_error("syncing restored archive quarantine", error))?;
        rustix::fs::fsync(&self.runtime)
            .map_err(|error| migration_fs_error("syncing restored legacy state", error))?;
        transition_archive_marker(
            &self.quarantine,
            ARCHIVE_PREPARED_MARKER,
            ARCHIVE_RESTORED_MARKER,
        )?;
        rustix::fs::fsync(&self.quarantine)
            .map_err(|error| migration_fs_error("syncing restored archive marker", error))?;
        rustix::fs::fsync(&self.runtime)
            .map_err(|error| migration_fs_error("syncing restored archive transaction", error))?;
        Ok(())
    }

    fn commit(self) -> Result<(), ControllerStateError> {
        validate_live_archive_transaction_layout(&self)?;
        validate_archive_files(
            &self.quarantine,
            &self.identities,
            self.moved.iter().map(String::as_str),
            self.expected_uid,
        )?;
        transition_archive_marker(
            &self.quarantine,
            ARCHIVE_PREPARED_MARKER,
            ARCHIVE_COMMITTED_MARKER,
        )?;
        rustix::fs::fsync(&self.quarantine)
            .map_err(|error| migration_fs_error("syncing committed archive marker", error))?;
        rustix::fs::fsync(&self.runtime)
            .map_err(|error| migration_fs_error("syncing committed archive transaction", error))?;
        Ok(())
    }
}

fn validate_live_archive_transaction_layout(
    transaction: &LegacyArchiveTransaction,
) -> Result<(), ControllerStateError> {
    let mut expected = transaction.moved.iter().cloned().collect::<BTreeSet<_>>();
    expected.insert(ARCHIVE_PREPARED_MARKER.to_owned());
    if archive_quarantine_entries(&transaction.quarantine)? != expected {
        return Err(ControllerStateError::Unsafe(
            "legacy archive transaction layout changed".to_owned(),
        ));
    }
    Ok(())
}

fn transition_archive_marker(
    quarantine: &OwnedFd,
    from: &str,
    to: &str,
) -> Result<(), ControllerStateError> {
    if entry_exists(quarantine, to)? {
        return Err(ControllerStateError::Unsafe(
            "legacy archive transaction has ambiguous phase markers".to_owned(),
        ));
    }
    rustix::fs::renameat(quarantine, from, quarantine, to)
        .map_err(|error| unsafe_error("transitioning legacy archive transaction", error))
}

fn validate_archive_files<'a>(
    quarantine: &OwnedFd,
    identities: &BTreeMap<String, DatabaseIdentity>,
    names: impl Iterator<Item = &'a str>,
    expected_uid: u32,
) -> Result<(), ControllerStateError> {
    for name in names {
        let expected = identities.get(name).ok_or_else(|| {
            ControllerStateError::Unsafe(
                "legacy archive transaction contains an unexpected file".to_owned(),
            )
        })?;
        let stat = rustix::fs::statat(quarantine, name, AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|error| unsafe_error("legacy archive transaction file", error))?;
        if validate_private_file_stat(&stat, expected_uid, "legacy archive transaction file")?
            != *expected
        {
            return Err(ControllerStateError::Unsafe(
                "legacy archive transaction file identity changed".to_owned(),
            ));
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ArchiveTransactionPhase {
    Prepared,
    Committed,
    Restored,
}

struct RecoveredArchiveTransaction {
    quarantine: OwnedFd,
    phase: ArchiveTransactionPhase,
    quarantined_names: BTreeSet<String>,
}

fn recover_legacy_archive_transactions(
    paths: &ControllerStatePaths,
) -> Result<(), ControllerStateError> {
    let parent = paths.legacy_database().parent().ok_or_else(|| {
        ControllerStateError::Invalid("legacy database has no parent directory".to_owned())
    })?;
    let Some(directories) = open_existing_directory_chain(parent)? else {
        return Ok(());
    };
    let runtime = directories.last().ok_or_else(|| {
        ControllerStateError::Invalid("legacy database has no parent descriptor".to_owned())
    })?;
    validate_directory(
        runtime,
        paths.expected_uid,
        true,
        "legacy runtime directory",
    )?;
    let mut transactions = Vec::new();
    let mut directory = rustix::fs::Dir::read_from(runtime)
        .map_err(|error| unsafe_error("legacy runtime directory", error))?;
    for entry in &mut directory {
        let entry = entry.map_err(|error| unsafe_error("legacy runtime directory entry", error))?;
        let name = OsStr::from_bytes(entry.file_name().to_bytes());
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.starts_with(ARCHIVE_QUARANTINE_PREFIX) {
            continue;
        }
        if !exact_archive_quarantine_name(name) {
            return Err(ControllerStateError::Unsafe(
                "legacy archive quarantine has a malformed name".to_owned(),
            ));
        }
        transactions.push(open_recovered_archive_transaction(
            runtime,
            name,
            paths.expected_uid,
        )?);
    }
    if transactions
        .iter()
        .filter(|transaction| transaction.phase == ArchiveTransactionPhase::Prepared)
        .count()
        > 1
    {
        return Err(ControllerStateError::Unsafe(
            "multiple prepared legacy archive transactions are ambiguous".to_owned(),
        ));
    }
    if let Some(transaction) = transactions
        .into_iter()
        .find(|transaction| transaction.phase == ArchiveTransactionPhase::Prepared)
    {
        restore_recovered_archive_transaction(runtime, transaction)?;
    }
    Ok(())
}

fn exact_archive_quarantine_name(name: &str) -> bool {
    name.strip_prefix(ARCHIVE_QUARANTINE_PREFIX)
        .is_some_and(|token| {
            token.len() == 32
                && token
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
}

fn open_recovered_archive_transaction(
    runtime: &OwnedFd,
    name: &str,
    expected_uid: u32,
) -> Result<RecoveredArchiveTransaction, ControllerStateError> {
    let quarantine =
        open_existing_child_directory(runtime, OsStr::new(name), "legacy archive quarantine")?;
    validate_directory(&quarantine, expected_uid, true, "legacy archive quarantine")?;
    let phases = [
        (ARCHIVE_PREPARED_MARKER, ArchiveTransactionPhase::Prepared),
        (ARCHIVE_COMMITTED_MARKER, ArchiveTransactionPhase::Committed),
        (ARCHIVE_RESTORED_MARKER, ArchiveTransactionPhase::Restored),
    ]
    .into_iter()
    .filter_map(|(marker, phase)| match entry_exists(&quarantine, marker) {
        Ok(true) => Some(Ok((marker, phase))),
        Ok(false) => None,
        Err(error) => Some(Err(error)),
    })
    .collect::<Result<Vec<_>, ControllerStateError>>()?;
    if phases.len() != 1 {
        return Err(ControllerStateError::Unsafe(
            "legacy archive transaction must have exactly one phase marker".to_owned(),
        ));
    }
    let (marker_name, phase) = phases[0];
    let marker = open_named_private_database(
        &quarantine,
        marker_name,
        expected_uid,
        false,
        "legacy archive transaction marker",
    )?;
    let identities = read_archive_marker(&marker)?;
    let mut entries = archive_quarantine_entries(&quarantine)?;
    if !entries.remove(marker_name) {
        return Err(ControllerStateError::Unsafe(
            "legacy archive transaction marker disappeared".to_owned(),
        ));
    }
    if entries.iter().any(|name| !identities.contains_key(name)) {
        return Err(ControllerStateError::Unsafe(
            "legacy archive quarantine contains unexpected entries".to_owned(),
        ));
    }

    match phase {
        ArchiveTransactionPhase::Committed => {
            if entries.len() != identities.len() {
                return Err(ControllerStateError::Unsafe(
                    "committed legacy archive transaction is incomplete".to_owned(),
                ));
            }
            validate_archive_files(
                &quarantine,
                &identities,
                entries.iter().map(String::as_str),
                expected_uid,
            )?;
        }
        ArchiveTransactionPhase::Restored => {
            if !entries.is_empty() {
                return Err(ControllerStateError::Unsafe(
                    "restored legacy archive transaction is not empty".to_owned(),
                ));
            }
        }
        ArchiveTransactionPhase::Prepared => {
            validate_prepared_archive_layout(
                runtime,
                &quarantine,
                &identities,
                &entries,
                expected_uid,
            )?;
        }
    }
    Ok(RecoveredArchiveTransaction {
        quarantine,
        phase,
        quarantined_names: entries,
    })
}

fn read_archive_marker(
    marker: &PrivateDatabase,
) -> Result<BTreeMap<String, DatabaseIdentity>, ControllerStateError> {
    let stat = rustix::fs::fstat(&marker.descriptor)
        .map_err(|error| unsafe_error("legacy archive transaction marker", error))?;
    if stat.st_size > 4096 {
        return Err(ControllerStateError::Unsafe(
            "legacy archive transaction marker is too large".to_owned(),
        ));
    }
    let descriptor = rustix::io::dup(&marker.descriptor)
        .map_err(|error| unsafe_error("reading legacy archive transaction marker", error))?;
    let mut contents = String::new();
    File::from(descriptor)
        .read_to_string(&mut contents)
        .map_err(|error| ControllerStateError::Unsafe(error.to_string()))?;
    if !contents.ends_with('\n') {
        return Err(ControllerStateError::Unsafe(
            "legacy archive transaction marker is truncated".to_owned(),
        ));
    }
    let mut lines = contents.lines();
    if lines.next() != Some(ARCHIVE_MARKER_HEADER) {
        return Err(ControllerStateError::Unsafe(
            "legacy archive transaction marker has an unknown version".to_owned(),
        ));
    }
    let mut identities = BTreeMap::new();
    for line in lines {
        let fields = line.split('\t').collect::<Vec<_>>();
        if fields.len() != 3 || !exact_database_family_name(fields[0]) {
            return Err(ControllerStateError::Unsafe(
                "legacy archive transaction marker is malformed".to_owned(),
            ));
        }
        let identity = DatabaseIdentity {
            device: fields[1].parse().map_err(|_| {
                ControllerStateError::Unsafe(
                    "legacy archive transaction marker is malformed".to_owned(),
                )
            })?,
            inode: fields[2].parse().map_err(|_| {
                ControllerStateError::Unsafe(
                    "legacy archive transaction marker is malformed".to_owned(),
                )
            })?,
        };
        if identities.insert(fields[0].to_owned(), identity).is_some() {
            return Err(ControllerStateError::Unsafe(
                "legacy archive transaction marker contains duplicate files".to_owned(),
            ));
        }
    }
    if !identities.contains_key(DATABASE_NAME) {
        return Err(ControllerStateError::Unsafe(
            "legacy archive transaction marker omits the database".to_owned(),
        ));
    }
    Ok(identities)
}

fn exact_database_family_name(name: &str) -> bool {
    name == DATABASE_NAME
        || SQLITE_SIDECAR_SUFFIXES
            .iter()
            .any(|suffix| name == format!("{DATABASE_NAME}{suffix}"))
}

fn archive_quarantine_entries(
    quarantine: &OwnedFd,
) -> Result<BTreeSet<String>, ControllerStateError> {
    let mut directory = rustix::fs::Dir::read_from(quarantine)
        .map_err(|error| unsafe_error("legacy archive quarantine", error))?;
    let mut entries = BTreeSet::new();
    for entry in &mut directory {
        let entry =
            entry.map_err(|error| unsafe_error("legacy archive quarantine entry", error))?;
        let name = OsStr::from_bytes(entry.file_name().to_bytes())
            .to_str()
            .ok_or_else(|| {
                ControllerStateError::Unsafe(
                    "legacy archive quarantine contains a non-UTF-8 entry".to_owned(),
                )
            })?
            .to_owned();
        if name == "." || name == ".." {
            continue;
        }
        entries.insert(name);
    }
    Ok(entries)
}

fn validate_prepared_archive_layout(
    runtime: &OwnedFd,
    quarantine: &OwnedFd,
    identities: &BTreeMap<String, DatabaseIdentity>,
    quarantined_names: &BTreeSet<String>,
    expected_uid: u32,
) -> Result<(), ControllerStateError> {
    for (name, expected) in identities {
        let active = entry_exists(runtime, name)?;
        let quarantined = quarantined_names.contains(name);
        if active == quarantined {
            return Err(ControllerStateError::Unsafe(
                "prepared legacy archive transaction has an ambiguous file layout".to_owned(),
            ));
        }
        let directory = if active { runtime } else { quarantine };
        let stat = rustix::fs::statat(directory, name, AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|error| unsafe_error("prepared legacy archive transaction file", error))?;
        if validate_private_file_stat(
            &stat,
            expected_uid,
            "prepared legacy archive transaction file",
        )? != *expected
        {
            return Err(ControllerStateError::Unsafe(
                "prepared legacy archive transaction file identity changed".to_owned(),
            ));
        }
    }
    for suffix in SQLITE_SIDECAR_SUFFIXES {
        let name = format!("{DATABASE_NAME}{suffix}");
        if !identities.contains_key(&name) && entry_exists(runtime, &name)? {
            return Err(ControllerStateError::Unsafe(
                "prepared legacy archive transaction has an unexpected active sidecar".to_owned(),
            ));
        }
    }
    Ok(())
}

fn restore_recovered_archive_transaction(
    runtime: &OwnedFd,
    transaction: RecoveredArchiveTransaction,
) -> Result<(), ControllerStateError> {
    for name in transaction
        .quarantined_names
        .iter()
        .filter(|name| name.as_str() != DATABASE_NAME)
        .rev()
    {
        rustix::fs::renameat(&transaction.quarantine, name, runtime, name)
            .map_err(|error| unsafe_error("recovering a legacy SQLite sidecar", error))?;
    }
    if transaction.quarantined_names.contains(DATABASE_NAME) {
        rustix::fs::renameat(
            &transaction.quarantine,
            DATABASE_NAME,
            runtime,
            DATABASE_NAME,
        )
        .map_err(|error| unsafe_error("recovering the legacy database", error))?;
    }
    rustix::fs::fsync(&transaction.quarantine)
        .map_err(|error| migration_fs_error("syncing recovered archive quarantine", error))?;
    rustix::fs::fsync(runtime)
        .map_err(|error| migration_fs_error("syncing recovered legacy state", error))?;
    transition_archive_marker(
        &transaction.quarantine,
        ARCHIVE_PREPARED_MARKER,
        ARCHIVE_RESTORED_MARKER,
    )?;
    rustix::fs::fsync(&transaction.quarantine)
        .map_err(|error| migration_fs_error("syncing recovered archive marker", error))?;
    rustix::fs::fsync(runtime)
        .map_err(|error| migration_fs_error("syncing recovered archive transaction", error))?;
    Ok(())
}

fn restore_legacy_archive_quarantine(
    runtime: &OwnedFd,
    quarantine: &OwnedFd,
    moved: &[String],
) -> Result<(), ControllerStateError> {
    rustix::fs::fchmod(quarantine, Mode::from_raw_mode(DIRECTORY_MODE as u16))
        .map_err(|error| unsafe_error("unlocking legacy archive quarantine", error))?;
    for name in moved.iter().skip(1).rev() {
        rustix::fs::renameat(quarantine, name, runtime, name)
            .map_err(|error| unsafe_error("restoring a legacy SQLite sidecar", error))?;
    }
    if let Some(database) = moved.first() {
        rustix::fs::renameat(quarantine, database, runtime, database)
            .map_err(|error| unsafe_error("restoring the legacy database", error))?;
    }
    Ok(())
}

fn archive_legacy_orphans(
    paths: &ControllerStatePaths,
    controller: &ControllerDirectory,
    orphans: &LegacyOrphans,
) -> Result<(), ControllerStateError> {
    archive_legacy_orphans_with_hooks(paths, controller, orphans, |_| Ok(()), |_, _| Ok(()))
}

fn archive_legacy_orphans_with_hooks<F, G>(
    paths: &ControllerStatePaths,
    controller: &ControllerDirectory,
    orphans: &LegacyOrphans,
    mut final_unlink_window: F,
    mut before_quarantine_move: G,
) -> Result<(), ControllerStateError>
where
    F: FnMut(&str) -> Result<(), ControllerStateError>,
    G: FnMut(&OwnedFd, &str) -> Result<(), ControllerStateError>,
{
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
    let (_quarantine_name, quarantine) = create_private_quarantine(
        runtime,
        ORPHAN_QUARANTINE_PREFIX,
        "orphan archive quarantine",
    )?;
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
        final_unlink_window(&name)?;
        before_quarantine_move(&quarantine, &name)?;
        rustix::fs::renameat_with(
            runtime,
            &name,
            &quarantine,
            &name,
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .map_err(|error| unsafe_error("quarantining an orphaned SQLite sidecar", error))?;
        let quarantined_stat = rustix::fs::statat(&quarantine, &name, AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|error| unsafe_error("quarantined orphaned SQLite sidecar", error))?;
        let quarantined_identity = validate_private_file_stat(
            &quarantined_stat,
            paths.expected_uid,
            "quarantined orphaned SQLite sidecar",
        )?;
        if quarantined_identity != sidecar.identity {
            rustix::fs::linkat(&quarantine, &name, runtime, &name, AtFlags::empty()).map_err(
                |error| unsafe_error("restoring a substituted orphaned SQLite sidecar", error),
            )?;
            rustix::fs::fchmod(&quarantine, Mode::from_raw_mode(0o100))
                .map_err(|error| unsafe_error("locking orphan archive quarantine", error))?;
            rustix::fs::fsync(&quarantine).map_err(|error| {
                migration_fs_error("syncing substituted orphan archive quarantine", error)
            })?;
            rustix::fs::fsync(runtime).map_err(|error| {
                migration_fs_error("syncing restored orphaned SQLite sidecar", error)
            })?;
            return Err(ControllerStateError::Unsafe(
                "orphaned legacy SQLite sidecar changed during archival".to_owned(),
            ));
        }
    }
    rustix::fs::fchmod(&quarantine, Mode::from_raw_mode(0o100))
        .map_err(|error| unsafe_error("locking retained orphan archive quarantine", error))?;
    rustix::fs::fsync(&quarantine)
        .map_err(|error| migration_fs_error("syncing orphan archive quarantine", error))?;
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

fn validate_retained_private_file_stat(
    stat: &rustix::fs::Stat,
    expected_uid: u32,
    label: &str,
) -> Result<DatabaseIdentity, ControllerStateError> {
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
        || stat.st_uid != expected_uid
        || stat.st_nlink > 1
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
    cleanup_migration_temps_with_hooks(
        controller,
        expected_uid,
        |_| Ok(()),
        |_| Ok(()),
        |_| Ok(()),
        |_| Ok(()),
    )
}

#[cfg(test)]
fn cleanup_migration_temps_with_hook<F>(
    controller: &ControllerDirectory,
    expected_uid: u32,
    before_unlink: F,
) -> Result<(), ControllerStateError>
where
    F: FnMut(&str) -> Result<(), ControllerStateError>,
{
    cleanup_migration_temps_with_hooks(
        controller,
        expected_uid,
        before_unlink,
        |_| Ok(()),
        |_| Ok(()),
        |_| Ok(()),
    )
}

fn cleanup_migration_temps_with_hooks<F, G, H, I>(
    controller: &ControllerDirectory,
    expected_uid: u32,
    mut before_unlink: F,
    mut before_quarantine_create: G,
    mut after_identity_check: H,
    mut after_final_validation: I,
) -> Result<(), ControllerStateError>
where
    F: FnMut(&str) -> Result<(), ControllerStateError>,
    G: FnMut(&str) -> Result<(), ControllerStateError>,
    H: FnMut(&str) -> Result<(), ControllerStateError>,
    I: FnMut(&str) -> Result<(), ControllerStateError>,
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
        let (quarantine, quarantine_directory) =
            create_cleanup_quarantine(&controller.descriptor, &mut before_quarantine_create)?;
        rustix::fs::renameat(
            &controller.descriptor,
            &name,
            &quarantine_directory,
            "candidate",
        )
        .map_err(|error| unsafe_error("quarantining a migration temporary file", error))?;
        let quarantined_stat = rustix::fs::statat(
            &quarantine_directory,
            "candidate",
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
                restore_quarantined_file(&controller.descriptor, &quarantine_directory, &name)?;
                return Err(error);
            }
        };
        if quarantined_identity != candidate.identity {
            restore_quarantined_file(&controller.descriptor, &quarantine_directory, &name)?;
            return Err(ControllerStateError::Unsafe(
                "migration temporary file changed during cleanup".to_owned(),
            ));
        }
        rustix::fs::fchmod(&quarantine_directory, Mode::from_raw_mode(0o100))
            .map_err(|error| unsafe_error("locking cleanup quarantine", error))?;
        after_identity_check(&quarantine)?;
        rustix::fs::fchmod(&quarantine_directory, Mode::from_raw_mode(0o100))
            .map_err(|error| unsafe_error("relocking cleanup quarantine", error))?;
        let final_stat = rustix::fs::statat(
            &quarantine_directory,
            "candidate",
            AtFlags::SYMLINK_NOFOLLOW,
        )
        .map_err(|error| unsafe_error("quarantined migration temporary file", error))?;
        let final_identity = match validate_private_file_stat(
            &final_stat,
            expected_uid,
            "quarantined migration temporary file",
        ) {
            Ok(identity) => identity,
            Err(error) => {
                restore_quarantined_file(&controller.descriptor, &quarantine_directory, &name)?;
                return Err(error);
            }
        };
        if final_identity != candidate.identity {
            restore_quarantined_file(&controller.descriptor, &quarantine_directory, &name)?;
            return Err(ControllerStateError::Unsafe(
                "migration temporary file changed after quarantine validation".to_owned(),
            ));
        }
        after_final_validation(&quarantine)?;
        // macOS has no identity-bound unlink. Retaining this private namespace
        // ensures a later pathname substitution can never make cleanup delete
        // an inode that was not validated.
        rustix::fs::fchmod(&quarantine_directory, Mode::from_raw_mode(0o100))
            .map_err(|error| unsafe_error("locking retained cleanup quarantine", error))?;
    }
    rustix::fs::fsync(&controller.descriptor)
        .map_err(|error| migration_fs_error("syncing migration cleanup", error))?;
    Ok(())
}

fn create_cleanup_quarantine<F>(
    directory: &OwnedFd,
    before_create: &mut F,
) -> Result<(String, OwnedFd), ControllerStateError>
where
    F: FnMut(&str) -> Result<(), ControllerStateError>,
{
    for _ in 0..128 {
        let mut random = [0_u8; 16];
        getrandom::fill(&mut random).map_err(|error| {
            ControllerStateError::Unsafe(format!("generating a cleanup quarantine name: {error}"))
        })?;
        let token = random
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let name = format!(".state.sqlite3.cleanup-quarantine-{token}");
        before_create(&name)?;
        match rustix::fs::mkdirat(directory, &name, Mode::from_raw_mode(DIRECTORY_MODE as u16)) {
            Ok(()) => {
                let quarantine = open_existing_child_directory(
                    directory,
                    OsStr::new(&name),
                    "cleanup quarantine",
                )?;
                rustix::fs::fchmod(&quarantine, Mode::from_raw_mode(DIRECTORY_MODE as u16))
                    .map_err(|error| unsafe_error("cleanup quarantine", error))?;
                return Ok((name, quarantine));
            }
            Err(error) if error == rustix::io::Errno::EXIST => continue,
            Err(error) => return Err(unsafe_error("creating a cleanup quarantine", error)),
        }
    }
    Err(ControllerStateError::Unsafe(
        "no collision-free cleanup quarantine name is available".to_owned(),
    ))
}

fn restore_quarantined_file(
    directory: &OwnedFd,
    quarantine_directory: &OwnedFd,
    original: &str,
) -> Result<(), ControllerStateError> {
    rustix::fs::fchmod(
        quarantine_directory,
        Mode::from_raw_mode(DIRECTORY_MODE as u16),
    )
    .map_err(|error| unsafe_error("unlocking cleanup quarantine", error))?;
    rustix::fs::linkat(
        quarantine_directory,
        "candidate",
        directory,
        original,
        AtFlags::empty(),
    )
    .map_err(|error| unsafe_error("restoring a substituted migration temporary file", error))?;
    rustix::fs::fchmod(quarantine_directory, Mode::from_raw_mode(0o100))
        .map_err(|error| unsafe_error("locking retained cleanup quarantine", error))?;
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

fn validate_database_family_binding(
    paths: &ControllerStatePaths,
    directory: &ControllerDirectory,
    database: &PrivateDatabase,
    sidecars: &BTreeMap<String, PrivateDatabase>,
    database_name: &str,
) -> Result<(), ControllerStateError> {
    validate_named_database_binding(paths, directory, database, database_name)?;
    let current_sidecars =
        open_private_sidecars(&directory.descriptor, database_name, paths.expected_uid)?;
    if current_sidecars.keys().ne(sidecars.keys()) {
        return Err(ControllerStateError::Unsafe(
            "controller database sidecar set changed during migration".to_owned(),
        ));
    }
    for (suffix, sidecar) in sidecars {
        validate_named_database_binding(
            paths,
            directory,
            sidecar,
            &format!("{database_name}{suffix}"),
        )?;
    }
    Ok(())
}

fn validate_consumed_database_family_binding(
    paths: &ControllerStatePaths,
    directory: &ControllerDirectory,
    database: &PrivateDatabase,
    sidecars: &BTreeMap<String, PrivateDatabase>,
    database_name: &str,
) -> Result<(), ControllerStateError> {
    validate_named_database_binding(paths, directory, database, database_name)?;
    for sidecar in sidecars.values() {
        let stat = rustix::fs::fstat(&sidecar.descriptor)
            .map_err(|error| unsafe_error("retained staged SQLite sidecar", error))?;
        if validate_retained_private_file_stat(
            &stat,
            paths.expected_uid,
            "retained staged SQLite sidecar",
        )? != sidecar.identity
        {
            return Err(ControllerStateError::Unsafe(
                "retained staged SQLite sidecar identity changed".to_owned(),
            ));
        }
    }
    let current_sidecars =
        open_private_sidecars(&directory.descriptor, database_name, paths.expected_uid)?;
    for (suffix, current) in &current_sidecars {
        let retained = sidecars.get(suffix).ok_or_else(|| {
            ControllerStateError::Unsafe(
                "staged SQLite sidecar set changed during snapshot consumption".to_owned(),
            )
        })?;
        if current.identity != retained.identity {
            return Err(ControllerStateError::Unsafe(
                "staged SQLite sidecar identity changed during snapshot consumption".to_owned(),
            ));
        }
        validate_named_database_binding(
            paths,
            directory,
            current,
            &format!("{database_name}{suffix}"),
        )?;
    }
    Ok(())
}

fn ensure_snapshot_monitor_safe(
    monitor: &DatabaseMutationMonitor,
    directory: &ControllerDirectory,
    database: &PrivateDatabase,
    sidecars: &BTreeMap<String, PrivateDatabase>,
    database_name: &str,
    paths: &ControllerStatePaths,
    validate_binding: bool,
) -> Result<(), ControllerStateError> {
    let current_sidecars =
        open_private_sidecars(&directory.descriptor, database_name, paths.expected_uid)?;
    monitor.ensure_snapshot_consumption_safe(sidecars, &current_sidecars)?;
    if validate_binding {
        validate_consumed_database_family_binding(
            paths,
            directory,
            database,
            sidecars,
            database_name,
        )?;
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

    fn new_for_controller_family(
        directory: &ControllerDirectory,
        database: &PrivateDatabase,
        sidecars: &BTreeMap<String, PrivateDatabase>,
    ) -> Result<Self, ControllerStateError> {
        let mut descriptors = directory.descriptors().to_vec();
        descriptors.push(&database.descriptor);
        descriptors.extend(sidecars.values().map(|sidecar| &sidecar.descriptor));
        Self::from_descriptors_with_directory_write(&descriptors, &directory.descriptor)
    }

    fn new_for_controller_family_identity(
        directory: &ControllerDirectory,
        database: &PrivateDatabase,
        sidecars: &BTreeMap<String, PrivateDatabase>,
    ) -> Result<Self, ControllerStateError> {
        let mut descriptors = directory.descriptors().to_vec();
        descriptors.push(&database.descriptor);
        descriptors.extend(sidecars.values().map(|sidecar| &sidecar.descriptor));
        Self::from_descriptors(&descriptors)
    }

    fn new_for_snapshot_input(
        directory: &ControllerDirectory,
        database: &PrivateDatabase,
        sidecars: &BTreeMap<String, PrivateDatabase>,
    ) -> Result<Self, ControllerStateError> {
        let mut descriptors = directory.descriptors().to_vec();
        descriptors.push(&database.descriptor);
        descriptors.extend(sidecars.values().map(|sidecar| &sidecar.descriptor));
        let mut write_descriptors = vec![
            directory.descriptor.as_raw_fd(),
            database.descriptor.as_raw_fd(),
        ];
        write_descriptors.extend(
            sidecars
                .values()
                .map(|sidecar| sidecar.descriptor.as_raw_fd()),
        );
        Self::from_descriptors_with_write_events(&descriptors, &write_descriptors)
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
        Self::from_descriptors_with_optional_directory_write(descriptors, None)
    }

    fn from_descriptors_with_directory_write(
        descriptors: &[&OwnedFd],
        directory: &OwnedFd,
    ) -> Result<Self, ControllerStateError> {
        Self::from_descriptors_with_optional_directory_write(
            descriptors,
            Some(directory.as_raw_fd()),
        )
    }

    fn from_descriptors_with_optional_directory_write(
        descriptors: &[&OwnedFd],
        directory: Option<RawFd>,
    ) -> Result<Self, ControllerStateError> {
        let write_descriptors = directory.into_iter().collect::<Vec<_>>();
        Self::from_descriptors_with_write_events(descriptors, &write_descriptors)
    }

    fn from_descriptors_with_write_events(
        descriptors: &[&OwnedFd],
        write_descriptors: &[RawFd],
    ) -> Result<Self, ControllerStateError> {
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
            let events = if write_descriptors.contains(&descriptor.as_raw_fd()) {
                identity_events | FilterFlag::NOTE_WRITE
            } else {
                identity_events
            };
            changes.push(KEvent::new(
                descriptor.as_raw_fd() as usize,
                EventFilter::EVFILT_VNODE,
                EvFlags::EV_ADD | EvFlags::EV_ENABLE | EvFlags::EV_CLEAR,
                events,
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
        if !self.observed_events()?.is_empty() {
            return Err(ControllerStateError::Unsafe(
                "controller database identity changed while opening the store".to_owned(),
            ));
        }
        Ok(())
    }

    fn ensure_snapshot_consumption_safe(
        &self,
        sidecars: &BTreeMap<String, PrivateDatabase>,
        current_sidecars: &BTreeMap<String, PrivateDatabase>,
    ) -> Result<(), ControllerStateError> {
        let identity_events = FilterFlag::NOTE_DELETE
            | FilterFlag::NOTE_RENAME
            | FilterFlag::NOTE_LINK
            | FilterFlag::NOTE_REVOKE;
        for event in self.observed_events()? {
            let flags = event.fflags();
            if !flags.intersects(identity_events) {
                continue;
            }
            let missing_consumed_sidecar = sidecars.iter().any(|(suffix, sidecar)| {
                sidecar.descriptor.as_raw_fd() as usize == event.ident()
                    && !current_sidecars.contains_key(suffix)
            });
            if missing_consumed_sidecar && !flags.intersects(FilterFlag::NOTE_REVOKE) {
                continue;
            }
            return Err(ControllerStateError::Unsafe(format!(
                "staged controller database identity changed during snapshot consumption: descriptor {}, flags {:?}",
                event.ident(),
                flags
            )));
        }
        Ok(())
    }

    fn observed_events(&self) -> Result<Vec<KEvent>, ControllerStateError> {
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
        events.truncate(count);
        Ok(events)
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

    const fn new_for_controller_family(
        _directory: &ControllerDirectory,
        _database: &PrivateDatabase,
        _sidecars: &BTreeMap<String, PrivateDatabase>,
    ) -> Result<Self, ControllerStateError> {
        Ok(Self)
    }

    const fn new_for_controller_family_identity(
        _directory: &ControllerDirectory,
        _database: &PrivateDatabase,
        _sidecars: &BTreeMap<String, PrivateDatabase>,
    ) -> Result<Self, ControllerStateError> {
        Ok(Self)
    }

    const fn new_for_snapshot_input(
        _directory: &ControllerDirectory,
        _database: &PrivateDatabase,
        _sidecars: &BTreeMap<String, PrivateDatabase>,
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

    fn ensure_snapshot_consumption_safe(
        &self,
        _sidecars: &BTreeMap<String, PrivateDatabase>,
        _current_sidecars: &BTreeMap<String, PrivateDatabase>,
    ) -> Result<(), ControllerStateError> {
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
    use std::cell::{Cell, RefCell};
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

    fn seeded_dual_paths(root: &Path) -> Result<ControllerStatePaths, Box<dyn std::error::Error>> {
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
        Ok(paths)
    }

    fn seeded_legacy_paths(
        root: &Path,
    ) -> Result<ControllerStatePaths, Box<dyn std::error::Error>> {
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
        let _controller = open_controller_directory(&paths)?;
        seed_test_store(paths.legacy_database(), "legacy")?;
        Ok(paths)
    }

    fn assert_snapshot_post_consumption_aba_refused(
        suffix: Option<&str>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let temp = TempDir::new()?;
        let root = temp.path().canonicalize()?;
        let paths = seeded_legacy_paths(&root)?;
        let connection = if suffix == Some("-wal") {
            let connection = Connection::open(paths.legacy_database())?;
            connection.pragma_update(None, "journal_mode", "WAL")?;
            connection.pragma_update(None, "wal_autocheckpoint", 0)?;
            connection.execute("UPDATE sandboxes SET updated_at_millis = 9", [])?;
            for sidecar_suffix in ["-wal", "-shm"] {
                fs::set_permissions(
                    PathBuf::from(format!(
                        "{}{sidecar_suffix}",
                        paths.legacy_database().display()
                    )),
                    fs::Permissions::from_mode(0o600),
                )?;
            }
            Some(connection)
        } else if suffix == Some("-journal") {
            let connection = Connection::open(paths.legacy_database())?;
            connection.pragma_update(None, "journal_mode", "PERSIST")?;
            connection.execute("UPDATE sandboxes SET updated_at_millis = 9", [])?;
            let journal = PathBuf::from(format!("{}-journal", paths.legacy_database().display()));
            fs::set_permissions(&journal, fs::Permissions::from_mode(0o600))?;
            Some(connection)
        } else {
            None
        };
        let controller = open_existing_controller_directory(&paths)?;
        let legacy = open_legacy_state(&paths)?
            .ok_or_else(|| std::io::Error::other("legacy state disappeared"))?;
        let displaced = root.join(format!(
            "post-consumption-displaced{}",
            suffix.unwrap_or("-main")
        ));
        let aba_completed = Cell::new(false);

        let result = migrate_legacy_store_with_snapshot_hooks(
            &paths,
            &controller,
            &legacy,
            None,
            |_| Ok(()),
            |staged_name| {
                let target =
                    controller_path(&paths, &format!("{staged_name}{}", suffix.unwrap_or("")));
                fs::rename(&target, &displaced)
                    .map_err(|error| ControllerStateError::Unsafe(error.to_string()))?;
                fs::rename(&displaced, &target)
                    .map_err(|error| ControllerStateError::Unsafe(error.to_string()))?;
                aba_completed.set(true);
                Ok(())
            },
        );

        assert!(
            aba_completed.get(),
            "post-consumption ABA hook did not complete"
        );
        assert!(matches!(result, Err(ControllerStateError::Unsafe(_))));
        assert!(!paths.durable_database().exists());
        drop(connection);
        Ok(())
    }

    #[test]
    fn snapshot_monitor_refuses_post_consumption_main_aba() -> Result<(), Box<dyn std::error::Error>>
    {
        assert_snapshot_post_consumption_aba_refused(None)
    }

    #[test]
    fn snapshot_monitor_refuses_post_consumption_wal_aba() -> Result<(), Box<dyn std::error::Error>>
    {
        assert_snapshot_post_consumption_aba_refused(Some("-wal"))
    }

    #[test]
    fn snapshot_monitor_refuses_post_consumption_journal_aba()
    -> Result<(), Box<dyn std::error::Error>> {
        assert_snapshot_post_consumption_aba_refused(Some("-journal"))
    }

    #[test]
    fn snapshot_refuses_a_post_copy_staged_wal_replacement()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = TempDir::new()?;
        let root = temp.path().canonicalize()?;
        let paths = seeded_legacy_paths(&root)?;
        let connection = Connection::open(paths.legacy_database())?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "wal_autocheckpoint", 0)?;
        connection.execute("UPDATE sandboxes SET updated_at_millis = 8", [])?;
        for suffix in ["-wal", "-shm"] {
            fs::set_permissions(
                PathBuf::from(format!("{}{suffix}", paths.legacy_database().display())),
                fs::Permissions::from_mode(0o600),
            )?;
        }
        let controller = open_existing_controller_directory(&paths)?;
        let legacy = open_legacy_state(&paths)?
            .ok_or_else(|| std::io::Error::other("legacy state disappeared"))?;
        let displaced = root.join("displaced-staged-wal");

        let result = migrate_legacy_store_with_after_stage_hook(
            &paths,
            &controller,
            &legacy,
            None,
            |staged_name| {
                let staged_wal = controller_path(&paths, &format!("{staged_name}-wal"));
                fs::rename(&staged_wal, &displaced)
                    .map_err(|error| ControllerStateError::Unsafe(error.to_string()))?;
                fs::copy(&displaced, &staged_wal)
                    .map_err(|error| ControllerStateError::Unsafe(error.to_string()))?;
                fs::set_permissions(&staged_wal, fs::Permissions::from_mode(0o600))
                    .map_err(|error| ControllerStateError::Unsafe(error.to_string()))?;
                Ok(())
            },
        );

        assert!(matches!(result, Err(ControllerStateError::Unsafe(_))));
        assert!(!paths.durable_database().exists());
        drop(connection);
        Ok(())
    }

    #[test]
    fn snapshot_refuses_a_post_copy_staged_journal_mutation()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = TempDir::new()?;
        let root = temp.path().canonicalize()?;
        let paths = seeded_legacy_paths(&root)?;
        let journal = PathBuf::from(format!("{}-journal", paths.legacy_database().display()));
        fs::write(&journal, b"ignored legacy journal")?;
        fs::set_permissions(&journal, fs::Permissions::from_mode(0o600))?;
        let controller = open_existing_controller_directory(&paths)?;
        let legacy = open_legacy_state(&paths)?
            .ok_or_else(|| std::io::Error::other("legacy state disappeared"))?;

        let result = migrate_legacy_store_with_after_stage_hook(
            &paths,
            &controller,
            &legacy,
            None,
            |staged_name| {
                let staged_journal = controller_path(&paths, &format!("{staged_name}-journal"));
                fs::write(&staged_journal, b"mutated staged journal")
                    .map_err(|error| ControllerStateError::Unsafe(error.to_string()))?;
                fs::set_permissions(&staged_journal, fs::Permissions::from_mode(0o600))
                    .map_err(|error| ControllerStateError::Unsafe(error.to_string()))?;
                Ok(())
            },
        );

        assert!(matches!(result, Err(ControllerStateError::Unsafe(_))));
        assert!(!paths.durable_database().exists());
        Ok(())
    }

    #[test]
    fn orphan_archival_preserves_a_final_window_sidecar_substitution()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = TempDir::new()?;
        let root = temp.path().canonicalize()?;
        let paths = seeded_legacy_paths(&root)?;
        fs::remove_file(paths.legacy_database())?;
        let orphan = PathBuf::from(format!("{}-wal", paths.legacy_database().display()));
        fs::write(&orphan, b"original orphan")?;
        fs::set_permissions(&orphan, fs::Permissions::from_mode(0o600))?;
        let replacement = root.join("replacement-orphan");
        let displaced = root.join("displaced-orphan");
        fs::write(&replacement, b"replacement orphan")?;
        fs::set_permissions(&replacement, fs::Permissions::from_mode(0o600))?;
        let controller = open_existing_controller_directory(&paths)?;
        let orphans = open_legacy_orphans(&paths)?
            .ok_or_else(|| std::io::Error::other("legacy orphan disappeared"))?;

        let result = archive_legacy_orphans_with_hooks(
            &paths,
            &controller,
            &orphans,
            |_| {
                fs::rename(&orphan, &displaced)
                    .map_err(|error| ControllerStateError::Unsafe(error.to_string()))?;
                fs::rename(&replacement, &orphan)
                    .map_err(|error| ControllerStateError::Unsafe(error.to_string()))?;
                Ok(())
            },
            |_, _| Ok(()),
        );

        assert!(matches!(result, Err(ControllerStateError::Unsafe(_))));
        assert_eq!(fs::read(&orphan)?, b"replacement orphan");
        assert_eq!(fs::read(&displaced)?, b"original orphan");
        Ok(())
    }

    #[test]
    fn orphan_archival_does_not_clobber_a_raced_quarantine_destination()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = TempDir::new()?;
        let root = temp.path().canonicalize()?;
        let paths = seeded_legacy_paths(&root)?;
        fs::remove_file(paths.legacy_database())?;
        let orphan = PathBuf::from(format!("{}-wal", paths.legacy_database().display()));
        fs::write(&orphan, b"original orphan")?;
        fs::set_permissions(&orphan, fs::Permissions::from_mode(0o600))?;
        let controller = open_existing_controller_directory(&paths)?;
        let orphans = open_legacy_orphans(&paths)?
            .ok_or_else(|| std::io::Error::other("legacy orphan disappeared"))?;

        let result = archive_legacy_orphans_with_hooks(
            &paths,
            &controller,
            &orphans,
            |_| Ok(()),
            |quarantine, name| {
                let descriptor =
                    create_private_file(quarantine, name, "raced orphan quarantine destination")?;
                let mut destination = File::from(descriptor);
                destination
                    .write_all(b"raced destination")
                    .map_err(|error| ControllerStateError::Unsafe(error.to_string()))?;
                destination
                    .sync_all()
                    .map_err(|error| ControllerStateError::Unsafe(error.to_string()))?;
                Ok(())
            },
        );

        assert!(matches!(result, Err(ControllerStateError::Unsafe(_))));
        assert_eq!(fs::read(&orphan)?, b"original orphan");
        let quarantine = fs::read_dir(
            paths
                .legacy_database()
                .parent()
                .ok_or_else(|| std::io::Error::other("legacy database has no parent"))?,
        )?
        .filter_map(Result::ok)
        .find(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(ORPHAN_QUARANTINE_PREFIX)
        })
        .ok_or_else(|| std::io::Error::other("orphan quarantine disappeared"))?;
        assert_eq!(
            fs::read(quarantine.path().join("state.sqlite3-wal"))?,
            b"raced destination"
        );
        Ok(())
    }

    #[test]
    fn dual_state_refuses_durable_replacement_in_final_unlink_window()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = TempDir::new()?;
        let root = temp.path().canonicalize()?;
        let paths = seeded_dual_paths(&root)?;
        let durable_connection = Connection::open(paths.durable_database())?;
        durable_connection.pragma_update(None, "journal_mode", "WAL")?;
        durable_connection.pragma_update(None, "wal_autocheckpoint", 0)?;
        durable_connection.execute("UPDATE sandboxes SET updated_at_millis = 8", [])?;
        let legacy_connection = Connection::open(paths.legacy_database())?;
        legacy_connection.pragma_update(None, "journal_mode", "WAL")?;
        legacy_connection.pragma_update(None, "wal_autocheckpoint", 0)?;
        legacy_connection.execute("UPDATE sandboxes SET updated_at_millis = 8", [])?;
        let legacy_family = std::iter::once(paths.legacy_database().to_path_buf())
            .chain(["-wal", "-shm"].map(|suffix| {
                PathBuf::from(format!("{}{suffix}", paths.legacy_database().display()))
            }))
            .map(|path| {
                fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
                Ok((path.clone(), fs::read(path)?))
            })
            .collect::<Result<Vec<_>, std::io::Error>>()?;
        for suffix in ["-wal", "-shm"] {
            fs::set_permissions(
                PathBuf::from(format!("{}{suffix}", paths.durable_database().display())),
                fs::Permissions::from_mode(0o600),
            )?;
        }
        let replacement = root.join("final-window-replacement.sqlite3");
        let replacement_store = Store::open(&replacement)?;
        drop(replacement_store);
        fs::set_permissions(&replacement, fs::Permissions::from_mode(0o600))?;
        let displaced = root.join("final-window-displaced.sqlite3");

        let result = open_controller_store_with_final_dual_unlink_hook(&paths, || {
            fs::rename(paths.durable_database(), &displaced)
                .map_err(|error| ControllerStateError::Unsafe(error.to_string()))?;
            fs::rename(&replacement, paths.durable_database())
                .map_err(|error| ControllerStateError::Unsafe(error.to_string()))?;
            Ok(())
        });
        assert!(matches!(result, Err(ControllerStateError::Unsafe(_))));
        for (path, before) in legacy_family {
            assert_eq!(fs::read(path)?, before);
        }
        drop(legacy_connection);
        drop(durable_connection);
        Ok(())
    }

    #[test]
    fn dual_state_refuses_durable_sidecar_substitution_before_archival()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = TempDir::new()?;
        let root = temp.path().canonicalize()?;
        let paths = seeded_dual_paths(&root)?;
        let durable_connection = Connection::open(paths.durable_database())?;
        durable_connection.pragma_update(None, "journal_mode", "WAL")?;
        durable_connection.pragma_update(None, "wal_autocheckpoint", 0)?;
        durable_connection.execute("UPDATE sandboxes SET updated_at_millis = 8", [])?;
        let legacy_connection = Connection::open(paths.legacy_database())?;
        legacy_connection.execute("UPDATE sandboxes SET updated_at_millis = 8", [])?;
        drop(legacy_connection);
        let durable_wal = PathBuf::from(format!("{}-wal", paths.durable_database().display()));
        let durable_shm = PathBuf::from(format!("{}-shm", paths.durable_database().display()));
        for sidecar in [&durable_wal, &durable_shm] {
            fs::set_permissions(sidecar, fs::Permissions::from_mode(0o600))?;
        }
        let legacy_before = fs::read(paths.legacy_database())?;
        let displaced = root.join("displaced-durable-wal");

        let result = open_controller_store_with_before_dual_archive(&paths, || {
            fs::rename(&durable_wal, &displaced)
                .map_err(|error| ControllerStateError::Unsafe(error.to_string()))?;
            fs::write(&durable_wal, b"replacement WAL")
                .map_err(|error| ControllerStateError::Unsafe(error.to_string()))?;
            fs::set_permissions(&durable_wal, fs::Permissions::from_mode(0o600))
                .map_err(|error| ControllerStateError::Unsafe(error.to_string()))?;
            Ok(())
        });
        assert!(matches!(result, Err(ControllerStateError::Unsafe(_))));
        assert_eq!(fs::read(paths.legacy_database())?, legacy_before);
        drop(durable_connection);
        Ok(())
    }

    #[test]
    fn dual_state_refuses_new_durable_wal_and_journal_in_final_unlink_window()
    -> Result<(), Box<dyn std::error::Error>> {
        for suffix in ["-wal", "-journal"] {
            let temp = TempDir::new()?;
            let root = temp.path().canonicalize()?;
            let paths = seeded_dual_paths(&root)?;
            let legacy_before = fs::read(paths.legacy_database())?;
            let sidecar = PathBuf::from(format!("{}{suffix}", paths.durable_database().display()));

            let result = open_controller_store_with_final_dual_unlink_hook(&paths, || {
                fs::write(&sidecar, b"new sidecar")
                    .map_err(|error| ControllerStateError::Unsafe(error.to_string()))?;
                fs::set_permissions(&sidecar, fs::Permissions::from_mode(0o600))
                    .map_err(|error| ControllerStateError::Unsafe(error.to_string()))?;
                Ok(())
            });

            assert!(matches!(result, Err(ControllerStateError::Unsafe(_))));
            assert_eq!(fs::read(paths.legacy_database())?, legacy_before);
        }
        Ok(())
    }

    #[test]
    fn dual_state_restores_legacy_after_outer_post_archive_validation_failure()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = TempDir::new()?;
        let root = temp.path().canonicalize()?;
        let paths = seeded_dual_paths(&root)?;
        let legacy_before = fs::read(paths.legacy_database())?;
        let replacement = root.join("outer-validation-replacement.sqlite3");
        let replacement_store = Store::open(&replacement)?;
        drop(replacement_store);
        fs::set_permissions(&replacement, fs::Permissions::from_mode(0o600))?;
        let displaced = root.join("outer-validation-displaced.sqlite3");

        let result = open_controller_store_with_after_dual_archive_hook(&paths, || {
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
    fn abandoned_temp_cleanup_does_not_clobber_a_raced_quarantine_destination()
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
        let abandoned = controller_path(&paths, ".state.sqlite3.migration-source-51");
        fs::write(&abandoned, b"abandoned")?;
        fs::set_permissions(&abandoned, fs::Permissions::from_mode(0o600))?;
        let inserted = RefCell::new(None);

        cleanup_migration_temps_with_hooks(
            &controller,
            paths.expected_uid,
            |_| Ok(()),
            |quarantine| {
                if inserted.borrow().is_none() {
                    let path = controller_path(&paths, quarantine);
                    fs::write(&path, b"raced destination")
                        .map_err(|error| ControllerStateError::Unsafe(error.to_string()))?;
                    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
                        .map_err(|error| ControllerStateError::Unsafe(error.to_string()))?;
                    *inserted.borrow_mut() = Some(path);
                }
                Ok(())
            },
            |_| Ok(()),
            |_| Ok(()),
        )?;
        let inserted = inserted
            .into_inner()
            .ok_or_else(|| std::io::Error::other("destination hook did not run"))?;
        assert_eq!(fs::read(inserted)?, b"raced destination");
        assert!(!abandoned.exists());
        Ok(())
    }

    #[test]
    fn abandoned_temp_cleanup_does_not_delete_a_post_check_substitution()
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
        let name = ".state.sqlite3.migration-source-52";
        let abandoned = controller_path(&paths, name);
        let replacement = root.join("replacement-after-check");
        let displaced = root.join("displaced-after-check");
        fs::write(&abandoned, b"abandoned")?;
        fs::set_permissions(&abandoned, fs::Permissions::from_mode(0o600))?;
        fs::write(&replacement, b"replacement")?;
        fs::set_permissions(&replacement, fs::Permissions::from_mode(0o600))?;

        let result = cleanup_migration_temps_with_hooks(
            &controller,
            paths.expected_uid,
            |_| Ok(()),
            |_| Ok(()),
            |quarantine| {
                let quarantine = controller_path(&paths, quarantine);
                let isolated = if quarantine.is_dir() {
                    fs::set_permissions(&quarantine, fs::Permissions::from_mode(0o700))
                        .map_err(|error| ControllerStateError::Unsafe(error.to_string()))?;
                    quarantine.join("candidate")
                } else {
                    quarantine
                };
                fs::rename(&isolated, &displaced)
                    .map_err(|error| ControllerStateError::Unsafe(error.to_string()))?;
                fs::rename(&replacement, &isolated)
                    .map_err(|error| ControllerStateError::Unsafe(error.to_string()))?;
                Ok(())
            },
            |_| Ok(()),
        );
        assert!(matches!(result, Err(ControllerStateError::Unsafe(_))));
        assert_eq!(fs::read(&abandoned)?, b"replacement");
        assert_eq!(fs::read(&displaced)?, b"abandoned");
        Ok(())
    }

    #[test]
    fn abandoned_temp_cleanup_never_deletes_a_final_window_substitution()
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
        let name = ".state.sqlite3.migration-source-53";
        let abandoned = controller_path(&paths, name);
        let replacement = root.join("replacement-final-window");
        let displaced = root.join("displaced-final-window");
        fs::write(&abandoned, b"abandoned")?;
        fs::set_permissions(&abandoned, fs::Permissions::from_mode(0o600))?;
        fs::write(&replacement, b"replacement")?;
        fs::set_permissions(&replacement, fs::Permissions::from_mode(0o600))?;
        let quarantine_path = RefCell::new(None);

        cleanup_migration_temps_with_hooks(
            &controller,
            paths.expected_uid,
            |_| Ok(()),
            |_| Ok(()),
            |_| Ok(()),
            |quarantine| {
                let quarantine = controller_path(&paths, quarantine);
                fs::set_permissions(&quarantine, fs::Permissions::from_mode(0o700))
                    .map_err(|error| ControllerStateError::Unsafe(error.to_string()))?;
                let isolated = quarantine.join("candidate");
                fs::rename(&isolated, &displaced)
                    .map_err(|error| ControllerStateError::Unsafe(error.to_string()))?;
                fs::rename(&replacement, &isolated)
                    .map_err(|error| ControllerStateError::Unsafe(error.to_string()))?;
                *quarantine_path.borrow_mut() = Some(quarantine);
                Ok(())
            },
        )?;

        let quarantine = quarantine_path
            .into_inner()
            .ok_or_else(|| std::io::Error::other("final-window hook did not run"))?;
        assert_eq!(fs::read(quarantine.join("candidate"))?, b"replacement");
        assert_eq!(fs::read(displaced)?, b"abandoned");
        assert!(!abandoned.exists());
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
