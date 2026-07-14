#![forbid(unsafe_code)]
#![deny(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use gascand::{Daemon, DaemonConfig, LocalApi, SocketPaths};
use std::time::Duration;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let idle_timeout = std::env::var("GASCAN_IDLE_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map_or(Duration::from_secs(300), Duration::from_millis);
    let config = DaemonConfig::new(SocketPaths::for_user()?, idle_timeout);
    let api = LocalApi::new(config.activity());
    Daemon::serve(config, api).await?;
    Ok(())
}
