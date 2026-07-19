use std::{
    fs,
    os::unix::fs::{MetadataExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    process::Command,
};

fn session_root_script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("apple-e2e-session-root.sh")
}

fn runner_script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("run-apple-e2e.sh")
}

fn write_executable(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

fn runner_fixture(build_helper: &str) -> tempfile::TempDir {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::create_dir_all(root.join("scripts")).unwrap();
    fs::copy(runner_script(), root.join("scripts/run-apple-e2e.sh")).unwrap();
    write_executable(
        &root.join("scripts/build-apple-attach-helper.sh"),
        build_helper,
    );
    write_executable(
        &root.join("scripts/apple-e2e-session-root.sh"),
        "#!/bin/sh\nset -eu\nprintf '%s\\n' \"$RUNNER_FIXTURE_ROOT/cleanup\"\n",
    );
    write_executable(
        &root.join("scripts/apple-e2e-cleanup.sh"),
        "#!/bin/sh\nset -eu\nprintf 'cleanup\\n' >>\"$RUNNER_FIXTURE_LOG\"\n",
    );
    write_executable(
        &root.join("scripts/apple-test-preflight.sh"),
        "#!/bin/sh\nset -eu\nprintf 'preflight:%s\\n' \"${GASCAN_APPLE_ATTACH_HELPER-unset}\" >>\"$RUNNER_FIXTURE_LOG\"\n",
    );
    write_executable(
        &root.join("bin/cargo"),
        "#!/bin/sh\nset -eu\nprintf 'cargo:%s:%s\\n' \"$*\" \"${GASCAN_APPLE_ATTACH_HELPER-unset}\" >>\"$RUNNER_FIXTURE_LOG\"\ncase \" $* \" in\n  *' build '*) mkdir -p \"$RUNNER_FIXTURE_ROOT/target/debug\"; printf '#!/bin/sh\\n' >\"$RUNNER_FIXTURE_ROOT/target/debug/gascan-e2e-cli\"; chmod 755 \"$RUNNER_FIXTURE_ROOT/target/debug/gascan-e2e-cli\";;\nesac\n",
    );
    fs::create_dir_all(root.join("cleanup")).unwrap();
    fixture
}

#[test]
fn runner_builds_and_exports_canonical_attach_helper_before_preflight_and_live_test() {
    let fixture = runner_fixture(
        "#!/bin/sh\nset -eu\nprintf 'helper-build\\n' >>\"$RUNNER_FIXTURE_LOG\"\nmkdir -p \"$RUNNER_FIXTURE_ROOT/target\"\nprintf '#!/bin/sh\\n' >\"$RUNNER_FIXTURE_ROOT/target/gascan-apple-attach\"\nchmod 755 \"$RUNNER_FIXTURE_ROOT/target/gascan-apple-attach\"\n",
    );
    let root = fixture.path();
    let log = root.join("runner.log");
    let output = Command::new(root.join("scripts/run-apple-e2e.sh"))
        .arg("apple_lifecycle")
        .env("RUNNER_FIXTURE_ROOT", root)
        .env("RUNNER_FIXTURE_LOG", &log)
        .env("PATH", format!("{}:/usr/bin:/bin", root.join("bin").display()))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let helper = fs::canonicalize(root.join("target/gascan-apple-attach")).unwrap();
    let records = fs::read_to_string(log).unwrap();
    let lines: Vec<_> = records.lines().collect();
    assert_eq!(lines[0], "cargo:build -p gascan-e2e --bin gascan-e2e-cli:unset");
    assert_eq!(lines[1], "helper-build");
    assert_eq!(lines[2], format!("preflight:{}", helper.display()));
    assert_eq!(
        lines[3],
        format!(
            "cargo:test -p gascan-e2e --test apple_lifecycle -- --ignored --test-threads=1 --nocapture:{}",
            helper.display()
        )
    );
}

#[test]
fn unusable_attach_helper_stops_before_preflight_and_live_test() {
    let fixture = runner_fixture(
        "#!/bin/sh\nset -eu\nprintf 'helper-build\\n' >>\"$RUNNER_FIXTURE_LOG\"\nmkdir -p \"$RUNNER_FIXTURE_ROOT/target\"\nprintf 'not executable\\n' >\"$RUNNER_FIXTURE_ROOT/target/gascan-apple-attach\"\nchmod 644 \"$RUNNER_FIXTURE_ROOT/target/gascan-apple-attach\"\n",
    );
    let root = fixture.path();
    let log = root.join("runner.log");
    let output = Command::new(root.join("scripts/run-apple-e2e.sh"))
        .arg("apple_lifecycle")
        .env("RUNNER_FIXTURE_ROOT", root)
        .env("RUNNER_FIXTURE_LOG", &log)
        .env("PATH", format!("{}:/usr/bin:/bin", root.join("bin").display()))
        .output()
        .unwrap();
    assert!(!output.status.success());
    let records = fs::read_to_string(log).unwrap();
    assert_eq!(
        records.lines().collect::<Vec<_>>(),
        ["cargo:build -p gascan-e2e --bin gascan-e2e-cli:unset", "helper-build"]
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("attach helper is not executable"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
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
