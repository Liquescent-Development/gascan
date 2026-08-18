use gascan_core::backend::BackendSelection;
use gascand::{
    ControllerStatePaths, MigrationFault, Store, open_controller_store,
    open_controller_store_with_fault,
};
use rusqlite::{Connection, OpenFlags};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

type TestResult = Result<(), Box<dyn std::error::Error>>;

struct ControllerFixture {
    _temp: TempDir,
    home: PathBuf,
    runtime: PathBuf,
    paths: ControllerStatePaths,
}

impl ControllerFixture {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let temp = TempDir::new()?;
        let root = temp.path().canonicalize()?;
        let home = root.join("home");
        let library = home.join("Library");
        let application_support = library.join("Application Support");
        create_private_directory(&home)?;
        create_private_directory(&library)?;
        create_private_directory(&application_support)?;
        let runtime = root.join("runtime");
        let paths = ControllerStatePaths::for_home_and_runtime(
            &home,
            &runtime,
            rustix::process::geteuid().as_raw(),
            BackendSelection::Apple,
        )?;
        Ok(Self {
            _temp: temp,
            home,
            runtime,
            paths,
        })
    }

    fn legacy_database(&self) -> &Path {
        self.paths
            .legacy_database()
            .expect("the shared scope always has a legacy database")
    }

    fn controller_directory(&self) -> PathBuf {
        self.home
            .join("Library/Application Support/dev.gascan/controller")
    }

    fn seed_store(&self, path: &Path, label: &str) -> TestResult {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
            if path == self.paths.durable_database() {
                let application = parent
                    .parent()
                    .ok_or_else(|| std::io::Error::other("missing application directory"))?;
                fs::set_permissions(application, fs::Permissions::from_mode(0o700))?;
            }
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

    fn capture_active_files(&self) -> Result<BTreeMap<PathBuf, Vec<u8>>, std::io::Error> {
        let mut files = BTreeMap::new();
        for database in [self.paths.durable_database(), self.legacy_database()] {
            for path in active_database_files(database) {
                if path.exists() {
                    let contents = fs::read(&path)?;
                    files.insert(path, contents);
                }
            }
        }
        Ok(files)
    }
}

#[test]
fn default_paths_split_durable_state_from_runtime_ipc() -> TestResult {
    let fixture = ControllerFixture::new()?;
    assert_eq!(
        fixture.paths.durable_database(),
        fixture
            .home
            .join("Library/Application Support/dev.gascan/controller/state.sqlite3")
    );
    assert_eq!(
        fixture.legacy_database(),
        fixture.runtime.join("state.sqlite3")
    );
    Ok(())
}

#[test]
fn fresh_open_creates_only_a_private_durable_store() -> TestResult {
    let fixture = ControllerFixture::new()?;
    let store = open_controller_store(&fixture.paths)?;
    assert!(store.list_sandboxes()?.is_empty());
    assert_private_directory(&fixture.controller_directory(), 0o700)?;
    assert_private_file(fixture.paths.durable_database(), 0o600)?;
    assert!(!fixture.legacy_database().exists());
    Ok(())
}

#[test]
fn paths_reject_relative_and_parent_components() -> TestResult {
    let fixture = ControllerFixture::new()?;
    let uid = rustix::process::geteuid().as_raw();

    for (home, runtime) in [
        (PathBuf::from("relative-home"), fixture.runtime.clone()),
        (fixture.home.clone(), PathBuf::from("relative-runtime")),
        (fixture.home.join("../home"), fixture.runtime.clone()),
        (fixture.home.clone(), fixture.runtime.join("../runtime")),
    ] {
        let error = ControllerStatePaths::for_home_and_runtime(
            &home,
            &runtime,
            uid,
            BackendSelection::Apple,
        )
        .expect_err("non-normal path must be rejected");
        assert_eq!(error.code().as_str(), "controller_state_invalid");
    }
    Ok(())
}

#[test]
fn open_rejects_symlinked_managed_components() -> TestResult {
    let fixture = ControllerFixture::new()?;
    let application_support = fixture.home.join("Library/Application Support");
    let target = fixture.home.join("target");
    create_private_directory(&target)?;
    std::os::unix::fs::symlink(&target, application_support.join("dev.gascan"))?;

    let error = failed_open(&fixture.paths)?;
    assert_eq!(error.code().as_str(), "controller_state_unsafe");
    assert!(!target.join("controller/state.sqlite3").exists());
    Ok(())
}

#[test]
fn open_rejects_non_regular_database() -> TestResult {
    let fixture = ControllerFixture::new()?;
    create_private_directory(&fixture.home.join("Library/Application Support/dev.gascan"))?;
    create_private_directory(&fixture.controller_directory())?;
    fs::create_dir(fixture.paths.durable_database())?;

    let error = failed_open(&fixture.paths)?;
    assert_eq!(error.code().as_str(), "controller_state_unsafe");
    Ok(())
}

#[test]
fn open_rejects_foreign_expected_owner() -> TestResult {
    let fixture = ControllerFixture::new()?;
    let foreign_uid = rustix::process::geteuid().as_raw().saturating_add(1);
    let paths = ControllerStatePaths::for_home_and_runtime(
        &fixture.home,
        &fixture.runtime,
        foreign_uid,
        BackendSelection::Apple,
    )?;

    let error = failed_open(&paths)?;
    assert_eq!(error.code().as_str(), "controller_state_unsafe");
    Ok(())
}

#[test]
fn open_rejects_unsafe_managed_directory_and_database_modes() -> TestResult {
    let fixture = ControllerFixture::new()?;
    let application_directory = fixture.home.join("Library/Application Support/dev.gascan");
    create_private_directory(&application_directory)?;
    fs::set_permissions(&application_directory, fs::Permissions::from_mode(0o755))?;
    let error = failed_open(&fixture.paths)?;
    assert_eq!(error.code().as_str(), "controller_state_unsafe");

    fs::set_permissions(&application_directory, fs::Permissions::from_mode(0o700))?;
    create_private_directory(&fixture.controller_directory())?;
    fs::write(fixture.paths.durable_database(), b"not a database")?;
    fs::set_permissions(
        fixture.paths.durable_database(),
        fs::Permissions::from_mode(0o644),
    )?;
    let error = failed_open(&fixture.paths)?;
    assert_eq!(error.code().as_str(), "controller_state_unsafe");
    Ok(())
}

#[test]
fn open_rejects_writable_home_library_and_application_support_ancestors() -> TestResult {
    for ancestor in ["", "Library", "Library/Application Support"] {
        let fixture = ControllerFixture::new()?;
        let path = if ancestor.is_empty() {
            fixture.home.clone()
        } else {
            fixture.home.join(ancestor)
        };
        fs::set_permissions(&path, fs::Permissions::from_mode(0o722))?;

        let error = failed_open(&fixture.paths)?;
        assert_eq!(error.code().as_str(), "controller_state_unsafe");
    }
    Ok(())
}

#[test]
fn open_accepts_non_writable_ancestor_modes() -> TestResult {
    let fixture = ControllerFixture::new()?;
    for ancestor in [
        fixture.home.clone(),
        fixture.home.join("Library"),
        fixture.home.join("Library/Application Support"),
    ] {
        fs::set_permissions(&ancestor, fs::Permissions::from_mode(0o755))?;
    }

    let store = open_controller_store(&fixture.paths)?;
    assert!(store.list_sandboxes()?.is_empty());
    Ok(())
}

#[test]
fn open_rejects_special_bits_on_managed_paths() -> TestResult {
    let fixture = ControllerFixture::new()?;
    let application_directory = fixture.home.join("Library/Application Support/dev.gascan");
    create_private_directory(&application_directory)?;
    fs::set_permissions(&application_directory, fs::Permissions::from_mode(0o1700))?;
    let error = failed_open(&fixture.paths)?;
    assert_eq!(error.code().as_str(), "controller_state_unsafe");

    fs::set_permissions(&application_directory, fs::Permissions::from_mode(0o700))?;
    create_private_directory(&fixture.controller_directory())?;
    fs::write(fixture.paths.durable_database(), b"not a database")?;
    fs::set_permissions(
        fixture.paths.durable_database(),
        fs::Permissions::from_mode(0o1600),
    )?;
    let error = failed_open(&fixture.paths)?;
    assert_eq!(error.code().as_str(), "controller_state_unsafe");
    Ok(())
}

#[test]
fn migration_legacy_only_preserves_logical_content() -> TestResult {
    let fixture = ControllerFixture::new()?;
    fixture.seed_store(fixture.legacy_database(), "legacy")?;

    let store = open_controller_store(&fixture.paths)?;
    assert_eq!(store.list_sandboxes()?.len(), 1);
    assert_eq!(
        store.list_sandboxes()?[0].id.as_str(),
        "legacy-aaaaaaaaaaaa"
    );
    assert!(!fixture.legacy_database().exists());
    assert_private_file(fixture.paths.durable_database(), 0o600)?;
    assert!(migration_backups(&fixture)?.iter().any(|path| {
        path.file_name()
            .is_some_and(|name| name.to_string_lossy().contains("legacy-backup"))
    }));
    drop(store);
    let reopened = open_controller_store(&fixture.paths)?;
    assert_eq!(
        reopened.list_sandboxes()?[0].id.as_str(),
        "legacy-aaaaaaaaaaaa"
    );
    Ok(())
}

#[test]
fn migration_backup_remains_recoverable_after_staging_reads_the_source() -> TestResult {
    let fixture = ControllerFixture::new()?;
    fixture.seed_store(fixture.legacy_database(), "recoverable")?;

    open_controller_store(&fixture.paths)?;
    let backup = migration_backup_database(&fixture)?;
    let recovered = Store::open(backup)?;
    assert_eq!(
        recovered.list_sandboxes()?[0].id.as_str(),
        "recoverable-aaaaaaaaaaaa"
    );
    Ok(())
}

#[test]
fn migration_includes_committed_uncheckpointed_wal_content() -> TestResult {
    let fixture = ControllerFixture::new()?;
    fs::create_dir(&fixture.runtime)?;
    fs::set_permissions(&fixture.runtime, fs::Permissions::from_mode(0o700))?;
    let store = Store::open(fixture.legacy_database())?;
    drop(store);
    fs::set_permissions(fixture.legacy_database(), fs::Permissions::from_mode(0o600))?;
    let connection = Connection::open_with_flags(
        fixture.legacy_database(),
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.pragma_update(None, "wal_autocheckpoint", 0)?;
    connection.execute(
        "INSERT INTO sandboxes (id, canonical_root, desired_state, actual_state, updated_at_millis) VALUES ('wal-aaaaaaaaaaaa', '/workspace/wal', 'running', 'stopped', 9)",
        [],
    )?;
    for path in active_database_files(fixture.legacy_database()) {
        if path.exists() {
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        }
    }
    assert!(
        fixture
            .legacy_database()
            .with_extension("sqlite3-wal")
            .exists()
    );

    let migrated = open_controller_store(&fixture.paths)?;
    assert_eq!(migrated.list_sandboxes()?.len(), 1);
    assert_eq!(
        migrated.list_sandboxes()?[0].id.as_str(),
        "wal-aaaaaaaaaaaa"
    );
    drop(connection);
    Ok(())
}

#[test]
fn migration_recovers_and_archives_a_hot_rollback_journal() -> TestResult {
    let fixture = ControllerFixture::new()?;
    fixture.seed_store(fixture.legacy_database(), "journal")?;
    let status = Command::new(std::env::current_exe()?)
        .arg("--exact")
        .arg("hot_journal_crash_helper")
        .arg("--nocapture")
        .env("GASCAN_TEST_HOT_JOURNAL_PATH", fixture.legacy_database())
        .status()?;
    assert!(!status.success());
    let journal = PathBuf::from(format!("{}-journal", fixture.legacy_database().display()));
    assert!(journal.exists());
    for path in active_database_files(fixture.legacy_database()) {
        if path.exists() {
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        }
    }

    let migrated = open_controller_store(&fixture.paths)?;
    assert_eq!(
        migrated.list_sandboxes()?[0].canonical_root.as_str(),
        "/workspace/journal"
    );
    assert!(!journal.exists());
    assert!(migration_backups(&fixture)?.iter().any(|path| {
        path.file_name()
            .is_some_and(|name| name.to_string_lossy().ends_with("-journal"))
    }));
    let recovered_backup = Store::open(migration_backup_database(&fixture)?)?;
    assert_eq!(
        recovered_backup.list_sandboxes()?[0]
            .canonical_root
            .as_str(),
        "/workspace/journal"
    );
    Ok(())
}

#[test]
fn hot_journal_crash_helper() {
    let Some(path) = std::env::var_os("GASCAN_TEST_HOT_JOURNAL_PATH") else {
        return;
    };
    let Ok(connection) = Connection::open(path) else {
        std::process::exit(2);
    };
    if connection
        .pragma_update(None, "journal_mode", "DELETE")
        .is_err()
        || connection
            .pragma_update(None, "synchronous", "FULL")
            .is_err()
        || connection
            .execute_batch(
                "BEGIN IMMEDIATE; UPDATE sandboxes SET canonical_root = '/workspace/uncommitted';",
            )
            .is_err()
    {
        std::process::exit(2);
    }
    std::process::abort();
}

#[test]
fn migration_durable_only_opens_without_creating_legacy_state() -> TestResult {
    let fixture = ControllerFixture::new()?;
    fixture.seed_store(fixture.paths.durable_database(), "durable")?;

    let store = open_controller_store(&fixture.paths)?;
    assert_eq!(
        store.list_sandboxes()?[0].id.as_str(),
        "durable-aaaaaaaaaaaa"
    );
    assert!(!fixture.legacy_database().exists());
    Ok(())
}

#[test]
fn migration_identical_dual_state_archives_legacy() -> TestResult {
    let fixture = ControllerFixture::new()?;
    fixture.seed_store(fixture.paths.durable_database(), "same")?;
    fixture.seed_store(fixture.legacy_database(), "same")?;

    let store = open_controller_store(&fixture.paths)?;
    assert_eq!(store.list_sandboxes()?[0].id.as_str(), "same-aaaaaaaaaaaa");
    assert!(!fixture.legacy_database().exists());
    assert!(
        !fixture
            .legacy_database()
            .with_extension("sqlite3-wal")
            .exists()
    );
    assert!(
        !fixture
            .legacy_database()
            .with_extension("sqlite3-shm")
            .exists()
    );
    Ok(())
}

#[test]
fn conflicting_active_databases_are_untouched() -> TestResult {
    let fixture = ControllerFixture::new()?;
    fixture.seed_store(fixture.paths.durable_database(), "durable")?;
    fixture.seed_store(fixture.legacy_database(), "legacy")?;
    let durable = Connection::open(fixture.paths.durable_database())?;
    let legacy = Connection::open(fixture.legacy_database())?;
    for connection in [&durable, &legacy] {
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "wal_autocheckpoint", 0)?;
        connection.execute(
            "UPDATE sandboxes SET updated_at_millis = updated_at_millis + 1",
            [],
        )?;
    }
    for database in [fixture.paths.durable_database(), fixture.legacy_database()] {
        for path in active_database_files(database) {
            if path.exists() {
                fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
            }
        }
        assert!(PathBuf::from(format!("{}-wal", database.display())).exists());
        assert!(PathBuf::from(format!("{}-shm", database.display())).exists());
    }
    let before = fixture.capture_active_files()?;

    let error = failed_open(&fixture.paths)?;
    assert_eq!(error.code().as_str(), "controller_state_conflict");
    assert!(error.to_string().contains("No data was changed"));
    assert_eq!(fixture.capture_active_files()?, before);
    drop(durable);
    drop(legacy);
    Ok(())
}

#[test]
fn migration_backup_name_collision_never_overwrites() -> TestResult {
    let fixture = ControllerFixture::new()?;
    fixture.seed_store(fixture.legacy_database(), "legacy")?;
    fs::create_dir_all(fixture.controller_directory())?;
    fs::set_permissions(
        fixture.controller_directory(),
        fs::Permissions::from_mode(0o700),
    )?;
    fs::set_permissions(
        fixture.home.join("Library/Application Support/dev.gascan"),
        fs::Permissions::from_mode(0o700),
    )?;
    let collision = fixture
        .controller_directory()
        .join("state.sqlite3.legacy-backup");
    fs::write(&collision, b"preserve me")?;
    fs::set_permissions(&collision, fs::Permissions::from_mode(0o600))?;

    open_controller_store(&fixture.paths)?;
    assert_eq!(fs::read(&collision)?, b"preserve me");
    assert!(migration_backups(&fixture)?.len() >= 2);
    Ok(())
}

#[test]
fn migration_rejects_malformed_legacy_schema_without_publishing_durable_state() -> TestResult {
    let fixture = ControllerFixture::new()?;
    fs::create_dir(&fixture.runtime)?;
    fs::set_permissions(&fixture.runtime, fs::Permissions::from_mode(0o700))?;
    fs::write(fixture.legacy_database(), b"not sqlite")?;
    fs::set_permissions(fixture.legacy_database(), fs::Permissions::from_mode(0o600))?;
    let before = fixture.capture_active_files()?;

    let error = failed_open(&fixture.paths)?;
    assert_eq!(error.code().as_str(), "controller_state_migration_failed");
    assert_eq!(fixture.capture_active_files()?, before);
    assert!(!fixture.paths.durable_database().exists());
    Ok(())
}

#[test]
fn migration_rejects_malformed_durable_schema_without_touching_it() -> TestResult {
    let fixture = ControllerFixture::new()?;
    let application = fixture.home.join("Library/Application Support/dev.gascan");
    create_private_directory(&application)?;
    create_private_directory(&fixture.controller_directory())?;
    fs::write(fixture.paths.durable_database(), b"not sqlite")?;
    fs::set_permissions(
        fixture.paths.durable_database(),
        fs::Permissions::from_mode(0o600),
    )?;
    let before = fs::read(fixture.paths.durable_database())?;

    let error = failed_open(&fixture.paths)?;
    assert_eq!(error.code().as_str(), "controller_state_migration_failed");
    assert_eq!(fs::read(fixture.paths.durable_database())?, before);
    Ok(())
}

#[test]
fn migration_cleans_only_exact_private_abandoned_temp_names() -> TestResult {
    let fixture = ControllerFixture::new()?;
    let application = fixture.home.join("Library/Application Support/dev.gascan");
    create_private_directory(&application)?;
    create_private_directory(&fixture.controller_directory())?;
    let abandoned = fixture
        .controller_directory()
        .join(".state.sqlite3.migration-source-7");
    let near_match = fixture
        .controller_directory()
        .join(".state.sqlite3.migration-source-recovery-notes");
    for path in [&abandoned, &near_match] {
        fs::write(path, b"private")?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }

    open_controller_store(&fixture.paths)?;
    assert!(!abandoned.exists());
    assert_eq!(fs::read(near_match)?, b"private");
    Ok(())
}

#[test]
fn migration_refuses_unsafe_abandoned_temp_without_removing_it() -> TestResult {
    let fixture = ControllerFixture::new()?;
    let application = fixture.home.join("Library/Application Support/dev.gascan");
    create_private_directory(&application)?;
    create_private_directory(&fixture.controller_directory())?;
    let abandoned = fixture
        .controller_directory()
        .join(".state.sqlite3.migration-snapshot-9");
    fs::write(&abandoned, b"unsafe")?;
    fs::set_permissions(&abandoned, fs::Permissions::from_mode(0o644))?;

    let error = failed_open(&fixture.paths)?;
    assert_eq!(error.code().as_str(), "controller_state_unsafe");
    assert_eq!(fs::read(abandoned)?, b"unsafe");
    Ok(())
}

#[test]
fn migration_archives_legacy_sidecars() -> TestResult {
    let fixture = ControllerFixture::new()?;
    fixture.seed_store(fixture.legacy_database(), "legacy")?;
    let connection = Connection::open(fixture.legacy_database())?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.pragma_update(None, "wal_autocheckpoint", 0)?;
    connection.execute("UPDATE sandboxes SET updated_at_millis = 11", [])?;
    for path in active_database_files(fixture.legacy_database()) {
        if path.exists() {
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        }
    }

    open_controller_store(&fixture.paths)?;
    for path in active_database_files(fixture.legacy_database()) {
        assert!(!path.exists());
    }
    let names = migration_backups(&fixture)?
        .into_iter()
        .filter_map(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .collect::<Vec<_>>();
    assert!(names.iter().any(|name| name.ends_with("-wal")));
    assert!(names.iter().any(|name| name.ends_with("-shm")));
    drop(connection);
    let recovered_backup = Store::open(migration_backup_database(&fixture)?)?;
    assert_eq!(recovered_backup.list_sandboxes()?[0].updated_at_millis, 11);
    Ok(())
}

#[test]
fn migration_fault_boundaries_recover_without_losing_legacy_content() -> TestResult {
    for fault in [
        MigrationFault::BeforeSnapshotComplete,
        MigrationFault::BeforeDurableRename,
        MigrationFault::AfterDurableRename,
        MigrationFault::DuringLegacyArchive,
        MigrationFault::AfterLegacyMoveBeforeValidation,
    ] {
        let fixture = ControllerFixture::new()?;
        fixture.seed_store(fixture.legacy_database(), "legacy")?;
        let error = match open_controller_store_with_fault(&fixture.paths, fault) {
            Ok(_) => return Err(std::io::Error::other("fault did not interrupt migration").into()),
            Err(error) => error,
        };
        assert_eq!(error.code().as_str(), "controller_state_migration_failed");

        let recovered = open_controller_store(&fixture.paths)?;
        assert_eq!(
            recovered.list_sandboxes()?[0].id.as_str(),
            "legacy-aaaaaaaaaaaa"
        );
        assert!(!fixture.legacy_database().exists());
    }
    Ok(())
}

#[test]
fn interrupted_post_move_archive_is_recovered_before_state_selection() -> TestResult {
    let fixture = ControllerFixture::new()?;
    fixture.seed_store(fixture.legacy_database(), "legacy")?;
    let legacy_before = fs::read(fixture.legacy_database())?;
    let status = Command::new(std::env::current_exe()?)
        .arg("--exact")
        .arg("archive_transaction_crash_helper")
        .arg("--nocapture")
        .env("GASCAN_TEST_ARCHIVE_HOME", &fixture.home)
        .env("GASCAN_TEST_ARCHIVE_RUNTIME", &fixture.runtime)
        .status()?;
    assert!(!status.success());
    assert!(!fixture.legacy_database().exists());

    let durable = Connection::open(fixture.paths.durable_database())?;
    durable.execute(
        "UPDATE sandboxes SET canonical_root = '/workspace/conflicting-durable'",
        [],
    )?;
    drop(durable);
    fs::set_permissions(
        fixture.paths.durable_database(),
        fs::Permissions::from_mode(0o600),
    )?;

    let error = failed_open(&fixture.paths)?;
    assert_eq!(
        error.code().as_str(),
        "controller_state_conflict",
        "unexpected recovery error: {error:?}"
    );
    assert_eq!(fs::read(fixture.legacy_database())?, legacy_before);
    let repeated = failed_open(&fixture.paths)?;
    assert_eq!(repeated.code().as_str(), "controller_state_conflict");
    Ok(())
}

#[test]
fn archive_transaction_crash_helper() {
    let (Some(home), Some(runtime)) = (
        std::env::var_os("GASCAN_TEST_ARCHIVE_HOME"),
        std::env::var_os("GASCAN_TEST_ARCHIVE_RUNTIME"),
    ) else {
        return;
    };
    let Ok(paths) = ControllerStatePaths::for_home_and_runtime(
        Path::new(&home),
        Path::new(&runtime),
        rustix::process::geteuid().as_raw(),
        BackendSelection::Apple,
    ) else {
        std::process::exit(2);
    };
    match open_controller_store_with_fault(&paths, MigrationFault::AfterLegacyMoveBeforeValidation)
    {
        Err(_) => std::process::abort(),
        Ok(_) => std::process::exit(2),
    }
}

#[test]
fn malformed_archive_transaction_is_refused_without_mutation() -> TestResult {
    let fixture = ControllerFixture::new()?;
    fixture.seed_store(fixture.paths.durable_database(), "durable")?;
    fs::create_dir_all(&fixture.runtime)?;
    fs::set_permissions(&fixture.runtime, fs::Permissions::from_mode(0o700))?;
    let quarantine = fixture
        .runtime
        .join(".state.sqlite3.archive-quarantine-00000000000000000000000000000000");
    create_private_directory(&quarantine)?;
    let unexpected = quarantine.join("unexpected");
    fs::write(&unexpected, b"do not touch")?;
    fs::set_permissions(&unexpected, fs::Permissions::from_mode(0o600))?;

    let error = failed_open(&fixture.paths)?;
    assert_eq!(error.code().as_str(), "controller_state_unsafe");
    assert_eq!(fs::read(unexpected)?, b"do not touch");
    Ok(())
}

#[test]
fn ambiguous_archive_transactions_are_refused_without_mutation() -> TestResult {
    let fixture = ControllerFixture::new()?;
    fixture.seed_store(fixture.paths.durable_database(), "same")?;
    fixture.seed_store(fixture.legacy_database(), "same")?;
    let before = fixture.capture_active_files()?;
    let metadata = fs::symlink_metadata(fixture.legacy_database())?;
    let marker = format!(
        "GASCAN_LEGACY_ARCHIVE_V1\nstate.sqlite3\t{}\t{}\n",
        metadata.dev(),
        metadata.ino()
    );
    for token in [
        "11111111111111111111111111111111",
        "22222222222222222222222222222222",
    ] {
        let quarantine = fixture
            .runtime
            .join(format!(".state.sqlite3.archive-quarantine-{token}"));
        create_private_directory(&quarantine)?;
        let prepared = quarantine.join("prepared");
        fs::write(&prepared, &marker)?;
        fs::set_permissions(&prepared, fs::Permissions::from_mode(0o600))?;
    }

    let error = failed_open(&fixture.paths)?;
    assert_eq!(error.code().as_str(), "controller_state_unsafe");
    assert_eq!(fixture.capture_active_files()?, before);
    Ok(())
}

fn active_database_files(database: &Path) -> [PathBuf; 4] {
    [
        database.to_path_buf(),
        PathBuf::from(format!("{}-wal", database.display())),
        PathBuf::from(format!("{}-shm", database.display())),
        PathBuf::from(format!("{}-journal", database.display())),
    ]
}

fn migration_backups(fixture: &ControllerFixture) -> Result<Vec<PathBuf>, std::io::Error> {
    if !fixture.controller_directory().exists() {
        return Ok(Vec::new());
    }
    fs::read_dir(fixture.controller_directory())?
        .filter_map(|entry| match entry {
            Ok(entry)
                if entry
                    .file_name()
                    .to_string_lossy()
                    .contains("legacy-backup") =>
            {
                Some(Ok(entry.path()))
            }
            Ok(_) => None,
            Err(error) => Some(Err(error)),
        })
        .collect()
}

fn migration_backup_database(fixture: &ControllerFixture) -> Result<PathBuf, std::io::Error> {
    migration_backups(fixture)?
        .into_iter()
        .find(|path| {
            path.file_name().is_some_and(|name| {
                let name = name.to_string_lossy();
                name.starts_with("state.sqlite3.legacy-backup")
                    && !name.ends_with("-wal")
                    && !name.ends_with("-shm")
                    && !name.ends_with("-journal")
            })
        })
        .ok_or_else(|| std::io::Error::other("legacy backup database is missing"))
}

fn create_private_directory(path: &Path) -> std::io::Result<()> {
    fs::create_dir(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

fn failed_open(
    paths: &ControllerStatePaths,
) -> Result<gascand::ControllerStateError, Box<dyn std::error::Error>> {
    match open_controller_store(paths) {
        Ok(_) => Err(std::io::Error::other("unsafe controller state was accepted").into()),
        Err(error) => Ok(error),
    }
}

fn assert_private_directory(path: &Path, mode: u32) -> TestResult {
    let metadata = fs::symlink_metadata(path)?;
    assert!(metadata.file_type().is_dir());
    assert_eq!(metadata.uid(), rustix::process::geteuid().as_raw());
    assert_eq!(metadata.permissions().mode() & 0o777, mode);
    Ok(())
}

fn assert_private_file(path: &Path, mode: u32) -> TestResult {
    let metadata = fs::symlink_metadata(path)?;
    assert!(metadata.file_type().is_file());
    assert_eq!(metadata.uid(), rustix::process::geteuid().as_raw());
    assert_eq!(metadata.permissions().mode() & 0o777, mode);
    Ok(())
}

/// **The scope directory is defended exactly as the controller directory is.**
///
/// Every other test in this file constructs its paths with
/// `BackendSelection::Apple`, which by design never creates a scope child --
/// so `controller/<backend>` arrived with the safety contract this file exists
/// to pin asserted nowhere. It is a directory a daemon creates and then trusts
/// a database inside, on the same terms as its parent.
#[test]
fn a_scoped_store_refuses_a_symlinked_scope_directory() -> TestResult {
    let fixture = ControllerFixture::new()?;
    let paths = ControllerStatePaths::for_home_and_runtime(
        &fixture.home,
        &fixture.runtime,
        rustix::process::geteuid().as_raw(),
        BackendSelection::Arca,
    )?;
    let application = fixture.home.join("Library/Application Support/dev.gascan");
    create_private_directory(&application)?;
    create_private_directory(&fixture.controller_directory())?;
    let target = fixture.home.join("scope-target");
    create_private_directory(&target)?;
    std::os::unix::fs::symlink(&target, fixture.controller_directory().join("arca"))?;

    let error = failed_open(&paths)?;
    assert_eq!(error.code().as_str(), "controller_state_unsafe");
    assert!(
        !target.join("state.sqlite3").exists(),
        "a symlinked scope directory was followed and written through"
    );
    Ok(())
}

#[test]
fn a_scoped_store_refuses_a_world_readable_scope_directory() -> TestResult {
    let fixture = ControllerFixture::new()?;
    let paths = ControllerStatePaths::for_home_and_runtime(
        &fixture.home,
        &fixture.runtime,
        rustix::process::geteuid().as_raw(),
        BackendSelection::Arca,
    )?;
    let application = fixture.home.join("Library/Application Support/dev.gascan");
    create_private_directory(&application)?;
    create_private_directory(&fixture.controller_directory())?;
    let scope = fixture.controller_directory().join("arca");
    create_private_directory(&scope)?;
    fs::set_permissions(&scope, fs::Permissions::from_mode(0o755))?;

    let error = failed_open(&paths)?;
    assert_eq!(error.code().as_str(), "controller_state_unsafe");

    // The same directory at 0700 opens, so the refusal above is the mode and
    // not the mere existence of the child.
    fs::set_permissions(&scope, fs::Permissions::from_mode(0o700))?;
    let store = open_controller_store(&paths)?;
    assert!(store.list_sandboxes()?.is_empty());
    Ok(())
}

#[test]
fn a_scoped_store_refuses_a_foreign_owner() -> TestResult {
    let fixture = ControllerFixture::new()?;
    let foreign_uid = rustix::process::geteuid().as_raw().saturating_add(1);
    let paths = ControllerStatePaths::for_home_and_runtime(
        &fixture.home,
        &fixture.runtime,
        foreign_uid,
        BackendSelection::Arca,
    )?;
    let error = failed_open(&paths)?;
    assert_eq!(error.code().as_str(), "controller_state_unsafe");
    Ok(())
}
