use std::{fs, os::unix::fs::PermissionsExt as _, path::PathBuf, process::Command};

fn helper() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tests/security/assert-unreachable.sh")
}

fn install_timeout(fixture: &tempfile::TempDir) {
    let timeout = fixture.path().join("timeout");
    fs::write(&timeout, "#!/bin/sh\nshift\nexec \"$@\"\n").unwrap();
    fs::set_permissions(&timeout, fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn reachable_http_error_is_connectivity_not_denial() {
    let fixture = tempfile::tempdir().unwrap();
    let curl = fixture.path().join("curl");
    fs::write(
        &curl,
        "#!/bin/sh\ncase \" $* \" in *' --fail '*) exit 22;; esac\nexit 0\n",
    )
    .unwrap();
    fs::set_permissions(&curl, fs::Permissions::from_mode(0o755)).unwrap();
    install_timeout(&fixture);

    let output = Command::new(helper())
        .arg("http://synthetic-http-500.test")
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", fixture.path().display()),
        )
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unexpectedly reachable"));
}

#[test]
fn refused_connection_is_bounded_network_denial() {
    let fixture = tempfile::tempdir().unwrap();
    let curl = fixture.path().join("curl");
    fs::write(&curl, "#!/bin/sh\nexit 7\n").unwrap();
    fs::set_permissions(&curl, fs::Permissions::from_mode(0o755)).unwrap();
    install_timeout(&fixture);

    let output = Command::new(helper())
        .arg("http://synthetic-refused.test")
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", fixture.path().display()),
        )
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn helper_is_executable_and_does_not_use_http_fail_mode() {
    let metadata = fs::metadata(helper()).unwrap();
    assert_ne!(metadata.permissions().mode() & 0o111, 0);
    let source = fs::read_to_string(helper()).unwrap();
    assert!(!source.contains("curl --silent --show-error --fail"));
    assert!(source.contains("--connect-timeout"));
    assert!(source.contains("--max-time"));
}
