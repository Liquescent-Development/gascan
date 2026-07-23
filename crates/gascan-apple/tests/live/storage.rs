use std::collections::BTreeMap;

use camino::Utf8Path;
use gascan_apple::{AppleBackend, CommandRunner, CommandSpec, ProcessRunner};
use gascan_core::{
    fake_runtime::FakeRuntime,
    manifest::Manifest,
    policy::PolicyCompiler,
    runtime::{
        CreateOutcome, CreateRequest, NetworkIsolation, RemoveRequest, RuntimeBackend,
        RuntimeCapabilities, RuntimeVersion,
    },
    sandbox::SandboxSpec,
};
use serde_json::Value;

use super::common::{LiveContext, TestError, exact_workspace_bind};

const GIB: u64 = 1024_u64.pow(3);
const CAPACITY_ROUNDING_BYTES: u64 = 64 * 1024_u64.pow(2);

async fn create_with_partial_cleanup<B: RuntimeBackend>(
    backend: &B,
    request: CreateRequest,
) -> Result<CreateOutcome, TestError> {
    match backend.create(request).await {
        Ok(created) => Ok(created),
        Err(failure) => {
            if !failure.created().is_empty() {
                backend
                    .remove(RemoveRequest::from_resources(failure.created().to_vec())?)
                    .await?;
            }
            Err(failure.into())
        }
    }
}

#[tokio::test]
async fn create_failure_cleanup_removes_exact_partial_resources() -> Result<(), TestError> {
    let root = tempfile::tempdir()?;
    let path = Utf8Path::from_path(root.path()).ok_or("non-UTF-8 test root")?;
    std::fs::write(
        path.join("gascan.toml"),
        "version = 1\nnetwork = 'networked'\n",
    )?;
    let spec = SandboxSpec::from_root("storage-partial-cleanup", path, Manifest::load(path)?)?;
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
    )?;
    let id = request.id().clone();
    let expected_names = [
        request.network().managed_name().ok_or("managed network")?,
        request.volumes()[0].name.as_str(),
        request.volumes()[1].name.as_str(),
        request.volumes()[2].name.as_str(),
        request.id().as_str(),
    ];
    for mutations in 1..=expected_names.len() {
        let runtime = FakeRuntime::default();
        runtime.fail_create_after_mutations(mutations).await;

        assert!(
            create_with_partial_cleanup(&runtime, request.clone())
                .await
                .is_err()
        );
        assert!(runtime.list_resources().await?.is_empty());
        let remove = runtime
            .calls()
            .await
            .into_iter()
            .find_map(|call| match call {
                gascan_core::runtime::RuntimeCall::Remove(request) => Some(request),
                _ => None,
            })
            .ok_or("remove call")?;
        assert_eq!(remove.resources().len(), mutations);
        assert_eq!(
            remove
                .resources()
                .iter()
                .map(|resource| resource.name())
                .collect::<Vec<_>>(),
            expected_names[..mutations]
        );
        assert!(remove.resources().iter().all(|resource| {
            resource.ownership() == gascan_core::runtime::ResourceOwnership::GasCanOwned
                && resource.sandbox_id() == Some(&id)
        }));
    }
    Ok(())
}

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

#[test]
fn validates_exact_managed_volume_labels_mounts_and_capacity_bounds() {
    let id = "live-storage-000000000000";
    let inventory = serde_json::json!([
        {"id":format!("gascan-mise-{id}"),"configuration":{"name":format!("gascan-mise-{id}"),"labels":{
            "dev.gascan.managed-by":"gascan","dev.gascan.sandbox-id":id}}},
        {"id":format!("gascan-cache-{id}"),"configuration":{"name":format!("gascan-cache-{id}"),"labels":{
            "dev.gascan.managed-by":"gascan","dev.gascan.sandbox-id":id}}},
        {"id":format!("gascan-config-{id}"),"configuration":{"name":format!("gascan-config-{id}"),"labels":{
            "dev.gascan.managed-by":"gascan","dev.gascan.sandbox-id":id}}}
    ]);
    let inspect = serde_json::json!([{"configuration":{"mounts":[
        {"type":{"volume":{"name":format!("gascan-mise-{id}")}},"source":"/host/volume.img","destination":"/home/workspace/.local/share/mise","options":[]},
        {"type":{"volume":{"name":format!("gascan-cache-{id}")}},"source":"/host/volume.img","destination":"/home/workspace/.cache","options":[]},
        {"type":{"volume":{"name":format!("gascan-config-{id}")}},"source":"/host/volume.img","destination":"/home/workspace/.config/gascan","options":[]}
    ]}}]);
    let capacities = BTreeMap::from([
        ("/home/workspace/.cache".to_owned(), 12 * GIB),
        (
            "/home/workspace/.config/gascan".to_owned(),
            2 * GIB + CAPACITY_ROUNDING_BYTES,
        ),
        ("/home/workspace/.local/share/mise".to_owned(), 11 * GIB),
    ]);

    assert_managed_storage(
        &inventory,
        &inspect,
        id,
        &[
            ("mise", "/home/workspace/.local/share/mise", 11 * GIB),
            ("cache", "/home/workspace/.cache", 12 * GIB),
            ("config", "/home/workspace/.config/gascan", 2 * GIB),
        ],
        &capacities,
    )
    .unwrap();

    let too_large = BTreeMap::from([
        ("/home/workspace/.cache".to_owned(), 12 * GIB),
        (
            "/home/workspace/.config/gascan".to_owned(),
            2 * GIB + CAPACITY_ROUNDING_BYTES + 1,
        ),
        ("/home/workspace/.local/share/mise".to_owned(), 11 * GIB),
    ]);
    assert!(
        assert_managed_storage(
            &inventory,
            &inspect,
            id,
            &[
                ("mise", "/home/workspace/.local/share/mise", 11 * GIB),
                ("cache", "/home/workspace/.cache", 12 * GIB),
                ("config", "/home/workspace/.config/gascan", 2 * GIB),
            ],
            &too_large,
        )
        .is_err()
    );
}

fn parse_guest_block_capacities(output: &[u8]) -> Result<BTreeMap<String, u64>, TestError> {
    String::from_utf8(output.to_vec())?
        .lines()
        .map(|line| {
            let mut fields = line.split_whitespace();
            let capacity = fields
                .next()
                .ok_or("guest block-capacity row is missing its size")?
                .parse::<u64>()?;
            let target = fields
                .next()
                .ok_or("guest block-capacity row is missing its mount target")?;
            if fields.next().is_some() {
                return Err("guest block-capacity row has unexpected fields".into());
            }
            Ok((target.to_owned(), capacity))
        })
        .collect()
}

fn assert_managed_storage(
    inventory: &Value,
    inspect: &Value,
    id: &str,
    expected: &[(&str, &str, u64)],
    capacities: &BTreeMap<String, u64>,
) -> Result<(), TestError> {
    let volume_records = inventory
        .as_array()
        .ok_or("volume inventory must be an array")?;
    let container = inspect
        .as_array()
        .and_then(|records| (records.len() == 1).then(|| &records[0]))
        .ok_or("container inspect must contain exactly one record")?;
    let mounts = container["configuration"]["mounts"]
        .as_array()
        .ok_or("container inspect has no mount array")?;
    let volume_mount_count = mounts
        .iter()
        .filter(|mount| mount["type"]["volume"].is_object())
        .count();
    if volume_mount_count != expected.len() {
        return Err(format!(
            "expected {} managed volume mounts, found {volume_mount_count}",
            expected.len()
        )
        .into());
    }

    if capacities.len() != expected.len() {
        return Err(format!("unexpected capacity targets: {capacities:?}").into());
    }
    for (kind, target, requested) in expected {
        let name = format!("gascan-{kind}-{id}");
        let matching_records = volume_records
            .iter()
            .filter(|record| {
                record["id"].as_str() == Some(name.as_str())
                    && record["configuration"]["name"].as_str() == Some(name.as_str())
            })
            .collect::<Vec<_>>();
        let [record] = matching_records.as_slice() else {
            return Err(format!("expected one exact volume inventory record for {name}").into());
        };
        let labels = &record["configuration"]["labels"];
        if labels["dev.gascan.managed-by"].as_str() != Some("gascan")
            || labels["dev.gascan.sandbox-id"].as_str() != Some(id)
        {
            return Err(format!("ownership labels mismatch for {name}: {labels:?}").into());
        }

        let matching_mounts = mounts
            .iter()
            .filter(|mount| {
                mount["type"]["volume"].is_object()
                    && mount["type"]["volume"]["name"].as_str() == Some(name.as_str())
                    && mount["destination"].as_str() == Some(*target)
            })
            .count();
        if matching_mounts != 1 {
            return Err(format!(
                "expected one exact {name} mount at {target}, found {matching_mounts}: {mounts:?}"
            )
            .into());
        }

        let observed = capacities
            .get(*target)
            .ok_or_else(|| format!("df omitted managed mount target {target}"))?;
        let maximum = requested
            .checked_add(CAPACITY_ROUNDING_BYTES)
            .ok_or("capacity assertion overflow")?;
        if !(*requested..=maximum).contains(observed) {
            return Err(format!(
                "{target} capacity {observed} is outside requested range {requested}..={maximum}"
            )
            .into());
        }
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires Apple silicon macOS 26+ with container service and locked workspace image"]
async fn independently_sized_managed_volumes_are_exact_and_cleanup() -> Result<(), TestError> {
    let root = tempfile::tempdir()?;
    let path = Utf8Path::from_path(root.path()).ok_or("non-UTF-8 live test root")?;
    std::fs::write(
        path.join("gascan.toml"),
        "version = 1\nnetwork = 'offline'\n\
         [storage]\ntools = '11GiB'\ncache = '12GiB'\nconfig = '2GiB'\n",
    )?;
    let spec = SandboxSpec::from_root("live-storage-capacity", path, Manifest::load(path)?)?;
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
    )?;
    let id = request.id().clone();
    let runner = ProcessRunner;
    let backend = AppleBackend::new(runner);
    let created = create_with_partial_cleanup(&backend, request).await?;

    let result = async {
        backend.start(&id).await?;
        let inventory = runner
            .run(CommandSpec::new(
                "container",
                ["volume", "list", "--format", "json"],
            ))
            .await?;
        let inspect = runner
            .run(CommandSpec::new("container", ["inspect", id.as_str()]))
            .await?;
        let df = runner
            .run(CommandSpec::new(
                "container",
                [
                    "exec",
                    id.as_str(),
                    "sh",
                    "-c",
                    "set -eu; for target in /home/workspace/.local/share/mise /home/workspace/.cache /home/workspace/.config/gascan; do source=$(df --output=source \"$target\" | tail -n 1 | tr -d ' '); device=${source##*/}; sectors=$(cat \"/sys/class/block/$device/size\"); printf '%s %s\\n' \"$((sectors * 512))\" \"$target\"; done",
                ],
            ))
            .await?;
        let capacities = parse_guest_block_capacities(&df.stdout)?;
        assert_managed_storage(
            &serde_json::from_slice(&inventory.stdout)?,
            &serde_json::from_slice(&inspect.stdout)?,
            id.as_str(),
            &[
                ("mise", "/home/workspace/.local/share/mise", 11 * GIB),
                ("cache", "/home/workspace/.cache", 12 * GIB),
                ("config", "/home/workspace/.config/gascan", 2 * GIB),
            ],
            &capacities,
        )
    }
    .await;

    let stop = backend.stop(&id).await;
    let remove = backend
        .remove(RemoveRequest::from_resources(created.created().to_vec())?)
        .await;
    if let Err(error) = stop {
        return Err(
            format!("live assertion result: {result:?}; cleanup stop failed: {error}").into(),
        );
    }
    remove?;
    assert!(
        !backend
            .list_resources()
            .await?
            .iter()
            .any(|resource| resource.sandbox_id() == Some(&id)),
        "live test-owned resources remain after cleanup"
    );
    result
}
