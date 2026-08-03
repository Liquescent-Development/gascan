use gascand::{ControllerStatePaths, open_controller_store};
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
