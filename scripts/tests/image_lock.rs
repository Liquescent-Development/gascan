use std::collections::BTreeMap;

use serde::Deserialize;

#[derive(Deserialize)]
struct Artifact {
    url: String,
    sha256: String,
}

#[derive(Deserialize)]
struct VersionedArtifact {
    version: String,
    url: String,
    sha256: String,
}

#[derive(Deserialize)]
struct Gascamp {
    revision: String,
}

#[derive(Deserialize)]
struct ImageLock {
    base_image: String,
    ubuntu_snapshot: String,
    mise: VersionedArtifact,
    tools: BTreeMap<String, String>,
    playwright_chromium: VersionedArtifact,
    gascamp: Gascamp,
    workspace_tag: String,
    workspace_bundles: WorkspaceBundles,
}

#[derive(Deserialize)]
struct WorkspaceBundles {
    media_type: String,
    platform: String,
    publication: String,
}

fn sha256_is_lower_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[test]
fn every_remote_image_input_is_immutable_and_checksummed() {
    let lock: ImageLock =
        toml::from_str(include_str!("../../images/workspace/versions.lock")).unwrap();
    assert!(lock.base_image.starts_with("ubuntu@sha256:"));
    assert!(sha256_is_lower_hex(
        lock.base_image.trim_start_matches("ubuntu@sha256:")
    ));
    assert!(lock.ubuntu_snapshot.ends_with('Z'));
    assert!(!lock.mise.version.is_empty());
    assert!(lock.mise.url.starts_with("https://"));
    assert!(sha256_is_lower_hex(&lock.mise.sha256));
    assert!(lock.tools.values().all(|version| {
        !version.is_empty()
            && !matches!(version.as_str(), "latest" | "stable" | "lts")
            && !version.contains('*')
    }));
    assert!(!lock.playwright_chromium.version.is_empty());
    assert!(lock.playwright_chromium.url.starts_with("https://"));
    assert!(sha256_is_lower_hex(&lock.playwright_chromium.sha256));
    assert_eq!(lock.gascamp.revision.len(), 40);
    assert!(
        lock.gascamp
            .revision
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    );
    assert!(!lock.workspace_tag.ends_with(":latest"));
    assert_eq!(
        lock.workspace_bundles.media_type,
        "application/vnd.gascan.workspace-bundle.v1+tar.zstd"
    );
    assert_eq!(lock.workspace_bundles.platform, "linux/arm64");
    assert_eq!(lock.workspace_bundles.publication, "pending");
}

#[test]
fn published_bundle_lock_requires_all_concrete_immutable_records() {
    use gascan_image_tools::bundle::{BundleError, BundlePublication, PublishedBundleLocks};

    let record = |suffix: &str, hash: char, size: u64| {
        format!(
            r#"
[workspace_bundles.{suffix}]
url = "https://github.com/example/gascan/releases/download/lock/{suffix}.tar.zst"
sha256 = "{}"
size = {size}
media_type = "application/vnd.gascan.workspace-bundle.v1+tar.zstd"
platform = "linux/arm64"
"#,
            hash.to_string().repeat(64)
        )
    };
    let valid = format!(
        "{}{}{}{}",
        r#"[workspace_bundles]
media_type = "application/vnd.gascan.workspace-bundle.v1+tar.zstd"
platform = "linux/arm64"
publication = "published"
"#,
        record("ubuntu_packages", 'a', 101),
        record("mise_runtimes", 'b', 202),
        record("gascamp_source_vendor", 'c', 303)
    );
    let locks = PublishedBundleLocks::from_toml(&valid).unwrap();
    assert_eq!(locks.ubuntu_packages.size, 101);
    assert_eq!(locks.mise_runtimes.size, 202);
    assert_eq!(locks.gascamp_source_vendor.size, 303);

    let pending = valid.replacen(
        "publication = \"published\"",
        "publication = \"pending\"",
        1,
    );
    assert_eq!(
        PublishedBundleLocks::from_toml(&pending).unwrap_err(),
        BundleError::InvalidPublicationState
    );
    assert!(matches!(
        BundlePublication::from_toml(&pending).unwrap(),
        BundlePublication::Pending(_)
    ));

    assert_eq!(
        PublishedBundleLocks::from_toml(&format!(
            "{}{}",
            r#"[workspace_bundles]
media_type = "application/vnd.gascan.workspace-bundle.v1+tar.zstd"
platform = "linux/arm64"
publication = "published"
"#,
            record("ubuntu_packages", 'a', 101)
        ))
        .unwrap_err(),
        BundleError::MissingLockRecord("mise_runtimes")
    );

    let first_record = valid.find("[workspace_bundles.ubuntu_packages]").unwrap();
    let wrong_platform = format!(
        "{}{}",
        &valid[..first_record],
        valid[first_record..].replacen("linux/arm64", "linux/amd64", 1)
    );
    assert_eq!(
        PublishedBundleLocks::from_toml(&wrong_platform).unwrap_err(),
        BundleError::InvalidLockRecord("ubuntu_packages")
    );

    let uppercase_hash = valid.replacen(&"a".repeat(64), &"A".repeat(64), 1);
    assert_eq!(
        PublishedBundleLocks::from_toml(&uppercase_hash).unwrap_err(),
        BundleError::InvalidLockRecord("ubuntu_packages")
    );

    let zero_size = valid.replacen("size = 101", "size = 0", 1);
    assert_eq!(
        PublishedBundleLocks::from_toml(&zero_size).unwrap_err(),
        BundleError::InvalidLockRecord("ubuntu_packages")
    );
}

#[test]
fn artifact_shape_requires_url_and_checksum() {
    let artifact = Artifact {
        url: "https://example.invalid/artifact".to_owned(),
        sha256: "0".repeat(64),
    };
    assert!(artifact.url.starts_with("https://"));
    assert!(sha256_is_lower_hex(&artifact.sha256));
}

#[test]
fn build_script_bounds_downloads_and_validates_redirect_hosts() {
    let script = include_str!("../prefetch-workspace-image.sh");
    for required in ["fetch-image-artifact", "validate-image-inspect"] {
        assert!(
            script.contains(required),
            "missing build safeguard: {required}"
        );
    }
    assert!(!script.contains("curl --"));
    assert!(!script.contains("--location"));
    let build = include_str!("../build-workspace-image.sh");
    assert!(!build.contains("fetch-image-artifact"));
    assert!(!build.contains("container image pull"));
}
