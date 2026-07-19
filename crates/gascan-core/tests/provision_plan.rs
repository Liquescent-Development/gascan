use camino::Utf8Path;
use gascan_core::manifest::Manifest;
use gascan_core::provision::{AppliedState, ProvisionStep, ProvisioningPlanner};
use std::error::Error;

type TestResult = Result<(), Box<dyn Error>>;

fn manifest(source: &str) -> Result<Manifest, Box<dyn Error>> {
    let root = tempfile::tempdir()?;
    std::fs::write(root.path().join("gascan.toml"), source)?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    Ok(Manifest::load(root)?)
}

#[test]
fn tool_change_requires_apply_and_emits_plain_mise_config() -> TestResult {
    let manifest = manifest("version = 1\n[tools]\nnode = 'lts'\n")?;
    let plan = ProvisioningPlanner::plan(&manifest, &AppliedState::empty())?;

    assert_eq!(plan.steps()[0], ProvisionStep::WriteSafeMiseConfig);
    assert_eq!(plan.steps()[1], ProvisionStep::InstallTools);
    let config = plan.safe_mise_toml()?.ok_or("safe config")?;
    assert!(config.contains("[tools]"));
    assert!(config.contains("node = \"lts\""));
    assert!(!config.contains("[env]"));
    assert!(!config.contains("hooks"));
    assert!(!config.contains("tasks"));
    assert!(!config.contains("plugins"));
    Ok(())
}

#[test]
fn safe_config_serializes_hostile_toml_text_without_creating_new_surfaces() -> TestResult {
    let manifest =
        manifest("version = 1\n[tools]\n\"odd\\\"tool\" = \"line\\n[env]\\nTOKEN='bad'\"\n")?;
    let plan = ProvisioningPlanner::plan(&manifest, &AppliedState::empty())?;
    let config = plan.safe_mise_toml()?.ok_or("safe config")?;
    let parsed: toml::Value = toml::from_str(&config)?;

    assert_eq!(
        parsed
            .get("tools")
            .and_then(|tools| tools.get("odd\"tool"))
            .and_then(toml::Value::as_str),
        Some("line\n[env]\nTOKEN='bad'")
    );
    assert_eq!(parsed.as_table().map(toml::map::Map::len), Some(1));
    Ok(())
}

#[test]
fn matching_applied_tool_hash_skips_tool_mutation_steps() -> TestResult {
    let manifest = manifest("version = 1\n[tools]\npython = '3.14'\nnode = '24'\n")?;
    let changed = ProvisioningPlanner::plan(&manifest, &AppliedState::empty())?;
    let applied = AppliedState::with_tool_hash(changed.desired_tool_hash());
    let unchanged = ProvisioningPlanner::plan(&manifest, &applied)?;

    assert!(
        !unchanged
            .steps()
            .contains(&ProvisionStep::WriteSafeMiseConfig)
    );
    assert!(!unchanged.steps().contains(&ProvisionStep::InstallTools));
    assert_eq!(unchanged.safe_mise_toml()?, None);
    Ok(())
}

#[test]
fn empty_tool_plan_still_contains_create_verification_boundaries() -> TestResult {
    let manifest = manifest("version = 1\n")?;
    let plan = ProvisioningPlanner::plan(&manifest, &AppliedState::empty())?;

    assert_eq!(
        plan.steps(),
        [ProvisionStep::VerifyGascamp, ProvisionStep::HealthCheck]
    );
    assert_eq!(plan.safe_mise_toml()?, None);
    Ok(())
}

#[test]
fn removing_last_tool_rewrites_an_empty_tools_only_config() -> TestResult {
    let with_node = manifest("version = 1\n[tools]\nnode = 'lts'\n")?;
    let installed = ProvisioningPlanner::plan(&with_node, &AppliedState::empty())?;
    let applied = AppliedState::with_tool_hash(installed.desired_tool_hash());
    let empty = manifest("version = 1\n")?;

    let removal = ProvisioningPlanner::plan(&empty, &applied)?;

    assert_eq!(removal.steps()[0], ProvisionStep::WriteSafeMiseConfig);
    let config = removal.safe_mise_toml()?.ok_or("removal config")?;
    let parsed: toml::Value = toml::from_str(&config)?;
    assert_eq!(
        parsed
            .get("tools")
            .and_then(toml::Value::as_table)
            .map(toml::map::Map::len),
        Some(0)
    );
    Ok(())
}

#[test]
fn planner_exposes_task_six_setup_boundary_without_executing_it() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::create_dir(root.path().join(".gascan"))?;
    std::fs::write(root.path().join(".gascan/setup.sh"), "#!/bin/sh\n")?;
    std::fs::write(
        root.path().join("gascan.toml"),
        "version = 1\nsetup = '.gascan/setup.sh'\n",
    )?;
    let root = Utf8Path::from_path(root.path()).ok_or("utf8 root")?;
    let manifest = Manifest::load(root)?;
    let plan = ProvisioningPlanner::plan(&manifest, &AppliedState::empty())?;

    assert!(plan.steps().contains(&ProvisionStep::RunSetup));
    assert!(plan.steps().contains(&ProvisionStep::VerifyGascamp));
    assert_eq!(plan.steps().last(), Some(&ProvisionStep::HealthCheck));
    Ok(())
}
