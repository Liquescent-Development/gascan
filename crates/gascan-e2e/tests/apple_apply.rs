#![forbid(unsafe_code)]
#![deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

mod apple_common;

use apple_common::{AppleE2e, TestResult};

#[test]
#[ignore = "requires supported Apple runtime, locked workspace image, and network access"]
fn apply_installs_large_npm_tool_and_neovim_with_storage_override() -> TestResult {
    let env = AppleE2e::new_networked("storage-tools")?;
    let root = std::path::Path::new(env.root());
    env.write_manifest(
        "version = 1\nname = 'storage-tools'\nnetwork = 'networked'\n\
         [storage]\ntools = '11GiB'\ncache = '12GiB'\nconfig = '2GiB'\n",
    )?;
    env.success_with_timeout(
        ["up", root.to_str().ok_or("non-UTF-8 root")?],
        std::time::Duration::from_secs(10 * 60),
    )?;

    env.write_manifest(
        "version = 1\nname = 'storage-tools'\nnetwork = 'networked'\n\
         [storage]\ntools = '11GiB'\ncache = '12GiB'\nconfig = '2GiB'\n\
         [tools]\nnode = '24.18.0'\n\"npm:@openai/codex\" = '0.10.0'\nneovim = '0.11.3'\n",
    )?;
    env.success_with_timeout(
        [
            "--sandbox",
            env.id(),
            "apply",
            root.to_str().ok_or("non-UTF-8 root")?,
        ],
        std::time::Duration::from_secs(20 * 60),
    )?;

    let inventory = env.success([
        "--sandbox",
        env.id(),
        "run",
        "--",
        "mise",
        "ls",
        "--current",
        "--installed",
        "--json",
    ])?;
    assert_exact_active_tools(&inventory.stdout, ["neovim", "node", "npm:@openai/codex"])?;

    env.success(["--sandbox", env.id(), "destroy", "--yes"])?;
    env.assert_no_owned_resources()
}

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

fn assert_exact_active_tools<const N: usize>(output: &[u8], expected: [&str; N]) -> TestResult {
    let inventory: serde_json::Value = serde_json::from_slice(output)?;
    let records = inventory
        .as_object()
        .ok_or("mise inventory must be a JSON object")?;
    let observed = records.keys().map(String::as_str).collect::<Vec<_>>();
    if observed != expected {
        return Err(format!("unexpected active tool set: {observed:?}").into());
    }
    for tool in expected {
        let entries = records[tool]
            .as_array()
            .ok_or("mise tool records must be an array")?;
        let [entry] = entries.as_slice() else {
            return Err(format!("mise returned multiple records for {tool}").into());
        };
        if entry["installed"].as_bool() != Some(true)
            || entry["active"].as_bool() != Some(true)
            || entry["version"]
                .as_str()
                .is_none_or(|version| version.trim().is_empty())
        {
            return Err(format!("mise returned an inactive or invalid record for {tool}").into());
        }
    }
    Ok(())
}

#[test]
fn exact_active_tools_rejects_extra_or_inactive_records() {
    let exact = br#"{
        "neovim":[{"installed":true,"active":true,"version":"0.11.3"}],
        "npm:@openai/codex":[{"installed":true,"active":true,"version":"0.10.0"}]
    }"#;
    assert!(assert_exact_active_tools(exact, ["neovim", "npm:@openai/codex"]).is_ok());

    let extra = br#"{
        "node":[{"installed":true,"active":true,"version":"24.0.0"}],
        "neovim":[{"installed":true,"active":true,"version":"0.11.3"}],
        "npm:@openai/codex":[{"installed":true,"active":true,"version":"0.10.0"}]
    }"#;
    assert!(assert_exact_active_tools(extra, ["neovim", "npm:@openai/codex"]).is_err());

    let inactive = br#"{
        "neovim":[{"installed":true,"active":false,"version":"0.11.3"}],
        "npm:@openai/codex":[{"installed":true,"active":true,"version":"0.10.0"}]
    }"#;
    assert!(assert_exact_active_tools(inactive, ["neovim", "npm:@openai/codex"]).is_err());
}
