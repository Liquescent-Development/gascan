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
}
#[async_trait::async_trait]
impl Provisioner for ConfiguredProvisioner {
    async fn provision(
        &self,
        _request: ProvisionRequest<'_>,
    ) -> Result<ProvisionResolution, ServiceError> {
        tokio::time::sleep(self.delay).await;
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
    let service = Arc::new(SandboxService::new(
        FakeRuntime::default(),
        store,
        Arc::new(ConfiguredProvisioner {
            delay: provision_delay,
        }),
    ));
    let config = DaemonConfig::new(paths, idle_timeout);
    let api = SandboxApi::new(service, config.activity());
    Daemon::serve(config, api).await?;
    Ok(())
}
