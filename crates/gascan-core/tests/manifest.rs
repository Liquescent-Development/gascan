use camino::Utf8Path;
use gascan_core::manifest::{Manifest, NetworkMode, UserMode};
use std::collections::BTreeMap;

fn load(source: &str) -> Result<Manifest, gascan_core::manifest::ManifestError> {
    let temp = tempfile::tempdir().expect("temporary manifest root");
    let root = Utf8Path::from_path(temp.path()).expect("UTF-8 temporary path");
    std::fs::write(root.join("gascan.toml"), source).expect("write manifest fixture");
    Manifest::load(root)
}

#[test]
fn unknown_manifest_key_is_rejected() {
    let error = load("version = 1\nnetwork = 'offline'\nssh_agent = true\n")
        .expect_err("unknown keys must fail closed");
    assert!(error.to_string().contains("unknown field `ssh_agent`"));
}

#[test]
fn omitted_policy_uses_security_defaults() {
    let manifest = load("version = 1\n").expect("minimal manifest parses");

    assert_eq!(manifest.name(), None);
    assert_eq!(manifest.network(), NetworkMode::Offline);
    assert_eq!(manifest.user(), UserMode::Workspace);
    assert!(manifest.gascamp().is_bundled());
    assert_eq!(manifest.gascamp().workspace_path(), None);
    assert_eq!(manifest.setup(), None);
    assert_eq!(manifest.tools(), &BTreeMap::new());
    assert_eq!(manifest.ports(), &BTreeMap::new());
}

#[test]
fn complete_manifest_preserves_ordered_declarations_and_units() {
    let manifest = load(
        "version = 1\nname = 'code'\nnetwork = 'networked'\nuser = 'root'\n\
         gascamp = '/workspace/gascamp'\nsetup = './.gascan/setup.sh'\n\
         [resources]\ncpus = 6\nmemory = '12GiB'\ndisk = '80GiB'\n\
         [tools]\nrust = 'stable'\nnode = 'lts'\n\
         [ports]\nweb = 3000\n",
    )
    .expect("documented manifest parses");

    assert_eq!(manifest.name(), Some("code"));
    assert_eq!(manifest.network(), NetworkMode::Networked);
    assert_eq!(manifest.user(), UserMode::Root);
    assert_eq!(
        manifest.gascamp().workspace_path(),
        Some(Utf8Path::new("/workspace/gascamp"))
    );
    assert_eq!(manifest.resources().cpus(), Some(6));
    assert_eq!(
        manifest.resources().memory().map(|value| value.bytes()),
        Some(12 * 1024 * 1024 * 1024)
    );
    assert_eq!(
        manifest.resources().disk().map(|value| value.bytes()),
        Some(80 * 1024 * 1024 * 1024)
    );
    assert_eq!(
        manifest
            .tools()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["node", "rust"]
    );
    assert_eq!(manifest.ports().get("web"), Some(&3000));
}

#[test]
fn invalid_versions_resource_units_and_setup_traversal_are_rejected() {
    for source in [
        "version = 2\n",
        "version = 1\n[resources]\nmemory = '12GB'\n",
        "version = 1\n[resources]\ndisk = '-1GiB'\n",
        "version = 1\nsetup = '../outside.sh'\n",
        "version = 1\nsetup = '/tmp/setup.sh'\n",
    ] {
        assert!(load(source).is_err(), "accepted invalid manifest: {source}");
    }
}

#[test]
fn resource_and_gascamp_policy_edges_are_rejected() {
    for source in [
        "version = 1\n[resources]\nthreads = 4\n",
        "version = 1\n[resources]\ncpus = 0\n",
        "version = 1\n[resources]\nmemory = '0GiB'\n",
        "version = 1\n[resources]\ndisk = '18446744073709551615TiB'\n",
        "version = 1\ngascamp = '/workspace/gascamp-sibling'\n",
    ] {
        assert!(load(source).is_err(), "accepted invalid policy: {source}");
    }
}

#[test]
fn load_uses_gascan_toml_and_rejects_non_directories() {
    let temp = tempfile::tempdir().expect("temporary root");
    let root = Utf8Path::from_path(temp.path()).expect("UTF-8 temporary path");
    std::fs::write(root.join("gascan.toml"), "version = 1\nname = 'loaded'\n")
        .expect("write manifest");
    assert_eq!(
        Manifest::load(root).expect("load manifest").name(),
        Some("loaded")
    );

    let file = root.join("not-a-directory");
    std::fs::write(&file, "data").expect("write fixture");
    assert!(Manifest::load(&file).is_err());
}

#[cfg(unix)]
#[test]
fn load_rejects_setup_symlink_that_escapes_the_canonical_root() {
    let temp = tempfile::tempdir().expect("temporary root");
    let root = Utf8Path::from_path(temp.path()).expect("UTF-8 temporary path");
    let outside = tempfile::tempdir().expect("outside directory");
    std::os::unix::fs::symlink(outside.path(), root.join("escape")).expect("escape symlink");
    std::fs::write(
        root.join("gascan.toml"),
        "version = 1\nsetup = './escape/setup.sh'\n",
    )
    .expect("write manifest");

    let error = Manifest::load(root).expect_err("setup symlink escape must fail closed");
    assert!(error.to_string().contains("outside the workspace root"));
}

#[cfg(unix)]
#[test]
fn load_classifies_an_unreadable_manifest_as_a_manifest_error()
-> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir()?;
    let root = Utf8Path::from_path(temp.path()).expect("UTF-8 temporary path");
    let manifest_path = root.join("gascan.toml");
    std::fs::write(&manifest_path, "version = 1\n")?;
    std::fs::set_permissions(&manifest_path, std::fs::Permissions::from_mode(0o000))?;

    let result = Manifest::load(root);
    std::fs::set_permissions(&manifest_path, std::fs::Permissions::from_mode(0o644))?;
    let error = result.expect_err("an unreadable manifest must fail to load");

    assert!(
        !error.is_project_root_error(),
        "an unreadable manifest is a manifest-content failure, not a project-root failure: {error}"
    );
    Ok(())
}

#[test]
fn load_allows_a_not_yet_created_setup_path_beneath_root() {
    let temp = tempfile::tempdir().expect("temporary root");
    let root = Utf8Path::from_path(temp.path()).expect("UTF-8 temporary path");
    std::fs::write(
        root.join("gascan.toml"),
        "version = 1\nsetup = './future/setup.sh'\n",
    )
    .expect("write manifest");

    assert_eq!(
        Manifest::load(root).expect("contained future path").setup(),
        Some(Utf8Path::new("./future/setup.sh"))
    );
}
