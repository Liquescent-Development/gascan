use std::{fs, path::PathBuf, process::Command};

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
