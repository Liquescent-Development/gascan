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
    workspace_build_mode: String,
    base_image: String,
    ubuntu_snapshot: String,
    mise: VersionedArtifact,
    tools: BTreeMap<String, String>,
    playwright_chromium: VersionedArtifact,
    gascamp: Gascamp,
    workspace_tag: String,
    workspace_bundles: WorkspaceBundles,
    workstation_artifacts: BTreeMap<String, WorkstationArtifact>,
    workstation_npm: WorkstationNpm,
}

#[derive(Clone, Deserialize)]
struct WorkstationArtifact {
    version: String,
    url: String,
    sha256: String,
    platform: String,
    kind: String,
    size: u64,
}

#[derive(Deserialize)]
struct WorkstationNpm {
    scripts: String,
    npm_version: String,
    package_manifest_sha256: String,
    package_lock_sha256: String,
    lifecycle_exceptions: BTreeMap<String, NpmLifecycleException>,
    claude_native: ClaudeNative,
}

#[derive(Deserialize)]
struct NpmLifecycleException {
    package: String,
    version: String,
    command: String,
    integrity: String,
    manifest_sha256: String,
    script_path: Option<String>,
    script_sha256: Option<String>,
}

#[derive(Deserialize)]
struct ClaudeNative {
    package: String,
    version: String,
    url: String,
    integrity: String,
    sha256: String,
    size: u64,
    binary_path: String,
    binary_sha256: String,
    binary_size: u64,
    platform: String,
}

const REQUIRED_WORKSTATION_ARTIFACTS: &[&str] =
    &["claude", "codex", "pi", "herdr", "glab", "neovim"];

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
    assert_eq!(lock.workspace_build_mode, "connected");
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
    assert_eq!(
        lock.workstation_artifacts.len(),
        REQUIRED_WORKSTATION_ARTIFACTS.len()
    );
    assert!(
        REQUIRED_WORKSTATION_ARTIFACTS
            .iter()
            .all(|name| lock.workstation_artifacts.contains_key(*name))
    );
    for (name, artifact) in lock.workstation_artifacts {
        assert!(
            !artifact.version.is_empty()
                && !matches!(artifact.version.as_str(), "latest" | "stable" | "lts")
                && !artifact.version.contains('*')
        );
        assert!(artifact.url.starts_with("https://"), "{name}");
        assert!(sha256_is_lower_hex(&artifact.sha256), "{name}");
        assert_eq!(artifact.platform, "linux-arm64", "{name}");
        assert!(artifact.size > 0, "{name}");
        if name == "herdr" {
            assert_eq!(artifact.kind, "raw_binary");
            assert!(artifact.size <= 64 * 1024 * 1024);
            assert!(artifact.url.ends_with("/herdr-linux-aarch64"));
        } else {
            assert!(
                matches!(artifact.kind.as_str(), "npm_tgz" | "tar_gz"),
                "{name}"
            );
        }
    }
    assert_eq!(lock.workstation_npm.scripts, "disabled");
    assert_eq!(lock.workstation_npm.npm_version, "11.12.1");
    assert!(sha256_is_lower_hex(
        &lock.workstation_npm.package_manifest_sha256
    ));
    assert!(sha256_is_lower_hex(
        &lock.workstation_npm.package_lock_sha256
    ));
    assert_eq!(lock.workstation_npm.lifecycle_exceptions.len(), 3);
    for (name, evidence) in lock.workstation_npm.lifecycle_exceptions {
        assert!(!evidence.package.is_empty(), "{name}");
        assert!(!evidence.version.is_empty(), "{name}");
        assert!(!evidence.command.is_empty(), "{name}");
        assert!(evidence.integrity.starts_with("sha512-"), "{name}");
        assert!(sha256_is_lower_hex(&evidence.manifest_sha256), "{name}");
        assert_eq!(
            evidence.script_path.is_some(),
            evidence.script_sha256.is_some(),
            "{name}"
        );
        if let Some(digest) = evidence.script_sha256 {
            assert!(sha256_is_lower_hex(&digest), "{name}");
        }
    }
    let native = lock.workstation_npm.claude_native;
    assert_eq!(native.package, "@anthropic-ai/claude-code-linux-arm64");
    assert_eq!(native.version, "2.1.218");
    assert!(native.url.starts_with("https://registry.npmjs.org/"));
    assert!(native.integrity.starts_with("sha512-"));
    assert!(sha256_is_lower_hex(&native.sha256));
    assert_eq!(native.size, 84_159_749);
    assert_eq!(native.binary_path, "package/claude");
    assert!(sha256_is_lower_hex(&native.binary_sha256));
    assert_eq!(native.binary_size, 269_990_816);
    assert_eq!(native.platform, "linux-arm64");
}

#[test]
fn workspace_build_mode_accepts_only_connected_and_keeps_bundles_pending() {
    #[derive(Deserialize)]
    struct ModeLock {
        workspace_build_mode: String,
        workspace_bundles: WorkspaceBundles,
    }

    let parse = |mode: &str| {
        toml::from_str::<ModeLock>(&format!(
            "workspace_build_mode = {mode}\n[workspace_bundles]\nmedia_type = \"application/vnd.gascan.workspace-bundle.v1+tar.zstd\"\nplatform = \"linux/arm64\"\npublication = \"pending\"\n"
        ))
        .map(|lock| {
            lock.workspace_build_mode == "connected"
                && lock.workspace_bundles.publication == "pending"
        })
        .unwrap_or(false)
    };

    assert!(parse("\"connected\""));
    for rejected in ["\"offline\"", "\"published\"", "\"CONNECTED\"", "1"] {
        assert!(!parse(rejected), "accepted invalid mode {rejected}");
    }
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

#[test]
fn build_dispatcher_has_no_mode_fallback_and_preserves_deferred_offline_entrypoint() {
    let dispatcher = include_str!("../build-workspace-image.sh");
    assert!(dispatcher.contains("workspace_build_mode"));
    assert!(dispatcher.contains("build-connected-workspace-image.sh"));
    assert!(dispatcher.contains("build-offline-workspace-image.sh"));
    assert!(!dispatcher.contains("auto"));

    let offline = include_str!("../build-offline-workspace-image.sh");
    assert!(offline.contains("UBUNTU_SNAPSHOT"));
    assert!(offline.contains("verify-workspace-image-inputs.sh"));
}
