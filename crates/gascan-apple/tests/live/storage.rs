use super::common::{LiveContext, TestError};

#[tokio::test]
#[ignore = "requires Apple silicon macOS 26+ with container service"]
async fn bind_mount_is_exact_and_named_volume_persists() -> Result<(), TestError> {
    let ctx = LiveContext::new("storage").await?;
    ctx.write_host("visible.txt", "host").await?;
    ctx.exec("printf changed > /workspace/visible.txt").await?;
    assert_eq!(ctx.read_host("visible.txt").await?, "changed");
    assert!(ctx.exec("test ! -e /workspace/../forbidden").await?.status == 0);
    ctx.write_cache("sentinel", "persisted").await?;
    ctx.recreate_container().await?;
    assert_eq!(ctx.read_cache("sentinel").await?, "persisted");
    ctx.cleanup().await
}
