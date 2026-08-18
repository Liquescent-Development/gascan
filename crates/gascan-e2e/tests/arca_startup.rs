#![forbid(unsafe_code)]
#![deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

//! What a user meets when the Arca backend is selected and cannot start.
//!
//! **No engine is needed to run any of this, and that is the point.** Every
//! failure here happens before the daemon dials anything: three undefaulted
//! environment variables are read, and an absent one is a startup error naming
//! the variable. The engine tier that does need a built `arca-engine` is
//! `arca_engine.rs`, and it is `#[ignore]`d; these run on every push, which is
//! where a regression in the diagnostic path would otherwise go unseen.
//!
//! Before the startup diagnostic carried these, the CLI gave a production
//! daemon `Stdio::null()` and dropped the diagnostic descriptor as soon as the
//! controller store opened -- so `GASCAN_ENGINE_SOCKET must name its socket`,
//! a message that names its own remedy, reached the user as
//! `daemon_readiness_failed`.

use std::ffi::OsString;
use std::process::Command;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// An Arca-selected CLI with no engine anywhere.
///
/// `GASCAN_STATE_PATH` is deliberately NOT set: the daemon must resolve its own
/// controller store, which under this backend is `controller/arca/`, so the
/// startup path under test is the production one.
struct ArcaStartup {
    gascan: OsString,
    gascand: OsString,
    _root: tempfile::TempDir,
    root_path: std::path::PathBuf,
    _runtime: tempfile::TempDir,
    runtime_root: std::path::PathBuf,
    account_home: std::path::PathBuf,
}

impl ArcaStartup {
    fn new() -> TestResult<Self> {
        let gascan = std::env::var_os("CARGO_BIN_EXE_gascan-e2e-cli").ok_or("gascan missing")?;
        let gascand =
            std::env::var_os("CARGO_BIN_EXE_gascan-e2e-daemon").ok_or("gascand missing")?;
        let root = tempfile::tempdir()?;
        let root_path = root.path().canonicalize()?;
        // `/private/tmp` and short names, because the engine socket lives
        // under this root and a unix socket path has 104 bytes. macOS's TMPDIR
        // is a `/var/folders/...` path some fifty bytes deep; MEASURED here,
        // the daemon refuses one built under the session scratch directory with
        // `path must be shorter than SUN_LEN` before it spawns anything, which
        // would make every test below fail for a reason it does not test.
        let runtime = tempfile::Builder::new()
            .prefix("gc-as-")
            .tempdir_in("/private/tmp")?;
        std::fs::set_permissions(
            runtime.path(),
            <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o700),
        )?;
        let runtime_root = runtime.path().canonicalize()?;
        let account_home = runtime_root.join("home");
        std::fs::create_dir(&account_home)?;
        std::fs::create_dir(account_home.join("Library"))?;
        std::fs::create_dir(account_home.join("Library/Application Support"))?;
        Ok(Self {
            gascan,
            gascand,
            _root: root,
            root_path,
            _runtime: runtime,
            runtime_root,
            account_home,
        })
    }

    /// The CLI with the Arca backend selected and every engine variable set.
    ///
    /// A test removes exactly the one it is about, so the failure it observes
    /// is attributable to that removal and not to an environment that was never
    /// complete.
    fn command(&self, arguments: &[&str]) -> Command {
        let mut command = Command::new(&self.gascan);
        command
            .args(arguments)
            .env("XDG_RUNTIME_DIR", &self.runtime_root)
            .env_remove("GASCAN_STATE_PATH")
            .env_remove("GASCAN_TEST_FAKE_BACKEND")
            .env("GASCAN_PID_PATH", self.runtime_root.join("daemon.pid"))
            .env("GASCAN_DAEMON", &self.gascand)
            .env("GASCAN_E2E_ACCOUNT_HOME", &self.account_home)
            .env(gascand::ARCA_BACKEND_ENV, "1")
            .env(gascand::ENGINE_BIN_ENV, self.runtime_root.join("no-engine"))
            .env(gascand::ENGINE_SOCKET_ENV, self.runtime_root.join("e.sock"))
            .env(
                gascand::ENGINE_STATE_ROOT_ENV,
                self.runtime_root.join("engine-state"),
            );
        command
    }

    fn root(&self) -> TestResult<&str> {
        self.root_path.to_str().ok_or("non UTF-8 root".into())
    }
}

/// The `code: message` line a supervisor failure renders on stderr.
///
/// Not `--json`: that path renders errors a *connected* daemon returned, and a
/// daemon that never started returned nothing. What a user sees here is
/// `Error: {code}: {message}`, and the code is the one the daemon wrote into
/// the startup diagnostic.
fn startup_failure(output: &std::process::Output) -> TestResult<String> {
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    stderr
        .lines()
        .find_map(|line| line.strip_prefix("Error: "))
        .map(str::to_owned)
        .ok_or_else(|| format!("no error line in stderr: {stderr}").into())
}

/// **Each of the three undefaulted variables reaches the user by name.**
///
/// Removing one at a time rather than all three: a test that cleared the whole
/// environment would pass on a daemon that only ever reported the first thing
/// it checked, and the variable a user actually forgot is the one they need
/// named.
#[test]
fn a_missing_engine_variable_reaches_the_user_by_name() -> TestResult {
    for (variable, expected_fragment) in [
        (gascand::ENGINE_BIN_ENV, "must name the engine executable"),
        (gascand::ENGINE_SOCKET_ENV, "must name its socket"),
        (gascand::ENGINE_STATE_ROOT_ENV, "must name its state root"),
    ] {
        let environment = ArcaStartup::new()?;
        let output = environment
            .command(&["up", environment.root()?])
            .env_remove(variable)
            .output()?;
        assert!(
            !output.status.success(),
            "gascan up succeeded with {variable} unset"
        );
        let failure = startup_failure(&output)?;
        let code = gascan_core::startup_diagnostic::ENGINE_ENVIRONMENT_INCOMPLETE;
        assert!(
            failure.starts_with(&format!("{code}: ")),
            "{variable}: expected {code}, got: {failure}"
        );
        assert!(
            failure.contains(variable),
            "{variable} is not named in: {failure}"
        );
        assert!(
            failure.contains(expected_fragment),
            "{variable}: {expected_fragment:?} is not in: {failure}"
        );
    }
    Ok(())
}

/// **The engine's own failure reaches the user, not a readiness timeout.**
///
/// `GASCAN_ENGINE_BIN` names a path that does not exist, so the supervisor
/// dials, misses, spawns, and the spawn fails. That is an `EngineError`, and
/// every `EngineError` variant used to end at a `Stdio::null()` stderr.
#[test]
fn an_engine_that_cannot_be_spawned_reaches_the_user_as_an_engine_error() -> TestResult {
    let environment = ArcaStartup::new()?;
    let output = environment.command(&["up", environment.root()?]).output()?;
    assert!(
        !output.status.success(),
        "gascan up succeeded with no engine"
    );
    let failure = startup_failure(&output)?;
    let code = gascan_core::startup_diagnostic::ENGINE_SUPERVISION_IO;
    assert!(
        failure.starts_with(&format!("{code}: ")),
        "expected {code}, got: {failure}"
    );
    assert!(
        failure.contains("engine supervision I/O error"),
        "the engine's own failure is not in: {failure}"
    );
    Ok(())
}
