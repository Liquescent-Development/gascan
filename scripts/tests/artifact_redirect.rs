use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use gascan_image_tools::{
    ArtifactClass, RedirectRules, install_verified_artifact, validate_cached_artifact,
    walk_redirects_with,
};
use reqwest::Url;
use sha2::{Digest, Sha256};
use std::os::unix::fs::symlink;

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
