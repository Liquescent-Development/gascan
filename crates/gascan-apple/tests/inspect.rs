use std::{collections::VecDeque, sync::Mutex};

use gascan_apple::{AppleInspector, CommandOutput, CommandRunner, CommandSpec};
use gascan_core::{
    runtime::{ContainerState, ResourceOwnership, RuntimeError, RuntimePort},
    sandbox::SandboxId,
};
use std::net::{IpAddr, Ipv4Addr};

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
        assert_eq!(
            actual.image,
            "ghcr.io/liquescent-development/gascan/workspace:fixture@sha256:\
             aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
        assert_eq!(actual.state, expected);
        if expected == ContainerState::Running {
            assert_eq!(
                actual.ports(),
                [RuntimePort {
                    host_address: IpAddr::V4(Ipv4Addr::LOCALHOST),
                    host_port: 22222,
                    guest_port: 22,
                }]
            );
        } else {
            assert!(actual.ports().is_empty());
        }
        assert_eq!(actual.ownership.managed_by, "gascan");
        assert_eq!(actual.ownership.sandbox_id, id());
    }
}

fn inspect_record(published_ports: &str) -> Vec<u8> {
    format!(
        r#"[{{"configuration":{{"id":"code-a1b2c3d4e5f6","image":"ghcr.io/liquescent-development/gascan/workspace:fixture@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","labels":{{"dev.gascan.managed-by":"gascan","dev.gascan.sandbox-id":"code-a1b2c3d4e5f6"}},"publishedPorts":{published_ports}}},"status":{{"state":"running"}}}}]"#
    )
    .into_bytes()
}

#[tokio::test]
async fn inspect_rejects_untrusted_published_port_shapes_and_values() {
    for published_ports in [
        r#"[{"hostAddress":"127.0.0.1","hostPort":22222,"containerPort":22,"protocol":"udp"}]"#,
        r#"[{"hostAddress":"0.0.0.0","hostPort":22222,"containerPort":22,"protocol":"tcp"}]"#,
        r#"[{"hostAddress":"192.0.2.1","hostPort":22222,"containerPort":22,"protocol":"tcp"}]"#,
        r#"[{"hostAddress":"::1","hostPort":22222,"containerPort":22,"protocol":"tcp"}]"#,
        r#"[{"hostAddress":"127.0.0.1","hostPort":0,"containerPort":22,"protocol":"tcp"}]"#,
        r#"[{"hostAddress":"127.0.0.1","hostPort":22222,"containerPort":0,"protocol":"tcp"}]"#,
        r#"[{"hostAddress":"127.0.0.1","hostPort":"22222","containerPort":22,"protocol":"tcp"}]"#,
        r#"[{"hostAddress":"127.0.0.1","hostPort":65536,"containerPort":22,"protocol":"tcp"}]"#,
        r#"[{"hostAddress":"127.0.0.1","hostPort":22222,"containerPort":22,"protocol":"tcp"},{"hostAddress":"127.0.0.1","hostPort":22222,"containerPort":22,"protocol":"tcp"}]"#,
    ] {
        let response = inspect_record(published_ports);
        let error = inspector(output(&response))
            .inspect(&id())
            .await
            .expect_err("untrusted published port must fail closed");
        assert_eq!(
            error.code(),
            "invalid_output",
            "published ports: {published_ports}"
        );
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
async fn foreign_container_names_do_not_have_to_be_valid_sandbox_ids() {
    let resources = inspector(output(
        br#"[{"configuration":{"id":"bleh","labels":{}},"status":{"state":"stopped"}}]"#,
    ))
    .list_resources()
    .await
    .unwrap();
    assert_eq!(resources.len(), 1);
    assert_eq!(resources[0].name(), "bleh");
    assert_eq!(resources[0].sandbox_id(), None);
    assert_eq!(resources[0].ownership(), ResourceOwnership::Foreign);
}

#[tokio::test]
async fn current_apple_image_object_is_accepted_without_weakening_inventory_classification() {
    let resources = inspector(output(
        br#"[{"configuration":{"id":"bleh","image":{"descriptor":{"digest":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},"reference":"docker.io/library/alpine:latest"},"labels":{}},"status":{"state":"stopped"}}]"#,
    ))
    .list_resources()
    .await
    .unwrap();
    assert_eq!(resources.len(), 1);
    assert_eq!(resources[0].name(), "bleh");
    assert_eq!(resources[0].ownership(), ResourceOwnership::Foreign);
}

#[tokio::test]
async fn current_apple_image_object_preserves_exact_digest_qualified_inspection() {
    let response = br#"[{"configuration":{"id":"code-a1b2c3d4e5f6","image":{"descriptor":{"digest":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},"reference":"ghcr.io/liquescent-development/gascan/workspace:fixture@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},"labels":{"dev.gascan.managed-by":"gascan","dev.gascan.sandbox-id":"code-a1b2c3d4e5f6"}},"status":{"state":"running"}}]"#;
    let actual = inspector(output(response))
        .inspect(&id())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        actual.image,
        "ghcr.io/liquescent-development/gascan/workspace:fixture@sha256:\
         aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    );
}

#[tokio::test]
async fn structured_image_requires_matching_reference_and_descriptor_digests() {
    for image in [
        r#"{"reference":"ghcr.io/liquescent-development/gascan/workspace:fixture@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#,
        r#"{"descriptor":{},"reference":"ghcr.io/liquescent-development/gascan/workspace:fixture@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#,
        r#"{"descriptor":{"digest":"sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"},"reference":"ghcr.io/liquescent-development/gascan/workspace:fixture@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#,
    ] {
        let response = format!(
            r#"[{{"configuration":{{"id":"code-a1b2c3d4e5f6","image":{image},"labels":{{"dev.gascan.managed-by":"gascan","dev.gascan.sandbox-id":"code-a1b2c3d4e5f6"}}}},"status":{{"state":"running"}}}}]"#
        );
        let error = inspector(output(response.as_bytes()))
            .inspect(&id())
            .await
            .expect_err("incomplete or mismatched structured image must fail closed");
        assert_eq!(error.code(), "invalid_output", "image: {image}");
    }
}

#[tokio::test]
async fn malformed_required_fields_and_unknown_states_fail_closed() {
    for bytes in [
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
async fn inspect_never_forges_owned_metadata_from_invalid_annotations() {
    for (labels, expected_code) in [
        (r#""dev.gascan.managed-by":"gascan""#, "invalid_output"),
        (
            r#""dev.gascan.managed-by":"gascan","dev.gascan.sandbox-id":"bad""#,
            "invalid_output",
        ),
        (
            r#""dev.gascan.managed-by":"gascan","dev.gascan.sandbox-id":"other-111111111111""#,
            "ownership_mismatch",
        ),
    ] {
        let record = format!(
            r#"[{{"configuration":{{"id":"code-a1b2c3d4e5f6","labels":{{{labels}}}}},"status":{{"state":"running"}}}}]"#
        );
        let error = inspector(output(record.as_bytes()))
            .inspect(&id())
            .await
            .expect_err("invalid ownership annotation must not produce a sandbox");
        assert_eq!(error.code(), expected_code, "labels: {labels}");
    }
}

#[tokio::test]
async fn inspect_rejects_missing_or_mutable_image_references() {
    for image in [
        String::new(),
        r#","image":"""#.to_owned(),
        r#","image":"ghcr.io/liquescent-development/gascan/workspace:latest""#.to_owned(),
        r#","image":"ghcr.io/liquescent-development/gascan/workspace:fixture@sha256:short""#
            .to_owned(),
    ] {
        let record = format!(
            r#"[{{"configuration":{{"id":"code-a1b2c3d4e5f6"{image},"labels":{{"dev.gascan.managed-by":"gascan","dev.gascan.sandbox-id":"code-a1b2c3d4e5f6"}}}},"status":{{"state":"running"}}}}]"#
        );
        let error = inspector(output(record.as_bytes()))
            .inspect(&id())
            .await
            .expect_err("unresolved image must not produce a sandbox");
        assert_eq!(error.code(), "invalid_output", "image field: {image}");
    }
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
