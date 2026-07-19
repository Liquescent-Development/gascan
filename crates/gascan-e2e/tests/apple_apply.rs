#![forbid(unsafe_code)]
#![deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

mod apple_common;

use apple_common::{AppleE2e, TestResult};

#[test]
#[ignore = "requires supported Apple runtime and the locked workspace image"]
fn changed_setup_is_reported_but_not_run_by_up_or_shell() -> TestResult {
    let env = AppleE2e::new("gate4-apply-setup")?;
    let root = std::path::Path::new(env.root());
    std::fs::create_dir(root.join(".gascan"))?;
    std::fs::write(
        root.join("gascan.toml"),
        "version = 1\nname = 'gate4-apply-setup'\nsetup = './.gascan/setup.sh'\n",
    )?;
    std::fs::write(
        root.join(".gascan/setup.sh"),
        "printf first > /workspace/result\n",
    )?;

    env.success(["up", root.to_str().ok_or("non-UTF-8 root")?])?;
    assert_eq!(std::fs::read_to_string(root.join("result"))?, "first");

    std::fs::write(
        root.join(".gascan/setup.sh"),
        "printf second > /workspace/result\n",
    )?;
    let up = env.success(["up", root.to_str().ok_or("non-UTF-8 root")?, "--json"])?;
    assert!(
        String::from_utf8_lossy(&up.stdout).contains("apply_required"),
        "changed setup was not reported: {}",
        String::from_utf8_lossy(&up.stdout)
    );
    env.success(["--sandbox", env.id(), "shell", "--", "true"])?;
    assert_eq!(std::fs::read_to_string(root.join("result"))?, "first");

    env.success([
        "--sandbox",
        env.id(),
        "apply",
        root.to_str().ok_or("non-UTF-8 root")?,
    ])?;
    assert_eq!(std::fs::read_to_string(root.join("result"))?, "second");
    env.success(["--sandbox", env.id(), "destroy", "--yes"])?;
    env.assert_no_owned_resources()
}
