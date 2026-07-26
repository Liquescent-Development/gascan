use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use gascan_image_tools::{
    ArtifactClass, RedirectRules, install_sri_artifact, install_verified_artifact,
    validate_cached_artifact, validate_cached_sri_artifact, walk_redirects_with,
};
use reqwest::Url;
use sha2::{Digest, Sha256, Sha512};
use std::{
    os::unix::fs::{PermissionsExt, symlink},
    process::Command,
};

#[test]
fn unapproved_intermediate_redirect_is_rejected_before_contact() {
    let contacts = Arc::new(AtomicUsize::new(0));
    let observed = contacts.clone();
    let rules = RedirectRules::for_test_http_origins(["approved.test".to_owned()], 3);
    let result = walk_redirects_with("http://approved.test/artifact", rules, move |url| {
        observed.fetch_add(1, Ordering::SeqCst);
        if url.host_str() == Some("approved.test") {
            Ok(Some(Url::parse("http://unapproved.test/intermediate")?))
        } else {
            Ok(None)
        }
    });

    assert!(result.is_err());
    assert_eq!(contacts.load(Ordering::SeqCst), 1);
}

#[test]
fn artifact_classes_own_exact_initial_and_redirect_hosts() {
    let bundle = RedirectRules::for_artifact(ArtifactClass::WorkspaceBundle);
    assert!(
        bundle
            .require_initial_url("https://example.invalid/bundle.tar.zst")
            .is_err()
    );
    assert!(
        walk_redirects_with(
            "https://github.com/Liquescent-Development/gascan/releases/download/x/bundle.tar.zst",
            bundle.clone(),
            |_| Ok(None),
        )
        .is_ok()
    );
    assert!(
        walk_redirects_with("https://example.invalid/bundle.tar.zst", bundle, |_| Ok(
            None
        ),)
        .is_err()
    );
}

#[test]
fn warm_cache_is_revalidated_and_failed_refresh_preserves_valid_bytes() {
    let temporary = tempfile::tempdir().unwrap();
    let destination = temporary.path().join("artifact");
    let valid = b"locked artifact bytes";
    let hash = format!("{:x}", Sha256::digest(valid));
    std::fs::write(&destination, valid).unwrap();
    validate_cached_artifact(&destination, &hash, valid.len() as u64).unwrap();

    std::fs::write(&destination, b"corrupt warm cache").unwrap();
    assert!(validate_cached_artifact(&destination, &hash, valid.len() as u64).is_err());
    std::fs::write(&destination, valid).unwrap();
    assert!(
        install_verified_artifact(
            b"failed refresh bytes".as_slice(),
            &destination,
            &hash,
            valid.len() as u64,
            ArtifactClass::WorkspaceBundle,
        )
        .is_err()
    );
    assert_eq!(std::fs::read(destination).unwrap(), valid);
}

#[test]
fn exact_size_and_code_owned_maximum_are_both_enforced() {
    let temporary = tempfile::tempdir().unwrap();
    let destination = temporary.path().join("artifact");
    let bytes = b"small";
    let hash = format!("{:x}", Sha256::digest(bytes));
    assert!(
        install_verified_artifact(
            bytes.as_slice(),
            &destination,
            &hash,
            6,
            ArtifactClass::Mise,
        )
        .is_err()
    );
    assert!(!destination.exists());
    assert!(ArtifactClass::Mise.maximum_bytes() < ArtifactClass::Chromium.maximum_bytes());
}

#[test]
fn cached_artifact_symlink_is_rejected_without_following() {
    let temporary = tempfile::tempdir().unwrap();
    let target = temporary.path().join("target");
    let cached = temporary.path().join("cached");
    let bytes = b"valid bytes";
    std::fs::write(&target, bytes).unwrap();
    symlink(&target, &cached).unwrap();
    let hash = format!("{:x}", Sha256::digest(bytes));
    assert!(validate_cached_artifact(&cached, &hash, bytes.len() as u64).is_err());
}

#[test]
fn npm_sri_publication_is_read_only_and_cache_links_or_mutations_are_rejected() {
    let temporary = tempfile::tempdir().unwrap();
    let destination = temporary.path().join("npm.tgz");
    let bytes = b"locked npm tarball";
    let integrity = format!("sha512-{}", BASE64.encode(Sha512::digest(bytes)));
    install_sri_artifact(bytes.as_slice(), &destination, &integrity, 1024).unwrap();
    assert_eq!(
        std::fs::metadata(&destination)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o444
    );
    validate_cached_sri_artifact(&destination, &integrity, 1024).unwrap();

    let peer = temporary.path().join("peer");
    std::fs::hard_link(&destination, peer).unwrap();
    assert!(validate_cached_sri_artifact(&destination, &integrity, 1024).is_err());
    std::fs::remove_file(&destination).unwrap();
    std::fs::write(&destination, b"mutated").unwrap();
    assert!(validate_cached_sri_artifact(&destination, &integrity, 1024).is_err());
}

#[test]
fn native_warm_cache_hard_link_is_rejected_before_binary_chmod_or_reuse() {
    let temporary = tempfile::tempdir().unwrap();
    let cached = temporary.path().join("native");
    let peer = temporary.path().join("peer");
    let bytes = b"locked native";
    std::fs::write(&cached, bytes).unwrap();
    std::fs::hard_link(&cached, &peer).unwrap();
    std::fs::set_permissions(&cached, std::fs::Permissions::from_mode(0o644)).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_fetch-image-artifact"))
        .args([
            "workstation-github",
            "https://github.com/example/unreachable",
            &format!("{:x}", Sha256::digest(bytes)),
        ])
        .arg(&cached)
        .arg(bytes.len().to_string())
        .env("HTTPS_PROXY", "http://127.0.0.1:9")
        .env("NO_PROXY", "")
        .output()
        .unwrap();
    assert!(!output.status.success(), "hard-linked cache was reused");
    assert_eq!(
        std::fs::metadata(peer).unwrap().permissions().mode() & 0o777,
        0o644,
        "downloader chmod mutated the peer hard link"
    );
}
