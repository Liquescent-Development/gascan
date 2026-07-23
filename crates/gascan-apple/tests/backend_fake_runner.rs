use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use camino::Utf8Path;
use gascan_apple::{AppleBackend, CommandOutput, CommandRunner, CommandSpec};
use gascan_core::{
    manifest::Manifest,
    policy::PolicyCompiler,
    runtime::{
        ContainerState, CreateRequest, ExecInput, ExecOutput, ExecRequest, NetworkIsolation,
        RemoveRequest, ResourceKind, ResourceOwnership, RuntimeBackend, RuntimeCapabilities,
        RuntimeError, RuntimeVersion,
    },
    sandbox::SandboxSpec,
};
use serde_json::json;

mod support;

#[derive(Clone, Default)]
struct StatefulAppleRunner(Arc<Mutex<State>>);

#[derive(Default)]
struct State {
    containers: BTreeMap<String, (String, String)>,
    volumes: BTreeMap<String, (String, String)>,
    networks: BTreeMap<String, (String, String)>,
    commands: Vec<CommandSpec>,
    faulted_inventory_commands: Vec<Vec<String>>,
    fail_run_after_insert: bool,
    fail_volume_after_insert: bool,
    fail_network_after_insert: bool,
    container_list_faults: VecDeque<InventoryFault>,
    volume_list_faults: VecDeque<InventoryFault>,
    network_list_faults: VecDeque<InventoryFault>,
    raw_network_list_record: Option<serde_json::Value>,
    after_successful_network_create_network_list_fault: Option<InventoryFault>,
    after_successful_volume_create_volume_list_fault: Option<InventoryFault>,
    after_successful_container_create_container_list_fault: Option<InventoryFault>,
}

#[derive(Clone, Copy)]
enum InventoryFault {
    InvalidJson,
    CommandIo,
    Absent,
    Foreign,
    Mismatched,
}

#[derive(Clone, Copy)]
enum InventoryTarget {
    Container,
    Volume,
    Network,
}

#[async_trait]
impl CommandRunner for StatefulAppleRunner {
    async fn run(&self, spec: CommandSpec) -> Result<CommandOutput, RuntimeError> {
        let mut state = self.0.lock().unwrap();
        state.commands.push(spec.clone());
        let args: Vec<&str> = spec.args.iter().map(String::as_str).collect();
        let value = match args.as_slice() {
            ["system", "version", "--format", "json"] => {
                json!([{"appName":"container","buildType":"release","commit":"5973b9cc626a3e7a499bb316a958237ebe14e2ed","version":"1.1.0"}])
            }
            ["image", "pull", _] => json!(null),
            ["list", "--all", "--format", "json"] => {
                if let Some(fault) = state.container_list_faults.pop_front() {
                    state.faulted_inventory_commands.push(spec.args.clone());
                    return fault_output(fault, &state.containers, InventoryTarget::Container);
                }
                json!(state.containers.iter().map(|(id, (sandbox, status))| json!({
                "configuration":{"id":id,"labels":{"dev.gascan.managed-by":"gascan","dev.gascan.sandbox-id":sandbox}},"status":{"state":status}
            })).collect::<Vec<_>>())
            }
            ["volume", "list", "--format", "json"] => {
                if let Some(fault) = state.volume_list_faults.pop_front() {
                    state.faulted_inventory_commands.push(spec.args.clone());
                    return fault_output(fault, &state.volumes, InventoryTarget::Volume);
                }
                json!(state.volumes.iter().map(|(name, (sandbox, manager))| json!({
                "id":name,"configuration":{"name":name,"labels":{"dev.gascan.managed-by":manager,"dev.gascan.sandbox-id":sandbox}}
            })).collect::<Vec<_>>())
            }
            ["network", "list", "--format", "json"] => {
                if let Some(fault) = state.network_list_faults.pop_front() {
                    state.faulted_inventory_commands.push(spec.args.clone());
                    return fault_output(fault, &state.networks, InventoryTarget::Network);
                }
                let mut records = state
                    .networks
                    .iter()
                    .map(|(name, (sandbox, manager))| {
                        json!({
                            "id":name,
                            "configuration":{
                                "name":name,
                                "labels":{
                                    "dev.gascan.managed-by":manager,
                                    "dev.gascan.sandbox-id":sandbox
                                }
                            },
                            "status":{}
                        })
                    })
                    .collect::<Vec<_>>();
                if let Some(record) = state.raw_network_list_record.clone() {
                    records.push(record);
                }
                json!(records)
            }
            ["inspect", id] => match state.containers.get(*id) {
                Some((sandbox, status)) => {
                    json!([{"configuration":{"id":id,"labels":{"dev.gascan.managed-by":"gascan","dev.gascan.sandbox-id":sandbox}},"status":{"state":status}}])
                }
                None => {
                    return Err(RuntimeError::CommandFailed {
                        operation: "container".into(),
                        exit_code: Some(1),
                        stderr: "not found".into(),
                    });
                }
            },
            [
                "network",
                "create",
                "--label",
                manager,
                "--label",
                sandbox,
                name,
            ] => {
                if state.networks.contains_key(*name) {
                    return conflict(name);
                }
                state.networks.insert(
                    (*name).into(),
                    (
                        sandbox.split_once('=').unwrap().1.into(),
                        manager.split_once('=').unwrap().1.into(),
                    ),
                );
                if let Some(fault) = state
                    .after_successful_network_create_network_list_fault
                    .take()
                {
                    let repeats = if matches!(
                        fault,
                        InventoryFault::Absent
                            | InventoryFault::Foreign
                            | InventoryFault::Mismatched
                    ) {
                        2
                    } else {
                        1
                    };
                    for _ in 0..repeats {
                        state.network_list_faults.push_back(fault);
                    }
                }
                if state.fail_network_after_insert {
                    return Err(RuntimeError::CommandIo {
                        operation: "container".into(),
                        message: "daemon disconnected".into(),
                    });
                }
                json!(null)
            }
            [
                "volume",
                "create",
                "--label",
                manager,
                "--label",
                sandbox,
                "-s",
                "104857600",
                name,
            ] => {
                if state.volumes.contains_key(*name) {
                    return conflict(name);
                }
                state.volumes.insert(
                    (*name).into(),
                    (
                        sandbox.split_once('=').unwrap().1.into(),
                        manager.split_once('=').unwrap().1.into(),
                    ),
                );
                if let Some(fault) = state
                    .after_successful_volume_create_volume_list_fault
                    .take()
                {
                    let repeats = if matches!(
                        fault,
                        InventoryFault::Absent
                            | InventoryFault::Foreign
                            | InventoryFault::Mismatched
                    ) {
                        2
                    } else {
                        1
                    };
                    for _ in 0..repeats {
                        state.volume_list_faults.push_back(fault);
                    }
                }
                if state.fail_volume_after_insert {
                    return Err(RuntimeError::CommandFailed {
                        operation: "container".into(),
                        exit_code: Some(1),
                        stderr: "already exists human diagnostic".into(),
                    });
                }
                json!(null)
            }
            args if args.first() == Some(&"run") => {
                let id = args[args.iter().position(|arg| *arg == "--name").unwrap() + 1];
                if state.containers.contains_key(id) {
                    return conflict(id);
                }
                state
                    .containers
                    .insert(id.into(), (id.into(), "stopped".into()));
                if let Some(fault) = state
                    .after_successful_container_create_container_list_fault
                    .take()
                {
                    let repeats = if matches!(
                        fault,
                        InventoryFault::Absent
                            | InventoryFault::Foreign
                            | InventoryFault::Mismatched
                    ) {
                        2
                    } else {
                        1
                    };
                    for _ in 0..repeats {
                        state.container_list_faults.push_back(fault);
                    }
                }
                if state.fail_run_after_insert {
                    return Err(RuntimeError::CommandIo {
                        operation: "container".into(),
                        message: "daemon disconnected".into(),
                    });
                }
                json!(null)
            }
            ["start", id] => {
                state.containers.get_mut(*id).unwrap().1 = "running".into();
                json!(null)
            }
            ["stop", "--time", "5", id] => {
                state.containers.get_mut(*id).unwrap().1 = "stopped".into();
                json!(null)
            }
            ["delete", id] => {
                state.containers.remove(*id);
                json!(null)
            }
            ["volume", "delete", name] => {
                state.volumes.remove(*name);
                json!(null)
            }
            ["network", "delete", name] => {
                state.networks.remove(*name);
                json!(null)
            }
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

fn fault_output<T>(
    fault: InventoryFault,
    resources: &BTreeMap<String, T>,
    target: InventoryTarget,
) -> Result<CommandOutput, RuntimeError> {
    let stdout = match fault {
        InventoryFault::InvalidJson => b"{".to_vec(),
        InventoryFault::CommandIo => {
            return Err(RuntimeError::CommandIo {
                operation: match target {
                    InventoryTarget::Container => "container list",
                    InventoryTarget::Volume => "container volume list",
                    InventoryTarget::Network => "container network list",
                }
                .into(),
                message: "injected inventory transport failure".into(),
            });
        }
        InventoryFault::Absent => b"[]".to_vec(),
        InventoryFault::Foreign if matches!(target, InventoryTarget::Container) => serde_json::to_vec(
            &resources
                .keys()
                .map(|id| {
                    json!({
                        "configuration":{"id":id,"labels":{}},"status":{"state":"stopped"}
                    })
                })
                .collect::<Vec<_>>(),
        )
        .unwrap(),
        InventoryFault::Foreign => serde_json::to_vec(
            &resources
                .keys()
                .map(|name| {
                    json!({
                        "id":name,"configuration":{"name":name,"labels":{}}
                    })
                })
                .collect::<Vec<_>>(),
        )
        .unwrap(),
        InventoryFault::Mismatched if matches!(target, InventoryTarget::Container) => serde_json::to_vec(
            &resources.keys().map(|id| json!({
                "configuration":{"id":id,"labels":{"dev.gascan.managed-by":"gascan","dev.gascan.sandbox-id":"gascan-mismatch-000000000000"}},
                "status":{"state":"stopped"}
            })).collect::<Vec<_>>(),
        ).unwrap(),
        InventoryFault::Mismatched => serde_json::to_vec(
            &resources.keys().map(|name| json!({
                "id":name,"configuration":{"name":name,"labels":{"dev.gascan.sandbox-id":"gascan-mismatch-000000000000"}}
            })).collect::<Vec<_>>(),
        ).unwrap(),
    };
    Ok(CommandOutput {
        status: 0,
        stdout,
        stderr: vec![],
    })
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

use support::fake_helper as fake_attach;

fn assert_only_faulted_command(runner: &StatefulAppleRunner, expected: &[&str]) {
    let state = runner.0.lock().unwrap();
    assert_eq!(state.faulted_inventory_commands.len(), 1);
    assert_eq!(state.faulted_inventory_commands[0], expected);
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
    assert_eq!(created.created().len(), 5);
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
    assert_eq!(listed.len(), 5);
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
async fn inventory_reports_owned_foreign_and_mismatched_networks() {
    let runner = StatefulAppleRunner::default();
    {
        let mut state = runner.0.lock().unwrap();
        state.networks.insert(
            "gascan-network-owned".into(),
            ("gascan-network-owned-000000000000".into(), "gascan".into()),
        );
        state.networks.insert(
            "foreign-network".into(),
            ("gascan-network-foreign-000000000000".into(), "other".into()),
        );
        state.raw_network_list_record = Some(json!({
            "id":"gascan-network-mismatched",
            "configuration":{
                "name":"gascan-network-mismatched",
                "labels":{"dev.gascan.sandbox-id":"gascan-network-mismatched-000000000000"}
            }
        }));
    }
    let resources = AppleBackend::new(runner).list_resources().await.unwrap();
    let ownership = |name: &str| {
        resources
            .iter()
            .find(|resource| resource.kind() == ResourceKind::Network && resource.name() == name)
            .unwrap()
            .ownership()
    };
    assert_eq!(
        ownership("gascan-network-owned"),
        ResourceOwnership::GasCanOwned
    );
    assert_eq!(ownership("foreign-network"), ResourceOwnership::Foreign);
    assert_eq!(
        ownership("gascan-network-mismatched"),
        ResourceOwnership::Mismatched
    );
}

#[tokio::test]
async fn inventory_rejects_network_id_and_name_disagreement() {
    let runner = StatefulAppleRunner::default();
    runner.0.lock().unwrap().raw_network_list_record = Some(json!({
        "id":"gascan-network-a",
        "configuration":{"name":"gascan-network-b","labels":{}}
    }));
    let error = AppleBackend::new(runner)
        .list_resources()
        .await
        .unwrap_err();
    assert_eq!(error.code(), "invalid_output");
}

#[tokio::test]
async fn network_inventory_mismatched_fault_reports_mismatched_ownership() {
    let runner = StatefulAppleRunner::default();
    {
        let mut state = runner.0.lock().unwrap();
        state.networks.insert(
            "gascan-network-mismatched-fault".into(),
            (
                "gascan-network-mismatched-fault-000000000000".into(),
                "gascan".into(),
            ),
        );
        state
            .network_list_faults
            .push_back(InventoryFault::Mismatched);
    }

    let resources = AppleBackend::new(runner).list_resources().await.unwrap();
    let network = resources
        .iter()
        .find(|resource| resource.kind() == ResourceKind::Network)
        .unwrap();
    assert_eq!(network.ownership(), ResourceOwnership::Mismatched);
}

#[tokio::test]
async fn network_inventory_command_io_names_network_list_operation() {
    let runner = StatefulAppleRunner::default();
    runner
        .0
        .lock()
        .unwrap()
        .network_list_faults
        .push_back(InventoryFault::CommandIo);

    let error = AppleBackend::new(runner)
        .list_resources()
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        RuntimeError::CommandIo { operation, .. } if operation == "container network list"
    ));
}

#[tokio::test]
async fn networked_create_labels_network_before_volumes_and_attaches_container() {
    let runner = StatefulAppleRunner::default();
    let backend = AppleBackend::new(runner.clone());
    let (_root, request) = request("apple-network-order");
    let network_name = request.network().managed_name().unwrap().to_owned();
    let sandbox_id = request.id().to_string();

    let outcome = backend.create(request).await.unwrap();

    assert_eq!(outcome.created().len(), 5);
    assert_eq!(outcome.created()[0].kind(), ResourceKind::Network);
    assert_eq!(outcome.created()[0].name(), network_name);
    assert!(
        outcome.created()[1..4]
            .iter()
            .all(|resource| resource.kind() == ResourceKind::Volume)
    );
    assert_eq!(outcome.created()[4].kind(), ResourceKind::Container);

    let state = runner.0.lock().unwrap();
    let network_index = state
        .commands
        .iter()
        .position(|command| {
            command.args.first().map(String::as_str) == Some("network")
                && command.args.get(1).map(String::as_str) == Some("create")
        })
        .unwrap();
    let volume_index = state
        .commands
        .iter()
        .position(|command| {
            command.args.first().map(String::as_str) == Some("volume")
                && command.args.get(1).map(String::as_str) == Some("create")
        })
        .unwrap();
    let run_index = state
        .commands
        .iter()
        .position(|command| command.args.first().map(String::as_str) == Some("run"))
        .unwrap();
    assert!(network_index < volume_index && volume_index < run_index);
    let network_args = &state.commands[network_index].args;
    assert!(
        network_args
            .windows(2)
            .any(|args| args == ["--label", "dev.gascan.managed-by=gascan"])
    );
    assert!(network_args.windows(2).any(|args| {
        args[0] == "--label" && args[1] == format!("dev.gascan.sandbox-id={sandbox_id}")
    }));
    let run_args = &state.commands[run_index].args;
    assert!(
        run_args
            .windows(2)
            .any(|args| args == ["--network", network_name.as_str()])
    );
}

#[tokio::test]
async fn transient_network_create_io_reconciles_exact_owned_side_effect() {
    let runner = StatefulAppleRunner::default();
    runner.0.lock().unwrap().fail_network_after_insert = true;
    let backend = AppleBackend::new(runner);
    let (_root, request) = request("apple-network-io");
    let failure = backend.create(request).await.unwrap_err();
    assert_eq!(failure.code(), "command_io");
    assert_eq!(failure.created().len(), 1);
    assert_eq!(failure.created()[0].kind(), ResourceKind::Network);
}

#[tokio::test]
async fn foreign_network_observation_is_never_returned_as_create_evidence() {
    let runner = StatefulAppleRunner::default();
    runner
        .0
        .lock()
        .unwrap()
        .after_successful_network_create_network_list_fault = Some(InventoryFault::Foreign);
    let backend = AppleBackend::new(runner);
    let (_root, request) = request("apple-network-foreign");
    let failure = backend.create(request).await.unwrap_err();
    assert_eq!(failure.code(), "ownership_mismatch");
    assert!(failure.created().is_empty());
}

#[tokio::test]
async fn remove_deletes_container_then_volumes_then_network() {
    let runner = StatefulAppleRunner::default();
    let backend = AppleBackend::new(runner.clone());
    let (_root, request) = request("apple-network-remove");
    let outcome = backend.create(request).await.unwrap();
    backend
        .remove(RemoveRequest::from_resources(outcome.created().to_vec()).unwrap())
        .await
        .unwrap();

    let state = runner.0.lock().unwrap();
    let deletion_kinds = state
        .commands
        .iter()
        .filter_map(|command| match command.args.as_slice() {
            [operation, _] if operation == "delete" => Some(ResourceKind::Container),
            [resource, operation, _] if resource == "volume" && operation == "delete" => {
                Some(ResourceKind::Volume)
            }
            [resource, operation, _] if resource == "network" && operation == "delete" => {
                Some(ResourceKind::Network)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        deletion_kinds,
        [
            ResourceKind::Container,
            ResourceKind::Volume,
            ResourceKind::Volume,
            ResourceKind::Volume,
            ResourceKind::Network
        ]
    );
    assert!(state.containers.is_empty());
    assert!(state.volumes.is_empty());
    assert!(state.networks.is_empty());
}

#[tokio::test]
async fn remove_refuses_a_network_changed_after_observation() {
    let runner = StatefulAppleRunner::default();
    let backend = AppleBackend::new(runner.clone());
    let (_root, request) = request("apple-network-changed");
    let outcome = backend.create(request).await.unwrap();
    let network = outcome
        .created()
        .iter()
        .find(|resource| resource.kind() == ResourceKind::Network)
        .unwrap()
        .clone();
    runner
        .0
        .lock()
        .unwrap()
        .networks
        .get_mut(network.name())
        .unwrap()
        .1 = "foreign".into();

    let error = backend
        .remove(RemoveRequest::from_resources(vec![network.clone()]).unwrap())
        .await
        .unwrap_err();

    assert_eq!(error.code(), "ownership_mismatch");
    let state = runner.0.lock().unwrap();
    assert!(state.networks.contains_key(network.name()));
    assert!(
        !state
            .commands
            .iter()
            .any(|command| { command.args.as_slice() == ["network", "delete", network.name()] })
    );
}

#[tokio::test]
async fn exec_bridge_accepts_input_while_output_is_pending() {
    let backend = AppleBackend::with_attach(StatefulAppleRunner::default(), fake_attach());
    let (_root, create) = request("apple-attach-bridge");
    let mut session = backend
        .exec(ExecRequest {
            id: create.id().clone(),
            argv: vec!["guest".to_owned()],
            stdin: Vec::new(),
            environment: BTreeMap::new(),
            tty: false,
        })
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        session.send(ExecInput::Close),
    )
    .await
    .expect("close must not deadlock behind a pending output read")
    .unwrap();

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let exit = loop {
        match tokio::time::timeout(std::time::Duration::from_secs(2), session.next())
            .await
            .expect("bridge output timed out")
        {
            Some(Ok(ExecOutput::Stdout(bytes))) => stdout.extend(bytes),
            Some(Ok(ExecOutput::Stderr(bytes))) => stderr.extend(bytes),
            Some(Ok(ExecOutput::Exit { code, signal })) => break (code, signal),
            Some(Err(error)) => panic!("bridge failed: {error}"),
            None => panic!("bridge closed without an exit"),
        }
    };
    assert!(stdout.is_empty());
    assert_eq!(stderr, [254, 1]);
    assert_eq!(exit, (42, 0));
    assert!(session.next().await.is_none());
}

#[tokio::test]
async fn exec_bridge_forwards_allowed_environment_in_start_frame() {
    let backend = AppleBackend::with_attach(StatefulAppleRunner::default(), fake_attach());
    let (_root, create) = request("apple-attach-environment");
    let mut session = backend
        .exec(ExecRequest {
            id: create.id().clone(),
            argv: vec!["guest".to_owned()],
            stdin: Vec::new(),
            environment: BTreeMap::from([
                ("LANG".to_owned(), "C.UTF-8".to_owned()),
                ("TERM".to_owned(), "xterm-256color".to_owned()),
            ]),
            tty: false,
        })
        .await
        .unwrap();

    assert_eq!(
        session.next().await.unwrap().unwrap(),
        ExecOutput::Exit {
            code: 42,
            signal: 0
        }
    );
}

#[tokio::test]
async fn exec_bridge_reports_unsupported_signal_as_terminal_error() {
    let backend = AppleBackend::with_attach(StatefulAppleRunner::default(), fake_attach());
    let (_root, create) = request("apple-attach-signal");
    let mut session = backend
        .exec(ExecRequest {
            id: create.id().clone(),
            argv: vec!["guest".to_owned()],
            stdin: Vec::new(),
            environment: BTreeMap::new(),
            tty: false,
        })
        .await
        .unwrap();
    session.send(ExecInput::Signal(2)).await.unwrap();
    let error = tokio::time::timeout(std::time::Duration::from_secs(2), session.next())
        .await
        .expect("unsupported signal must be rejected promptly")
        .expect("bridge closed without a typed error")
        .expect_err("unsupported signal unexpectedly succeeded");
    assert!(matches!(error, RuntimeError::UnsupportedCapability { .. }));
    assert!(session.next().await.is_none());
}

#[tokio::test]
async fn exec_bridge_turns_premature_helper_eof_into_one_terminal_error() {
    let backend = AppleBackend::with_attach(StatefulAppleRunner::default(), fake_attach());
    let (_root, create) = request("no-terminal");
    let mut session = backend
        .exec(ExecRequest {
            id: create.id().clone(),
            argv: vec!["guest".to_owned()],
            stdin: Vec::new(),
            environment: BTreeMap::new(),
            tty: false,
        })
        .await
        .unwrap();
    let error = tokio::time::timeout(std::time::Duration::from_secs(2), session.next())
        .await
        .expect("premature helper EOF must terminate promptly")
        .expect("bridge closed without a typed terminal error")
        .expect_err("premature helper EOF unexpectedly succeeded");
    assert!(matches!(error, RuntimeError::InvalidOutput { .. }));
    assert!(session.next().await.is_none());
}

#[tokio::test]
async fn exec_session_cancel_aborts_helper_input_ack_and_coordinator() {
    let backend = AppleBackend::with_attach(StatefulAppleRunner::default(), fake_attach());
    let (_root, create) = request("block-input");
    let mut session = backend
        .exec(ExecRequest {
            id: create.id().clone(),
            argv: vec!["guest".to_owned()],
            stdin: Vec::new(),
            environment: BTreeMap::new(),
            tty: false,
        })
        .await
        .unwrap();
    session
        .send(ExecInput::Stdin(vec![7; 8 * 1024 * 1024]))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    session.cancel();
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(2), session.next())
            .await
            .expect("cancelled Apple coordinator did not close promptly")
            .is_none()
    );
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
    let backend = AppleBackend::new(runner.clone());
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
async fn transient_io_after_mutation_reconciles_exact_owned_side_effect() {
    let runner = StatefulAppleRunner::default();
    runner.0.lock().unwrap().fail_run_after_insert = true;
    let backend = AppleBackend::new(runner.clone());
    let (_root, request) = request("apple-partial");
    let failure = backend.create(request).await.unwrap_err();
    assert_eq!(failure.code(), "command_io");
    assert_eq!(failure.created().len(), 5);
}

#[tokio::test]
async fn command_failed_human_diagnostic_is_preserved_and_collision_is_not_claimed() {
    let runner = StatefulAppleRunner::default();
    runner.0.lock().unwrap().fail_volume_after_insert = true;
    let backend = AppleBackend::new(runner);
    let (_root, request) = request("apple-command-failed");
    let failure = backend.create(request).await.unwrap_err();
    assert_eq!(failure.code(), "command_failed");
    assert_eq!(failure.created().len(), 1);
    assert_eq!(failure.created()[0].kind(), ResourceKind::Network);
    assert!(matches!(
        failure.source(),
        RuntimeError::CommandFailed {
            exit_code: Some(1),
            ..
        }
    ));
}

#[tokio::test]
async fn successful_volume_create_then_inventory_parse_failure_reconciles_created_evidence() {
    let runner = StatefulAppleRunner::default();
    runner
        .0
        .lock()
        .unwrap()
        .after_successful_volume_create_volume_list_fault = Some(InventoryFault::InvalidJson);
    let backend = AppleBackend::new(runner.clone());
    let (_root, request) = request("apple-volume-parse");
    let failure = backend.create(request).await.unwrap_err();
    assert_eq!(failure.code(), "invalid_output");
    assert_eq!(failure.created().len(), 2);
    assert_only_faulted_command(&runner, &["volume", "list", "--format", "json"]);
}

#[tokio::test]
async fn successful_container_create_then_inventory_parse_failure_reconciles_all_evidence() {
    let runner = StatefulAppleRunner::default();
    runner
        .0
        .lock()
        .unwrap()
        .after_successful_container_create_container_list_fault = Some(InventoryFault::InvalidJson);
    let backend = AppleBackend::new(runner.clone());
    let (_root, request) = request("apple-container-parse");
    let failure = backend.create(request).await.unwrap_err();
    assert_eq!(failure.code(), "invalid_output");
    assert_eq!(failure.created().len(), 5);
    assert_only_faulted_command(&runner, &["list", "--all", "--format", "json"]);
}

#[tokio::test]
async fn successful_volume_create_then_persistent_absence_never_claims_created_evidence() {
    let runner = StatefulAppleRunner::default();
    runner
        .0
        .lock()
        .unwrap()
        .after_successful_volume_create_volume_list_fault = Some(InventoryFault::Absent);
    let backend = AppleBackend::new(runner);
    let (_root, request) = request("apple-volume-absent");
    let failure = backend.create(request).await.unwrap_err();
    assert_eq!(failure.code(), "ownership_mismatch");
    assert_eq!(failure.created().len(), 1);
    assert_eq!(failure.created()[0].kind(), ResourceKind::Network);
}

#[tokio::test]
async fn successful_volume_create_then_ownership_verification_never_claims_foreign_evidence() {
    let runner = StatefulAppleRunner::default();
    runner
        .0
        .lock()
        .unwrap()
        .after_successful_volume_create_volume_list_fault = Some(InventoryFault::Foreign);
    let backend = AppleBackend::new(runner);
    let (_root, request) = request("apple-volume-foreign");
    let failure = backend.create(request).await.unwrap_err();
    assert_eq!(failure.code(), "ownership_mismatch");
    assert_eq!(failure.created().len(), 1);
    assert_eq!(failure.created()[0].kind(), ResourceKind::Network);
}

#[tokio::test]
async fn successful_container_create_then_persistent_absence_retains_only_prior_volumes() {
    let runner = StatefulAppleRunner::default();
    runner
        .0
        .lock()
        .unwrap()
        .after_successful_container_create_container_list_fault = Some(InventoryFault::Absent);
    let backend = AppleBackend::new(runner);
    let (_root, request) = request("apple-container-absent");
    let failure = backend.create(request).await.unwrap_err();
    assert_eq!(failure.code(), "ownership_mismatch");
    assert_eq!(failure.created().len(), 4);
}

#[tokio::test]
async fn successful_container_create_then_foreign_observation_retains_only_prior_volumes() {
    let runner = StatefulAppleRunner::default();
    runner
        .0
        .lock()
        .unwrap()
        .after_successful_container_create_container_list_fault = Some(InventoryFault::Foreign);
    let backend = AppleBackend::new(runner);
    let (_root, request) = request("apple-container-foreign");
    let failure = backend.create(request).await.unwrap_err();
    assert_eq!(failure.code(), "ownership_mismatch");
    assert_eq!(failure.created().len(), 4);
    assert!(
        failure
            .created()
            .iter()
            .filter(|resource| resource.kind() == ResourceKind::Volume)
            .count()
            == 3
    );
}

#[tokio::test]
async fn successful_container_create_then_mismatched_observation_retains_only_prior_volumes() {
    let runner = StatefulAppleRunner::default();
    runner
        .0
        .lock()
        .unwrap()
        .after_successful_container_create_container_list_fault = Some(InventoryFault::Mismatched);
    let backend = AppleBackend::new(runner);
    let (_root, request) = request("apple-container-mismatched");
    let failure = backend.create(request).await.unwrap_err();
    assert_eq!(failure.code(), "ownership_mismatch");
    assert_eq!(failure.created().len(), 4);
}

#[tokio::test]
async fn successful_volume_create_then_volume_list_command_error_reconciles_created_evidence() {
    let runner = StatefulAppleRunner::default();
    runner
        .0
        .lock()
        .unwrap()
        .after_successful_volume_create_volume_list_fault = Some(InventoryFault::CommandIo);
    let backend = AppleBackend::new(runner);
    let (_root, request) = request("apple-volume-list-io");
    let failure = backend.create(request).await.unwrap_err();
    assert_eq!(failure.code(), "command_io");
    assert_eq!(failure.created().len(), 2);
}

#[tokio::test]
async fn successful_container_create_then_container_list_command_error_reconciles_all_evidence() {
    let runner = StatefulAppleRunner::default();
    runner
        .0
        .lock()
        .unwrap()
        .after_successful_container_create_container_list_fault = Some(InventoryFault::CommandIo);
    let backend = AppleBackend::new(runner);
    let (_root, request) = request("apple-container-list-io");
    let failure = backend.create(request).await.unwrap_err();
    assert_eq!(failure.code(), "command_io");
    assert_eq!(failure.created().len(), 5);
}
