use std::{collections::VecDeque, sync::Mutex};

use gascan_apple::{AppleInspector, CommandOutput, CommandRunner, CommandSpec};
use gascan_core::{
    runtime::{ContainerState, ResourceOwnership, RuntimeError},
    sandbox::SandboxId,
};

struct FixtureRunner(Mutex<VecDeque<Result<CommandOutput, RuntimeError>>>);

#[async_trait::async_trait]
impl CommandRunner for FixtureRunner {
    async fn run(&self, _spec: CommandSpec) -> Result<CommandOutput, RuntimeError> {
        self.0.lock().unwrap().pop_front().unwrap()
    }
}

fn output(bytes: &[u8]) -> Result<CommandOutput, RuntimeError> {
    Ok(CommandOutput {
        status: 0,
        stdout: bytes.to_vec(),
        stderr: vec![],
    })
}

fn inspector(response: Result<CommandOutput, RuntimeError>) -> AppleInspector<FixtureRunner> {
    AppleInspector::new(FixtureRunner(Mutex::new([response].into())))
}

fn id() -> SandboxId {
    SandboxId::try_from("code-a1b2c3d4e5f6".to_owned()).unwrap()
}

#[tokio::test]
async fn inspect_maps_running_and_stopped_fixtures() {
    for (bytes, expected) in [
        (
            include_bytes!("fixtures/container-running-1.0.json").as_slice(),
            ContainerState::Running,
        ),
        (
            include_bytes!("fixtures/container-stopped-1.0.json").as_slice(),
            ContainerState::Stopped,
        ),
    ] {
        let actual = inspector(output(bytes))
            .inspect(&id())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(actual.id, id());
        assert_eq!(actual.state, expected);
        assert_eq!(actual.ownership.managed_by, "gascan");
        assert_eq!(actual.ownership.sandbox_id, id());
    }
}

#[tokio::test]
async fn mixed_list_classifies_owned_foreign_and_mismatched_resources() {
    let resources = inspector(output(include_bytes!(
        "fixtures/container-list-mixed-1.0.json"
    )))
    .list_resources()
    .await
    .unwrap();
    assert_eq!(
        resources
            .iter()
            .map(|r| (r.name(), r.ownership()))
            .collect::<Vec<_>>(),
        [
            ("code-a1b2c3d4e5f6", ResourceOwnership::GasCanOwned),
            ("foreign-111111111111", ResourceOwnership::Foreign),
            ("collision-222222222222", ResourceOwnership::Mismatched),
        ]
    );
    assert_ne!(
        resources,
        inspector(output(include_bytes!(
            "fixtures/container-list-mixed-1.0.json"
        )))
        .list_resources()
        .await
        .unwrap(),
        "each inventory has fresh removal proofs"
    );
}

#[tokio::test]
async fn malformed_required_fields_and_unknown_states_fail_closed() {
    for bytes in [
        br#"[{"configuration":{"id":"bad"},"status":{"state":"running"}}]"#.as_slice(),
        br#"[{"configuration":{"id":"code-a1b2c3d4e5f6"},"status":{}}]"#.as_slice(),
        br#"[{"configuration":{"id":"code-a1b2c3d4e5f6"},"status":{"state":"paused"}}]"#.as_slice(),
    ] {
        assert!(inspector(output(bytes)).list_resources().await.is_err());
    }
    let error = inspector(output(
        br#"[{"configuration":{"id":"code-a1b2c3d4e5f6"},"status":{"state":"paused"}}]"#,
    ))
    .inspect(&id())
    .await
    .unwrap_err();
    assert_eq!(error.code(), "unknown_actual_state");
}

#[tokio::test]
async fn only_documented_cli_not_found_exit_code_is_absence() {
    let missing = RuntimeError::CommandFailed {
        operation: "container".into(),
        exit_code: Some(1),
        stderr: "diagnostic wording is not parsed".into(),
    };
    assert_eq!(inspector(Err(missing)).inspect(&id()).await.unwrap(), None);

    let other = RuntimeError::CommandFailed {
        operation: "container".into(),
        exit_code: Some(2),
        stderr: "another failure".into(),
    };
    assert!(inspector(Err(other)).inspect(&id()).await.is_err());
}
