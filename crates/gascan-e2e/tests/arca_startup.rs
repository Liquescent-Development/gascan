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

/// **`gascan doctor` answers when the daemon cannot start.**
///
/// This is the acceptance for the pair. The host half of the report is real --
/// measured in the CLI's own process, by the same
/// `gascan_core::doctor::host` functions the daemon calls -- and every check
/// that needed a daemon carries the daemon's own startup diagnostic as its
/// detail, rather than "the daemon could not be reached".
///
/// Doctor used to sit behind `connect_with_recovery_progress`, which is the
/// defect in one sentence: the command a user runs BECAUSE the daemon will not
/// start required the daemon to start. `gascan engine fetch` already had this
/// early return and its comment already stated the principle -- "Requiring a
/// daemon here would make the remedy depend on the thing it repairs".
#[test]
fn doctor_reports_real_host_facts_and_names_the_runtime_cause() -> TestResult {
    let environment = ArcaStartup::new()?;
    let output = environment
        .command(&["doctor", "--json"])
        .env_remove(gascand::ENGINE_BIN_ENV)
        .output()?;
    assert!(
        !output.status.success(),
        "doctor passed with no daemon and no engine"
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).map_err(|error| {
        format!(
            "doctor produced no JSON report ({error}): stdout={}, stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })?;
    let checks = report["checks"]
        .as_array()
        .ok_or("doctor report has no checks")?;
    let check = |id: &str| -> TestResult<serde_json::Value> {
        checks
            .iter()
            .find(|check| check["id"] == id)
            .cloned()
            .ok_or_else(|| format!("{id} is missing from the report").into())
    };

    // **The host half is measured, and the assertion is that it was measured
    // -- not that this machine passes.** Asserting `pass` here would encode
    // "the test host is aarch64 on macOS 26+" into a test about whether the
    // CLI can answer without a daemon, so the same correct code would fail on
    // an Intel host or an older macOS for a reason it does not test. What must
    // be true everywhere is that these facts carry their own evidence rather
    // than the daemon's cause.
    let code = gascan_core::startup_diagnostic::ENGINE_ENVIRONMENT_INCOMPLETE;
    for (id, evidence) in [
        ("host.architecture", "current process target is"),
        ("host.macos", "ProductVersion"),
    ] {
        let fact = check(id)?;
        let detail = fact["detail"].as_str().unwrap_or_default();
        assert!(
            !detail.contains(code),
            "{id} carries the daemon's cause instead of a measurement: {fact}"
        );
        assert!(
            detail.contains(evidence),
            "{id} was not measured here: {fact}"
        );
        assert_ne!(fact["status"], "unknown", "{id} was not evaluated: {fact}");
    }

    // The engine executable is a host fact too, and it names the variable
    // rather than repeating the daemon's diagnostic: the CLI measured it.
    let engine_binary = check("runtime.cli")?;
    assert_eq!(engine_binary["status"], "fail");
    assert!(
        engine_binary["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains(gascand::ENGINE_BIN_ENV)),
        "runtime.cli does not name the variable: {engine_binary}"
    );

    // Everything that needed a daemon carries the daemon's own cause. Asserted
    // over every such check and not just one, because a report that named the
    // cause once and said "unknown" everywhere else is the degraded fallback
    // this deliberately is not.
    for id in [
        "runtime.version",
        "runtime.service",
        "runtime.schema",
        "storage.state",
        "storage.images",
    ] {
        let fact = check(id)?;
        assert_eq!(fact["status"], "fail", "{id}: {fact}");
        assert!(
            fact["detail"].as_str().is_some_and(|d| d.contains(code)),
            "{id} does not carry the daemon's cause: {fact}"
        );
    }

    // **Nothing that never needed a daemon is blamed on one.** The workspace
    // is a stat of the CLI's own directory and `ssh.client` is a stat of
    // `/usr/bin/ssh`; a report that told the user the engine environment was
    // why `/usr/bin/ssh` could not be checked would be saying something false.
    for id in ["workspace.access", "ssh.client"] {
        let fact = check(id)?;
        assert!(
            !fact["detail"].as_str().unwrap_or_default().contains(code),
            "{id} blames the daemon's startup cause for a fact measured here: {fact}"
        );
    }

    // The remedies are the engine backend's. An Arca user told to install
    // Apple container is the defect `DoctorRemedies` exists to close, and the
    // CLI now assembles this report itself -- a second place that could get it
    // wrong.
    for check_value in checks {
        let remedy = check_value["remedy"].as_str().unwrap_or_default();
        assert!(
            !remedy.contains("Apple container"),
            "{} carries Apple's remedy on the Arca backend: {remedy}",
            check_value["id"]
        );
    }
    Ok(())
}

/// The engine's artifacts are reported by the CLI, with their own remedy.
///
/// `engine_artifact_fact` is the check Task 13 built to say `run gascan engine
/// fetch`, and Task 11's startup ordering made it unreachable in exactly the
/// state it describes: a daemon that cannot start because the artifacts are
/// missing cannot be asked whether the artifacts are missing.
#[test]
fn the_engine_artifact_check_is_answered_without_a_daemon() -> TestResult {
    let environment = ArcaStartup::new()?;
    let output = environment
        .command(&["doctor", "--json"])
        .env_remove(gascand::ENGINE_BIN_ENV)
        .output()?;
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let kernel = report["checks"]
        .as_array()
        .ok_or("doctor report has no checks")?
        .iter()
        .find(|check| check["id"] == "runtime.kernel")
        .cloned()
        .ok_or("runtime.kernel is missing from the report")?;
    let detail = kernel["detail"].as_str().unwrap_or_default();
    let code = gascan_core::startup_diagnostic::ENGINE_ENVIRONMENT_INCOMPLETE;
    assert!(
        !detail.contains(code),
        "runtime.kernel fell through to the daemon's cause instead of being measured: {kernel}"
    );
    // Asserted on the REMEDY, not on the detail's prose. The detail depends on
    // what this developer's account actually has installed -- `engine_artifact_fact`
    // has three branches and the digest-mismatch one returns the error's own
    // string, which need not mention "engine artifacts" at all. The remedy is
    // the same in every failing branch and is the thing the check exists to say.
    let remedy = kernel["remedy"].as_str().unwrap_or_default();
    assert!(
        remedy.contains("gascan engine fetch") || remedy.contains("engine/arca-pin.json"),
        "runtime.kernel does not carry the artifact remedy: {kernel}"
    );
    Ok(())
}
