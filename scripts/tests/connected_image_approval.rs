use sha2::{Digest, Sha256};
use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Command,
};

const DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

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
        "{\"status\":\"succeeded\"}\n",
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
        "#!/bin/sh\nset -eu\ntest $# -eq 2\ncat \"$1\"\n",
    );
    let mut command = Command::new("bash");
    command
        .arg(repository_root().join("scripts/approve-connected-workspace-image.sh"))
        .env("GASCAN_APPROVAL_TEST_ROOT", &root)
        .env("GASCAN_GATE_ARTIFACTS", root.join(".artifacts"))
        .env("GASCAN_APPROVAL_RECEIPT_VALIDATOR", validator);
    Fixture {
        _temp: temp,
        root,
        command,
        reference,
    }
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
                .any(|entry| entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".approved-image."))
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
fn successful_replacement_preserves_the_prior_pair_modes() {
    let mut f = fixture();
    let approval = f.root.join("images/workspace/approved-image.txt");
    let evidence = f.root.join("docs/evidence/connected-workspace-image.md");
    fs::write(&approval, "previous-approval").unwrap();
    fs::write(&evidence, "previous-evidence\n").unwrap();
    fs::set_permissions(&approval, fs::Permissions::from_mode(0o640)).unwrap();
    fs::set_permissions(&evidence, fs::Permissions::from_mode(0o604)).unwrap();
    let approval_identity = (
        fs::metadata(&approval).unwrap().uid(),
        fs::metadata(&approval).unwrap().gid(),
    );
    let evidence_identity = (
        fs::metadata(&evidence).unwrap().uid(),
        fs::metadata(&evidence).unwrap().gid(),
    );

    let output = f.command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(mode(&approval), 0o640);
    assert_eq!(mode(&evidence), 0o604);
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
}

#[test]
fn signals_at_every_replacement_boundary_restore_exact_prior_pair_or_absence() {
    for boundary in [
        "before-evidence-replacement",
        "after-evidence-replacement",
        "before-approval-replacement",
        "after-approval-replacement",
    ] {
        for (action, code) in [("INT", 130), ("TERM", 143)] {
            for prior_exists in [false, true] {
                let mut f = fixture();
                let approval = f.root.join("images/workspace/approved-image.txt");
                let evidence = f.root.join("docs/evidence/connected-workspace-image.md");
                if prior_exists {
                    fs::write(&approval, "previous-approval").unwrap();
                    fs::write(&evidence, "previous-evidence\n").unwrap();
                    fs::set_permissions(&approval, fs::Permissions::from_mode(0o640)).unwrap();
                    fs::set_permissions(&evidence, fs::Permissions::from_mode(0o604)).unwrap();
                }
                f.command
                    .env("GASCAN_APPROVAL_TEST_BOUNDARY", boundary)
                    .env("GASCAN_APPROVAL_TEST_ACTION", action);

                let output = f.command.output().unwrap();
                assert_eq!(
                    output.status.code(),
                    Some(code),
                    "{boundary} {action} prior={prior_exists}: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
                if prior_exists {
                    assert_eq!(fs::read_to_string(&approval).unwrap(), "previous-approval");
                    assert_eq!(
                        fs::read_to_string(&evidence).unwrap(),
                        "previous-evidence\n"
                    );
                    assert_eq!(mode(&approval), 0o640);
                    assert_eq!(mode(&evidence), 0o604);
                } else {
                    assert!(!approval.exists(), "{boundary} {action}");
                    assert!(!evidence.exists(), "{boundary} {action}");
                }
            }
        }
    }
}
