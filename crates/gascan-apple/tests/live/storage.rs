use super::common::{LiveContext, TestError, exact_workspace_bind};

#[test]
fn bind_inspect_requires_one_exact_read_write_workspace_source() {
    let source = std::path::Path::new("/tmp/gascan-feas-42-workspace");
    let exact = serde_json::json!([{"configuration":{"mounts":[{
        "type":"bind","source":source,"destination":"/workspace","options":["rw"]
    }]}}]);
    assert!(exact_workspace_bind(&exact, source).is_some());
    let broader = serde_json::json!([{"configuration":{"mounts":[{
        "type":"bind","source":"/tmp","destination":"/workspace","options":["rw"]
    }]}}]);
    assert!(exact_workspace_bind(&broader, source).is_none());
}

#[tokio::test]
#[ignore = "requires Apple silicon macOS 26+ with container service"]
async fn bind_mount_is_exact_and_named_volume_persists() -> Result<(), TestError> {
    let ctx = LiveContext::new("storage").await?;
    let inspect = ctx.inspect().await?;
    let workspace = ctx.canonical_workspace()?;
    assert!(exact_workspace_bind(&inspect, &workspace).is_some());
    ctx.write_host("visible.txt", "host").await?;
    ctx.exec("printf changed > /workspace/visible.txt").await?;
    assert_eq!(ctx.read_host("visible.txt").await?, "changed");
    assert!(ctx.exec("test ! -e /workspace/../forbidden").await?.status == 0);
    ctx.write_cache("sentinel", "persisted").await?;
    ctx.recreate_container().await?;
    assert_eq!(ctx.read_cache("sentinel").await?, "persisted");
    ctx.cleanup().await
}
