use camino::Utf8Path;
use gascan_core::manifest::{GascampSource, Manifest, NetworkMode, UserMode};
use std::collections::BTreeMap;

fn parse(source: &str) -> Result<Manifest, gascan_core::manifest::ManifestError> {
    Manifest::parse(source)
}

#[test]
fn unknown_manifest_key_is_rejected() {
    let error = parse("version = 1\nnetwork = 'offline'\nssh_agent = true\n")
        .expect_err("unknown keys must fail closed");
    assert!(error.to_string().contains("unknown field `ssh_agent`"));
}

#[test]
fn omitted_policy_uses_security_defaults() {
    let manifest = parse("version = 1\n").expect("minimal manifest parses");

    assert_eq!(manifest.name, None);
    assert_eq!(manifest.network, NetworkMode::Offline);
    assert_eq!(manifest.user, UserMode::Workspace);
    assert_eq!(manifest.gascamp, GascampSource::Bundled);
    assert_eq!(manifest.setup, None);
    assert_eq!(manifest.tools, BTreeMap::new());
    assert_eq!(manifest.ports, BTreeMap::new());
}

#[test]
fn complete_manifest_preserves_ordered_declarations_and_units() {
    let manifest = parse(
        "version = 1\nname = 'code'\nnetwork = 'networked'\nuser = 'root'\n\
         gascamp = '/workspace/gascamp'\nsetup = './.gascan/setup.sh'\n\
         [resources]\ncpus = 6\nmemory = '12GiB'\ndisk = '80GiB'\n\
         [tools]\nrust = 'stable'\nnode = 'lts'\n\
         [ports]\nweb = 3000\n",
    )
    .expect("documented manifest parses");

    assert_eq!(manifest.name.as_deref(), Some("code"));
    assert_eq!(manifest.network, NetworkMode::Networked);
    assert_eq!(manifest.user, UserMode::Root);
    assert_eq!(
        manifest.gascamp,
        GascampSource::Workspace(Utf8Path::new("/workspace/gascamp").to_owned())
    );
    assert_eq!(manifest.resources.cpus, Some(6));
    assert_eq!(
        manifest.resources.memory.map(|value| value.bytes()),
        Some(12 * 1024 * 1024 * 1024)
    );
    assert_eq!(
        manifest.resources.disk.map(|value| value.bytes()),
        Some(80 * 1024 * 1024 * 1024)
    );
    assert_eq!(
        manifest
            .tools
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["node", "rust"]
    );
    assert_eq!(manifest.ports.get("web"), Some(&3000));
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
        assert!(
            parse(source).is_err(),
            "accepted invalid manifest: {source}"
        );
    }
}

#[test]
fn load_uses_gascan_toml_and_rejects_non_directories() {
    let temp = tempfile::tempdir().expect("temporary root");
    let root = Utf8Path::from_path(temp.path()).expect("UTF-8 temporary path");
    std::fs::write(root.join("gascan.toml"), "version = 1\nname = 'loaded'\n")
        .expect("write manifest");
    assert_eq!(
        Manifest::load(root).expect("load manifest").name.as_deref(),
        Some("loaded")
    );

    let file = root.join("not-a-directory");
    std::fs::write(&file, "data").expect("write fixture");
    assert!(Manifest::load(&file).is_err());
}
