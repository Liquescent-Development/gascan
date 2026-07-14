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
    let script = include_str!("../build-workspace-image.sh");
    for required in [
        "--connect-timeout 15",
        "--max-time 120",
        "--progress-bar",
        "--proto-redir '=https'",
        "validate_download_url",
        "release-assets.githubusercontent.com",
        "cdn.playwright.dev",
        "validate-image-inspect",
    ] {
        assert!(
            script.contains(required),
            "missing build safeguard: {required}"
        );
    }
}
