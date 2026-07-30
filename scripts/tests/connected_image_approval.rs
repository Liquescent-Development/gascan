use sha2::{Digest, Sha256};
use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt, symlink},
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, Instant},
};

const DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SOURCE_DIGEST: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn executable(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    command: Command,
    reference: String,
}

fn fixture() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    for directory in ["scripts", "images/workspace", "docs/evidence", ".artifacts"] {
        fs::create_dir_all(root.join(directory)).unwrap();
    }
    let reference = format!("gascan-workspace:candidate@sha256:{DIGEST}");
    fs::write(root.join("images/workspace/versions.lock"), "locked\n").unwrap();
    fs::write(
        root.join(".artifacts/workspace-image-build.json"),
        format!("{{\"status\":\"succeeded\",\"source_digest\":\"{SOURCE_DIGEST}\"}}\n"),
    )
    .unwrap();
    fs::write(
        root.join(".artifacts/workspace-image-ref"),
        format!("{reference}\n"),
    )
    .unwrap();
    fs::write(
        root.join(".artifacts/connected-workspace-image-candidate.txt"),
        format!("{reference}\n"),
    )
    .unwrap();
    fs::write(
        root.join(".artifacts/connected-workspace-image-apple-live.txt"),
        format!("{reference}\n"),
    )
    .unwrap();
    let validator = root.join("scripts/test-receipt-validator");
    executable(
        &validator,
        "#!/bin/sh\nset -eu\ntest $# -eq 2\ntest -z \"${CALLS:-}\" || printf 'validator\\n' >>\"$CALLS\"\ncat \"$1\"\n",
    );
    let source_digest = root.join("scripts/test-source-digest");
    executable(
        &source_digest,
        &format!("#!/bin/sh\nset -eu\ntest $# -eq 1\nprintf '%s\\n' '{SOURCE_DIGEST}'\n"),
    );
    let mut command = Command::new("bash");
    command
        .arg(repository_root().join("scripts/approve-connected-workspace-image.sh"))
        .env("GASCAN_APPROVAL_TEST_ROOT", &root)
        .env("GASCAN_GATE_ARTIFACTS", root.join(".artifacts"))
        .env("GASCAN_APPROVAL_RECEIPT_VALIDATOR", validator)
        .env("GASCAN_APPROVAL_SOURCE_DIGEST_COMMAND", source_digest);
    Fixture {
        _temp: temp,
        root,
        command,
        reference,
    }
}

#[test]
fn caller_environment_cannot_bypass_or_replace_the_real_approval_lock() {
    for bypass in ["held", "command"] {
        let mut f = fixture();
        let lock = f.root.join(".artifacts/workspace-image-approval.lock");
        fs::write(&lock, "").unwrap();
        fs::set_permissions(&lock, fs::Permissions::from_mode(0o644)).unwrap();
        let calls = f.root.join("calls");
        f.command.env("CALLS", &calls);
        match bypass {
            "held" => {
                f.command.env("GASCAN_APPROVAL_LOCK_HELD", &f.root);
            }
            "command" => {
                f.command
                    .env("GASCAN_APPROVAL_LOCK_COMMAND", "/usr/bin/true");
            }
            _ => unreachable!(),
        }

        let output = f.command.output().unwrap();
        assert!(
            !output.status.success(),
            "{bypass} caller environment bypassed the unsafe real lock"
        );
        assert!(!calls.exists(), "{bypass} bypass allowed validation");
        assert!(
            !f.root.join("images/workspace/approved-image.txt").exists(),
            "{bypass} bypass allowed publication"
        );
    }
}

#[test]
fn source_drift_after_build_receipt_is_rejected_before_publication() {
    let mut f = fixture();
    let source = f.root.join("images/workspace/helper");
    fs::write(&source, "built\n").unwrap();
    assert!(
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(&f.root)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .args(["add", "images/workspace"])
            .current_dir(&f.root)
            .status()
            .unwrap()
            .success()
    );
    let digest_command = repository_root().join("scripts/workspace-image-source-digest.sh");
    let built_digest = Command::new("bash")
        .arg(&digest_command)
        .arg(&f.root)
        .output()
        .unwrap();
    assert!(built_digest.status.success());
    let built_digest = String::from_utf8(built_digest.stdout)
        .unwrap()
        .trim()
        .to_owned();
    fs::write(
        f.root.join(".artifacts/workspace-image-build.json"),
        format!("{{\"status\":\"succeeded\",\"source_digest\":\"{built_digest}\"}}\n"),
    )
    .unwrap();
    fs::write(&source, "drifted\n").unwrap();
    for (path, contents) in [
        ("images/workspace/approved-image.txt", "previous-approval"),
        (
            "images/workspace/approved-source.sha256",
            "previous-source\n",
        ),
        (
            "docs/evidence/connected-workspace-image.md",
            "previous-evidence\n",
        ),
    ] {
        fs::write(f.root.join(path), contents).unwrap();
    }
    f.command
        .env("GASCAN_APPROVAL_SOURCE_DIGEST_COMMAND", digest_command);

    let output = f.command.output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("source changed after build"));
    assert_eq!(
        fs::read_to_string(f.root.join("images/workspace/approved-image.txt")).unwrap(),
        "previous-approval"
    );
    assert_eq!(
        fs::read_to_string(f.root.join("images/workspace/approved-source.sha256")).unwrap(),
        "previous-source\n"
    );
    assert_eq!(
        fs::read_to_string(f.root.join("docs/evidence/connected-workspace-image.md")).unwrap(),
        "previous-evidence\n"
    );
}

#[test]
fn concurrent_approval_waits_before_validation_and_publication() {
    let mut first = fixture();
    let mut second = Command::new("bash");
    second
        .arg(repository_root().join("scripts/approve-connected-workspace-image.sh"))
        .env("GASCAN_APPROVAL_TEST_ROOT", &first.root)
        .env("GASCAN_GATE_ARTIFACTS", first.root.join(".artifacts"))
        .env(
            "GASCAN_APPROVAL_RECEIPT_VALIDATOR",
            first.root.join("scripts/test-receipt-validator"),
        )
        .env(
            "GASCAN_APPROVAL_SOURCE_DIGEST_COMMAND",
            first.root.join("scripts/test-source-digest"),
        );
    let calls = first.root.join("calls");
    let ready = first.root.join("first-ready");
    let release = first.root.join("release-first");
    let waiting = first.root.join("second-waiting-for-lock");
    let blocking_digest = first.root.join("scripts/blocking-source-digest");
    executable(
        &blocking_digest,
        &format!(
            "#!/bin/sh\nset -eu\n: >'{}'\nwhile ! test -e '{}'; do sleep 0.01; done\nprintf '%s\\n' '{}'\n",
            ready.display(),
            release.display(),
            SOURCE_DIGEST
        ),
    );
    first
        .command
        .env("GASCAN_APPROVAL_SOURCE_DIGEST_COMMAND", blocking_digest)
        .env("CALLS", &calls);
    second
        .env("CALLS", &calls)
        .env("GASCAN_SAFE_LOCK_TEST_WAITING_FILE", &waiting);
    fs::write(
        first.root.join("images/workspace/approved-image.txt"),
        "previous-approval",
    )
    .unwrap();

    let mut first_child = first.command.spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    while !ready.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        ready.exists(),
        "first approval never reached source validation"
    );
    let mut second_child = second.spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while !waiting.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    let reached_contention = waiting.exists();
    let second_waited = second_child.try_wait().unwrap().is_none();
    let validation_count = fs::read_to_string(&calls)
        .unwrap_or_default()
        .lines()
        .count();
    let publication_unchanged =
        fs::read_to_string(first.root.join("images/workspace/approved-image.txt")).unwrap()
            == "previous-approval";

    fs::write(&release, "").unwrap();
    let first_status = first_child.wait().unwrap();
    let second_status = second_child.wait().unwrap();

    assert!(
        reached_contention,
        "second approval never reached lock contention"
    );
    assert!(
        second_waited,
        "second approval did not wait for the repository lock"
    );
    assert_eq!(
        validation_count, 1,
        "second approval validated while the first owned the repository lock"
    );
    assert!(
        publication_unchanged,
        "second approval published while the first owned the repository lock"
    );
    assert!(first_status.success());
    assert!(second_status.success());
}

#[test]
fn unsafe_existing_approval_locks_are_rejected_before_validation() {
    for kind in ["symlink", "permissive"] {
        let mut f = fixture();
        let lock = f.root.join(".artifacts/workspace-image-approval.lock");
        let other = f.root.join(format!("{kind}-target"));
        fs::write(&other, "unchanged").unwrap();
        match kind {
            "symlink" => symlink(&other, &lock).unwrap(),
            "permissive" => {
                fs::write(&lock, "").unwrap();
                fs::set_permissions(&lock, fs::Permissions::from_mode(0o644)).unwrap();
            }
            _ => unreachable!(),
        }
        let calls = f.root.join("calls");
        f.command.env("CALLS", &calls);

        let output = f.command.output().unwrap();
        assert!(!output.status.success(), "{kind}");
        assert!(!calls.exists(), "{kind} lock allowed validation");
        assert_eq!(fs::read_to_string(&other).unwrap(), "unchanged");
    }
}

#[test]
fn otherwise_safe_hardlinked_approval_lock_is_rejected_before_validation() {
    let mut f = fixture();
    let lock = f.root.join(".artifacts/workspace-image-approval.lock");
    let target = f.root.join("hardlink-target");
    fs::write(&target, "unchanged").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
    fs::hard_link(&target, &lock).unwrap();
    let calls = f.root.join("calls");
    f.command.env("CALLS", &calls);

    let output = f.command.output().unwrap();
    assert!(!output.status.success());
    assert!(!calls.exists(), "hardlinked lock allowed validation");
    assert_eq!(fs::read_to_string(&target).unwrap(), "unchanged");
    assert_eq!(
        fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

fn digest(path: &Path) -> String {
    format!("{:x}", Sha256::digest(fs::read(path).unwrap()))
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn matching_candidate_and_live_acceptance_publish_exact_approval_and_evidence() {
    let mut f = fixture();
    fs::write(
        f.root.join("images/workspace/approved-image.txt"),
        "previous-approval",
    )
    .unwrap();
    fs::write(
        f.root.join("docs/evidence/connected-workspace-image.md"),
        "previous-evidence\n",
    )
    .unwrap();
    fs::write(
        f.root.join("images/workspace/approved-source.sha256"),
        "previous-source\n",
    )
    .unwrap();

    let output = f.command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, format!("{}\n", f.reference).as_bytes());
    assert_eq!(
        fs::read(f.root.join("images/workspace/approved-image.txt")).unwrap(),
        f.reference.as_bytes()
    );
    assert_eq!(
        fs::read_to_string(f.root.join("images/workspace/approved-source.sha256")).unwrap(),
        format!("{SOURCE_DIGEST}\n")
    );
    let evidence =
        fs::read_to_string(f.root.join("docs/evidence/connected-workspace-image.md")).unwrap();
    assert!(evidence.contains("- status: `PASS`"));
    assert!(evidence.contains(&format!("- image: `{}`", f.reference)));
    assert!(evidence.contains(&format!(
        "- versions lock SHA-256: `{}`",
        digest(&f.root.join("images/workspace/versions.lock"))
    )));
    assert!(evidence.contains(&format!(
        "- build receipt SHA-256: `{}`",
        digest(&f.root.join(".artifacts/workspace-image-build.json"))
    )));
    assert!(evidence.contains(&format!("- source SHA-256: `{SOURCE_DIGEST}`")));
}

#[test]
fn invalid_source_digest_is_rejected_before_publication() {
    let mut f = fixture();
    let invalid_source_digest = f.root.join("scripts/invalid-source-digest");
    executable(
        &invalid_source_digest,
        "#!/bin/sh\nprintf 'not-a-digest\\n'\n",
    );
    f.command.env(
        "GASCAN_APPROVAL_SOURCE_DIGEST_COMMAND",
        invalid_source_digest,
    );

    let output = f.command.output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("source digest is invalid"));
    assert!(!f.root.join("images/workspace/approved-image.txt").exists());
    assert!(
        !f.root
            .join("images/workspace/approved-source.sha256")
            .exists()
    );
    assert!(
        !f.root
            .join("docs/evidence/connected-workspace-image.md")
            .exists()
    );
}

#[test]
fn interruption_after_evidence_publication_restores_the_previous_pair() {
    for (action, code) in [("FAIL", None), ("INT", Some(130)), ("TERM", Some(143))] {
        let mut f = fixture();
        fs::write(
            f.root.join("images/workspace/approved-image.txt"),
            "previous-approval",
        )
        .unwrap();
        fs::write(
            f.root.join("docs/evidence/connected-workspace-image.md"),
            "previous-evidence\n",
        )
        .unwrap();
        f.command
            .env("GASCAN_APPROVAL_TEST_BOUNDARY", "after-evidence")
            .env("GASCAN_APPROVAL_TEST_ACTION", action);

        let output = f.command.output().unwrap();
        assert!(!output.status.success(), "{action}");
        if let Some(code) = code {
            assert_eq!(output.status.code(), Some(code), "{action}");
        }
        assert_eq!(
            fs::read_to_string(f.root.join("images/workspace/approved-image.txt")).unwrap(),
            "previous-approval"
        );
        assert_eq!(
            fs::read_to_string(f.root.join("docs/evidence/connected-workspace-image.md")).unwrap(),
            "previous-evidence\n"
        );
        assert!(
            !fs::read_dir(f.root.join("docs/evidence"))
                .unwrap()
                .any(|entry| entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".connected-workspace-image."))
        );
        assert!(
            !fs::read_dir(f.root.join("images/workspace"))
                .unwrap()
                .any(|entry| {
                    let name = entry.unwrap().file_name();
                    let name = name.to_string_lossy();
                    name.starts_with(".approved-image.") || name.starts_with(".approved-source.")
                })
        );
    }
}

#[test]
fn interruption_immediately_before_evidence_replacement_restores_the_previous_pair() {
    let mut f = fixture();
    fs::write(
        f.root.join("images/workspace/approved-image.txt"),
        "previous-approval",
    )
    .unwrap();
    fs::write(
        f.root.join("docs/evidence/connected-workspace-image.md"),
        "previous-evidence\n",
    )
    .unwrap();
    f.command
        .env(
            "GASCAN_APPROVAL_TEST_BOUNDARY",
            "before-evidence-replacement",
        )
        .env("GASCAN_APPROVAL_TEST_ACTION", "INT");

    let output = f.command.output().unwrap();
    assert_eq!(output.status.code(), Some(130));
    assert_eq!(
        fs::read_to_string(f.root.join("images/workspace/approved-image.txt")).unwrap(),
        "previous-approval"
    );
    assert_eq!(
        fs::read_to_string(f.root.join("docs/evidence/connected-workspace-image.md")).unwrap(),
        "previous-evidence\n"
    );
}

#[test]
fn successful_replacement_preserves_the_prior_triple_modes() {
    let mut f = fixture();
    let approval = f.root.join("images/workspace/approved-image.txt");
    let evidence = f.root.join("docs/evidence/connected-workspace-image.md");
    let source = f.root.join("images/workspace/approved-source.sha256");
    fs::write(&approval, "previous-approval").unwrap();
    fs::write(&evidence, "previous-evidence\n").unwrap();
    fs::write(&source, "previous-source\n").unwrap();
    fs::set_permissions(&approval, fs::Permissions::from_mode(0o640)).unwrap();
    fs::set_permissions(&evidence, fs::Permissions::from_mode(0o604)).unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o600)).unwrap();
    let approval_identity = (
        fs::metadata(&approval).unwrap().uid(),
        fs::metadata(&approval).unwrap().gid(),
    );
    let evidence_identity = (
        fs::metadata(&evidence).unwrap().uid(),
        fs::metadata(&evidence).unwrap().gid(),
    );
    let source_identity = (
        fs::metadata(&source).unwrap().uid(),
        fs::metadata(&source).unwrap().gid(),
    );

    let output = f.command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(mode(&approval), 0o640);
    assert_eq!(mode(&evidence), 0o604);
    assert_eq!(mode(&source), 0o600);
    assert_eq!(
        (
            fs::metadata(&approval).unwrap().uid(),
            fs::metadata(&approval).unwrap().gid(),
        ),
        approval_identity
    );
    assert_eq!(
        (
            fs::metadata(&evidence).unwrap().uid(),
            fs::metadata(&evidence).unwrap().gid(),
        ),
        evidence_identity
    );
    assert_eq!(
        (
            fs::metadata(&source).unwrap().uid(),
            fs::metadata(&source).unwrap().gid(),
        ),
        source_identity
    );
}

#[test]
fn failures_and_signals_at_every_replacement_boundary_restore_exact_prior_triple_or_absence() {
    for boundary in [
        "before-evidence-replacement",
        "after-evidence-replacement",
        "before-approval-replacement",
        "after-approval-replacement",
        "before-source-replacement",
        "after-source-replacement",
    ] {
        for (action, code) in [("FAIL", None), ("INT", Some(130)), ("TERM", Some(143))] {
            for prior_exists in [false, true] {
                let mut f = fixture();
                let approval = f.root.join("images/workspace/approved-image.txt");
                let evidence = f.root.join("docs/evidence/connected-workspace-image.md");
                let source = f.root.join("images/workspace/approved-source.sha256");
                if prior_exists {
                    fs::write(&approval, "previous-approval").unwrap();
                    fs::write(&evidence, "previous-evidence\n").unwrap();
                    fs::write(&source, "previous-source\n").unwrap();
                    fs::set_permissions(&approval, fs::Permissions::from_mode(0o640)).unwrap();
                    fs::set_permissions(&evidence, fs::Permissions::from_mode(0o604)).unwrap();
                    fs::set_permissions(&source, fs::Permissions::from_mode(0o600)).unwrap();
                }
                f.command
                    .env("GASCAN_APPROVAL_TEST_BOUNDARY", boundary)
                    .env("GASCAN_APPROVAL_TEST_ACTION", action);

                let output = f.command.output().unwrap();
                assert!(
                    !output.status.success(),
                    "{boundary} {action} prior={prior_exists}"
                );
                if let Some(code) = code {
                    assert_eq!(
                        output.status.code(),
                        Some(code),
                        "{boundary} {action} prior={prior_exists}: {}",
                        String::from_utf8_lossy(&output.stderr)
                    );
                }
                if prior_exists {
                    assert_eq!(fs::read_to_string(&approval).unwrap(), "previous-approval");
                    assert_eq!(
                        fs::read_to_string(&evidence).unwrap(),
                        "previous-evidence\n"
                    );
                    assert_eq!(fs::read_to_string(&source).unwrap(), "previous-source\n");
                    assert_eq!(mode(&approval), 0o640);
                    assert_eq!(mode(&evidence), 0o604);
                    assert_eq!(mode(&source), 0o600);
                } else {
                    assert!(!approval.exists(), "{boundary} {action}");
                    assert!(!evidence.exists(), "{boundary} {action}");
                    assert!(!source.exists(), "{boundary} {action}");
                }
            }
        }
    }
}
