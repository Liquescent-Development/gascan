#![forbid(unsafe_code)]
#![deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

mod apple_common;

use apple_common::{AppleE2e, TestResult};
use serde::de::{Error as _, MapAccess, Visitor};
use std::collections::BTreeMap;

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
    assert_exact_active_tools(
        &inventory.stdout,
        [
            ("neovim", "0.11.3"),
            ("node", "24.18.0"),
            ("npm:@openai/codex", "0.10.0"),
        ],
    )?;

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
#[serde(deny_unknown_fields)]
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

#[test]
fn exact_active_tools_rejects_extra_or_inactive_records() {
    let exact = br#"{
        "neovim":[{"installed":true,"active":true,"version":"0.11.3"}],
        "node":[{"installed":true,"active":true,"version":"24.18.0"}],
        "npm:@openai/codex":[{"installed":true,"active":true,"version":"0.10.0"}]
    }"#;
    let expected = [
        ("neovim", "0.11.3"),
        ("node", "24.18.0"),
        ("npm:@openai/codex", "0.10.0"),
    ];
    assert!(assert_exact_active_tools(exact, expected).is_ok());

    let extra = br#"{
        "go":[{"installed":true,"active":true,"version":"1.26.5"}],
        "neovim":[{"installed":true,"active":true,"version":"0.11.3"}],
        "node":[{"installed":true,"active":true,"version":"24.18.0"}],
        "npm:@openai/codex":[{"installed":true,"active":true,"version":"0.10.0"}]
    }"#;
    assert!(assert_exact_active_tools(extra, expected).is_err());

    let inactive = br#"{
        "neovim":[{"installed":true,"active":false,"version":"0.11.3"}],
        "node":[{"installed":true,"active":true,"version":"24.18.0"}],
        "npm:@openai/codex":[{"installed":true,"active":true,"version":"0.10.0"}]
    }"#;
    assert!(assert_exact_active_tools(inactive, expected).is_err());

    let wrong_version = br#"{
        "neovim":[{"installed":true,"active":true,"version":"0.11.4"}],
        "node":[{"installed":true,"active":true,"version":"24.18.0"}],
        "npm:@openai/codex":[{"installed":true,"active":true,"version":"0.10.0"}]
    }"#;
    assert!(assert_exact_active_tools(wrong_version, expected).is_err());

    let unknown_field = br#"{
        "neovim":[{"installed":true,"active":true,"version":"0.11.3","source":"unexpected"}],
        "node":[{"installed":true,"active":true,"version":"24.18.0"}],
        "npm:@openai/codex":[{"installed":true,"active":true,"version":"0.10.0"}]
    }"#;
    assert!(assert_exact_active_tools(unknown_field, expected).is_err());

    let duplicate_tool = br#"{
        "neovim":[{"installed":true,"active":true,"version":"0.11.3"}],
        "node":[{"installed":true,"active":true,"version":"24.18.0"}],
        "node":[{"installed":true,"active":true,"version":"24.18.0"}],
        "npm:@openai/codex":[{"installed":true,"active":true,"version":"0.10.0"}]
    }"#;
    assert!(assert_exact_active_tools(duplicate_tool, expected).is_err());

    let multiple_records = br#"{
        "neovim":[{"installed":true,"active":true,"version":"0.11.3"}],
        "node":[
            {"installed":true,"active":true,"version":"24.18.0"},
            {"installed":true,"active":true,"version":"24.18.0"}
        ],
        "npm:@openai/codex":[{"installed":true,"active":true,"version":"0.10.0"}]
    }"#;
    assert!(assert_exact_active_tools(multiple_records, expected).is_err());
}
