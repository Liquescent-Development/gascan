#![forbid(unsafe_code)]
#![deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use gascan_core::fake_runtime::FakeRuntime;
use gascand::{
    Daemon, DaemonConfig, ProvisionRequest, ProvisionResolution, Provisioner, SandboxApi,
    SandboxService, ServiceError, SocketPaths, Store,
};
use std::{sync::Arc, time::Duration};

struct ConfiguredProvisioner {
    delay: Duration,
    fail: bool,
}
#[async_trait::async_trait]
impl Provisioner for ConfiguredProvisioner {
    async fn provision(
        &self,
        _request: ProvisionRequest<'_>,
    ) -> Result<ProvisionResolution, ServiceError> {
        tokio::time::sleep(self.delay).await;
        if self.fail {
            return Err(ServiceError::Provision("configured failure".to_owned()));
        }
        Ok(ProvisionResolution::default())
    }
    async fn health_check(
        &self,
        _id: &gascan_core::sandbox::SandboxId,
    ) -> Result<(), ServiceError> {
        Ok(())
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let idle_timeout = std::env::var("GASCAN_IDLE_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map_or(Duration::from_secs(300), Duration::from_millis);
    let paths = SocketPaths::for_user()?;
    paths.prepare_directory()?;
    let state_path = std::env::var_os("GASCAN_STATE_PATH")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| paths.directory().join("state.sqlite3"));
    let store = Store::open(state_path)?;
    let provision_delay = std::env::var("GASCAN_FAKE_PROVISION_DELAY_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map_or(Duration::ZERO, Duration::from_millis);
    let provision_fail = std::env::var_os("GASCAN_FAKE_PROVISION_FAIL").is_some();
    let fake_state_path = std::env::var_os("GASCAN_FAKE_STATE_PATH")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| paths.directory().join("fake-runtime.json"));
    let runtime = FakeRuntime::persistent(
        gascan_core::fake_runtime::fixture_capabilities(),
        fake_state_path,
    )
    .await?;
    if std::env::var_os("GASCAN_FAKE_CAPABILITIES_FAIL").is_some() {
        runtime
            .inject_failure(gascan_core::fake_runtime::FailureBoundary::Capabilities)
            .await;
    }
    if let Some(delay) = std::env::var("GASCAN_FAKE_LOGS_FAIL_AFTER_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
    {
        let failing = runtime.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(delay)).await;
            failing
                .inject_failure(gascan_core::fake_runtime::FailureBoundary::Logs)
                .await;
        });
    }
    let service = Arc::new(SandboxService::new(
        runtime,
        store,
        Arc::new(ConfiguredProvisioner {
            delay: provision_delay,
            fail: provision_fail,
        }),
    ));
    let config = DaemonConfig::new(paths, idle_timeout);
    let api = SandboxApi::new(service, config.activity());
    Daemon::serve(config, api).await?;
    Ok(())
}
