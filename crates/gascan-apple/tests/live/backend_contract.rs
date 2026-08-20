use gascan_apple::{AppleBackend, ProcessRunner};
use gascan_conformance::{CreateRequestFixture, backend_contract};
use gascan_core::runtime::RuntimeBackend;
use std::time::{SystemTime, UNIX_EPOCH};

#[tokio::test]
#[ignore = "requires Apple silicon macOS 26+ with container service and locked workspace image"]
async fn backend_contract_holds_on_apple() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let name = format!("gascan-live-backend-{}-{nonce}", std::process::id());
    let fixture = CreateRequestFixture::pinned(&name, "offline");
    let backend = AppleBackend::new(ProcessRunner);
    backend_contract(&backend, &fixture).await;
    assert!(
        !backend
            .list_resources()
            .await
            .unwrap()
            .iter()
            .any(|resource| resource.name().starts_with(&name))
    );
}
