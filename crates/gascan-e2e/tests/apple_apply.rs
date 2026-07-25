#![forbid(unsafe_code)]
#![deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

mod apple_common;

use apple_common::{AppleE2e, TestResult};
use serde::de::{Error as _, MapAccess, Visitor};
use std::collections::BTreeMap;

const PERSISTENT_WORKSTATION_SENTINELS: [&str; 15] = [
    "/home/workspace/.local/share/mise/image-replace-sentinel",
    "/home/workspace/.cache/mise/image-replace-sentinel",
    "/home/workspace/.config/gascan/image-replace-sentinel",
    "/home/workspace/.config/gascan/agents/claude/image-replace-sentinel",
    "/home/workspace/.config/gascan/agents/codex/image-replace-sentinel",
    "/home/workspace/.config/gascan/agents/pi/image-replace-sentinel",
    "/home/workspace/.config/gascan/herdr/image-replace-sentinel",
    "/home/workspace/.config/gascan/gh/image-replace-sentinel",
    "/home/workspace/.config/gascan/glab/image-replace-sentinel",
    "/home/workspace/.cache/claude/image-replace-sentinel",
    "/home/workspace/.cache/codex/image-replace-sentinel",
    "/home/workspace/.cache/pi/image-replace-sentinel",
    "/home/workspace/.cache/herdr/image-replace-sentinel",
    "/home/workspace/.cache/gh/image-replace-sentinel",
    "/home/workspace/.cache/glab/image-replace-sentinel",
];

#[test]
#[ignore = "requires supported Apple runtime, two compatible digest-qualified workspace images, and network access"]
fn image_replace_preserves_durable_resources_and_rolls_back_failure() -> TestResult {
    let predecessor = std::env::var("GASCAN_E2E_PREDECESSOR_IMAGE")
        .map_err(|_| "GASCAN_E2E_PREDECESSOR_IMAGE must name the compatible predecessor fixture")?;
    let approved = apple_common::approved_workspace_image()?;
    apple_common::validate_distinct_image_fixtures(&predecessor, approved)?;

    let env = AppleE2e::new_networked("image-replace")?;
    let root = std::path::Path::new(env.root());
    std::fs::create_dir(root.join(".gascan"))?;
    std::fs::write(
        root.join("gascan.toml"),
        "version = 1\nname = 'image-replace'\nnetwork = 'networked'\n\
         setup = './.gascan/setup.sh'\n",
    )?;
    std::fs::write(
        root.join(".gascan/setup.sh"),
        "#!/bin/sh\nset -eu\n\
         count=0\n\
         test ! -f /workspace/setup-count || read -r count </workspace/setup-count\n\
         count=$((count + 1))\n\
         printf '%s\\n' \"$count\" >/workspace/setup-count\n",
    )?;
    env.success_with_timeout(
        ["up", root.to_str().ok_or("non-UTF-8 root")?],
        std::time::Duration::from_secs(10 * 60),
    )?;
    assert_eq!(std::fs::read_to_string(root.join("setup-count"))?, "1\n");

    for path in PERSISTENT_WORKSTATION_SENTINELS {
        env.success([
            "--sandbox",
            env.id(),
            "run",
            "--",
            "sh",
            "-c",
            &format!("printf durable >{}", shell_quote(path)),
        ])?;
    }
    env.success(["--sandbox", env.id(), "down"])?;
    std::thread::sleep(std::time::Duration::from_secs(6));
    env.success(["up", root.to_str().ok_or("non-UTF-8 root")?])?;
    env.assert_owned_container_running()?;
    env.success(["--sandbox", env.id(), "run", "--", "true"])?;
    assert_compatible_fixture(&env)?;

    env.replace_owned_container_image(&predecessor, std::time::Duration::from_secs(10 * 60))?;
    env.seed_stored_image_resolution(&predecessor)?;
    assert_compatible_fixture(&env)?;
    let predecessor_snapshot = env.owned_runtime_snapshot()?;
    assert!(gascan_core::runtime::same_immutable_image(
        predecessor_snapshot.container_image(),
        &predecessor
    ));
    env.write_image_replace_root_sentinel()?;
    env.assert_image_replace_root_sentinel(true)?;

    let status = env.status_json()?;
    assert_image_changed(&status, &predecessor, approved)?;

    let up = env.success(["up", root.to_str().ok_or("non-UTF-8 root")?, "--json"])?;
    assert_json_phase(&up.stdout, "apply_required")?;
    assert_eq!(env.owned_runtime_snapshot()?, predecessor_snapshot);
    assert_eq!(std::fs::read_to_string(root.join("setup-count"))?, "1\n");
    env.assert_image_replace_root_sentinel(true)?;

    let apply = env.success_with_timeout(
        [
            "--sandbox",
            env.id(),
            "apply",
            root.to_str().ok_or("non-UTF-8 root")?,
            "--json",
        ],
        std::time::Duration::from_secs(10 * 60),
    )?;
    for phase in [
        "before_provision",
        "after_provision",
        "before_health",
        "after_health",
        "image_replaced",
    ] {
        assert_json_phase(&apply.stdout, phase)?;
    }
    assert_eq!(std::fs::read_to_string(root.join("setup-count"))?, "2\n");
    let approved_snapshot = env.owned_runtime_snapshot()?;
    assert!(gascan_core::runtime::same_immutable_image(
        approved_snapshot.container_image(),
        approved
    ));
    predecessor_snapshot.assert_retained_identities_equal(&approved_snapshot)?;
    assert_compatible_fixture(&env)?;
    env.assert_image_replace_root_sentinel(false)?;

    env.replace_owned_container_image(&predecessor, std::time::Duration::from_secs(10 * 60))?;
    env.seed_stored_image_resolution(&predecessor)?;
    env.write_image_replace_root_sentinel()?;
    std::fs::write(
        root.join(".gascan/setup.sh"),
        "#!/bin/sh\nset -eu\n\
         printf attempted >/workspace/setup-failure-ran\n\
         exit 42\n",
    )?;
    let failed = env.invoke_with_timeout(
        [
            "--sandbox",
            env.id(),
            "apply",
            root.to_str().ok_or("non-UTF-8 root")?,
            "--json",
        ],
        std::time::Duration::from_secs(10 * 60),
    )?;
    env.assert_exit_code(&failed, 70)?;
    assert_json_phase(&failed.stdout, "image_rollback")?;
    assert_json_error(&failed.stdout)?;
    assert_eq!(
        std::fs::read_to_string(root.join("setup-failure-ran"))?,
        "attempted"
    );
    let rolled_back = env.owned_runtime_snapshot()?;
    assert!(gascan_core::runtime::same_immutable_image(
        rolled_back.container_image(),
        &predecessor
    ));
    predecessor_snapshot.assert_retained_identities_equal(&rolled_back)?;
    assert_compatible_fixture(&env)?;
    env.assert_image_replace_root_sentinel(false)?;

    env.success(["--sandbox", env.id(), "destroy", "--yes"])?;
    env.assert_no_owned_resources()
}

fn assert_compatible_fixture(env: &AppleE2e) -> TestResult {
    let probes = PERSISTENT_WORKSTATION_SENTINELS
        .iter()
        .map(|path| format!("test \"$(cat {})\" = durable", shell_quote(path)))
        .collect::<Vec<_>>()
        .join("; ");
    let output = env.success([
        "--sandbox",
        env.id(),
        "run",
        "--",
        "sh",
        "-c",
        &format!("set -eu; test \"$(id -un)\" = workspace; {probes}"),
    ])?;
    if output.stdout.is_empty() {
        Ok(())
    } else {
        Err("fixture compatibility probe produced unexpected output".into())
    }
}

#[test]
fn persistent_workstation_sentinels_cover_every_managed_agent_and_forge_path() {
    for required in [
        "/home/workspace/.config/gascan/agents/claude/image-replace-sentinel",
        "/home/workspace/.config/gascan/agents/codex/image-replace-sentinel",
        "/home/workspace/.config/gascan/agents/pi/image-replace-sentinel",
        "/home/workspace/.config/gascan/herdr/image-replace-sentinel",
        "/home/workspace/.config/gascan/gh/image-replace-sentinel",
        "/home/workspace/.config/gascan/glab/image-replace-sentinel",
        "/home/workspace/.cache/claude/image-replace-sentinel",
        "/home/workspace/.cache/codex/image-replace-sentinel",
        "/home/workspace/.cache/pi/image-replace-sentinel",
        "/home/workspace/.cache/herdr/image-replace-sentinel",
        "/home/workspace/.cache/gh/image-replace-sentinel",
        "/home/workspace/.cache/glab/image-replace-sentinel",
    ] {
        assert!(
            PERSISTENT_WORKSTATION_SENTINELS.contains(&required),
            "missing persistence sentinel: {required}"
        );
    }
}

fn assert_image_changed(status: &serde_json::Value, current: &str, requested: &str) -> TestResult {
    let requirements = status["apply_requirements"]
        .as_array()
        .ok_or("status apply_requirements must be an array")?;
    let exact = requirements
        .iter()
        .filter(|requirement| requirement["reason"] == "image_changed")
        .collect::<Vec<_>>();
    let [requirement] = exact.as_slice() else {
        return Err(format!("expected one image_changed requirement: {requirements:?}").into());
    };
    let observed_current = requirement["current"]
        .as_str()
        .ok_or("image_changed current reference must be a string")?;
    let observed_requested = requirement["requested"]
        .as_str()
        .ok_or("image_changed requested reference must be a string")?;
    if !gascan_core::runtime::same_immutable_image(observed_current, current)
        || !gascan_core::runtime::same_immutable_image(observed_requested, requested)
    {
        return Err(format!("unexpected image replacement requirement: {requirement:?}").into());
    }
    Ok(())
}

fn assert_json_phase(output: &[u8], expected: &str) -> TestResult {
    let found = std::str::from_utf8(output)?
        .lines()
        .map(serde_json::from_str::<serde_json::Value>)
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .any(|event| event["phase"] == expected);
    if found {
        Ok(())
    } else {
        Err(format!("operation stream omitted phase {expected}").into())
    }
}

fn assert_json_error(output: &[u8]) -> TestResult {
    let found = std::str::from_utf8(output)?
        .lines()
        .map(serde_json::from_str::<serde_json::Value>)
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .any(|event| event["error"].is_object());
    if found {
        Ok(())
    } else {
        Err("failed replacement stream omitted its primary error".into())
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[test]
#[ignore = "requires supported Apple runtime and the locked workspace image"]
fn workstation_defaults_are_exact_credential_free_and_offline() -> TestResult {
    let env = AppleE2e::new("workstation-offline")?;
    let root = std::path::Path::new(env.root());
    env.success_with_timeout(
        ["up", root.to_str().ok_or("non-UTF-8 root")?],
        std::time::Duration::from_secs(10 * 60),
    )?;
    let contract = env.success_with_timeout(
        [
            "--sandbox",
            env.id(),
            "run",
            "--",
            "/opt/gascan/tests/workstation-contract.sh",
        ],
        std::time::Duration::from_secs(5 * 60),
    )?;
    if contract.stdout != b"workstation-contract-ok\n" {
        return Err(format!(
            "unexpected workstation contract output: {}",
            String::from_utf8_lossy(&contract.stdout)
        )
        .into());
    }
    env.success(["--sandbox", env.id(), "destroy", "--yes"])?;
    env.assert_no_owned_resources()
}

#[test]
#[ignore = "requires supported Apple runtime, locked workspace image, and network access"]
fn workstation_tools_override_wins_without_mutating_immutable_defaults() -> TestResult {
    const OVERRIDE: &str = "1.26.4";
    let env = AppleE2e::new_networked("workstation-override")?;
    let root = std::path::Path::new(env.root());
    env.success_with_timeout(
        ["up", root.to_str().ok_or("non-UTF-8 root")?],
        std::time::Duration::from_secs(10 * 60),
    )?;
    let before = env.success([
        "--sandbox",
        env.id(),
        "run",
        "--",
        "sh",
        "-c",
        "find /opt/gascan/workstation -type f -exec sha256sum {} + | LC_ALL=C sort | sha256sum",
    ])?;

    env.write_manifest(&format!(
        "version = 1\nname = 'workstation-override'\nnetwork = 'networked'\n\
         [tools]\ngo = '{OVERRIDE}'\n"
    ))?;
    env.success_with_timeout(
        [
            "--sandbox",
            env.id(),
            "apply",
            root.to_str().ok_or("non-UTF-8 root")?,
        ],
        std::time::Duration::from_secs(20 * 60),
    )?;
    let proof = env.success([
        "--sandbox",
        env.id(),
        "run",
        "--",
        "sh",
        "-c",
        "set -eu; test \"$(command -v go)\" = /home/workspace/.local/share/mise/shims/go; go version; \
         find /opt/gascan/workstation -type f -exec sha256sum {} + | LC_ALL=C sort | sha256sum",
    ])?;
    let text = std::str::from_utf8(&proof.stdout)?;
    let mut lines = text.lines();
    if lines.next() != Some(&format!("go version go{OVERRIDE} linux/arm64")) {
        return Err(format!("mise override did not win exactly: {text}").into());
    }
    let after = lines
        .next()
        .ok_or("immutable workstation digest is absent")?;
    if lines.next().is_some() || before.stdout != format!("{after}\n").as_bytes() {
        return Err("immutable /opt/gascan/workstation content changed during override".into());
    }

    env.success(["--sandbox", env.id(), "destroy", "--yes"])?;
    env.assert_no_owned_resources()
}

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
    assert_exact_active_tools(&inventory.stdout, EXPECTED_APPLIED_TOOLS)?;

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

#[derive(serde::Deserialize)]
struct MiseToolRecord {
    version: String,
    installed: bool,
    active: bool,
}

struct MiseInventory(BTreeMap<String, Vec<MiseToolRecord>>);

impl<'de> serde::Deserialize<'de> for MiseInventory {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct InventoryVisitor;

        impl<'de> Visitor<'de> for InventoryVisitor {
            type Value = MiseInventory;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a mise tool inventory object with unique tool keys")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut records = BTreeMap::new();
                while let Some((tool, versions)) =
                    map.next_entry::<String, Vec<MiseToolRecord>>()?
                {
                    if records.insert(tool, versions).is_some() {
                        return Err(A::Error::custom("duplicate mise tool key"));
                    }
                }
                Ok(MiseInventory(records))
            }
        }

        deserializer.deserialize_map(InventoryVisitor)
    }
}

fn assert_exact_active_tools<const N: usize>(
    output: &[u8],
    expected: [(&str, &str); N],
) -> TestResult {
    let MiseInventory(records) = serde_json::from_slice(output)?;
    let expected =
        BTreeMap::from(expected.map(|(tool, version)| (tool.to_owned(), version.to_owned())));
    if !records.keys().eq(expected.keys()) {
        return Err(format!(
            "unexpected active tool set: {:?}",
            records.keys().collect::<Vec<_>>()
        )
        .into());
    }
    for (tool, expected_version) in expected {
        let entries = &records[&tool];
        let [entry] = entries.as_slice() else {
            return Err(format!("mise returned multiple records for {tool}").into());
        };
        if !entry.installed || !entry.active || entry.version != expected_version {
            return Err(format!(
                "mise returned an inactive or unexpected version for {tool}: {}",
                entry.version
            )
            .into());
        }
    }
    Ok(())
}

const EXPECTED_TOOLS: [(&str, &str); 3] = [
    ("neovim", "0.11.3"),
    ("node", "24.18.0"),
    ("npm:@openai/codex", "0.10.0"),
];

const EXPECTED_APPLIED_TOOLS: [(&str, &str); 10] = [
    ("elixir", "1.20.2-otp-29"),
    ("erlang", "29.0.3"),
    ("go", "1.26.5"),
    ("java", "25.0.2"),
    ("neovim", "0.11.3"),
    ("node", "24.18.0"),
    ("npm:@openai/codex", "0.10.0"),
    ("python", "3.14.6"),
    ("ruby", "3.4.10"),
    ("rust", "1.97.0"),
];

#[test]
fn exact_active_tools_accepts_exact_minimal_inventory() {
    let exact = br#"{
        "neovim":[{"installed":true,"active":true,"version":"0.11.3"}],
        "node":[{"installed":true,"active":true,"version":"24.18.0"}],
        "npm:@openai/codex":[{"installed":true,"active":true,"version":"0.10.0"}]
    }"#;
    assert!(assert_exact_active_tools(exact, EXPECTED_TOOLS).is_ok());
}

#[test]
fn exact_active_tools_rejects_tool_set_flags_and_version_mismatches() {
    let extra = br#"{
        "go":[{"installed":true,"active":true,"version":"1.26.5"}],
        "neovim":[{"installed":true,"active":true,"version":"0.11.3"}],
        "node":[{"installed":true,"active":true,"version":"24.18.0"}],
        "npm:@openai/codex":[{"installed":true,"active":true,"version":"0.10.0"}]
    }"#;
    assert!(assert_exact_active_tools(extra, EXPECTED_TOOLS).is_err());

    let inactive = br#"{
        "neovim":[{"installed":true,"active":false,"version":"0.11.3"}],
        "node":[{"installed":true,"active":true,"version":"24.18.0"}],
        "npm:@openai/codex":[{"installed":true,"active":true,"version":"0.10.0"}]
    }"#;
    assert!(assert_exact_active_tools(inactive, EXPECTED_TOOLS).is_err());

    let wrong_version = br#"{
        "neovim":[{"installed":true,"active":true,"version":"0.11.4"}],
        "node":[{"installed":true,"active":true,"version":"24.18.0"}],
        "npm:@openai/codex":[{"installed":true,"active":true,"version":"0.10.0"}]
    }"#;
    assert!(assert_exact_active_tools(wrong_version, EXPECTED_TOOLS).is_err());
}

#[test]
fn exact_active_tools_accepts_realistic_mise_metadata() {
    let realistic_metadata = br#"{
        "neovim":[{
            "installed":true,
            "active":true,
            "version":"0.11.3",
            "source":{"type":"global","path":"/home/workspace/.config/gascan/mise.toml"},
            "requested_version":"0.11.3",
            "install_path":"/home/workspace/.local/share/mise/installs/neovim/0.11.3",
            "symlinked_to":null
        }],
        "node":[{
            "installed":true,
            "active":true,
            "version":"24.18.0",
            "source":{"type":"global","path":"/home/workspace/.config/gascan/mise.toml"},
            "requested_version":"24.18.0",
            "install_path":"/opt/gascan/mise/installs/node/24.18.0",
            "symlinked_to":null
        }],
        "npm:@openai/codex":[{
            "installed":true,
            "active":true,
            "version":"0.10.0",
            "source":{"type":"global","path":"/home/workspace/.config/gascan/mise.toml"},
            "requested_version":"0.10.0",
            "install_path":"/home/workspace/.local/share/mise/installs/npm-openai-codex/0.10.0",
            "symlinked_to":null
        }]
    }"#;
    assert!(assert_exact_active_tools(realistic_metadata, EXPECTED_TOOLS).is_ok());
}

#[test]
fn exact_active_tools_rejects_duplicate_tools_and_multiple_records() {
    let duplicate_tool = br#"{
        "neovim":[{"installed":true,"active":true,"version":"0.11.3"}],
        "node":[{"installed":true,"active":true,"version":"24.18.0"}],
        "node":[{"installed":true,"active":true,"version":"24.18.0"}],
        "npm:@openai/codex":[{"installed":true,"active":true,"version":"0.10.0"}]
    }"#;
    assert!(assert_exact_active_tools(duplicate_tool, EXPECTED_TOOLS).is_err());

    let multiple_records = br#"{
        "neovim":[{"installed":true,"active":true,"version":"0.11.3"}],
        "node":[
            {"installed":true,"active":true,"version":"24.18.0"},
            {"installed":true,"active":true,"version":"24.18.0"}
        ],
        "npm:@openai/codex":[{"installed":true,"active":true,"version":"0.10.0"}]
    }"#;
    assert!(assert_exact_active_tools(multiple_records, EXPECTED_TOOLS).is_err());
}
