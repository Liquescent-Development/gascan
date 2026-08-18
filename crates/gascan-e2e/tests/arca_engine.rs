#![forbid(unsafe_code)]

//! One pass of the product over a real engine: up, exec, logs, restart, down.
//!
//! **This is the only instrument in the repository that runs `gascand`'s own
//! engine command line.** Every other test of the supervisor spawns through a
//! fixture -- a counting spawner, `/usr/bin/false`, a helper that binds the
//! socket itself -- so none of them can see what arguments the daemon actually
//! passes. MEASURED before this file existed: the daemon passed `--socket-path`
//! alone and the pinned engine exits **64** on `Missing expected argument
//! '--state-root <state-root>'`, so the spawn arm of `ensure_engine` could never
//! have succeeded on any host.

mod arca_common;

use arca_common::{ArcaE2e, STARTUP_MARKER, TestResult};
use std::time::Duration;

/// A bound long enough for a cold guest boot and short enough to fail rather
/// than hang.
///
/// The engine builds an initfs on first use, boots a virtual machine and mounts
/// four block devices. A `gascan up` against a warm store on this host is a few
/// seconds; the bound is generous because the failure this guards against is a
/// test that never returns, which under `--test-threads=1` stalls the whole
/// tier.
const BOOT: Duration = Duration::from_secs(180);

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

/// **The whole control plane over one real sandbox on the engine.**
///
/// What it proves, and each of these is reachable no other way:
///
/// - `gascand` **spawned an engine that serves**, from a command line it built
///   itself out of `GASCAN_ENGINE_BIN`, `GASCAN_ENGINE_SOCKET`,
///   `GASCAN_ENGINE_STATE_ROOT` and the artifacts `gascan engine fetch`
///   installed. Nothing in this file passes the engine an argument.
/// - `gascan up` drives `Create` and `Start` **through the daemon's policy**,
///   not through a hand-built `CreateRequest` the way the `gascan-arca` tier
///   does, so the compiled topology is the product's.
/// - `gascan run` reaches the guest and returns **the guest's own exit status**,
///   which is `Exec`'s contract and not the CLI's.
/// - `gascan logs` returns what PID 1 wrote to **both** streams.
/// - **A restarted daemon adopts what its predecessor left running.** This is
///   the property Task 11's dial-first supervisor exists for: the engine is
///   deliberately not reaped when the daemon that spawned it dies, so the second
///   daemon must dial the surviving engine rather than start a second one, and
///   `reconcile()` must find the sandbox still there.
/// - `gascan down` then stops **the adopted sandbox**, so the adoption is
///   control and not merely observation.
///
/// **The restart is placed between `logs` and `down` on purpose.** A restart
/// after `down` would prove only that a stopped sandbox survives, which is a
/// database property. Between them, the sandbox the second daemon adopts is a
/// running one, and the `down` that follows is the evidence it can act on it.
#[test]
#[ignore = "requires a built arca-engine named by GASCAN_ARCA_ENGINE_BIN, a base OCI layout \
            named by GASCAN_ARCA_BASE_OCI_LAYOUT, and the boot artifacts gascan engine fetch \
            installs"]
fn a_daemon_spawned_engine_runs_a_sandbox_through_up_exec_logs_restart_and_down() -> TestResult {
    let env = ArcaE2e::new("arca-e2e", "networked")?;
    let root = env.root().to_str().ok_or("non-UTF-8 root")?.to_owned();

    // Nothing is listening on this socket, so this `up` takes the supervisor's
    // spawn arm. An engine holding the lock afterwards is the evidence.
    env.success(["up", &root])?;
    let spawned = env
        .engine_pid()
        .ok_or("no engine holds the socket after `gascan up`")?;

    assert_eq!(env.status_json()?["actual_state"], "running");

    // The guest's own status, not the CLI's. `Exec` carries the exit code in
    // its final frame and a client that lost it would report success for a
    // command that failed.
    let exit = env.invoke(["--sandbox", env.id(), "run", "--", "sh", "-c", "exit 42"])?;
    assert_eq!(
        exit.status.code(),
        Some(42),
        "guest exit status was not carried back: stdout={} stderr={} daemon_stderr={}",
        String::from_utf8_lossy(&exit.stdout),
        String::from_utf8_lossy(&exit.stderr),
        env.bounded_daemon_stderr()
    );

    let echoed = env.success([
        "--sandbox",
        env.id(),
        "run",
        "--",
        "sh",
        "-c",
        "echo GASCAN_ARCA_E2E_EXEC",
    ])?;
    assert!(
        contains(&echoed.stdout, "GASCAN_ARCA_E2E_EXEC"),
        "exec stdout did not reach the client: {}",
        String::from_utf8_lossy(&echoed.stdout)
    );

    // PID 1 writes both markers as it starts, so a log that is merely late is
    // told from one that is empty by waiting for them rather than reading once.
    env.until("both PID 1 markers reach `gascan logs`", BOOT, || {
        let logs = env.success(["--sandbox", env.id(), "logs"])?;
        let seen = String::from_utf8_lossy(&logs.stdout).into_owned()
            + &String::from_utf8_lossy(&logs.stderr);
        Ok(seen.contains(&format!("{STARTUP_MARKER}-stdout"))
            && seen.contains(&format!("{STARTUP_MARKER}-stderr")))
    })?;

    // The daemon dies; the engine does not. Everything after this is the
    // adoption path.
    let first_daemon = env.daemon_pid().ok_or("the daemon wrote no pid")?;
    env.kill_daemon()?;
    // Liveness and not just the recorded pid. `<socket>.lock` outlives the
    // engine that wrote it, so reading the file alone would report a dead
    // engine as a surviving one and turn this into an assertion about a file.
    assert_eq!(
        (env.engine_pid(), env.engine_alive()),
        (Some(spawned), Some(true)),
        "the engine did not outlive the daemon that spawned it"
    );

    // A fresh daemon, which reconciles before it serves. It must adopt the
    // surviving engine rather than spawn a second -- a second could not bind
    // the socket, and the `flock` holder is still the first.
    assert_eq!(env.status_json()?["actual_state"], "running");
    let second_daemon = env
        .daemon_pid()
        .ok_or("the restarted daemon wrote no pid")?;
    assert_ne!(
        second_daemon, first_daemon,
        "no daemon restart happened, so nothing adopted anything"
    );
    assert_eq!(
        (env.engine_pid(), env.engine_alive()),
        (Some(spawned), Some(true)),
        "the restarted daemon started a second engine instead of adopting the first"
    );

    // Acting on the adopted sandbox, not merely reporting it.
    env.success(["--sandbox", env.id(), "down"])?;
    assert_eq!(env.status_json()?["actual_state"], "stopped");

    env.success(["--sandbox", env.id(), "destroy", "--yes"])?;
    Ok(())
}
