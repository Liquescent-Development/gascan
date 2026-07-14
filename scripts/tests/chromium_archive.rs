use std::{fs, io::Write, process::Command};

use zip::{ZipWriter, write::SimpleFileOptions};

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
