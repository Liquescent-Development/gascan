use std::{fs, os::unix::fs::MetadataExt as _, path::PathBuf, process::Command};

fn session_root_script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("apple-e2e-session-root.sh")
}

#[test]
fn long_tmpdir_cannot_lengthen_gate4_socket_paths() {
    let long_tmpdir = format!(
        "/private/var/folders/{}/T",
        "very-long-component".repeat(12)
    );
    let output = Command::new(session_root_script())
        .env("TMPDIR", &long_tmpdir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let cleanup_root = PathBuf::from(String::from_utf8(output.stdout).unwrap().trim());
    assert!(!cleanup_root.starts_with(&long_tmpdir));
    assert_eq!(fs::canonicalize(&cleanup_root).unwrap(), cleanup_root);
    let metadata = fs::symlink_metadata(&cleanup_root).unwrap();
    assert!(metadata.is_dir());
    let uid = Command::new("id").arg("-u").output().unwrap();
    let uid: u32 = String::from_utf8(uid.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_eq!(metadata.uid(), uid);
    assert_eq!(metadata.mode() & 0o777, 0o700);

    // tempfile uses 6 random bytes and mktemp uses the 12 Xs in the runner.
    // The daemon first binds an 11-byte staging filename below the socket directory.
    let longest_bind_path = cleanup_root
        .join("session-XXXXXXXXXXXX")
        .join("gascan-gate4-runtime-XXXXXX")
        .join("gascan/.XXXXXXXXXX");
    assert!(
        longest_bind_path.as_os_str().as_encoded_bytes().len() < 104,
        "{} is too long for macOS sockaddr_un.sun_path",
        longest_bind_path.display()
    );
}
