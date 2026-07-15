use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use camino::Utf8Path;
use gascan_apple::{AppleBackend, CommandOutput, CommandRunner, CommandSpec};
use gascan_core::{
    manifest::Manifest,
    policy::PolicyCompiler,
    runtime::{
        ContainerState, CreateRequest, NetworkIsolation, RemoveRequest, RuntimeBackend,
        RuntimeCapabilities, RuntimeError, RuntimeVersion,
    },
    sandbox::SandboxSpec,
};
use serde_json::json;

#[derive(Clone, Default)]
struct StatefulAppleRunner(Arc<Mutex<State>>);

#[derive(Default)]
struct State {
    containers: BTreeMap<String, (String, String)>,
    volumes: BTreeMap<String, (String, String)>,
    commands: Vec<CommandSpec>,
    fail_run_after_insert: bool,
}

#[async_trait]
impl CommandRunner for StatefulAppleRunner {
    async fn run(&self, spec: CommandSpec) -> Result<CommandOutput, RuntimeError> {
        let mut state = self.0.lock().unwrap();
        state.commands.push(spec.clone());
        let args: Vec<&str> = spec.args.iter().map(String::as_str).collect();
        let value = match args.as_slice() {
            ["system", "version", "--format", "json"] => json!([{"appName":"container","version":"1.1.0"}]),
            ["image", "pull", _] => json!(null),
            ["list", "--all", "--format", "json"] => json!(state.containers.iter().map(|(id, (sandbox, status))| json!({
                "configuration":{"id":id,"labels":{"dev.gascan.managed-by":"gascan","dev.gascan.sandbox-id":sandbox}},"status":{"state":status}
            })).collect::<Vec<_>>()),
            ["volume", "list", "--format", "json"] => json!(state.volumes.iter().map(|(name, (sandbox, manager))| json!({
                "id":name,"configuration":{"name":name,"labels":{"dev.gascan.managed-by":manager,"dev.gascan.sandbox-id":sandbox}}
            })).collect::<Vec<_>>()),
            ["inspect", id] => match state.containers.get(*id) {
                Some((sandbox, status)) => json!([{"configuration":{"id":id,"labels":{"dev.gascan.managed-by":"gascan","dev.gascan.sandbox-id":sandbox}},"status":{"state":status}}]),
                None => return Err(RuntimeError::CommandFailed { operation: "container".into(), exit_code: Some(1), stderr: "not found".into() }),
            },
            ["volume", "create", "--label", manager, "--label", sandbox, "-s", "104857600", name] => {
                if state.volumes.contains_key(*name) { return conflict(name); }
                state.volumes.insert((*name).into(), (sandbox.split_once('=').unwrap().1.into(), manager.split_once('=').unwrap().1.into()));
                json!(null)
            }
            args if args.first() == Some(&"run") => {
                let id = args[args.iter().position(|arg| *arg == "--name").unwrap() + 1];
                if state.containers.contains_key(id) { return conflict(id); }
                state.containers.insert(id.into(), (id.into(), "stopped".into()));
                if state.fail_run_after_insert {
                    return Err(RuntimeError::CommandIo { operation: "container".into(), message: "daemon disconnected".into() });
                }
                json!(null)
            }
            ["start", id] => { state.containers.get_mut(*id).unwrap().1 = "running".into(); json!(null) }
            ["stop", "--time", "5", id] => { state.containers.get_mut(*id).unwrap().1 = "stopped".into(); json!(null) }
            ["delete", id] => { state.containers.remove(*id); json!(null) }
            ["volume", "delete", name] => { state.volumes.remove(*name); json!(null) }
            ["logs", id] => json!(format!("log:{id}")),
            ["logs", "--since", _, id] => json!(format!("log:{id}")),
            other => panic!("unexpected command: {other:?}"),
        };
        let stdout = if matches!(args.first(), Some(&"logs")) {
            value.as_str().unwrap().as_bytes().to_vec()
        } else {
            serde_json::to_vec(&value).unwrap()
        };
        Ok(CommandOutput {
            status: 0,
            stdout,
            stderr: vec![],
        })
    }
}

fn conflict(resource: &str) -> Result<CommandOutput, RuntimeError> {
    Err(RuntimeError::CommandFailed {
        operation: resource.into(),
        exit_code: Some(1),
        stderr: "already exists".into(),
    })
}

fn request(name: &str) -> (tempfile::TempDir, CreateRequest) {
    let root = tempfile::tempdir().unwrap();
    let path = Utf8Path::from_path(root.path()).unwrap();
    std::fs::write(
        path.join("gascan.toml"),
        "version = 1\nnetwork = 'networked'\n",
    )
    .unwrap();
    let spec = SandboxSpec::from_root(name, path, Manifest::load(path).unwrap()).unwrap();
    let capabilities = RuntimeCapabilities {
        version: RuntimeVersion::new(1, 1, 0),
        bind_mounts: true,
        named_volumes: true,
        tty: true,
        signals: true,
        loopback_publish: true,
        resource_limits: true,
        offline: NetworkIsolation::Proven,
    };
    (root, PolicyCompiler::compile(spec, &capabilities).unwrap())
}

#[tokio::test]
async fn apple_backend_satisfies_non_attach_runtime_contract() {
    let runner = StatefulAppleRunner::default();
    let backend = AppleBackend::new(runner.clone());
    let (_root, request) = request("apple-contract");
    let id = request.id().clone();
    backend.pull(request.image()).await.unwrap();
    assert!(backend.inspect(&id).await.unwrap().is_none());
    assert_eq!(
        backend.capabilities().await.unwrap().version,
        RuntimeVersion::new(1, 1, 0)
    );
    let created = backend.create(request).await.unwrap();
    assert_eq!(created.created().len(), 4);
    assert_eq!(
        backend.inspect(&id).await.unwrap().unwrap().state,
        ContainerState::Stopped
    );
    backend.start(&id).await.unwrap();
    backend.start(&id).await.unwrap();
    assert_eq!(
        backend.inspect(&id).await.unwrap().unwrap().state,
        ContainerState::Running
    );
    assert_eq!(
        backend.logs(&id, Some(42)).await.unwrap(),
        format!("log:{id}").as_bytes()
    );
    backend.stop(&id).await.unwrap();
    backend.stop(&id).await.unwrap();
    let listed = backend.list_resources().await.unwrap();
    assert_eq!(listed.len(), 4);
    backend
        .remove(RemoveRequest::from_resources(created.created().to_vec()).unwrap())
        .await
        .unwrap();
    assert!(backend.inspect(&id).await.unwrap().is_none());
    assert!(backend.list_resources().await.unwrap().is_empty());
    let commands = &runner.0.lock().unwrap().commands;
    let unique: BTreeSet<_> = commands
        .iter()
        .map(|command| command.program.as_str())
        .collect();
    assert_eq!(unique, BTreeSet::from(["container"]));
}

#[tokio::test]
async fn remove_refuses_identity_mismatch_after_immediate_reinventory() {
    let runner = StatefulAppleRunner::default();
    let backend = AppleBackend::new(runner.clone());
    let (_root, request) = request("apple-mismatch");
    let created = backend.create(request).await.unwrap();
    let volume = created
        .created()
        .iter()
        .find(|item| item.kind() == gascan_core::runtime::ResourceKind::Volume)
        .unwrap()
        .clone();
    runner
        .0
        .lock()
        .unwrap()
        .volumes
        .get_mut(volume.name())
        .unwrap()
        .1 = "foreign".into();
    let error = backend
        .remove(RemoveRequest::from_resources(vec![volume.clone()]).unwrap())
        .await
        .unwrap_err();
    assert_eq!(error.code(), "ownership_mismatch");
    assert!(runner.0.lock().unwrap().volumes.contains_key(volume.name()));
}

#[tokio::test]
async fn remove_refuses_forged_owned_resource_without_the_opaque_observation() {
    let runner = StatefulAppleRunner::default();
    let backend = AppleBackend::new(runner);
    let (_root, request) = request("apple-forged");
    let id = request.id().clone();
    backend.create(request).await.unwrap();
    let forged = gascan_core::runtime::RuntimeResource::discovered(
        gascan_core::runtime::ResourceIdentity::new(
            gascan_core::runtime::ResourceKind::Container,
            id.to_string(),
        )
        .unwrap(),
        Some(id),
        gascan_core::runtime::ResourceOwnership::GasCanOwned,
    );
    let error = backend
        .remove(RemoveRequest::from_resources(vec![forged]).unwrap())
        .await
        .unwrap_err();
    assert_eq!(error.code(), "ownership_mismatch");
}

#[tokio::test]
async fn create_failure_after_runtime_mutation_preserves_structured_cleanup_evidence() {
    let runner = StatefulAppleRunner::default();
    runner.0.lock().unwrap().fail_run_after_insert = true;
    let backend = AppleBackend::new(runner);
    let (_root, request) = request("apple-partial");
    let failure = backend.create(request).await.unwrap_err();
    assert_eq!(failure.code(), "command_io");
    assert_eq!(failure.created().len(), 4);
}
