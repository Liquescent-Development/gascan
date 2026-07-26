use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::PathBuf,
    process::Command,
};

fn script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("tests/image/immutable-tree-snapshot.sh")
}

#[test]
fn snapshot_changes_when_any_regular_file_beneath_root_changes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("opt/gascan");
    fs::create_dir_all(root.join("workstation")).unwrap();
    fs::create_dir_all(root.join("mise")).unwrap();
    fs::write(root.join("workstation/tool"), "tool-v1").unwrap();
    fs::write(root.join("mise/runtime"), "runtime-v1").unwrap();

    let before = Command::new(script()).arg(&root).output().unwrap();
    assert!(
        before.status.success(),
        "{}",
        String::from_utf8_lossy(&before.stderr)
    );
    fs::write(root.join("mise/runtime"), "runtime-v2").unwrap();
    let after = Command::new(script()).arg(&root).output().unwrap();
    assert!(
        after.status.success(),
        "{}",
        String::from_utf8_lossy(&after.stderr)
    );
    assert_ne!(before.stdout, after.stdout);
}

#[test]
fn snapshot_is_deterministic_and_rejects_non_directory_roots() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("opt/gascan");
    fs::create_dir_all(root.join("z")).unwrap();
    fs::create_dir_all(root.join("a")).unwrap();
    fs::write(root.join("z/file"), "z").unwrap();
    fs::write(root.join("a/file"), "a").unwrap();

    let first = Command::new(script()).arg(&root).output().unwrap();
    let second = Command::new(script()).arg(&root).output().unwrap();
    assert!(first.status.success());
    assert_eq!(first.stdout, second.stdout);
    assert_eq!(first.stdout.len(), 65);

    let missing = Command::new(script())
        .arg(temp.path().join("missing"))
        .output()
        .unwrap();
    assert!(!missing.status.success());
}

#[test]
fn snapshot_changes_when_a_symlink_target_changes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("opt/gascan");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("target-a"), "same").unwrap();
    fs::write(root.join("target-b"), "same").unwrap();
    symlink("target-a", root.join("current")).unwrap();

    let before = Command::new(script()).arg(&root).output().unwrap();
    assert!(before.status.success());
    fs::remove_file(root.join("current")).unwrap();
    symlink("target-b", root.join("current")).unwrap();
    let after = Command::new(script()).arg(&root).output().unwrap();
    assert!(after.status.success());
    assert_ne!(before.stdout, after.stdout);
}

#[test]
fn snapshot_changes_when_directory_metadata_changes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("opt/gascan");
    let directory = root.join("workstation");
    fs::create_dir_all(&directory).unwrap();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).unwrap();

    let before = Command::new(script()).arg(&root).output().unwrap();
    assert!(before.status.success());
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
    let after = Command::new(script()).arg(&root).output().unwrap();
    assert!(after.status.success());
    assert_ne!(before.stdout, after.stdout);
}

#[test]
fn snapshot_rejects_unsupported_entry_types() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("opt/gascan");
    fs::create_dir_all(&root).unwrap();
    let fifo = root.join("unsupported-fifo");
    let created = Command::new("mkfifo").arg(&fifo).status().unwrap();
    assert!(created.success());

    let output = Command::new(script()).arg(&root).output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unsupported immutable tree entry"));
}
