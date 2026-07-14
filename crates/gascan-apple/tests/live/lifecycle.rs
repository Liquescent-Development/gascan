use super::common::{LiveContext, TestError};

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
