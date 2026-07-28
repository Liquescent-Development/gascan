use camino::Utf8Path;
use gascan_core::manifest::{
    DEFAULT_CACHE_STORAGE_BYTES, DEFAULT_CONFIG_STORAGE_BYTES, DEFAULT_TOOLS_STORAGE_BYTES,
    Manifest, NetworkMode, ShellPrompt, Ssh, UserMode,
};
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
fn shell_prompt_defaults_accepts_supported_values_and_rejects_invalid_configuration()
-> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        load("version = 1\n")?.shell().prompt(),
        ShellPrompt::Standard
    );
    assert_eq!(
        load("version = 1\n[shell]\n")?.shell().prompt(),
        ShellPrompt::Standard
    );
    for (value, expected) in [
        ("standard", ShellPrompt::Standard),
        ("starship", ShellPrompt::Starship),
        ("starship-nerd-font", ShellPrompt::StarshipNerdFont),
    ] {
        let manifest = load(&format!("version = 1\n[shell]\nprompt = '{value}'\n"))?;
        assert_eq!(manifest.shell().prompt(), expected);
        assert_eq!(expected.as_str(), value);
    }
    assert!(
        load("version = 1\n[shell]\nprompt = 'spaceship'\n")
            .unwrap_err()
            .to_string()
            .contains("unknown variant")
    );
    assert!(
        load("version = 1\n[shell]\ncommand = 'bash'\n")
            .unwrap_err()
            .to_string()
            .contains("unknown field `command`")
    );
    Ok(())
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
fn ssh_policy_defaults_to_the_resolved_network_mode() {
    for (source, expected_enabled) in [
        ("version = 1\nnetwork = 'networked'\n", true),
        ("version = 1\nnetwork = 'offline'\n", false),
        (
            "version = 1\nnetwork = 'networked'\n[ssh]\nenabled = false\n",
            false,
        ),
        (
            "version = 1\nnetwork = 'networked'\n[ssh]\nenabled = true\nhost_port = 2222\n",
            true,
        ),
    ] {
        let manifest = load(source).expect("valid SSH policy parses");
        assert_eq!(
            manifest.ssh().enabled(),
            expected_enabled,
            "resolved wrong SSH state for {source}"
        );
    }

    let offline = load("version = 1\nnetwork = 'offline'\n").expect("offline policy parses");
    assert_eq!(offline.ssh(), &Ssh::default());
    assert_eq!(offline.ssh().host_port(), None);
}

#[test]
fn ssh_policy_rejects_explicit_enablement_while_offline() {
    let error = load("version = 1\nnetwork = 'offline'\n[ssh]\nenabled = true\n")
        .expect_err("offline SSH must fail closed");

    match error {
        gascan_core::manifest::ManifestError::Invalid(message) => assert_eq!(
            message,
            "ssh requires network = \"networked\"; disable SSH or enable sandbox networking"
        ),
        other => panic!("expected invalid SSH/network policy, got {other}"),
    }
}

#[test]
fn ssh_policy_rejects_unknown_zero_privileged_and_disabled_host_ports() {
    for source in [
        "version = 1\n[ssh]\nagent_forwarding = true\n",
        "version = 1\n[ssh]\nhost_port = 0\n",
        "version = 1\n[ssh]\nhost_port = 1023\n",
        "version = 1\n[ssh]\nenabled = false\nhost_port = 22222\n",
    ] {
        assert!(
            load(source).is_err(),
            "accepted invalid SSH policy: {source}"
        );
    }
}

#[test]
fn storage_defaults_and_partial_overrides_are_independent() {
    let defaults = load("version = 1\n").unwrap();
    assert_eq!(
        defaults.storage().tools().bytes(),
        DEFAULT_TOOLS_STORAGE_BYTES
    );
    assert_eq!(
        defaults.storage().cache().bytes(),
        DEFAULT_CACHE_STORAGE_BYTES
    );
    assert_eq!(
        defaults.storage().config().bytes(),
        DEFAULT_CONFIG_STORAGE_BYTES
    );

    let partial = load("version = 1\n[storage]\ntools = '30GiB'\n").unwrap();
    assert_eq!(partial.storage().tools().bytes(), 30 * 1024_u64.pow(3));
    assert_eq!(
        partial.storage().cache().bytes(),
        DEFAULT_CACHE_STORAGE_BYTES
    );
    assert_eq!(
        partial.storage().config().bytes(),
        DEFAULT_CONFIG_STORAGE_BYTES
    );
}

#[test]
fn storage_invalid_boundaries_are_rejected_and_maximum_is_accepted() {
    for source in [
        "version = 1\n[storage]\ntools = '0GiB'\n",
        "version = 1\n[storage]\ncache = '10GB'\n",
        "version = 1\n[storage]\nconfig = '513GiB'\n",
        "version = 1\n[storage]\nunknown = '1GiB'\n",
    ] {
        assert!(
            load(source).is_err(),
            "accepted invalid storage policy: {source}"
        );
    }

    for field in ["tools", "cache", "config"] {
        let source = format!("version = 1\n[storage]\n{field} = '512GiB'\n");
        assert!(
            load(&source).is_ok(),
            "rejected maximum storage policy: {source}"
        );
    }
}

#[test]
fn storage_values_above_maximum_report_the_field() {
    for field in ["tools", "cache", "config"] {
        let source = format!("version = 1\n[storage]\n{field} = '513GiB'\n");
        let error = load(&source).expect_err("oversized managed volume must be rejected");
        match error {
            gascan_core::manifest::ManifestError::Invalid(message) => {
                assert_eq!(message, format!("storage.{field} must not exceed 512GiB"));
            }
            other => panic!("expected field-specific invalid error, got {other}"),
        }
    }
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
