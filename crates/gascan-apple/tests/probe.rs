use async_trait::async_trait;
use gascan_apple::{
    APPLE_1_1_COMMIT, AppleCompatibility, AppleProbe, AppleReleaseEvidence, CommandOutput,
    CommandRunner, CommandSpec,
};
use gascan_core::runtime::{NetworkIsolation, RuntimeError, RuntimeVersion};

struct FixtureRunner(&'static [u8]);

#[async_trait]
impl CommandRunner for FixtureRunner {
    async fn run(&self, spec: CommandSpec) -> Result<CommandOutput, RuntimeError> {
        assert_eq!(
            spec,
            CommandSpec::new("container", ["system", "version", "--format", "json"])
        );
        Ok(CommandOutput {
            status: 0,
            stdout: self.0.to_vec(),
            stderr: Vec::new(),
        })
    }
}

fn probe_with_output(output: &'static [u8]) -> AppleProbe<FixtureRunner> {
    AppleProbe::new(FixtureRunner(output))
}

struct StatusRunner(&'static [u8]);

#[async_trait]
impl CommandRunner for StatusRunner {
    async fn run(&self, spec: CommandSpec) -> Result<CommandOutput, RuntimeError> {
        assert_eq!(
            spec,
            CommandSpec::new("container", ["system", "status", "--format", "json"])
        );
        Ok(CommandOutput {
            status: 0,
            stdout: self.0.to_vec(),
            stderr: Vec::new(),
        })
    }
}

#[tokio::test]
async fn structured_status_is_exact_and_proves_service_readiness() {
    let status = AppleProbe::new(StatusRunner(include_bytes!(
        "fixtures/system-status-1.1.0.json"
    )))
    .status()
    .await
    .unwrap();
    assert_eq!(
        status.app_root,
        "/Users/test/Library/Application Support/com.apple.container/"
    );
    assert_eq!(status.api_server_version, RuntimeVersion::new(1, 1, 0));
}

#[tokio::test]
async fn malformed_or_nonrunning_status_fails_closed() {
    for output in [br#"{}"#.as_slice(), br#"{"status":"stopped"}"#.as_slice()] {
        assert!(
            AppleProbe::new(StatusRunner(output))
                .status()
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn status_rejects_trailing_version_garbage_and_commit_mismatch() {
    for output in [
        br#"{"apiServerAppName":"container-apiserver","apiServerBuild":"release","apiServerCommit":"5973b9cc626a3e7a499bb316a958237ebe14e2ed","apiServerVersion":"container-apiserver version 1.1.0 (build: release, commit: 5973b9c) trailing","appRoot":"/tmp/","installRoot":"/usr/local/","status":"running"}"#.as_slice(),
        br#"{"apiServerAppName":"container-apiserver","apiServerBuild":"release","apiServerCommit":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","apiServerVersion":"container-apiserver version 1.1.0 (build: release, commit: 5973b9c)","appRoot":"/tmp/","installRoot":"/usr/local/","status":"running"}"#.as_slice(),
        br#"{"apiServerAppName":"container-apiserver","apiServerBuild":"release","apiServerCommit":"5973b9cc626a3e7a499bb316a958237ebe14e2ed","apiServerVersion":"container-apiserver version 1.1.0 (build: release, commit: 5973B9C)","appRoot":"/tmp/","installRoot":"/usr/local/","status":"running"}"#.as_slice(),
    ] {
        assert!(AppleProbe::new(StatusRunner(output)).status().await.is_err());
    }
}

fn classifies_only_apple_container_1_1_through_1_x_releases() {
    let cases = [
        ("1.0.9", false),
        ("1.1.0", true),
        ("1.1.1", true),
        ("1.2.0", true),
        ("1.99.99", true),
        ("2.0.0", false),
    ];

    for (raw_version, accepted) in cases {
        let parts: Vec<_> = raw_version
            .split('.')
            .map(|part| part.parse::<u64>().unwrap())
            .collect();
        let evidence = AppleReleaseEvidence {
            version: RuntimeVersion::new(parts[0], parts[1], parts[2]),
            commit: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
        };

        assert_eq!(
            evidence.compatibility().is_ok(),
            accepted,
            "{raw_version} acceptance"
        );
    }
}

#[tokio::test]
async fn distinguishes_certified_and_compatible_untested_release_evidence() {
    let certified = AppleReleaseEvidence {
        version: RuntimeVersion::new(1, 1, 0),
        commit: APPLE_1_1_COMMIT.to_owned(),
    };
    assert_eq!(
        certified.compatibility().unwrap(),
        AppleCompatibility::Certified
    );

    let untested = probe_with_output(include_bytes!("fixtures/system-version-1.2.0.json"))
        .release_evidence()
        .await
        .unwrap();
    assert_eq!(untested.version, RuntimeVersion::new(1, 2, 0));
    assert_eq!(
        untested.compatibility().unwrap(),
        AppleCompatibility::CompatibleUntested
    );
}

#[tokio::test]
async fn compatible_untested_releases_enable_ordinary_capabilities_but_not_offline() {
    let capabilities = probe_with_output(include_bytes!("fixtures/system-version-1.2.0.json"))
        .base_capabilities()
        .await
        .unwrap();

    assert!(capabilities.bind_mounts);
    assert!(capabilities.named_volumes);
    assert!(capabilities.tty);
    assert!(capabilities.signals);
    assert!(capabilities.loopback_publish);
    assert!(capabilities.resource_limits);
    assert_eq!(capabilities.offline, NetworkIsolation::Unsupported);

    for output in [
        br#"[{"appName":"container","buildType":"release","commit":"5973b9cc626a3e7a499bb316a958237ebe14e2ed","version":"1.1.0"}]"#.as_slice(),
        br#"[{"appName":"helper","version":"9.0.0"},{"appName":"container","buildType":"release","commit":"5973b9cc626a3e7a499bb316a958237ebe14e2ed","version":"1.1.0","future":true}]"#.as_slice(),
    ] {
        assert_eq!(
            probe_with_output(output)
                .base_capabilities()
                .await
                .unwrap()
                .offline,
            NetworkIsolation::Proven
        );
    }
}

#[tokio::test]
async fn offline_request_is_rejected_before_mount_construction_without_proof() {
    let capability = probe_with_output(
        br#"[{"appName":"container","buildType":"release","commit":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","version":"1.1.1"}]"#,
    )
    .base_capabilities()
    .await
    .unwrap()
    .offline;
    let mut mount_constructed = false;
    let result = gascan_apple::offline_network_args(capability, || {
        mount_constructed = true;
    });
    assert!(matches!(
        result,
        Err(RuntimeError::UnsupportedCapability { .. })
    ));
    assert!(!mount_constructed);
}

#[test]
fn proven_offline_form_is_exact_and_constructs_mount_after_gate() {
    let mut mount_constructed = false;
    let args = gascan_apple::offline_network_args(NetworkIsolation::Proven, || {
        mount_constructed = true;
    })
    .unwrap();
    assert_eq!(args, ["--network", "none"]);
    assert!(mount_constructed);
}

#[tokio::test]
async fn rejects_missing_duplicate_or_malformed_container_version() {
    for output in [
        br#"[{"appName":"container-apiserver","version":"1.0.0"}]"#.as_slice(),
        br#"[{"appName":"container","version":"1.0.0"},{"appName":"container","version":"1.0.1"}]"#
            .as_slice(),
        br#"[{"appName":"container","version":"container version 1.0.0"}]"#.as_slice(),
        br#"[{"appName":"container","version":"1.0"}]"#.as_slice(),
    ] {
        assert!(matches!(
            probe_with_output(output).version().await,
            Err(RuntimeError::InvalidOutput { .. })
        ));
    }
}

#[tokio::test]
async fn ignores_unknown_fields_and_extra_apps() {
    let version = probe_with_output(
        br#"[{"appName":"helper","future":true},42,{"unknown":"shape"},{"appName":"container","buildType":"release","commit":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","version":"1.9.3","future":true}]"#,
    )
    .version()
    .await
    .unwrap();

    assert_eq!(version, RuntimeVersion::new(1, 9, 3));
}
