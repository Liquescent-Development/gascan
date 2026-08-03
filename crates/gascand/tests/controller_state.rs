use gascand::{
    ControllerStatePaths, MigrationFault, Store, open_controller_store,
    open_controller_store_with_fault,
};
use rusqlite::{Connection, OpenFlags};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
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
        )?;
        Ok(Self {
            _temp: temp,
            home,
            runtime,
            paths,
        })
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
        for database in [self.paths.durable_database(), self.paths.legacy_database()] {
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
        fixture.paths.legacy_database(),
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
    assert!(!fixture.paths.legacy_database().exists());
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
        let error = ControllerStatePaths::for_home_and_runtime(&home, &runtime, uid)
            .expect_err("non-normal path must be rejected");
        assert_eq!(error.code(), "controller_state_invalid");
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
    assert_eq!(error.code(), "controller_state_unsafe");
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
    assert_eq!(error.code(), "controller_state_unsafe");
    Ok(())
}

#[test]
fn open_rejects_foreign_expected_owner() -> TestResult {
    let fixture = ControllerFixture::new()?;
    let foreign_uid = rustix::process::geteuid().as_raw().saturating_add(1);
    let paths =
        ControllerStatePaths::for_home_and_runtime(&fixture.home, &fixture.runtime, foreign_uid)?;

    let error = failed_open(&paths)?;
    assert_eq!(error.code(), "controller_state_unsafe");
    Ok(())
}

#[test]
fn open_rejects_unsafe_managed_directory_and_database_modes() -> TestResult {
    let fixture = ControllerFixture::new()?;
    let application_directory = fixture.home.join("Library/Application Support/dev.gascan");
    create_private_directory(&application_directory)?;
    fs::set_permissions(&application_directory, fs::Permissions::from_mode(0o755))?;
    let error = failed_open(&fixture.paths)?;
    assert_eq!(error.code(), "controller_state_unsafe");

    fs::set_permissions(&application_directory, fs::Permissions::from_mode(0o700))?;
    create_private_directory(&fixture.controller_directory())?;
    fs::write(fixture.paths.durable_database(), b"not a database")?;
    fs::set_permissions(
        fixture.paths.durable_database(),
        fs::Permissions::from_mode(0o644),
    )?;
    let error = failed_open(&fixture.paths)?;
    assert_eq!(error.code(), "controller_state_unsafe");
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
        assert_eq!(error.code(), "controller_state_unsafe");
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
    assert_eq!(error.code(), "controller_state_unsafe");

    fs::set_permissions(&application_directory, fs::Permissions::from_mode(0o700))?;
    create_private_directory(&fixture.controller_directory())?;
    fs::write(fixture.paths.durable_database(), b"not a database")?;
    fs::set_permissions(
        fixture.paths.durable_database(),
        fs::Permissions::from_mode(0o1600),
    )?;
    let error = failed_open(&fixture.paths)?;
    assert_eq!(error.code(), "controller_state_unsafe");
    Ok(())
}

#[test]
fn migration_legacy_only_preserves_logical_content() -> TestResult {
    let fixture = ControllerFixture::new()?;
    fixture.seed_store(fixture.paths.legacy_database(), "legacy")?;

    let store = open_controller_store(&fixture.paths)?;
    assert_eq!(store.list_sandboxes()?.len(), 1);
    assert_eq!(
        store.list_sandboxes()?[0].id.as_str(),
        "legacy-aaaaaaaaaaaa"
    );
    assert!(!fixture.paths.legacy_database().exists());
    assert_private_file(fixture.paths.durable_database(), 0o600)?;
    assert!(migration_backups(&fixture)?.iter().any(|path| {
        path.file_name()
            .is_some_and(|name| name.to_string_lossy().contains("legacy-backup"))
    }));
    Ok(())
}

#[test]
fn migration_includes_committed_uncheckpointed_wal_content() -> TestResult {
    let fixture = ControllerFixture::new()?;
    fs::create_dir(&fixture.runtime)?;
    fs::set_permissions(&fixture.runtime, fs::Permissions::from_mode(0o700))?;
    let store = Store::open(fixture.paths.legacy_database())?;
    drop(store);
    fs::set_permissions(
        fixture.paths.legacy_database(),
        fs::Permissions::from_mode(0o600),
    )?;
    let connection = Connection::open_with_flags(
        fixture.paths.legacy_database(),
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.pragma_update(None, "wal_autocheckpoint", 0)?;
    connection.execute(
        "INSERT INTO sandboxes (id, canonical_root, desired_state, actual_state, updated_at_millis) VALUES ('wal-aaaaaaaaaaaa', '/workspace/wal', 'running', 'stopped', 9)",
        [],
    )?;
    for path in active_database_files(fixture.paths.legacy_database()) {
        if path.exists() {
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        }
    }
    assert!(
        fixture
            .paths
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
fn migration_durable_only_opens_without_creating_legacy_state() -> TestResult {
    let fixture = ControllerFixture::new()?;
    fixture.seed_store(fixture.paths.durable_database(), "durable")?;

    let store = open_controller_store(&fixture.paths)?;
    assert_eq!(
        store.list_sandboxes()?[0].id.as_str(),
        "durable-aaaaaaaaaaaa"
    );
    assert!(!fixture.paths.legacy_database().exists());
    Ok(())
}

#[test]
fn migration_identical_dual_state_archives_legacy() -> TestResult {
    let fixture = ControllerFixture::new()?;
    fixture.seed_store(fixture.paths.durable_database(), "same")?;
    fixture.seed_store(fixture.paths.legacy_database(), "same")?;

    let store = open_controller_store(&fixture.paths)?;
    assert_eq!(store.list_sandboxes()?[0].id.as_str(), "same-aaaaaaaaaaaa");
    assert!(!fixture.paths.legacy_database().exists());
    assert!(
        !fixture
            .paths
            .legacy_database()
            .with_extension("sqlite3-wal")
            .exists()
    );
    assert!(
        !fixture
            .paths
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
    fixture.seed_store(fixture.paths.legacy_database(), "legacy")?;
    let durable = Connection::open(fixture.paths.durable_database())?;
    let legacy = Connection::open(fixture.paths.legacy_database())?;
    for connection in [&durable, &legacy] {
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "wal_autocheckpoint", 0)?;
        connection.execute(
            "UPDATE sandboxes SET updated_at_millis = updated_at_millis + 1",
            [],
        )?;
    }
    for database in [
        fixture.paths.durable_database(),
        fixture.paths.legacy_database(),
    ] {
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
    assert_eq!(error.code(), "controller_state_conflict");
    assert!(error.to_string().contains("No data was changed"));
    assert_eq!(fixture.capture_active_files()?, before);
    drop(durable);
    drop(legacy);
    Ok(())
}

#[test]
fn migration_backup_name_collision_never_overwrites() -> TestResult {
    let fixture = ControllerFixture::new()?;
    fixture.seed_store(fixture.paths.legacy_database(), "legacy")?;
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
    fs::write(fixture.paths.legacy_database(), b"not sqlite")?;
    fs::set_permissions(
        fixture.paths.legacy_database(),
        fs::Permissions::from_mode(0o600),
    )?;
    let before = fixture.capture_active_files()?;

    let error = failed_open(&fixture.paths)?;
    assert_eq!(error.code(), "controller_state_migration_failed");
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
    assert_eq!(error.code(), "controller_state_migration_failed");
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
    assert_eq!(error.code(), "controller_state_unsafe");
    assert_eq!(fs::read(abandoned)?, b"unsafe");
    Ok(())
}

#[test]
fn migration_archives_legacy_sidecars() -> TestResult {
    let fixture = ControllerFixture::new()?;
    fixture.seed_store(fixture.paths.legacy_database(), "legacy")?;
    let connection = Connection::open(fixture.paths.legacy_database())?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.pragma_update(None, "wal_autocheckpoint", 0)?;
    connection.execute("UPDATE sandboxes SET updated_at_millis = 11", [])?;
    for path in active_database_files(fixture.paths.legacy_database()) {
        if path.exists() {
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        }
    }

    open_controller_store(&fixture.paths)?;
    for path in active_database_files(fixture.paths.legacy_database()) {
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
    Ok(())
}

#[test]
fn migration_fault_boundaries_recover_without_losing_legacy_content() -> TestResult {
    for fault in [
        MigrationFault::BeforeSnapshotComplete,
        MigrationFault::BeforeDurableRename,
        MigrationFault::AfterDurableRename,
        MigrationFault::DuringLegacyArchive,
    ] {
        let fixture = ControllerFixture::new()?;
        fixture.seed_store(fixture.paths.legacy_database(), "legacy")?;
        let error = match open_controller_store_with_fault(&fixture.paths, fault) {
            Ok(_) => return Err(std::io::Error::other("fault did not interrupt migration").into()),
            Err(error) => error,
        };
        assert_eq!(error.code(), "controller_state_migration_failed");

        let recovered = open_controller_store(&fixture.paths)?;
        assert_eq!(
            recovered.list_sandboxes()?[0].id.as_str(),
            "legacy-aaaaaaaaaaaa"
        );
        assert!(!fixture.paths.legacy_database().exists());
    }
    Ok(())
}

fn active_database_files(database: &Path) -> [PathBuf; 3] {
    [
        database.to_path_buf(),
        PathBuf::from(format!("{}-wal", database.display())),
        PathBuf::from(format!("{}-shm", database.display())),
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
