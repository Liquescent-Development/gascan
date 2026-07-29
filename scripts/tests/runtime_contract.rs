use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn fixture(include_copy: bool, include_service_reference: bool) -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    fs::create_dir_all(root.join("images/workspace/bin")).unwrap();
    fs::create_dir_all(root.join("crates/gascand/src")).unwrap();
    fs::write(
        root.join("images/workspace/runtime-contract.toml"),
        "version = 1\n[[helpers]]\npath = \"/usr/local/bin/helper\"\nsource = \"images/workspace/bin/helper\"\n",
    )
    .unwrap();
    fs::write(
        root.join("images/workspace/bin/helper"),
        "#!/bin/sh\nexit 0\n",
    )
    .unwrap();
    fs::write(
        root.join("images/workspace/Dockerfile"),
        if include_copy {
            "COPY --chmod=0555 images/workspace/bin/helper /usr/local/bin/helper\n"
        } else {
            "FROM scratch\n"
        },
    )
    .unwrap();
    fs::write(
        root.join("crates/gascand/src/service.rs"),
        if include_service_reference {
            "const HELPER: &str = \"/usr/local/bin/helper\";\n"
        } else {
            "const HELPER: &str = \"/usr/local/bin/other\";\n"
        },
    )
    .unwrap();
    Fixture { _temp: temp, root }
}

fn validate(root: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_validate-runtime-contract"))
        .arg(root)
        .output()
        .unwrap()
}

#[test]
fn repository_runtime_contract_is_complete() {
    let output = validate(&repository_root());
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn missing_exact_copy_or_service_reference_is_rejected() {
    for fixture in [fixture(false, true), fixture(true, false)] {
        let output = validate(&fixture.root);
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("/usr/local/bin/helper"));
    }
}
