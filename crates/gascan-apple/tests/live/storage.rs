use super::common::{LiveContext, TestError, exact_workspace_bind};

#[test]
fn bind_inspect_requires_one_exact_read_write_workspace_source() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let alias = temp.path().join("workspace-alias");
    std::os::unix::fs::symlink(&workspace, &alias).unwrap();
    let source = std::fs::canonicalize(&workspace).unwrap();
    let exact = serde_json::json!([{"configuration":{"mounts":[
        {"type":{"virtiofs":{}},"source":alias,"destination":"/workspace","options":[]},
        {"type":{"virtiofs":{}},"source":"/var/lib/container/volumes/cache","destination":"/opt/gascan","options":[]}
    ]}}]);
    assert!(exact_workspace_bind(&exact, &source).is_some());
    let broader = serde_json::json!([{"configuration":{"mounts":[{
        "type":{"virtiofs":{}},"source":temp.path(),"destination":"/workspace","options":["rw"]
    }]}}]);
    assert!(exact_workspace_bind(&broader, &source).is_none());
    let broader_extra = serde_json::json!([{"configuration":{"mounts":[
        {"type":{"virtiofs":{}},"source":alias,"destination":"/workspace","options":[]},
        {"type":{"virtiofs":{}},"source":temp.path(),"destination":"/broader","options":[]}
    ]}}]);
    assert!(exact_workspace_bind(&broader_extra, &source).is_none());
    let named_volume = serde_json::json!([{"configuration":{"mounts":[{
        "type":{"volume":{}},"source":source,"destination":"/workspace","options":["rw"]
    }]}}]);
    assert!(exact_workspace_bind(&named_volume, &source).is_none());
    let duplicate_workspace = serde_json::json!([{"configuration":{"mounts":[
        {"type":{"virtiofs":{}},"source":alias,"destination":"/workspace","options":[]},
        {"type":{"virtiofs":{}},"source":"/tmp/other","destination":"/workspace","options":[]}
    ]}}]);
    assert!(exact_workspace_bind(&duplicate_workspace, &source).is_none());
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
    ctx.write_cache("sentinel", "persisted").await?;
    ctx.recreate_container().await?;
    assert_eq!(ctx.read_cache("sentinel").await?, "persisted");
    ctx.cleanup().await
}
