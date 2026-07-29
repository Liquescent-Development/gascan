use std::{
    ffi::OsString,
    fs,
    io::Write,
    os::unix::{
        ffi::{OsStrExt, OsStringExt},
        fs::{OpenOptionsExt, PermissionsExt, symlink},
    },
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, Instant},
};

use sha2::{Digest, Sha256};
use zip::{ZipWriter, write::SimpleFileOptions};

const EMPTY_TREE_DIGEST: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

fn output_digest(output: &Path) -> String {
    format!("{:x}", Sha256::digest(output.as_os_str().as_bytes()))
}

fn staging_prefix(output: &Path) -> String {
    format!(".chromium-staging-{}-", output_digest(output))
}

fn lock_path(output: &Path) -> PathBuf {
    output.parent().unwrap().join(format!(
        ".chromium-extraction-{}.lock",
        output_digest(output)
    ))
}

fn archive(entries: &[(&str, Entry)]) -> (tempfile::TempDir, std::path::PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("chromium.zip");
    let file = fs::File::create(&path).unwrap();
    let mut zip = ZipWriter::new(file);
    for (name, entry) in entries {
        match entry {
            Entry::File(contents) => {
                zip.start_file(name, SimpleFileOptions::default().unix_permissions(0o755))
                    .unwrap();
                zip.write_all(contents).unwrap();
            }
            Entry::Symlink(target) => zip
                .add_symlink(name, target, SimpleFileOptions::default())
                .unwrap(),
        }
    }
    zip.finish().unwrap();
    (temp, path)
}

enum Entry {
    File(&'static [u8]),
    Symlink(&'static str),
}

fn validate(entries: &[(&str, Entry)]) -> (std::process::Output, tempfile::TempDir) {
    let (archive_temp, path) = archive(entries);
    let output_temp = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_extract-reviewed-chromium"))
        .args([path.as_os_str(), output_temp.path().as_os_str()])
        .output()
        .unwrap();
    drop(archive_temp);
    (output, output_temp)
}

#[test]
fn reviewed_chrome_linux_tree_is_extracted() {
    let (output, directory) = validate(&[
        ("chrome-linux/chrome", Entry::File(b"browser")),
        ("chrome-linux/resources/data", Entry::File(b"data")),
    ]);
    assert!(output.status.success());
    assert_eq!(
        fs::read(directory.path().join("chrome-linux/chrome")).unwrap(),
        b"browser"
    );
}

#[test]
fn traversal_absolute_symlink_duplicate_and_wrong_layout_are_rejected() {
    for entries in [
        vec![("chrome-linux/../escape", Entry::File(b"bad"))],
        vec![("/chrome-linux/chrome", Entry::File(b"bad"))],
        vec![("chrome-linux\\..\\escape", Entry::File(b"bad"))],
        vec![("chrome-linux/chrome", Entry::Symlink("../../escape"))],
        vec![
            ("chrome-linux/chrome", Entry::File(b"one")),
            ("chrome-linux//chrome", Entry::File(b"two")),
        ],
        vec![("other/chrome", Entry::File(b"bad"))],
    ] {
        let (output, directory) = validate(&entries);
        assert!(!output.status.success(), "malicious archive was accepted");
        assert!(fs::read_dir(directory.path()).unwrap().next().is_none());
    }
}

#[test]
fn refresh_atomically_replaces_valid_tree_and_preserves_it_on_failure() {
    let output = tempfile::tempdir().unwrap();
    let (first_temp, first) = archive(&[("chrome-linux/chrome", Entry::File(b"first"))]);
    assert!(
        Command::new(env!("CARGO_BIN_EXE_extract-reviewed-chromium"))
            .args([first.as_os_str(), output.path().as_os_str()])
            .status()
            .unwrap()
            .success()
    );
    let (second_temp, second) = archive(&[("chrome-linux/chrome", Entry::File(b"second"))]);
    assert!(
        Command::new(env!("CARGO_BIN_EXE_extract-reviewed-chromium"))
            .args([second.as_os_str(), output.path().as_os_str()])
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(
        fs::read(output.path().join("chrome-linux/chrome")).unwrap(),
        b"second"
    );
    let (bad_temp, bad) = archive(&[("../escape", Entry::File(b"bad"))]);
    assert!(
        !Command::new(env!("CARGO_BIN_EXE_extract-reviewed-chromium"))
            .args([bad.as_os_str(), output.path().as_os_str()])
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(
        fs::read(output.path().join("chrome-linux/chrome")).unwrap(),
        b"second"
    );
    drop((first_temp, second_temp, bad_temp));
}

#[test]
fn extraction_preserves_a_foreign_live_transaction_in_the_same_parent() {
    let parent = tempfile::tempdir().unwrap();
    let output = parent.path().join("target-output");
    let foreign_output = parent
        .path()
        .join(OsString::from_vec(b"foreign-output-\xfe".to_vec()));
    let other_raw_output = parent
        .path()
        .join(OsString::from_vec(b"foreign-output-\xff".to_vec()));
    assert_eq!(
        foreign_output.to_string_lossy(),
        other_raw_output.to_string_lossy()
    );
    assert_ne!(
        staging_prefix(&foreign_output),
        staging_prefix(&other_raw_output)
    );
    let foreign_lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .open(lock_path(&foreign_output))
        .unwrap();
    rustix::fs::flock(&foreign_lock, rustix::fs::FlockOperation::LockExclusive).unwrap();
    let foreign_staging_name = format!("{}live", staging_prefix(&foreign_output));
    let foreign_staging = parent.path().join(&foreign_staging_name);
    let foreign_receipt = parent
        .path()
        .join(format!("{foreign_staging_name}.receipt"));
    fs::create_dir(&foreign_staging).unwrap();
    fs::write(
        &foreign_receipt,
        format!("chromium exchange receipt v1\t{foreign_staging_name}\t-\t{EMPTY_TREE_DIGEST}\n"),
    )
    .unwrap();

    let (_archive_temp, archive) = archive(&[("chrome-linux/chrome", Entry::File(b"browser"))]);
    let extraction = Command::new(env!("CARGO_BIN_EXE_extract-reviewed-chromium"))
        .args([archive.as_os_str(), output.as_os_str()])
        .output()
        .unwrap();

    assert!(
        extraction.status.success(),
        "target extraction failed: {}",
        String::from_utf8_lossy(&extraction.stderr)
    );
    assert!(
        foreign_staging.is_dir(),
        "target extraction removed another output's live staging tree"
    );
    assert!(
        foreign_receipt.is_file(),
        "target extraction removed another output's live receipt"
    );
}

#[test]
fn unsafe_existing_output_locks_are_rejected() {
    let (_archive_temp, archive) = archive(&[("chrome-linux/chrome", Entry::File(b"browser"))]);

    let symlink_parent = tempfile::tempdir().unwrap();
    let symlink_output = symlink_parent.path().join("symlink-output");
    let symlink_target = symlink_parent.path().join("lock-target");
    fs::write(&symlink_target, b"unchanged").unwrap();
    symlink(&symlink_target, lock_path(&symlink_output)).unwrap();
    let symlink_result = Command::new(env!("CARGO_BIN_EXE_extract-reviewed-chromium"))
        .args([archive.as_os_str(), symlink_output.as_os_str()])
        .output()
        .unwrap();
    assert!(!symlink_result.status.success());
    assert_eq!(fs::read(&symlink_target).unwrap(), b"unchanged");
    assert!(!symlink_output.exists());

    let permissive_parent = tempfile::tempdir().unwrap();
    let permissive_output = permissive_parent.path().join("permissive-output");
    let permissive_lock = lock_path(&permissive_output);
    fs::write(&permissive_lock, b"").unwrap();
    fs::set_permissions(&permissive_lock, fs::Permissions::from_mode(0o644)).unwrap();
    let permissive_result = Command::new(env!("CARGO_BIN_EXE_extract-reviewed-chromium"))
        .args([archive.as_os_str(), permissive_output.as_os_str()])
        .output()
        .unwrap();
    assert!(!permissive_result.status.success());
    assert!(!permissive_output.exists());
}

#[test]
fn same_output_extraction_waits_for_the_current_owner_before_recovery() {
    let parent = tempfile::tempdir().unwrap();
    let output = parent.path().join("shared-output");
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .open(lock_path(&output))
        .unwrap();
    rustix::fs::flock(&lock, rustix::fs::FlockOperation::LockExclusive).unwrap();

    let staging_name = format!("{}live", staging_prefix(&output));
    let staging = parent.path().join(&staging_name);
    let receipt = parent.path().join(format!("{staging_name}.receipt"));
    fs::create_dir(&staging).unwrap();
    fs::write(
        &receipt,
        format!("chromium exchange receipt v1\t{staging_name}\t-\t{EMPTY_TREE_DIGEST}\n"),
    )
    .unwrap();

    let (_archive_temp, archive) = archive(&[("chrome-linux/chrome", Entry::File(b"browser"))]);
    let mut extraction = Command::new(env!("CARGO_BIN_EXE_extract-reviewed-chromium"))
        .args([archive.as_os_str(), output.as_os_str()])
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline && receipt.exists() && extraction.try_wait().unwrap().is_none()
    {
        thread::sleep(Duration::from_millis(10));
    }

    let remained_blocked = extraction.try_wait().unwrap().is_none();
    let transaction_preserved = staging.is_dir() && receipt.is_file();
    drop(lock);
    let extraction_status = extraction.wait().unwrap();

    assert!(
        remained_blocked,
        "same-output extraction entered recovery while another owner held the output lock"
    );
    assert!(
        transaction_preserved,
        "same-output extraction removed the current owner's live transaction"
    );
    assert!(extraction_status.success());
    assert!(!staging.exists());
    assert!(!receipt.exists());
    assert_eq!(
        fs::read(output.join("chrome-linux/chrome")).unwrap(),
        b"browser"
    );
}
