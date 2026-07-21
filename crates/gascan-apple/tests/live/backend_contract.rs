use std::time::{SystemTime, UNIX_EPOCH};

use camino::Utf8Path;
use gascan_apple::{AppleBackend, ProcessRunner};
use gascan_core::{
    manifest::Manifest,
    policy::PolicyCompiler,
    runtime::{
        NetworkIsolation, RemoveRequest, RuntimeBackend, RuntimeCapabilities, RuntimeVersion,
    },
    sandbox::SandboxSpec,
};

#[tokio::test]
#[ignore = "requires Apple silicon macOS 26+ with container service and locked workspace image"]
async fn backend_contract() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let name = format!("gascan-live-backend-{}-{nonce}", std::process::id());
    let root = tempfile::tempdir().unwrap();
    let path = Utf8Path::from_path(root.path()).unwrap();
    std::fs::write(
        path.join("gascan.toml"),
        "version = 1\nnetwork = 'offline'\n",
    )
    .unwrap();
    let spec = SandboxSpec::from_root(&name, path, Manifest::load(path).unwrap()).unwrap();
    let request = PolicyCompiler::compile(
        spec,
        &RuntimeCapabilities {
            version: RuntimeVersion::new(1, 1, 0),
            bind_mounts: true,
            named_volumes: true,
            tty: true,
            signals: true,
            loopback_publish: true,
            resource_limits: true,
            offline: NetworkIsolation::Proven,
        },
    )
    .unwrap();
    let id = request.id().clone();
    let backend = AppleBackend::new(ProcessRunner);
    assert!(backend.inspect(&id).await.unwrap().is_none());
    let created = backend.create(request).await.unwrap();
    backend.start(&id).await.unwrap();
    backend.start(&id).await.unwrap();
    backend.stop(&id).await.unwrap();
    backend.stop(&id).await.unwrap();
    backend
        .remove(RemoveRequest::from_resources(created.created().to_vec()).unwrap())
        .await
        .unwrap();
    assert!(backend.inspect(&id).await.unwrap().is_none());
    assert!(
        !backend
            .list_resources()
            .await
            .unwrap()
            .iter()
            .any(|resource| resource.name().starts_with(&name))
    );
}
