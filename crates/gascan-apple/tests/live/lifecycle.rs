use serde_json::json;

use super::common::{LiveContext, TestError, container_state};

#[test]
fn reads_lifecycle_state_from_the_exact_apple_status_path() {
    let inspect = json!([{
        "status": {"networks": [], "state": "running"},
        "configuration": {"status": "not-the-runtime-state"}
    }]);
    assert_eq!(container_state(&inspect), Some("running"));
    assert_eq!(container_state(&json!({"status": "running"})), None);
}

#[tokio::test]
#[ignore = "requires Apple silicon macOS 26+ with container service"]
async fn stop_start_are_idempotent_and_inspect_is_structured() -> Result<(), TestError> {
    let ctx = LiveContext::new("lifecycle").await?;
    assert!(ctx.inspect().await?.is_object() || ctx.inspect().await?.is_array());
    ctx.stop().await?;
    ctx.stop().await?;
    assert!(!ctx.is_running().await?);
    ctx.start().await?;
    ctx.start().await?;
    assert!(ctx.is_running().await?);
    ctx.cleanup().await
}
