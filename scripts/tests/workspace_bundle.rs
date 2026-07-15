use std::fs;

use gascan_image_tools::bundle::{BundleError, BundleLock, validate_bundle};
use serde_json::json;
use sha2::{Digest, Sha256};

const MEDIA_TYPE: &str = "application/vnd.gascan.workspace-bundle.v1+tar.zstd";

#[derive(Clone)]
struct RawEntry<'a> {
    path: &'a str,
    kind: u8,
    body: &'a [u8],
    link: Option<&'a str>,
}

fn append_octal(field: &mut [u8], value: u64) {
    let width = field.len();
    let rendered = format!("{:0width$o}\0", value, width = width - 1);
    field.copy_from_slice(rendered.as_bytes());
}

fn raw_tar(entries: &[RawEntry<'_>], truncate: bool) -> Vec<u8> {
    let mut out = Vec::new();
    for entry in entries {
        let mut header = [0_u8; 512];
        header[..entry.path.len()].copy_from_slice(entry.path.as_bytes());
        append_octal(&mut header[100..108], 0o755);
        append_octal(&mut header[108..116], 0);
        append_octal(&mut header[116..124], 0);
        append_octal(&mut header[124..136], entry.body.len() as u64);
        append_octal(&mut header[136..148], 0);
        header[148..156].fill(b' ');
        header[156] = entry.kind;
        if let Some(link) = entry.link {
            header[157..157 + link.len()].copy_from_slice(link.as_bytes());
        }
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        let checksum: u64 = header.iter().map(|byte| u64::from(*byte)).sum();
        let rendered = format!("{checksum:06o}\0 ");
        header[148..156].copy_from_slice(rendered.as_bytes());
        out.extend_from_slice(&header);
        out.extend_from_slice(entry.body);
        out.resize(out.len().div_ceil(512) * 512, 0);
    }
    out.extend_from_slice(&[0_u8; 1024]);
    if truncate {
        out.truncate(out.len().saturating_sub(1200));
    }
    zstd::stream::encode_all(out.as_slice(), 1).unwrap()
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn manifest(files: serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "version": 1,
        "platform": "linux/arm64",
        "files": files
    }))
    .unwrap()
}

fn run(entries: &[RawEntry<'_>], truncate: bool) -> Result<(), BundleError> {
    let archive = raw_tar(entries, truncate);
    let temp = tempfile::tempdir().unwrap();
    let archive_path = temp.path().join("bundle.tar.zst");
    fs::write(&archive_path, &archive).unwrap();
    let lock = BundleLock {
        url: "https://example.invalid/bundle.tar.zst".to_owned(),
        sha256: hash(&archive),
        size: archive.len() as u64,
        media_type: MEDIA_TYPE.to_owned(),
        platform: "linux/arm64".to_owned(),
    };
    validate_bundle(&lock, &archive_path, &temp.path().join("output")).map(|_| ())
}

fn regular_manifest(path: &str, body: &[u8]) -> Vec<u8> {
    manifest(json!([{
        "path": path,
        "kind": "file",
        "size": body.len(),
        "sha256": hash(body)
    }]))
}

#[test]
fn extracts_a_manifested_bundle_atomically() {
    let body = b"verified\n";
    let manifest = manifest(json!([
        {"path":"payload","kind":"directory"},
        {"path":"payload/data.txt","kind":"file","size":body.len(),"sha256":hash(body)},
        {"path":"payload/link","kind":"symlink","target":"data.txt"}
    ]));
    let archive = raw_tar(
        &[
            RawEntry {
                path: "bundle-manifest.json",
                kind: b'0',
                body: &manifest,
                link: None,
            },
            RawEntry {
                path: "payload",
                kind: b'5',
                body: b"",
                link: None,
            },
            RawEntry {
                path: "payload/data.txt",
                kind: b'0',
                body,
                link: None,
            },
            RawEntry {
                path: "payload/link",
                kind: b'2',
                body: b"",
                link: Some("data.txt"),
            },
        ],
        false,
    );
    let temp = tempfile::tempdir().unwrap();
    let archive_path = temp.path().join("bundle.tar.zst");
    fs::write(&archive_path, &archive).unwrap();
    let destination = temp.path().join("published");
    let evidence = validate_bundle(
        &BundleLock {
            url: "https://example.invalid/bundle.tar.zst".into(),
            sha256: hash(&archive),
            size: archive.len() as u64,
            media_type: MEDIA_TYPE.into(),
            platform: "linux/arm64".into(),
        },
        &archive_path,
        &destination,
    )
    .unwrap();
    assert_eq!(evidence.entries, 3);
    assert_eq!(evidence.regular_files, 1);
    assert_eq!(
        fs::read(destination.join("payload/data.txt")).unwrap(),
        body
    );
    assert_eq!(
        fs::read_link(destination.join("payload/link")).unwrap(),
        std::path::Path::new("data.txt")
    );
}

fn assert_rejected(error: BundleError, entries: &[RawEntry<'_>]) {
    let result = run(entries, false);
    assert_eq!(result.unwrap_err(), error);
}

#[test]
fn rejects_traversal_and_absolute_paths() {
    for path in ["../escape", "/absolute"] {
        let body = b"x";
        let manifest = regular_manifest(path, body);
        assert_rejected(
            BundleError::UnsafePath(path.to_owned()),
            &[
                RawEntry {
                    path: "bundle-manifest.json",
                    kind: b'0',
                    body: &manifest,
                    link: None,
                },
                RawEntry {
                    path,
                    kind: b'0',
                    body,
                    link: None,
                },
            ],
        );
    }
}

#[test]
fn rejects_duplicate_archive_entries() {
    let body = b"x";
    let manifest = regular_manifest("same", body);
    assert_rejected(
        BundleError::DuplicateArchiveEntry("same".to_owned()),
        &[
            RawEntry {
                path: "bundle-manifest.json",
                kind: b'0',
                body: &manifest,
                link: None,
            },
            RawEntry {
                path: "same",
                kind: b'0',
                body,
                link: None,
            },
            RawEntry {
                path: "same",
                kind: b'0',
                body,
                link: None,
            },
        ],
    );
}

#[test]
fn rejects_device_nodes_and_escaping_links() {
    let device_manifest =
        manifest(json!([{"path":"device","kind":"file","size":0,"sha256":hash(b"")} ]));
    assert_rejected(
        BundleError::UnsupportedEntryType("device".to_owned()),
        &[
            RawEntry {
                path: "bundle-manifest.json",
                kind: b'0',
                body: &device_manifest,
                link: None,
            },
            RawEntry {
                path: "device",
                kind: b'3',
                body: b"",
                link: None,
            },
        ],
    );

    let link_manifest =
        manifest(json!([{"path":"links/out","kind":"symlink","target":"../../outside"}]));
    assert_rejected(
        BundleError::UnsafeLink("links/out".to_owned()),
        &[
            RawEntry {
                path: "bundle-manifest.json",
                kind: b'0',
                body: &link_manifest,
                link: None,
            },
            RawEntry {
                path: "links/out",
                kind: b'2',
                body: b"",
                link: Some("../../outside"),
            },
        ],
    );

    let hardlink_manifest = regular_manifest("hardlink", b"");
    assert_rejected(
        BundleError::UnsupportedEntryType("hardlink".to_owned()),
        &[
            RawEntry {
                path: "bundle-manifest.json",
                kind: b'0',
                body: &hardlink_manifest,
                link: None,
            },
            RawEntry {
                path: "hardlink",
                kind: b'1',
                body: b"",
                link: Some("../../outside"),
            },
        ],
    );
}

#[test]
fn rejects_truncated_archives_without_publishing_destination() {
    let manifest = regular_manifest("file", b"content");
    let entries = [
        RawEntry {
            path: "bundle-manifest.json",
            kind: b'0',
            body: &manifest,
            link: None,
        },
        RawEntry {
            path: "file",
            kind: b'0',
            body: b"content",
            link: None,
        },
    ];
    let result = run(&entries, true);
    assert!(matches!(result, Err(BundleError::Archive(_))));
}

#[test]
fn rejects_extra_and_missing_manifest_entries() {
    let empty = manifest(json!([]));
    assert_rejected(
        BundleError::UnexpectedArchiveEntry("extra".to_owned()),
        &[
            RawEntry {
                path: "bundle-manifest.json",
                kind: b'0',
                body: &empty,
                link: None,
            },
            RawEntry {
                path: "extra",
                kind: b'0',
                body: b"",
                link: None,
            },
        ],
    );

    let manifest = regular_manifest("missing", b"x");
    assert_rejected(
        BundleError::MissingArchiveEntry("missing".to_owned()),
        &[RawEntry {
            path: "bundle-manifest.json",
            kind: b'0',
            body: &manifest,
            link: None,
        }],
    );
}

#[test]
fn rejects_per_file_hash_mismatch() {
    let manifest = regular_manifest("file", b"expected");
    assert_rejected(
        BundleError::FileHashMismatch("file".to_owned()),
        &[
            RawEntry {
                path: "bundle-manifest.json",
                kind: b'0',
                body: &manifest,
                link: None,
            },
            RawEntry {
                path: "file",
                kind: b'0',
                body: b"altered!",
                link: None,
            },
        ],
    );
}

#[test]
fn validation_failure_never_publishes_destination() {
    let body = b"altered";
    let manifest = regular_manifest("file", b"expected");
    let archive = raw_tar(
        &[
            RawEntry {
                path: "bundle-manifest.json",
                kind: b'0',
                body: &manifest,
                link: None,
            },
            RawEntry {
                path: "file",
                kind: b'0',
                body,
                link: None,
            },
        ],
        false,
    );
    let temp = tempfile::tempdir().unwrap();
    let archive_path = temp.path().join("bundle.tar.zst");
    fs::write(&archive_path, &archive).unwrap();
    let destination = temp.path().join("output");
    let lock = BundleLock {
        url: "https://example.invalid/bundle.tar.zst".into(),
        sha256: hash(&archive),
        size: archive.len() as u64,
        media_type: MEDIA_TYPE.into(),
        platform: "linux/arm64".into(),
    };
    assert_eq!(
        validate_bundle(&lock, &archive_path, &destination).unwrap_err(),
        BundleError::FileSizeMismatch("file".to_owned())
    );
    assert!(!destination.exists());
}

#[test]
fn rejects_unsorted_manifest_and_non_manifest_first_entry() {
    let unsorted = manifest(json!([
        {"path":"z","kind":"file","size":0,"sha256":hash(b"")},
        {"path":"a","kind":"file","size":0,"sha256":hash(b"")}
    ]));
    assert_rejected(
        BundleError::NonCanonicalManifest,
        &[RawEntry {
            path: "bundle-manifest.json",
            kind: b'0',
            body: &unsorted,
            link: None,
        }],
    );

    assert_rejected(
        BundleError::ManifestMustBeFirst,
        &[RawEntry {
            path: "other",
            kind: b'0',
            body: b"",
            link: None,
        }],
    );
}

#[test]
fn outer_lock_size_and_hash_are_exact() {
    let archive = raw_tar(&[], false);
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("archive");
    fs::write(&path, &archive).unwrap();
    let mut lock = BundleLock {
        url: "https://example.invalid/archive".into(),
        sha256: hash(&archive),
        size: archive.len() as u64 + 1,
        media_type: MEDIA_TYPE.into(),
        platform: "linux/arm64".into(),
    };
    assert_eq!(
        validate_bundle(&lock, &path, &temp.path().join("out")).unwrap_err(),
        BundleError::ArchiveSizeMismatch
    );
    lock.size -= 1;
    lock.sha256 = "0".repeat(64);
    assert_eq!(
        validate_bundle(&lock, &path, &temp.path().join("out")).unwrap_err(),
        BundleError::ArchiveHashMismatch
    );
}
