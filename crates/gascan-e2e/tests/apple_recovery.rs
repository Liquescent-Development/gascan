#![forbid(unsafe_code)]
#![deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

mod apple_common;

use apple_common::{AppleE2e, TestResult};

#[test]
#[ignore = "requires supported Apple runtime and the locked workspace image"]
fn cli_recovers_from_stale_daemon_metadata_and_runtime_truth() -> TestResult {
    let env = AppleE2e::new("gate4-recovery")?;
    env.success(["up", env.root().to_str().ok_or("non-UTF-8 root")?])?;
    env.kill_daemon()?;

    std::fs::write(
        env.state_path().with_file_name("daemon.pid"),
        "2147483647\n",
    )?;
    assert_eq!(env.status_json()?["actual_state"], "running");

    env.success(["--sandbox", env.id(), "down"])?;
    assert_eq!(env.status_json()?["actual_state"], "stopped");
    env.success(["up", env.root().to_str().ok_or("non-UTF-8 root")?])?;
    assert_eq!(env.status_json()?["actual_state"], "running");

    env.success([
        "--sandbox",
        env.id(),
        "apply",
        env.root().to_str().ok_or("non-UTF-8 root")?,
    ])?;
    env.success(["--sandbox", env.id(), "destroy", "--yes"])?;
    env.assert_no_owned_resources()
}
