use camino::{Utf8Path, Utf8PathBuf};
use gascan_core::manifest::Manifest;
use gascan_core::provision::{AppliedState, ProvisionStep, ProvisioningPlanner};
use std::error::Error;

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

fn canonical_root(temp: &tempfile::TempDir) -> TestResult<Utf8PathBuf> {
    Utf8PathBuf::from_path_buf(temp.path().canonicalize()?).map_err(|_| "UTF-8 root".into())
}

fn setup_manifest(root: &Utf8Path, setup: &str) -> TestResult {
    std::fs::write(
        root.join("gascan.toml"),
        format!("version = 1\nsetup = {setup:?}\n"),
    )?;
    Ok(())
}

#[test]
fn setup_is_planned_by_canonical_relative_path_and_content_digest() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = canonical_root(&temp)?;
    std::fs::create_dir(root.join("scripts"))?;
    std::fs::write(root.join("scripts/setup.sh"), b"abc")?;
    setup_manifest(&root, "./scripts/setup.sh")?;
    let manifest = Manifest::load(&root)?;

    let initial = ProvisioningPlanner::plan_for_root(&root, &manifest, &AppliedState::empty())?;
    let setup = initial.setup_script().ok_or("planned setup")?;
    assert_eq!(
        setup.canonical_relative_path(),
        Utf8Path::new("scripts/setup.sh")
    );
    assert_eq!(
        setup.sha256(),
        "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert!(initial.steps().contains(&ProvisionStep::RunSetup));

    let unchanged = ProvisioningPlanner::plan_for_root(
        &root,
        &manifest,
        &AppliedState::with_setup_sha256(setup.sha256()),
    )?;
    assert!(!unchanged.steps().contains(&ProvisionStep::RunSetup));

    std::fs::write(root.join("scripts/setup.sh"), b"changed")?;
    let changed = ProvisioningPlanner::plan_for_root(
        &root,
        &Manifest::load(&root)?,
        &AppliedState::with_setup_sha256(setup.sha256()),
    )?;
    assert!(changed.steps().contains(&ProvisionStep::RunSetup));
    assert_ne!(
        changed.setup_script().ok_or("changed setup")?.sha256(),
        setup.sha256()
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn setup_rejects_symlinks_even_when_they_resolve_inside_the_root() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = canonical_root(&temp)?;
    std::fs::write(root.join("real.sh"), b"true\n")?;
    std::os::unix::fs::symlink("real.sh", root.join("setup.sh"))?;
    setup_manifest(&root, "setup.sh")?;
    let manifest = Manifest::load(&root)?;

    let error = ProvisioningPlanner::plan_for_root(&root, &manifest, &AppliedState::empty())
        .expect_err("setup symlink must be rejected");
    assert_eq!(
        error.to_string(),
        "setup script path contains a symbolic link"
    );
    Ok(())
}

#[test]
fn setup_rejects_non_regular_inputs() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = canonical_root(&temp)?;
    std::fs::create_dir(root.join("setup"))?;
    setup_manifest(&root, "setup")?;
    let manifest = Manifest::load(&root)?;

    let error = ProvisioningPlanner::plan_for_root(&root, &manifest, &AppliedState::empty())
        .expect_err("setup directory must be rejected");
    assert_eq!(error.to_string(), "setup script is not a regular file");
    Ok(())
}

#[test]
fn manifest_rejects_setup_traversal_before_planning() -> TestResult {
    let temp = tempfile::tempdir()?;
    let root = canonical_root(&temp)?;
    setup_manifest(&root, "../setup.sh")?;

    assert!(Manifest::load(&root).is_err());
    Ok(())
}
