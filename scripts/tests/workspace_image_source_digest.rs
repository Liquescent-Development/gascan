use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned()
}

fn source_fixture() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    fs::create_dir_all(root.join("images/workspace/bin")).unwrap();
    fs::write(root.join("images/workspace/bin/helper"), "one\n").unwrap();
    fs::write(root.join("images/workspace/approved-image.txt"), "image\n").unwrap();
    fs::write(
        root.join("images/workspace/approved-source.sha256"),
        format!("{}\n", "0".repeat(64)),
    )
    .unwrap();
    Command::new("git")
        .args(["init", "-q"])
        .current_dir(&root)
        .status()
        .unwrap();
    Command::new("git")
        .args(["config", "core.excludesfile", "/dev/null"])
        .current_dir(&root)
        .status()
        .unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(&root)
        .status()
        .unwrap();
    (temp, root)
}

fn source_digest(root: &Path) -> Output {
    Command::new("bash")
        .arg(repository_root().join("scripts/workspace-image-source-digest.sh"))
        .arg(root)
        .output()
        .unwrap()
}

#[test]
fn digest_is_stable_and_changes_with_image_source() {
    let (_temp, root) = source_fixture();
    let first = source_digest(&root);
    let second = source_digest(&root);
    assert!(first.status.success());
    assert_eq!(first.stdout, second.stdout);
    fs::write(root.join("images/workspace/bin/helper"), "two\n").unwrap();
    let changed = source_digest(&root);
    assert!(changed.status.success());
    assert_ne!(first.stdout, changed.stdout);
}

#[test]
fn approval_outputs_do_not_change_the_digest() {
    let (_temp, root) = source_fixture();
    let first = source_digest(&root);
    fs::write(
        root.join("images/workspace/approved-image.txt"),
        "replacement\n",
    )
    .unwrap();
    fs::write(
        root.join("images/workspace/approved-source.sha256"),
        format!("{}\n", "f".repeat(64)),
    )
    .unwrap();
    assert_eq!(first.stdout, source_digest(&root).stdout);
}

#[test]
fn unsafe_or_empty_source_tree_is_rejected() {
    let (_temp, root) = source_fixture();
    fs::remove_file(root.join("images/workspace/bin/helper")).unwrap();
    std::os::unix::fs::symlink(
        root.join("images/workspace/approved-image.txt"),
        root.join("images/workspace/bin/helper"),
    )
    .unwrap();
    assert!(!source_digest(&root).status.success());

    Command::new("git")
        .args(["rm", "--cached", "images/workspace/bin/helper"])
        .current_dir(&root)
        .status()
        .unwrap();
    assert!(!source_digest(&root).status.success());
}
