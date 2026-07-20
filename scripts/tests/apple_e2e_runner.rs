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
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", root.join("bin").display()),
        )
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
    assert_eq!(
        lines[0],
        "cargo:build -p gascan-e2e --bin gascan-e2e-cli:unset"
    );
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
fn runner_accepts_apple_apply_as_an_explicit_single_target() {
    let fixture = runner_fixture(
        "#!/bin/sh\nset -eu\nmkdir -p \"$RUNNER_FIXTURE_ROOT/target\"\nprintf '#!/bin/sh\\n' >\"$RUNNER_FIXTURE_ROOT/target/gascan-apple-attach\"\nchmod 755 \"$RUNNER_FIXTURE_ROOT/target/gascan-apple-attach\"\n",
    );
    let root = fixture.path();
    let log = root.join("runner.log");
    let output = Command::new(root.join("scripts/run-apple-e2e.sh"))
        .arg("apple_apply")
        .env("RUNNER_FIXTURE_ROOT", root)
        .env("RUNNER_FIXTURE_LOG", &log)
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", root.join("bin").display()),
        )
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let records = fs::read_to_string(log).unwrap();
    assert!(records.lines().any(|line| line.contains(
        "cargo:test -p gascan-e2e --test apple_apply -- --ignored --test-threads=1 --nocapture:"
    )));
    assert!(!records.contains("--test apple_lifecycle"));
    assert!(!records.contains("--test apple_recovery"));
}

#[test]
fn runner_accepts_apple_security_as_an_explicit_single_target() {
    let fixture = runner_fixture(
        "#!/bin/sh\nset -eu\nmkdir -p \"$RUNNER_FIXTURE_ROOT/target\"\nprintf '#!/bin/sh\\n' >\"$RUNNER_FIXTURE_ROOT/target/gascan-apple-attach\"\nchmod 755 \"$RUNNER_FIXTURE_ROOT/target/gascan-apple-attach\"\n",
    );
    let root = fixture.path();
    let log = root.join("runner.log");
    let output = Command::new(root.join("scripts/run-apple-e2e.sh"))
        .arg("apple_security")
        .env("RUNNER_FIXTURE_ROOT", root)
        .env("RUNNER_FIXTURE_LOG", &log)
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", root.join("bin").display()),
        )
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let records = fs::read_to_string(log).unwrap();
    assert!(records.lines().any(|line| line.contains(
        "cargo:test -p gascan-e2e --test apple_security -- --ignored --test-threads=1 --nocapture:"
    )));
    assert!(!records.contains("--test apple_lifecycle"));
    assert!(!records.contains("--test apple_recovery"));
    assert!(!records.contains("--test apple_apply"));
}

#[test]
fn security_dns_inventory_uses_literal_normal_user_apple_command() {
    let source = fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../crates/gascan-e2e/tests/apple_security.rs"),
    )
    .unwrap();
    let compact: String = source.split_whitespace().collect();
    assert!(compact.contains(
        "Command::new(\"container\").args([\"system\",\"dns\",\"list\",\"--format\",\"json\"])"
    ));
    assert!(!compact
        .contains("Command::new(\"sudo\").args([\"-n\",\"container\",\"system\",\"dns\",\"list\""));
    assert!(compact.contains(
        "Command::new(\"sudo\").args([\"-n\",\"container\",\"system\",\"dns\",\"create\""
    ));
    assert!(compact.contains(
        "Command::new(\"sudo\").args([\"-n\",\"container\",\"system\",\"dns\",\"delete\""
    ));
}

#[test]
fn security_mutation_arms_cleanup_and_bounded_probes_require_discriminating_controls() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let source =
        fs::read_to_string(root.join("crates/gascan-e2e/tests/apple_security.rs")).unwrap();
    let create = source
        .split("impl<'a> OwnedDnsRoute")
        .nth(1)
        .unwrap()
        .split("fn create(")
        .nth(1)
        .unwrap()
        .split("fn url(")
        .next()
        .unwrap();
    assert!(
        create.find("let mut route = Self").unwrap()
            < create.find("Command::new(\"sudo\")").unwrap()
    );
    assert!(create.contains("route.cleanup()"));
    assert!(
        create.find("record_dns_domain").unwrap() < create.find("Command::new(\"sudo\")").unwrap()
    );
    assert!(source.contains("combine_test_and_cleanup(\"test-owned DNS route\""));
    let compact: String = source.split_whitespace().collect();
    assert!(compact.contains("env.runtime_root().join(format!(\"synthetic-outside-{}\""));
    assert!(!source.contains(".parent().ok_or(\"security root has no session parent\")?"));
    let created = source.find("OwnedDnsRoute::create(&env)").unwrap();
    let abort = source
        .find("GASCAN_SECURITY_ABORT_AFTER_DNS_CREATE")
        .unwrap();
    let durable_abort_marker = source.find("record_abort_probe_reached").unwrap();
    let process_abort = source.find("std::process::abort()").unwrap();
    let ordinary_cleanup = source.find("let route_cleanup = route.cleanup()").unwrap();
    assert!(created < abort && abort < ordinary_cleanup);
    assert!(abort < durable_abort_marker && durable_abort_marker < process_abort);

    let ports = fs::read_to_string(root.join("tests/security/ports.sh")).unwrap();
    assert!(ports.contains("curl --silent --fail --max-time 1"));
    assert!(ports.contains("guest listener did not become reachable"));

    let resources = fs::read_to_string(root.join("tests/security/resources.sh")).unwrap();
    assert!(resources.contains("test \"$memory_status\" -ne 124"));
}

#[test]
fn failed_live_test_runs_cleanup_and_cleanup_failure_is_nonzero() {
    let fixture = runner_fixture(
        "#!/bin/sh\nset -eu\nmkdir -p \"$RUNNER_FIXTURE_ROOT/target\"\nprintf '#!/bin/sh\\n' >\"$RUNNER_FIXTURE_ROOT/target/gascan-apple-attach\"\nchmod 755 \"$RUNNER_FIXTURE_ROOT/target/gascan-apple-attach\"\n",
    );
    let root = fixture.path();
    let log = root.join("runner.log");
    write_executable(
        &root.join("bin/cargo"),
        "#!/bin/sh\nset -eu\nprintf 'cargo:%s\\n' \"$*\" >>\"$RUNNER_FIXTURE_LOG\"\ncase \" $* \" in\n *' build '*) mkdir -p \"$RUNNER_FIXTURE_ROOT/target/debug\"; printf '#!/bin/sh\\n' >\"$RUNNER_FIXTURE_ROOT/target/debug/gascan-e2e-cli\"; chmod 755 \"$RUNNER_FIXTURE_ROOT/target/debug/gascan-e2e-cli\";;\n *' test '*) printf '{\"dns_domain\":\"gascan-00112233445566778899aabbccddeeff.test\"}\\n' >\"$GASCAN_E2E_CLEANUP_MANIFEST\"; chmod 600 \"$GASCAN_E2E_CLEANUP_MANIFEST\"; exit 23;;\nesac\n",
    );
    write_executable(
        &root.join("scripts/apple-e2e-cleanup.sh"),
        "#!/bin/sh\nset -eu\nprintf 'cleanup:%s\\n' \"$1\" >>\"$RUNNER_FIXTURE_LOG\"\nexit 29\n",
    );

    let output = Command::new(root.join("scripts/run-apple-e2e.sh"))
        .arg("apple_security")
        .env("RUNNER_FIXTURE_ROOT", root)
        .env("RUNNER_FIXTURE_LOG", &log)
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", root.join("bin").display()),
        )
        .output()
        .unwrap();

    assert!(!output.status.success());
    let records = fs::read_to_string(log).unwrap();
    assert!(records.contains("cleanup:"));
    assert!(root.join("cleanup").read_dir().unwrap().any(|entry| entry
        .unwrap()
        .path()
        .extension()
        .is_some_and(|extension| extension == "json")));
}

#[test]
fn stale_cleanup_record_is_reconciled_before_preflight() {
    let fixture = runner_fixture(
        "#!/bin/sh\nset -eu\nprintf 'helper-build\\n' >>\"$RUNNER_FIXTURE_LOG\"\nmkdir -p \"$RUNNER_FIXTURE_ROOT/target\"\nprintf '#!/bin/sh\\n' >\"$RUNNER_FIXTURE_ROOT/target/gascan-apple-attach\"\nchmod 755 \"$RUNNER_FIXTURE_ROOT/target/gascan-apple-attach\"\n",
    );
    let root = fixture.path();
    let log = root.join("runner.log");
    fs::write(root.join("cleanup/stale.json"), "{}\n").unwrap();

    let output = Command::new(root.join("scripts/run-apple-e2e.sh"))
        .arg("apple_security")
        .env("RUNNER_FIXTURE_ROOT", root)
        .env("RUNNER_FIXTURE_LOG", &log)
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", root.join("bin").display()),
        )
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let records = fs::read_to_string(log).unwrap();
    assert!(records.find("cleanup").unwrap() < records.find("preflight:").unwrap());
}

#[test]
fn security_root_probe_uses_guest_sudo_without_requesting_unsupported_runtime_user() {
    let source = fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../crates/gascan-e2e/tests/apple_security.rs"),
    )
    .unwrap();
    let compact: String = source.split_whitespace().collect();
    assert_eq!(
        compact
            .match_indices("write_manifest(root,\"offline\",\"root\"")
            .count(),
        1
    );
    assert!(compact.contains(
        "require_failure_code(\"rootuserrequest\",&root_request,\"unsupported_capability\")"
    ));
    assert!(!compact
        .contains("require_failure_code(\"rootuserrequest\",&root_request,\"unsupported_user\")"));
    assert!(compact.contains("env.assert_no_owned_resources()?"));
    assert!(compact.contains(
        "\"run\",\"--\",\"sudo\",\"-n\",\"bash\",\"/workspace/.gascan/security/offline-network.sh\""
    ));
}

#[test]
fn security_process_non_claim_uses_the_stable_manifest_wire_rejection() {
    let source = fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../crates/gascan-e2e/tests/apple_security.rs"),
    )
    .unwrap();
    let compact: String = source.split_whitespace().collect();
    assert!(
        compact.contains("require_failure_code(\"processrequest\",&process,\"invalid_request\")")
    );
    assert!(
        !compact.contains("require_failure_code(\"processrequest\",&process,\"invalid_manifest\")")
    );
    let process_rejection = compact
        .split("require_failure_code(\"processrequest\"")
        .nth(1)
        .unwrap();
    assert!(process_rejection
        .starts_with(",&process,\"invalid_request\")?;env.assert_no_owned_resources()?"));
}

#[test]
fn runner_default_targets_remain_lifecycle_and_recovery_only() {
    let fixture = runner_fixture(
        "#!/bin/sh\nset -eu\nmkdir -p \"$RUNNER_FIXTURE_ROOT/target\"\nprintf '#!/bin/sh\\n' >\"$RUNNER_FIXTURE_ROOT/target/gascan-apple-attach\"\nchmod 755 \"$RUNNER_FIXTURE_ROOT/target/gascan-apple-attach\"\n",
    );
    let root = fixture.path();
    let log = root.join("runner.log");
    let output = Command::new(root.join("scripts/run-apple-e2e.sh"))
        .env("RUNNER_FIXTURE_ROOT", root)
        .env("RUNNER_FIXTURE_LOG", &log)
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", root.join("bin").display()),
        )
        .output()
        .unwrap();
    assert!(output.status.success());
    let records = fs::read_to_string(log).unwrap();
    assert!(records.contains("--test apple_lifecycle"));
    assert!(records.contains("--test apple_recovery"));
    assert!(!records.contains("--test apple_apply"));
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
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", root.join("bin").display()),
        )
        .output()
        .unwrap();
    assert!(!output.status.success());
    let records = fs::read_to_string(log).unwrap();
    assert_eq!(
        records.lines().collect::<Vec<_>>(),
        [
            "cargo:build -p gascan-e2e --bin gascan-e2e-cli:unset",
            "helper-build"
        ]
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
