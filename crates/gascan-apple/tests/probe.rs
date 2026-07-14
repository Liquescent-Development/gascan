use async_trait::async_trait;
use gascan_apple::{AppleProbe, CommandOutput, CommandRunner, CommandSpec};
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

#[tokio::test]
async fn accepts_supported_major_and_rejects_future_major() {
    let supported = probe_with_output(include_bytes!("fixtures/system-version-1.0.0.json"))
        .base_capabilities()
        .await;
    assert_eq!(supported.unwrap().version, RuntimeVersion::new(1, 0, 0));

    let future = probe_with_output(include_bytes!("fixtures/system-version-unsupported.json"))
        .version()
        .await;
    assert!(matches!(
        future,
        Err(RuntimeError::UnsupportedVersion { .. })
    ));
}

#[tokio::test]
async fn leaves_live_capabilities_unverified() {
    let capabilities = probe_with_output(include_bytes!("fixtures/system-version-1.0.0.json"))
        .base_capabilities()
        .await
        .unwrap();

    assert!(!capabilities.bind_mounts);
    assert!(!capabilities.named_volumes);
    assert!(!capabilities.tty);
    assert!(!capabilities.signals);
    assert!(!capabilities.loopback_publish);
    assert!(!capabilities.resource_limits);
    assert_eq!(capabilities.offline, NetworkIsolation::Unverified);
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
        br#"[{"appName":"helper","future":true},42,{"unknown":"shape"},{"appName":"container","version":"1.9.3","future":true}]"#,
    )
    .version()
    .await
    .unwrap();

    assert_eq!(version, RuntimeVersion::new(1, 9, 3));
}
