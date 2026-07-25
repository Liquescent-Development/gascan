use sha2::{Digest, Sha256};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
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
